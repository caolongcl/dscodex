//! In-process [`HttpTransport`] for the DeepSeek provider. Replaces the old
//! local HTTP proxy: instead of Codex POSTing Responses JSON to a loopback port
//! that translates and forwards, this runs the same translation inside Codex's
//! own process and talks straight to DeepSeek's `/v1/chat/completions`.

use std::time::Duration;

use bytes::Bytes;
use codex_client::ByteStream;
use codex_client::HttpTransport;
use codex_client::Request;
use codex_client::Response;
use codex_client::StreamResponse;
use codex_client::TransportError;
use futures::StreamExt;
use http::HeaderMap;
use http::HeaderValue;
use http::StatusCode;
use http::header;

use crate::responses::ResponsesRequest;
use crate::stream::collect_final_response;
use crate::stream::translate_chat_sse;
use crate::translate::NamespaceMap;
use crate::translate::responses_to_chat;

/// Default DeepSeek upstream base (OpenAI Chat Completions compatible).
pub const DEFAULT_UPSTREAM_URL: &str = "https://api.deepseek.com/v1";

const UPSTREAM_TIMEOUT: Duration = Duration::from_secs(300);

/// Translates Responses ⇄ DeepSeek Chat Completions in process. Cheap to clone.
#[derive(Clone)]
pub struct DeepSeekTransport {
    http: reqwest::Client,
    upstream_chat_url: String,
}

impl DeepSeekTransport {
    /// `upstream_base_url` is the DeepSeek base (e.g. `https://api.deepseek.com/v1`) —
    /// exactly the provider `base_url` Codex is configured with.
    pub fn new(upstream_base_url: &str) -> Self {
        // Direct-connect to DeepSeek: a user's HTTP(S)_PROXY often points at a
        // local Privoxy/Clash that mangles SSE long-polling, and api.deepseek.com
        // is reachable without one. Matches the old sidecar's behaviour.
        let http = reqwest::Client::builder()
            .timeout(UPSTREAM_TIMEOUT)
            .no_proxy()
            .build()
            .unwrap_or_default();
        Self {
            http,
            upstream_chat_url: format!(
                "{}/chat/completions",
                upstream_base_url.trim_end_matches('/')
            ),
        }
    }

    /// Translate the inbound Responses request and POST it to DeepSeek.
    /// `force_stream` makes the upstream stream even for a one-shot caller, so the
    /// non-streaming path can reuse the SSE translator (one path to maintain).
    async fn send_upstream(
        &self,
        req: &Request,
        force_stream: bool,
    ) -> Result<(reqwest::Response, String, NamespaceMap), TransportError> {
        let body = req
            .prepare_body_for_send()
            .map_err(TransportError::Build)?
            .body
            .unwrap_or_default();
        let responses_req: ResponsesRequest = serde_json::from_slice(&body)
            .map_err(|e| TransportError::Build(format!("invalid Responses request body: {e}")))?;
        let (mut chat_req, namespace_map) = responses_to_chat(responses_req);
        if force_stream {
            chat_req.stream = true;
        }
        let model = chat_req.model.clone();

        let mut builder = self
            .http
            .post(&self.upstream_chat_url)
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::ACCEPT, "application/json")
            .json(&chat_req);
        // Forward the bearer token Codex already attached (env_key DEEPSEEK_API_KEY).
        if let Some(auth) = req.headers.get(header::AUTHORIZATION) {
            builder = builder.header(header::AUTHORIZATION, auth.clone());
        }
        let resp = builder
            .send()
            .await
            .map_err(|e| TransportError::Network(e.to_string()))?;
        Ok((resp, model, namespace_map))
    }

    /// DeepSeek returns non-2xx as a single JSON body (never SSE); surface the
    /// real status so Codex doesn't wrap it in a synthetic failure.
    fn upstream_error(&self, status: StatusCode, body: String) -> TransportError {
        TransportError::Http {
            status,
            url: Some(self.upstream_chat_url.clone()),
            headers: None,
            body: Some(body),
        }
    }
}

impl HttpTransport for DeepSeekTransport {
    async fn execute(&self, req: Request) -> Result<Response, TransportError> {
        let (resp, model, namespace_map) = self.send_upstream(&req, /*force_stream*/ true).await?;
        let status = resp.status();
        if !status.is_success() {
            return Err(self.upstream_error(status, resp.text().await.unwrap_or_default()));
        }
        let response = collect_final_response(resp, model, unix_secs(), namespace_map).await;
        let body = serde_json::to_vec(&response)
            .map(Bytes::from)
            .map_err(|e| TransportError::Build(e.to_string()))?;
        Ok(Response {
            status: StatusCode::OK,
            headers: json_headers(),
            body,
        })
    }

    async fn stream(&self, req: Request) -> Result<StreamResponse, TransportError> {
        let (resp, model, namespace_map) = self.send_upstream(&req, /*force_stream*/ false).await?;
        let status = resp.status();
        if !status.is_success() {
            return Err(self.upstream_error(status, resp.text().await.unwrap_or_default()));
        }
        let bytes: ByteStream = translate_chat_sse(resp, model, unix_secs(), namespace_map)
            .map(Ok::<Bytes, TransportError>)
            .boxed();
        Ok(StreamResponse {
            status: StatusCode::OK,
            headers: sse_headers(),
            bytes,
        })
    }
}

fn sse_headers() -> HeaderMap {
    let mut h = HeaderMap::new();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/event-stream"),
    );
    h
}

fn json_headers() -> HeaderMap {
    let mut h = HeaderMap::new();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    h
}

fn unix_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default()
}
