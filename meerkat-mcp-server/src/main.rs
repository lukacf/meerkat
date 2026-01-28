//! Meerkat MCP Server
//!
//! Exposes Meerkat agent capabilities as MCP tools:
//! - meerkat_run: Run a new agent with a prompt
//! - meerkat_resume: Resume an existing session

use meerkat_mcp_server::{EventNotifier, handle_tools_call_with_notifier, tools_list};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::sync::{Arc, Mutex, mpsc};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

fn main() {
    init_tracing();

    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    let stdout_lock = Arc::new(Mutex::new(()));
    let (notify_tx, notify_rx) = mpsc::sync_channel::<Value>(256);
    let notify_lock = stdout_lock.clone();
    std::thread::spawn(move || {
        let mut notify_stdout = std::io::stdout();
        while let Ok(message) = notify_rx.recv() {
            let Ok(line) = serde_json::to_string(&message) else {
                continue;
            };
            let _guard = notify_lock.lock().unwrap();
            let _ = writeln!(notify_stdout, "{line}");
            let _ = notify_stdout.flush();
        }
    });
    let notifier: EventNotifier = {
        let notify_tx = notify_tx.clone();
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
            let _ = notify_tx.try_send(payload);
        })
    };
    let reader = BufReader::new(stdin.lock());

    // Initialize runtime for async operations
    let rt = tokio::runtime::Runtime::new().expect("Failed to create Tokio runtime");

    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };

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
                    let _guard = stdout_lock.lock().unwrap();
                    writeln!(stdout, "{}", error).unwrap();
                    stdout.flush().unwrap();
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
                    eprintln!("Unknown notification: {}", method);
                }
            }
            continue;
        }

        // This is a request, send a response
        let response = rt.block_on(handle_request(&request, Some(notifier.clone())));
        {
            let _guard = stdout_lock.lock().unwrap();
            writeln!(stdout, "{}", response).unwrap();
            stdout.flush().unwrap();
        }
    }
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
mod tests {
    use super::init_tracing;

    #[test]
    fn test_tracing_subscriber_initializes() {
        init_tracing();
        assert!(tracing::dispatcher::has_been_set());
    }
}
