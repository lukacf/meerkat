//! Meerkat REST API Server
//!
//! Provides HTTP endpoints for running and managing Meerkat agents.
//!
//! # Environment Variables
//!
//! - `ANTHROPIC_API_KEY`: Required API key for Anthropic

use meerkat_core::{Config, ProviderConfig};
use meerkat_rest::{AppState, router};
use std::net::SocketAddr;
use tower_http::{cors::CorsLayer, trace::TraceLayer};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize tracing
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "meerkat_rest=info,tower_http=debug".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    // Build app state
    let state = AppState::default();
    tracing::info!(
        store_path = %state.store_path.display(),
        default_model = %state.default_model,
        max_tokens = state.max_tokens,
        "Starting Meerkat REST server"
    );

    // Check for API key early
    let mut config = state
        .config_store
        .get()
        .unwrap_or_else(|_| Config::default());
    if let Err(err) = config.apply_env_overrides() {
        tracing::warn!("Failed to apply env overrides: {}", err);
    }
    let has_api_key = match &config.provider {
        ProviderConfig::Anthropic { api_key, .. } => api_key.is_some(),
        ProviderConfig::OpenAI { api_key, .. } => api_key.is_some(),
        ProviderConfig::Gemini { api_key } => api_key.is_some(),
    };
    if !has_api_key {
        tracing::warn!(
            "No provider API key configured (config or environment). \
             API calls will fail until a key is set."
        );
    }

    // Parse host and port from config (non-secret settings)
    let addr: SocketAddr = format!("{}:{}", state.rest_host, state.rest_port)
        .parse()
        .map_err(|e| format!("Invalid host:port combination: {}", e))?;

    // Build router with middleware
    let app = router(state)
        .layer(TraceLayer::new_for_http())
        .layer(CorsLayer::permissive());

    tracing::info!("Listening on http://{}", addr);

    // Run server with graceful shutdown
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    tracing::info!("Server shutdown complete");
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(e) = tokio::signal::ctrl_c().await {
            tracing::error!("Failed to install Ctrl+C handler: {}", e);
        }
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut sig) => {
                sig.recv().await;
            }
            Err(e) => {
                tracing::error!("Failed to install signal handler: {}", e);
            }
        }
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
