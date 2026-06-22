use std::io::Write as _;
use std::time::Duration;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use axum::Router;
use axum::body::Body;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::http::HeaderValue;
use axum::http::StatusCode;
use axum::http::header;
use axum::response::IntoResponse;
use axum::response::Response;
use axum::response::sse::Sse;
use axum::routing::get;
use axum::routing::post;
use bytes::Bytes;
use tokio::net::TcpListener;
use tracing::error;
use tracing::info;
use tracing::warn;

use crate::Args;
use crate::responses::ResponsesRequest;
use crate::stream::translate_response_stream;
use crate::translate::responses_to_chat;

pub const DEFAULT_LISTEN_ADDR: &str = "127.0.0.1:0";
pub const DEFAULT_UPSTREAM_URL: &str = "https://api.deepseek.com/v1";

const UPSTREAM_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Clone)]
struct AppState {
    http: reqwest::Client,
    upstream_chat_url: String,
}

pub async fn serve(args: Args) -> anyhow::Result<()> {
    let listener = TcpListener::bind(&args.listen).await?;
    let local_addr = listener.local_addr()?;

    if args.print_listen {
        println!("http://{local_addr}");
        std::io::stdout().flush()?;
    }

    serve_on_listener(listener, &args.upstream).await
}

/// Run the proxy on an already-bound listener. This is the entry point used
/// when Codex auto-spawns the proxy in-process (so the caller can bind first
/// to detect port conflicts before deciding whether to spawn at all).
pub async fn serve_on_listener(
    listener: TcpListener,
    upstream_base_url: &str,
) -> anyhow::Result<()> {
    let local_addr = listener.local_addr()?;
    let upstream_chat_url = format!(
        "{}/chat/completions",
        upstream_base_url.trim_end_matches('/')
    );

    // Always direct-connect to DeepSeek. The user's HTTP_PROXY / HTTPS_PROXY
    // env vars often point at a local Privoxy / Clash that mangles or fails
    // SSE long-polling. DeepSeek's `api.deepseek.com` is reachable from
    // mainland China without a proxy, so this is the safe default for a
    // local provider proxy.
    let http = reqwest::Client::builder()
        .timeout(UPSTREAM_TIMEOUT)
        .no_proxy()
        .build()?;

    info!(
        %local_addr,
        upstream = %upstream_chat_url,
        "codex-deepseek-proxy listening"
    );

    let state = AppState {
        http,
        upstream_chat_url,
    };
    let router = Router::new()
        .route("/v1/responses", post(responses_handler))
        .route("/readyz", get(readiness_handler))
        .with_state(state);

    axum::serve(listener, router).await?;
    Ok(())
}

async fn readiness_handler() -> StatusCode {
    StatusCode::OK
}

/// Translate the incoming Responses request to a DeepSeek Chat Completions
/// request, POST it upstream, and either:
///
/// - **`stream: true`** — translate the upstream SSE chat-completions stream
///   into a Responses-shaped SSE event stream (B2). Codex always streams.
/// - **`stream: false`** — pass the upstream JSON through untouched. The
///   payload shape will *not* be Responses-compatible until a later phase
///   adds non-streaming response translation; provided so curl-style probes
///   keep working.
async fn responses_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ErrorResponse> {
    let req: ResponsesRequest = serde_json::from_slice(&body)
        .map_err(|e| ErrorResponse::bad_request(format!("invalid Responses request body: {e}")))?;

    let (chat_req, namespace_map) = responses_to_chat(req);
    let model_for_stream = chat_req.model.clone();
    let want_stream = chat_req.stream;
    let chat_body = serde_json::to_vec(&chat_req)
        .map_err(|e| ErrorResponse::internal(format!("serializing chat request: {e}")))?;

    let mut upstream_builder = state
        .http
        .post(&state.upstream_chat_url)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::ACCEPT, "application/json");
    if let Some(auth) = headers.get(header::AUTHORIZATION) {
        upstream_builder = upstream_builder.header(header::AUTHORIZATION, auth);
    } else {
        warn!("no Authorization header on inbound request; DeepSeek will likely 401");
    }

    // Keep a copy of the outgoing body so we can dump it to stderr if the
    // upstream returns 4xx; this is the single most useful diagnostic when
    // DeepSeek rejects a request for protocol-correctness reasons.
    let chat_body_for_dump = chat_body.clone();

    let upstream_resp = upstream_builder.body(chat_body).send().await.map_err(|e| {
        error!(error = %e, "upstream request failed");
        ErrorResponse::bad_gateway(format!("upstream request failed: {e}"))
    })?;

    if !want_stream {
        return pass_through(upstream_resp).await;
    }

    // Upstream HTTP errors (4xx / 5xx) are never streamed by DeepSeek — they
    // come back as a single JSON body. Forward as-is so Codex sees the actual
    // status code instead of a synthetic `response.failed` wrapping it.
    if !upstream_resp.status().is_success() {
        let status = upstream_resp.status();
        // Dump the exact request body we sent — without this we're guessing
        // at why DeepSeek 4xx'd us.
        let body_preview = String::from_utf8_lossy(&chat_body_for_dump);
        error!(
            %status,
            request_body = %body_preview,
            "upstream returned non-success; dumping translated chat request"
        );
        return pass_through(upstream_resp).await;
    }

    let created_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default();
    let stream = translate_response_stream(upstream_resp, model_for_stream, created_at, namespace_map);
    Ok(Sse::new(stream).into_response())
}

/// Build an axum Response from the full upstream payload (B1-style passthrough).
async fn pass_through(upstream_resp: reqwest::Response) -> Result<Response, ErrorResponse> {
    let status = upstream_resp.status();
    let upstream_headers = upstream_resp.headers().clone();
    let upstream_bytes = upstream_resp
        .bytes()
        .await
        .map_err(|e| ErrorResponse::bad_gateway(format!("upstream body read failed: {e}")))?;

    let mut builder = Response::builder().status(status);
    if let Some(headers_mut) = builder.headers_mut()
        && let Some(ct) = upstream_headers.get(header::CONTENT_TYPE)
        && let Ok(value) = HeaderValue::from_bytes(ct.as_bytes())
    {
        headers_mut.insert(header::CONTENT_TYPE, value);
    }
    Ok(builder
        .body(Body::from(upstream_bytes))
        .unwrap_or_else(|_| (StatusCode::INTERNAL_SERVER_ERROR, "response build").into_response()))
}

struct ErrorResponse {
    status: StatusCode,
    body: String,
}

impl ErrorResponse {
    fn bad_request(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            body: msg.into(),
        }
    }
    fn bad_gateway(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_GATEWAY,
            body: msg.into(),
        }
    }
    fn internal(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            body: msg.into(),
        }
    }
}

impl IntoResponse for ErrorResponse {
    fn into_response(self) -> Response {
        (self.status, self.body).into_response()
    }
}
