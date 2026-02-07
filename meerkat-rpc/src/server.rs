//! RPC server main loop.
//!
//! Wires together the JSON-RPC transport, method router, and notification
//! channel. Uses `tokio::select!` to multiplex incoming requests and
//! outgoing notifications.

use std::sync::Arc;

use tokio::io::{AsyncBufRead, AsyncWrite, BufReader};
use tokio::sync::mpsc;

use meerkat_core::ConfigStore;

use crate::protocol::RpcNotification;
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
        let (notification_tx, notification_rx) = mpsc::channel(256);
        let notification_sink = NotificationSink::new(notification_tx);
        let router = MethodRouter::new(runtime, config_store, notification_sink);
        let transport = JsonlTransport::new(reader, writer);
        Self {
            transport,
            router,
            notification_rx,
        }
    }

    /// Run the server until EOF or a fatal I/O error.
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
                            if let Some(response) = self.router.dispatch(request).await {
                                self.transport.write_response(&response).await?;
                            }
                        }
                        Ok(None) => break, // EOF - clean shutdown
                        Err(TransportError::Parse(err)) => {
                            let response = crate::protocol::RpcResponse::error(
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
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    let reader = BufReader::new(stdin);
    let mut server = RpcServer::new(reader, stdout, runtime, config_store);
    server.run().await
}
