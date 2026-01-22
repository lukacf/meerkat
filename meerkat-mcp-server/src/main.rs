//! Meerkat MCP Server
//!
//! Exposes Meerkat agent capabilities as MCP tools:
//! - meerkat_run: Run a new agent with a prompt
//! - meerkat_resume: Resume an existing session

use meerkat_mcp_server::{handle_tools_call, tools_list};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};

fn main() {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
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
                writeln!(stdout, "{}", error).unwrap();
                stdout.flush().unwrap();
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
        let response = rt.block_on(handle_request(&request));
        writeln!(stdout, "{}", response).unwrap();
        stdout.flush().unwrap();
    }
}

async fn handle_request(request: &Value) -> Value {
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

            match handle_tools_call(tool_name, &arguments).await {
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
