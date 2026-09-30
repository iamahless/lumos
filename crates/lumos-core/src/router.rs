//! Routing: axum's router, re-exported with a Lumos `serve`.
//!
//! Lumos does not reinvent HTTP: [`Router`] is axum's router, and the
//! routing constructors ([`get`], [`post`], …) are axum's. Controllers
//! generate these routers via `#[controller]`; [`serve`] binds and runs one
//! with graceful shutdown.

pub use axum::http::StatusCode;
pub use axum::routing::{delete, get, patch, post, put};
pub use axum::Router;

use crate::{AppError, Result};

/// Serves `router` on `address` (e.g. `"127.0.0.1:3000"`) until Ctrl-C.
///
/// Shutdown is graceful: in-flight requests complete before the server
/// stops. Binding and I/O failures are [`AppError::Internal`] with operator
/// context, and the bound address is logged at startup.
///
/// # Examples
///
/// ```rust,no_run
/// use lumos_core::{get, Router};
///
/// # #[tokio::main]
/// # async fn main() -> lumos_core::Result<()> {
/// let router = Router::new().route("/", get(|| async { "Hello, Lumos!" }));
/// lumos_core::serve(router, "127.0.0.1:3000").await
/// # }
/// ```
pub async fn serve(router: Router, address: &str) -> Result<()> {
    let listener = tokio::net::TcpListener::bind(address)
        .await
        .map_err(|error| AppError::internal(format!("failed to bind {address}: {error}")))?;
    let bound = listener
        .local_addr()
        .map(|addr| addr.to_string())
        .unwrap_or_else(|_| address.to_string());
    tracing::info!(address = %bound, "lumos listening");
    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .map_err(|error| AppError::internal(format!("server error: {error}")))
}

/// Resolves on Ctrl-C (ignoring signal-subscription failures, which only
/// happen on runtimes without signal support, where shutdown then falls
/// back to task cancellation).
async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}
