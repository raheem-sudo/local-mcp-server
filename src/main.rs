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
    transport::sse_server::{SseServer, SseServerConfig},
};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

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

    // Render (and most PaaS hosts) assign a port dynamically via $PORT and
    // require the service to bind 0.0.0.0. Locally, PORT is usually unset,
    // so we fall back to 8080 to match the earlier local instructions.
    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8080);
    let bind_addr: std::net::SocketAddr = format!("0.0.0.0:{port}").parse()?;

    let sse_config = SseServerConfig {
        bind: bind_addr,
        sse_path: "/sse".to_string(),
        post_path: "/message".to_string(),
        ct: CancellationToken::new(),
        sse_keep_alive: None,
    };

    // SseServer::new() gives us the raw axum Router instead of binding
    // and serving immediately, so we can merge in extra routes — here, a
    // plain GET /healthz that Render's health checker can hit. (The /sse
    // route itself is a permanently-open stream, which is a poor fit for
    // a health check.)
    let (sse_server, sse_router) = SseServer::new(sse_config);
    let router = sse_router.route(
        "/healthz",
        axum::routing::get(|| async { "ok" }),
    );

    let listener = tokio::net::TcpListener::bind(sse_server.config.bind).await?;
    let ct = sse_server.config.ct.child_token();
    let axum_server =
        axum::serve(listener, router).with_graceful_shutdown(async move { ct.cancelled().await });

    tokio::spawn(async move {
        if let Err(e) = axum_server.await {
            tracing::error!("HTTP server error: {e}");
        }
    });

    // Attach our MCP service: a new `Counter` instance is created per
    // client session.
    let ct = sse_server.with_service(Counter::new);

    tracing::info!("MCP SSE server listening on http://{bind_addr}/sse");
    println!("MCP SSE server ready at http://{bind_addr}/sse (Ctrl+C to stop)");

    tokio::signal::ctrl_c().await?;
    tracing::info!("shutting down");
    ct.cancel();

    Ok(())
}