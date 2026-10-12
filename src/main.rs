//! `reddit-mcp` — stdio MCP server entry point.
//!
//! stdout carries the MCP protocol; all logging goes to stderr (required for
//! stdio transports).

use reddit_mcp::config::Config;
use reddit_mcp::tools::RedditServer;
use rmcp::service::ServiceExt;
use rmcp::transport::stdio;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let config = match Config::load() {
        Ok(cfg) => cfg,
        Err(e) => {
            tracing::error!(error = %e, "failed to load configuration");
            return Err(e.into());
        }
    };
    tracing::info!(
        preset = %config.preset_name,
        configured = config.is_configured(),
        "starting reddit-mcp"
    );

    let server = RedditServer::new(config).map_err(|e| anyhow::anyhow!("{e}"))?;
    let service = server.serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}
