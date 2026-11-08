//! The MCP service over HTTP, for the GUI's Settings › Tools › MCP Server page.
//!
//! Unlike the stdio mode (`openidea --mcp`, a separate process), this server
//! runs inside the app and serves the project of the window that started it:
//! the tools then see exactly what that window's Services / Database views see,
//! and `start_service` starts the usual IDE runs (visible in the UI), not
//! children of a helper process.
//!
//! - Bound to `127.0.0.1` only, mounted at `/mcp` (streamable HTTP transport).
//! - Every request must carry `Authorization: Bearer <token>`: the port is open
//!   to any local process, and these tools can start services and read data.
//! - The caller owns the runtime: [`start`] spawns the axum server and
//!   [`Handle::stop`] cancels it.

use crate::server::OpenIdeaMcp;
use axum::Router;
use axum::extract::Request;
use axum::http::{StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

/// A running HTTP MCP server: where it listens and how to stop it.
pub struct Handle {
    pub addr: SocketAddr,
    cancel: CancellationToken,
}

impl Handle {
    /// Stops the server: the listener closes and active sessions end.
    pub fn stop(&self) {
        self.cancel.cancel();
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        // The server must not outlive the state that owns the handle.
        self.cancel.cancel();
    }
}

/// Starts serving `window`'s project at `127.0.0.1:port` (`port` 0 picks a
/// free one; the real address is in [`Handle::addr`]).
///
/// Fails when the port is taken or the host has no such project open.
pub async fn start(
    host: Arc<ide_host::Host>,
    window: &str,
    port: u16,
    token: String,
    allow_write: bool,
) -> Result<Handle, String> {
    if host.project(window).is_err() {
        return Err(format!("no project is open in window {window:?}"));
    }
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port))
        .await
        .map_err(|e| format!("cannot listen on 127.0.0.1:{port}: {e}"))?;
    let addr = listener.local_addr().map_err(|e| format!("cannot read the bound address: {e}"))?;
    let cancel = CancellationToken::new();
    let config = StreamableHttpServerConfig::default().with_cancellation_token(cancel.clone());
    let window: Arc<str> = window.into();
    // One service per session: each MCP client gets its own tool state.
    let service = StreamableHttpService::new(
        move || Ok(OpenIdeaMcp::new(host.clone(), allow_write, window.clone())),
        Arc::new(LocalSessionManager::default()),
        config,
    );
    let token: Arc<str> = token.into();
    let guard = middleware::from_fn(move |req: Request, next: Next| {
        let token = token.clone();
        async move { require_token(token, req, next).await }
    });
    let app = Router::new().nest_service("/mcp", service).layer(guard);
    let shutdown = cancel.clone();
    tokio::spawn(async move {
        let served = axum::serve(listener, app).with_graceful_shutdown(shutdown.cancelled_owned()).await;
        if let Err(e) = served {
            tracing::warn!("MCP HTTP server stopped: {e}");
        }
    });
    Ok(Handle { addr, cancel })
}

/// Passes the request on only with the server's `Authorization: Bearer` token.
async fn require_token(token: Arc<str>, req: Request, next: Next) -> Response {
    let authorized = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .is_some_and(|value| value.trim() == &*token);
    if !authorized {
        return (StatusCode::UNAUTHORIZED, "missing or wrong bearer token").into_response();
    }
    next.run(req).await
}

/// A fresh token, hex-encoded (the client configuration shows it, so keep it
/// copy-friendly). `RandomState` is seeded from the OS, which is enough for a
/// loopback server's bearer token and keeps the crate free of an RNG crate.
pub fn new_token() -> String {
    use std::collections::hash_map::RandomState;
    use std::fmt::Write;
    use std::hash::{BuildHasher, Hasher};
    let mut bytes = [0u8; 32];
    let mut filled = 0;
    while filled < bytes.len() {
        let mut hasher = RandomState::new().build_hasher();
        hasher.write_usize(filled);
        let word = hasher.finish().to_le_bytes();
        let take = (bytes.len() - filled).min(word.len());
        bytes[filled..filled + take].copy_from_slice(&word[..take]);
        filled += take;
    }
    bytes.iter().fold(String::with_capacity(64), |mut out, byte| {
        let _ = write!(out, "{byte:02x}");
        out
    })
}

#[cfg(test)]
mod tests {
    use super::new_token;

    #[test]
    fn tokens_are_hex_and_distinct() {
        let a = new_token();
        let b = new_token();
        assert_eq!(a.len(), 64);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }
}
