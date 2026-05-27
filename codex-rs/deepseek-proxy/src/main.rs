use clap::Parser;
use codex_deepseek_proxy::Args;
use codex_deepseek_proxy::run_main;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    run_main(Args::parse()).await
}
