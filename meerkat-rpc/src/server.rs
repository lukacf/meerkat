//! RPC server main loop.
//!
//! Wires together the JSON-RPC transport, method router, and notification
//! channel. Uses `tokio::select!` to multiplex incoming requests, outgoing
//! responses from spawned dispatch tasks, and event notifications.

use std::sync::Arc;

use tokio::io::{AsyncBufRead, AsyncWrite, BufReader};
use tokio::sync::mpsc;

use meerkat_core::ConfigStore;

use crate::NOTIFICATION_CHANNEL_CAPACITY;
use crate::protocol::{RpcNotification, RpcResponse};
use crate::router::{MethodRouter, NotificationSink};
use crate::session_runtime::SessionRuntime;
use crate::transport::{JsonlTransport, TransportError};

/// Errors from the RPC server.
#[derive(Debug, thiserror::Error)]
pub enum ServerError {
    /// Transport-level error.
    #[error("Transport error: {0}")]
    Transport(#[from] TransportError),
    /// I/O error.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

/// JSON-RPC server over async reader/writer streams.
pub struct RpcServer<R, W> {
    transport: JsonlTransport<R, W>,
    router: MethodRouter,
    notification_rx: mpsc::Receiver<RpcNotification>,
    /// Channel for responses from concurrently dispatched requests.
    response_rx: mpsc::Receiver<RpcResponse>,
    response_tx: mpsc::Sender<RpcResponse>,
}

impl<R: AsyncBufRead + Unpin, W: AsyncWrite + Unpin> RpcServer<R, W> {
    /// Create a new RPC server.
    ///
    /// The server reads JSON-RPC requests from `reader`, dispatches them
    /// via the `MethodRouter`, and writes responses/notifications to `writer`.
    pub fn new(
        reader: R,
        writer: W,
        runtime: Arc<SessionRuntime>,
        config_store: Arc<dyn ConfigStore>,
    ) -> Self {
        Self::new_with_skill_runtime(reader, writer, runtime, config_store, None)
    }

    /// Create a new RPC server with an optional skill runtime for introspection.
    pub fn new_with_skill_runtime(
        reader: R,
        writer: W,
        runtime: Arc<SessionRuntime>,
        config_store: Arc<dyn ConfigStore>,
        skill_runtime: Option<Arc<meerkat_core::skills::SkillRuntime>>,
    ) -> Self {
        let (notification_tx, notification_rx) = mpsc::channel(NOTIFICATION_CHANNEL_CAPACITY);
        let notification_sink = NotificationSink::new(notification_tx);
        let router =
            MethodRouter::new(runtime, config_store, notification_sink)
                .with_skill_runtime(skill_runtime);
        let transport = JsonlTransport::new(reader, writer);
        let (response_tx, response_rx) = mpsc::channel(NOTIFICATION_CHANNEL_CAPACITY);
        Self {
            transport,
            router,
            notification_rx,
            response_rx,
            response_tx,
        }
    }

    /// Run the server until EOF or a fatal I/O error.
    ///
    /// Requests are dispatched concurrently in spawned tasks so that long-running
    /// operations (e.g. `session/create`, `turn/start`) do not block the server
    /// from processing other requests (e.g. `turn/interrupt`).
    ///
    /// Parse errors are reported to the client and do not terminate the loop.
    /// EOF (reader returns `None`) triggers graceful shutdown.
    pub async fn run(&mut self) -> Result<(), ServerError> {
        loop {
            tokio::select! {
                biased;

                // Read the next request from the transport.
                msg = self.transport.read_message() => {
                    match msg {
                        Ok(Some(request)) => {
                            let router = self.router.clone();
                            let resp_tx = self.response_tx.clone();
                            tokio::spawn(async move {
                                if let Some(response) = router.dispatch(request).await {
                                    let _ = resp_tx.send(response).await;
                                }
                            });
                        }
                        Ok(None) => break, // EOF - clean shutdown
                        Err(TransportError::Parse(err)) => {
                            let response = RpcResponse::error(
                                None,
                                crate::error::PARSE_ERROR,
                                format!("Parse error: {err}"),
                            );
                            self.transport.write_response(&response).await?;
                        }
                        Err(TransportError::Io(err)) => {
                            return Err(ServerError::Io(err));
                        }
                    }
                }

                // Write responses from completed dispatch tasks.
                Some(response) = self.response_rx.recv() => {
                    self.transport.write_response(&response).await?;
                }

                // Forward queued notifications to the transport.
                Some(notification) = self.notification_rx.recv() => {
                    self.transport.write_notification(&notification).await?;
                }
            }
        }

        // Graceful shutdown: close all sessions.
        self.router.runtime().shutdown().await;
        Ok(())
    }
}

/// Start the RPC server on stdin/stdout.
///
/// This is the main entry point called from the CLI `rpc` subcommand.
pub async fn serve_stdio(
    runtime: Arc<SessionRuntime>,
    config_store: Arc<dyn ConfigStore>,
) -> Result<(), ServerError> {
    serve_stdio_with_skill_runtime(runtime, config_store, None).await
}

/// Start the RPC server on stdin/stdout with an optional skill runtime.
pub async fn serve_stdio_with_skill_runtime(
    runtime: Arc<SessionRuntime>,
    config_store: Arc<dyn ConfigStore>,
    skill_runtime: Option<Arc<meerkat_core::skills::SkillRuntime>>,
) -> Result<(), ServerError> {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    let reader = BufReader::new(stdin);
    let mut server =
        RpcServer::new_with_skill_runtime(reader, stdout, runtime, config_store, skill_runtime);
    server.run().await
}
