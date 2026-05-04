//! Localhost loopback OAuth callback server.
//!
//! The host opens the authorize URL in the user's browser; the browser
//! redirects to the configured loopback callback URL. Anthropic and Gemini use
//! ephemeral loopback ports; OpenAI ChatGPT mirrors Codex's fixed localhost
//! callback contract. This module parses the query, validates the state, and
//! returns the code to the caller.
//!
//! Reference-CLI parity: Codex `codex-rs/login/src/server.rs`, Gemini CLI
//! `packages/core/src/code_assist/oauth2.ts:113-360`.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse};
use axum::routing::get;
use parking_lot::Mutex;
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::time::timeout;

use super::OAuthError;

#[derive(Debug, Clone)]
pub struct LoopbackOutcome {
    pub code: String,
    pub state: String,
}

pub struct LoopbackHandle {
    pub redirect_url: String,
    shutdown: oneshot::Sender<()>,
    receiver: oneshot::Receiver<Result<LoopbackOutcome, OAuthError>>,
}

pub struct LoopbackBinding {
    pub redirect_url: String,
    shutdown: oneshot::Sender<()>,
    receiver: oneshot::Receiver<Result<LoopbackOutcome, OAuthError>>,
    expected_state: Arc<Mutex<Option<String>>>,
}

impl LoopbackBinding {
    pub fn expect_state(self, expected_state: String) -> LoopbackHandle {
        *self.expected_state.lock() = Some(expected_state);
        LoopbackHandle {
            redirect_url: self.redirect_url,
            shutdown: self.shutdown,
            receiver: self.receiver,
        }
    }
}

impl LoopbackHandle {
    /// Await the callback outcome. Fires once on first valid hit; fails
    /// with `StateMismatch` if state doesn't match, `CallbackParse` if the
    /// query is malformed, `Timeout` after `deadline`.
    pub async fn wait(self, deadline: Duration) -> Result<LoopbackOutcome, OAuthError> {
        let LoopbackHandle {
            shutdown,
            receiver,
            redirect_url: _,
        } = self;
        let outcome = match timeout(deadline, receiver).await {
            Ok(Ok(outcome)) => outcome,
            Ok(Err(_)) => Err(OAuthError::CallbackParse("receiver closed".into())),
            Err(_) => Err(OAuthError::Timeout),
        };
        let _ = shutdown.send(());
        outcome
    }
}

type ResultSender = oneshot::Sender<Result<LoopbackOutcome, OAuthError>>;

#[derive(Clone)]
struct CallbackState {
    expected_state: Arc<Mutex<Option<String>>>,
    result_tx: Arc<Mutex<Option<ResultSender>>>,
}

async fn callback_handler(
    State(state): State<CallbackState>,
    Query(params): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    let Some(expected_state) = state.expected_state.lock().clone() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Html("<h1>Authorization callback is not ready</h1>".to_string()),
        );
    };
    let tx = state.result_tx.lock().take();
    match (
        params.get("code"),
        params.get("state"),
        params.get("error"),
        tx,
    ) {
        (_, _, Some(err), Some(tx)) => {
            let err = if err == "access_denied" {
                OAuthError::UserDenied
            } else {
                OAuthError::CallbackParse(format!("provider error: {err}"))
            };
            let _ = tx.send(Err(err));
            (
                StatusCode::BAD_REQUEST,
                Html("<h1>Authorization error</h1>".to_string()),
            )
        }
        (Some(code), Some(actual_state), _, Some(tx)) => {
            if actual_state == &expected_state {
                let _ = tx.send(Ok(LoopbackOutcome {
                    code: code.clone(),
                    state: actual_state.clone(),
                }));
                (
                    StatusCode::OK,
                    Html(
                        "<h1>Authorization complete</h1><p>You may close this window.</p>"
                            .to_string(),
                    ),
                )
            } else {
                let _ = tx.send(Err(OAuthError::StateMismatch));
                (
                    StatusCode::BAD_REQUEST,
                    Html("<h1>State mismatch</h1>".to_string()),
                )
            }
        }
        _ => (
            StatusCode::BAD_REQUEST,
            Html("<h1>Invalid callback</h1>".to_string()),
        ),
    }
}

/// Bind a loopback listener and return a `LoopbackHandle`. The caller
/// opens `handle.redirect_url` (with the right query params) in the
/// browser; `handle.wait(deadline)` returns the `(code, state)` pair or
/// an `OAuthError`.
pub async fn run_loopback_callback(
    expected_state: String,
    path: &str,
) -> Result<LoopbackHandle, OAuthError> {
    Ok(bind_loopback_callback(path)
        .await?
        .expect_state(expected_state))
}

/// Bind a loopback listener before the OAuth authority has minted state.
///
/// Some callers need the redirect URL before state can be admitted. They bind
/// first, use `redirect_url` to ask their authority to start the flow, then
/// call [`LoopbackBinding::expect_state`] before opening the browser.
pub async fn bind_loopback_callback(path: &str) -> Result<LoopbackBinding, OAuthError> {
    bind_loopback_callback_with_redirect(path, "127.0.0.1", &[0]).await
}

/// Bind a loopback listener while advertising a provider-specific redirect host
/// and preferred ports. A port value of `0` asks the OS for an ephemeral port.
pub async fn bind_loopback_callback_with_redirect(
    path: &str,
    redirect_host: &str,
    preferred_ports: &[u16],
) -> Result<LoopbackBinding, OAuthError> {
    let (result_tx, result_rx) = oneshot::channel();
    let expected_state = Arc::new(Mutex::new(None));
    let state = CallbackState {
        expected_state: Arc::clone(&expected_state),
        result_tx: Arc::new(Mutex::new(Some(result_tx))),
    };

    let app = Router::new()
        .route(path, get(callback_handler))
        .with_state(state);
    let ports = if preferred_ports.is_empty() {
        &[0][..]
    } else {
        preferred_ports
    };
    let mut last_error = None;
    let mut listener = None;
    for port in ports {
        let bind_addr = format!("127.0.0.1:{port}");
        match TcpListener::bind(&bind_addr).await {
            Ok(bound) => {
                listener = Some(bound);
                break;
            }
            Err(err) => {
                last_error = Some(format!("{bind_addr}: {err}"));
            }
        }
    }
    let listener = listener.ok_or_else(|| {
        OAuthError::InvalidConfig(format!(
            "bind: {}",
            last_error.unwrap_or_else(|| "no callback ports configured".to_string())
        ))
    })?;
    let addr = listener
        .local_addr()
        .map_err(|e| OAuthError::InvalidConfig(format!("addr: {e}")))?;
    let redirect_url = format!("http://{}:{}{}", redirect_host, addr.port(), path);

    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                let _ = shutdown_rx.await;
            })
            .await;
    });

    Ok(LoopbackBinding {
        redirect_url,
        shutdown: shutdown_tx,
        receiver: result_rx,
        expected_state,
    })
}
