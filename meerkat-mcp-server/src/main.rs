//! Meerkat MCP Server
//!
//! Exposes Meerkat agent capabilities as MCP tools:
//! - meerkat_run: Run a new agent with a prompt
//! - meerkat_resume: Resume an existing session

use meerkat_mcp_server::{EventNotifier, handle_tools_call_with_notifier, tools_list};
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    init_tracing();

    let (out_tx, mut out_rx) = mpsc::channel::<String>(256);
    let (shutdown_tx, mut shutdown_rx) = oneshot::channel::<()>();

    let writer_handle = tokio::spawn(async move {
        let mut stdout = tokio::io::stdout();

        loop {
            tokio::select! {
                maybe_line = out_rx.recv() => {
                    let Some(line) = maybe_line else {
                        break;
                    };

                    if stdout.write_all(line.as_bytes()).await.is_err() {
                        break;
                    }
                    if stdout.write_all(b"\n").await.is_err() {
                        break;
                    }
                    let _ = stdout.flush().await;
                }
                _ = &mut shutdown_rx => {
                    while let Ok(line) = out_rx.try_recv() {
                        if stdout.write_all(line.as_bytes()).await.is_err() {
                            return;
                        }
                        if stdout.write_all(b"\n").await.is_err() {
                            return;
                        }
                        let _ = stdout.flush().await;
                    }
                    break;
                }
            }
        }
    });

    let notifier: EventNotifier = {
        let out_tx = out_tx.clone();
        Arc::new(move |session_id, event| {
            let event_value =
                serde_json::to_value(event).unwrap_or_else(|_| json!({ "type": "unknown" }));
            let payload = json!({
                "jsonrpc": "2.0",
                "method": "notifications/meerkat_event",
                "params": {
                    "session_id": session_id,
                    "event": event_value
                }
            });
            let _ = out_tx.try_send(payload.to_string());
        })
    };
    let stdin = tokio::io::stdin();
    let mut reader = BufReader::new(stdin).lines();

    while let Some(line) = reader.next_line().await? {
        if line.is_empty() {
            continue;
        }

        let request: Value = match serde_json::from_str(&line) {
            Ok(r) => r,
            Err(e) => {
                let error = json!({
                    "jsonrpc": "2.0",
                    "id": null,
                    "error": {
                        "code": -32700,
                        "message": format!("Parse error: {}", e)
                    }
                });
                {
                    if out_tx.send(error.to_string()).await.is_err() {
                        break;
                    }
                }
                continue;
            }
        };

        // Check if this is a notification (no id field)
        let is_notification = request.get("id").is_none();

        if is_notification {
            // Notifications don't get responses
            let method = request.get("method").and_then(|m| m.as_str()).unwrap_or("");
            match method {
                "notifications/initialized" | "notifications/cancelled" => {}
                _ => {
                    tracing::warn!("Unknown notification: {}", method);
                }
            }
            continue;
        }

        // This is a request, send a response
        let response = handle_request(&request, Some(notifier.clone())).await;
        if out_tx.send(response.to_string()).await.is_err() {
            break;
        }
    }

    let _ = shutdown_tx.send(());
    let _ = writer_handle.await;
    Ok(())
}

fn init_tracing() {
    let subscriber = tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .with(tracing_subscriber::fmt::layer());

    let _ = subscriber.try_init();
}

async fn handle_request(request: &Value, notifier: Option<EventNotifier>) -> Value {
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    let method = request.get("method").and_then(|m| m.as_str()).unwrap_or("");

    match method {
        "initialize" => {
            json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "protocolVersion": "2024-11-05",
                    "capabilities": {
                        "tools": {}
                    },
                    "serverInfo": {
                        "name": "meerkat-mcp-server",
                        "version": env!("CARGO_PKG_VERSION")
                    }
                }
            })
        }
        "ping" => {
            json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {}
            })
        }
        "tools/list" => {
            json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "tools": tools_list()
                }
            })
        }
        "tools/call" => {
            let params = request.get("params").cloned().unwrap_or(json!({}));
            let tool_name = params.get("name").and_then(|n| n.as_str()).unwrap_or("");
            let arguments = params.get("arguments").cloned().unwrap_or(json!({}));

            match handle_tools_call_with_notifier(tool_name, &arguments, notifier).await {
                Ok(result) => {
                    json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": result
                    })
                }
                Err(e) => {
                    json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": {
                            "code": -32000,
                            "message": e
                        }
                    })
                }
            }
        }
        _ => {
            json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": {
                    "code": -32601,
                    "message": format!("Method not found: {}", method)
                }
            })
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::init_tracing;

    #[test]
    fn test_tracing_subscriber_initializes() {
        init_tracing();
        assert!(tracing::dispatcher::has_been_set());
    }
}
