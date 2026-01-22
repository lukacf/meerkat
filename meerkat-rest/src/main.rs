//! Meerkat REST API Server
//!
//! Provides HTTP endpoints for running and managing Meerkat agents.
//!
//! # Environment Variables
//!
//! - `ANTHROPIC_API_KEY`: Required API key for Anthropic
//! - `RKAT_STORE_PATH`: Optional path for session storage (default: ~/.local/share/meerkat/sessions)
//! - `RKAT_MODEL`: Default model to use (default: claude-opus-4-5)
//! - `RKAT_MAX_TOKENS`: Default max tokens per turn (default: 4096)
//! - `RKAT_HOST`: Host to bind to (default: 127.0.0.1)
//! - `RKAT_PORT`: Port to bind to (default: 8080)

use meerkat_rest::{router, AppState};
use std::net::SocketAddr;
use tower_http::{cors::CorsLayer, trace::TraceLayer};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() {
    // Initialize tracing
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "meerkat_rest=info,tower_http=debug".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    // Check for API key early
    if std::env::var("ANTHROPIC_API_KEY").is_err() {
        tracing::warn!(
            "ANTHROPIC_API_KEY not set - API calls will fail. \
             Set the environment variable before making requests."
        );
    }

    // Build app state
    let state = AppState::default();
    tracing::info!(
        store_path = %state.store_path.display(),
        default_model = %state.default_model,
        max_tokens = state.max_tokens,
        "Starting Meerkat REST server"
    );

    // Build router with middleware
    let app = router(state)
        .layer(TraceLayer::new_for_http())
        .layer(CorsLayer::permissive());

    // Parse host and port
    let host = std::env::var("RKAT_HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
    let port: u16 = std::env::var("RKAT_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(8080);

    let addr: SocketAddr = format!("{}:{}", host, port)
        .parse()
        .expect("Invalid host:port combination");

    tracing::info!("Listening on http://{}", addr);

    // Run server with graceful shutdown
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .unwrap();

    tracing::info!("Server shutdown complete");
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("Failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("Failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {
            tracing::info!("Received Ctrl+C, shutting down...");
        },
        _ = terminate => {
            tracing::info!("Received SIGTERM, shutting down...");
        },
    }
}
