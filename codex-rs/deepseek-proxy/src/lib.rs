//! Proxy that exposes the OpenAI Responses API on a local port and forwards
//! to DeepSeek's `/v1/chat/completions` (OpenAI-compatible) endpoint. The
//! translation layer is built up over phases B1–B4; this skeleton only
//! validates HTTP plumbing.

use clap::Parser;

mod chat;
mod responses;
mod server;
mod stream;
mod translate;

pub use server::DEFAULT_LISTEN_ADDR;
pub use server::DEFAULT_UPSTREAM_URL;
pub use server::serve;

/// CLI args for `codex deepseek-proxy`.
#[derive(Debug, Clone, Parser)]
#[command(
    name = "deepseek-proxy",
    about = "Local Responses API → DeepSeek bridge"
)]
pub struct Args {
    /// Address to bind. Use port 0 for an ephemeral port.
    #[arg(long, default_value = DEFAULT_LISTEN_ADDR)]
    pub listen: String,

    /// DeepSeek upstream base URL (OpenAI Chat Completions compatible).
    #[arg(long, default_value = DEFAULT_UPSTREAM_URL)]
    pub upstream: String,

    /// Print the bound address as `http://HOST:PORT` (one line, flushed) once
    /// the listener is ready. Useful for parents capturing the ephemeral port.
    #[arg(long)]
    pub print_listen: bool,
}

pub async fn run_main(args: Args) -> anyhow::Result<()> {
    init_tracing();
    serve(args).await
}

/// Best-effort install of an env-filtered tracing subscriber so
/// `RUST_LOG=codex_deepseek_proxy=debug` surfaces our logs. No-op if a
/// subscriber has already been installed (e.g. by the parent process).
fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init();
}
