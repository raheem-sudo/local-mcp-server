//! A minimal Model Context Protocol (MCP) server in Rust, served over
//! Server-Sent Events (SSE) — the transport your config's
//! `"type": "sse"` / `"url": "http://localhost:8080/sse"` expects.
//!
//! It exposes a tiny "Counter" tool set (increment / decrement / get_value)
//! so you have something concrete to call once connected. Swap the tools
//! for your own logic.
//!
//! Run it locally:
//!     cargo run
//! then point an MCP client at:
//!     http://localhost:8080/sse
//!
//! To expose it remotely instead of locally, either:
//!   - bind to 0.0.0.0 (already done below) and put it behind a reverse
//!     proxy / tunnel (nginx, Caddy, ngrok, cloudflared, etc.) with TLS, or
//!   - deploy the binary to a VM/container and open the port, then use
//!     that host's URL (e.g. "https://your-domain.com/sse") in the config.

use std::sync::Arc;

use rmcp::{
    ServerHandler,
    handler::server::tool::ToolRouter,
    model::{Implementation, ServerCapabilities, ServerInfo},
    tool, tool_handler, tool_router,
    transport::sse_server::SseServer,
};
use tokio::sync::Mutex;

/// Example MCP "server": holds whatever state your tools need.
/// Here it's just a shared counter.
#[derive(Clone)]
struct Counter {
    value: Arc<Mutex<i64>>,
    tool_router: ToolRouter<Counter>,
}

#[tool_router]
impl Counter {
    fn new() -> Self {
        Self {
            value: Arc::new(Mutex::new(0)),
            tool_router: Self::tool_router(),
        }
    }

    #[tool(description = "Increment the counter by 1 and return the new value")]
    async fn increment(&self) -> String {
        let mut v = self.value.lock().await;
        *v += 1;
        v.to_string()
    }

    #[tool(description = "Decrement the counter by 1 and return the new value")]
    async fn decrement(&self) -> String {
        let mut v = self.value.lock().await;
        *v -= 1;
        v.to_string()
    }

    #[tool(description = "Get the current counter value")]
    async fn get_value(&self) -> String {
        let v = self.value.lock().await;
        v.to_string()
    }
}

// Wires the #[tool]-annotated methods above into the MCP `ServerHandler`
// trait (handles `tools/list` and `tools/call` for you).
#[tool_handler]
impl ServerHandler for Counter {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            server_info: Implementation {
                name: "rust-mcp-sse-server".to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
                ..Default::default()
            },
            instructions: Some(
                "Example Rust MCP server. Exposes increment/decrement/get_value tools \
                 over an in-memory counter."
                    .to_string(),
            ),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            ..Default::default()
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    // Bind 0.0.0.0 so the same binary works for a purely local run
    // (reach it at http://localhost:8080/sse) or, behind a reverse proxy /
    // tunnel, as a remote server (reach it at https://your-host/sse).
    let bind_addr = "0.0.0.0:8080".parse()?;

    // SseServer::serve() starts listening immediately and defaults to
    // exposing the SSE stream at "/sse" and the message-post endpoint at
    // "/message" — matching the URL in your config.
    let sse_server = SseServer::serve(bind_addr).await?;

    // Attach our service: a new `Counter` instance is created per client
    // session. `with_service` returns a CancellationToken you can use for
    // graceful shutdown.
    let ct = sse_server.with_service(Counter::new);

    tracing::info!("MCP SSE server listening on http://{bind_addr}/sse");
    println!("MCP SSE server ready at http://{bind_addr}/sse (Ctrl+C to stop)");

    tokio::signal::ctrl_c().await?;
    tracing::info!("shutting down");
    ct.cancel();

    Ok(())
}