//! The HTTP server: reserved API and WebSocket prefixes, then the embedded web UI.

use std::net::SocketAddr;

use axum::Router;
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::response::Response;
use axum::routing::any;

use crate::assets::{self, Embedded};

/// Builds the router. `/api` and `/ws` are reserved: they answer 404 until their
/// handlers exist, and they never fall through to the web UI.
pub fn router() -> Router {
    Router::new()
        .route("/api", any(reserved))
        .route("/api/{*rest}", any(reserved))
        .route("/ws", any(reserved))
        .route("/ws/{*rest}", any(reserved))
        .fallback(web_ui)
}

async fn reserved() -> StatusCode {
    StatusCode::NOT_FOUND
}

async fn web_ui(method: Method, uri: Uri, headers: HeaderMap) -> Response {
    assets::respond(&Embedded, &method, uri.path(), &headers)
}

/// Binds `listen`, prints the bound address, and serves until Ctrl-C or SIGTERM.
pub async fn serve(listen: SocketAddr) -> std::io::Result<()> {
    let listener = tokio::net::TcpListener::bind(listen).await?;
    println!("lodger listening on http://{}", listener.local_addr()?);
    axum::serve(listener, router())
        .with_graceful_shutdown(shutdown_signal())
        .await
}

async fn shutdown_signal() {
    let ctrl_c = tokio::signal::ctrl_c();
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .expect("install the SIGTERM handler");
    tokio::select! {
        _ = ctrl_c => {}
        _ = term.recv() => {}
    }
}
