//! In-process DeepSeek bridge for Codex. Codex speaks the OpenAI Responses API;
//! DeepSeek exposes an OpenAI-compatible Chat Completions API. This crate
//! translates between them as an [`HttpTransport`](codex_client::HttpTransport)
//! that Codex's model client drives directly — no local proxy process or port.

mod chat;
mod responses;
mod stream;
mod translate;
mod transport;

pub use transport::DEFAULT_UPSTREAM_URL;
pub use transport::DeepSeekTransport;
