//! Out-of-band `/polish` request runner.
//!
//! The polish call does NOT go through codex-core's turn machinery — it's a
//! one-shot HTTP POST to the active provider's `/v1/responses` endpoint that
//! lives entirely in the TUI process. The user's main conversation is never
//! touched: codex-core's thread state, the TUI transcript, rollouts, and
//! `/resume` history are all unaffected.
//!
//! Auth is resolved against the provider config in the same order Codex's
//! main turn flow uses: `experimental_bearer_token` → `env_key` env var →
//! `auth.command`. AWS SigV4 is intentionally not supported (Bedrock isn't
//! a target for `/polish`).

use std::process::Stdio;
use std::time::Duration;

use codex_model_provider_info::ModelProviderInfo;
use codex_protocol::config_types::ModelProviderAuthInfo;
use eventsource_stream::Eventsource;
use futures::StreamExt;
use serde_json::Value;
use serde_json::json;
use tokio::process::Command;

const POLISH_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Debug)]
pub(crate) struct PolishCallParams {
    pub provider: ModelProviderInfo,
    pub model: String,
    pub draft: String,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum PolishError {
    #[error("provider has no base_url; cannot polish")]
    NoBaseUrl,
    #[error("no usable auth on provider (need env_key or auth.command)")]
    NoAuth,
    #[error("auth command failed: {0}")]
    AuthCommand(String),
    #[error("environment variable {0} is unset or empty")]
    AuthEnvMissing(String),
    #[error("http error: {0}")]
    Http(String),
    #[error("upstream returned {status}: {body}")]
    Upstream { status: u16, body: String },
    #[error("stream error: {0}")]
    Stream(String),
    #[error("polish output was empty")]
    EmptyOutput,
}

pub(crate) async fn run_polish(params: PolishCallParams) -> Result<String, PolishError> {
    let PolishCallParams {
        provider,
        model,
        draft,
    } = params;

    let base_url = provider.base_url.as_deref().ok_or(PolishError::NoBaseUrl)?;
    let url = format!("{}/responses", base_url.trim_end_matches('/'));
    let token = resolve_token(&provider).await?;
    let body = build_request_body(&model, &draft);

    let client = reqwest::Client::builder()
        .timeout(POLISH_TIMEOUT)
        .no_proxy()
        .build()
        .map_err(|e| PolishError::Http(e.to_string()))?;

    let resp = client
        .post(&url)
        .bearer_auth(token)
        .header("content-type", "application/json")
        .header("accept", "text/event-stream")
        .json(&body)
        .send()
        .await
        .map_err(|e| PolishError::Http(e.to_string()))?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(PolishError::Upstream {
            status: status.as_u16(),
            body,
        });
    }

    accumulate_response_text(resp).await
}

fn build_request_body(model: &str, draft: &str) -> Value {
    let instruction = format!(
        "You are refining a prompt the user is about to send to a coding \
assistant. Rewrite the draft below to be clearer, more specific, and easier \
for an LLM to act on. Preserve the user's intent; do not answer the prompt. \
Output ONLY the polished prompt as plain text — no preamble, no markdown \
fences, no quotation marks, no commentary.\n\nDraft:\n{draft}"
    );
    json!({
        "model": model,
        "input": instruction,
        "stream": true,
    })
}

async fn resolve_token(provider: &ModelProviderInfo) -> Result<String, PolishError> {
    if let Some(token) = provider
        .experimental_bearer_token
        .as_deref()
        .filter(|t| !t.is_empty())
    {
        return Ok(token.to_string());
    }
    if let Some(env_name) = provider.env_key.as_deref()
        && !env_name.is_empty()
    {
        return match std::env::var(env_name) {
            Ok(v) if !v.is_empty() => Ok(v),
            _ => Err(PolishError::AuthEnvMissing(env_name.to_string())),
        };
    }
    if let Some(auth) = provider.auth.as_ref() {
        return run_auth_command(auth).await;
    }
    Err(PolishError::NoAuth)
}

async fn run_auth_command(auth: &ModelProviderAuthInfo) -> Result<String, PolishError> {
    let output = Command::new(&auth.command)
        .args(&auth.args)
        .current_dir(auth.cwd.as_path())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .output()
        .await
        .map_err(|e| PolishError::AuthCommand(format!("spawn: {e}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(PolishError::AuthCommand(format!(
            "exit {}: {stderr}",
            output.status
        )));
    }
    let s = String::from_utf8(output.stdout)
        .map_err(|_| PolishError::AuthCommand("non-UTF-8 stdout".to_string()))?
        .trim()
        .to_string();
    if s.is_empty() {
        return Err(PolishError::AuthCommand("empty stdout".to_string()));
    }
    Ok(s)
}

async fn accumulate_response_text(resp: reqwest::Response) -> Result<String, PolishError> {
    let mut sse = resp.bytes_stream().eventsource();
    let mut out = String::new();
    let mut saw_completed = false;
    while let Some(item) = sse.next().await {
        let ev = item.map_err(|e| PolishError::Stream(e.to_string()))?;
        if ev.data == "[DONE]" {
            break;
        }
        let Ok(v) = serde_json::from_str::<Value>(&ev.data) else {
            continue;
        };
        match v.get("type").and_then(Value::as_str) {
            Some("response.output_text.delta") => {
                if let Some(delta) = v.get("delta").and_then(Value::as_str) {
                    out.push_str(delta);
                }
            }
            Some("response.completed") => {
                saw_completed = true;
                // Some implementations don't emit a [DONE] sentinel after
                // response.completed; we exit early to avoid hanging on a
                // peer that keeps the connection open.
                break;
            }
            Some("response.failed") => {
                let msg = v
                    .get("error")
                    .and_then(|e| e.get("message"))
                    .and_then(Value::as_str)
                    .unwrap_or("polish stream reported failure")
                    .to_string();
                return Err(PolishError::Stream(msg));
            }
            _ => {}
        }
    }

    let trimmed = out.trim().to_string();
    if trimmed.is_empty() {
        return Err(PolishError::EmptyOutput);
    }
    let _ = saw_completed; // soft signal; not currently propagated
    Ok(trimmed)
}
