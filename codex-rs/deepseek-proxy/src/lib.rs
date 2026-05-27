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
pub use server::serve_on_listener;

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

/// In-process auto-start helper.
///
/// Given a Codex provider `base_url` like `http://127.0.0.1:38440/v1`:
///
/// - If `base_url` points to a loopback address, try to bind that host:port.
///   On success: spawn the proxy on it as a Tokio task and return `Ok(true)`.
///   On `AddrInUse`: someone (the user, or a previous Codex instance in the
///   same shell session) is already serving — assume it's compatible and
///   return `Ok(false)`.
///   On any other bind error: surface it.
/// - If `base_url` is non-loopback or unparseable: skip with `Ok(false)`;
///   the user is pointing Codex at an external proxy and we shouldn't try
///   to take over.
pub async fn ensure_running(base_url: &str, upstream: impl Into<String>) -> std::io::Result<bool> {
    let Some(addr) = loopback_listen_addr(base_url) else {
        tracing::debug!(
            base_url,
            "base_url is not a loopback URL; skipping auto-spawn"
        );
        return Ok(false);
    };

    match tokio::net::TcpListener::bind(&addr).await {
        Ok(listener) => {
            let upstream = upstream.into();
            tokio::spawn(async move {
                if let Err(err) = serve_on_listener(listener, &upstream).await {
                    tracing::error!(error = %err, "codex-deepseek-proxy task exited with error");
                }
            });
            tracing::info!(addr = %addr, "auto-spawned codex-deepseek-proxy");
            Ok(true)
        }
        Err(err) if err.kind() == std::io::ErrorKind::AddrInUse => {
            tracing::info!(
                addr = %addr,
                "port already bound; assuming an existing codex-deepseek-proxy is serving there"
            );
            Ok(false)
        }
        Err(err) => Err(err),
    }
}

/// Extract `"HOST:PORT"` from a `base_url` that points at a loopback address.
/// Returns `None` for non-loopback hosts so callers know not to auto-spawn.
fn loopback_listen_addr(base_url: &str) -> Option<String> {
    let url = reqwest::Url::parse(base_url).ok()?;
    if url.scheme() != "http" {
        return None;
    }
    let host = url.host_str()?;
    if !matches!(host, "127.0.0.1" | "localhost" | "::1") {
        return None;
    }
    let port = url.port()?;
    Some(format!("{host}:{port}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_addr_extracts_host_port() {
        assert_eq!(
            loopback_listen_addr("http://127.0.0.1:38440/v1"),
            Some("127.0.0.1:38440".to_string())
        );
        assert_eq!(
            loopback_listen_addr("http://localhost:9999"),
            Some("localhost:9999".to_string())
        );
    }

    #[test]
    fn loopback_addr_skips_non_loopback() {
        assert!(loopback_listen_addr("https://api.deepseek.com/v1").is_none());
        assert!(loopback_listen_addr("http://10.0.0.5:38440/v1").is_none());
        assert!(loopback_listen_addr("http://example.com/v1").is_none());
    }

    #[test]
    fn loopback_addr_requires_port() {
        // Without an explicit port we can't bind; skip.
        assert!(loopback_listen_addr("http://127.0.0.1/v1").is_none());
    }

    #[test]
    fn loopback_addr_requires_http_scheme() {
        // No real proxy use case for https on loopback; skip.
        assert!(loopback_listen_addr("https://127.0.0.1:38440/v1").is_none());
    }
}
