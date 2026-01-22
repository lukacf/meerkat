//! MCP Test Server for RAIK testing
//!
//! Provides simple tools for testing MCP integration:
//! - echo: Returns input as output
//! - add: Adds two numbers
//! - slow: Sleeps for N seconds (for timeout testing)
//! - fail: Always returns an error

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::time::Duration;

fn main() {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    let reader = BufReader::new(stdin.lock());

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
            // Handle known notifications silently
            let method = request.get("method").and_then(|m| m.as_str()).unwrap_or("");
            match method {
                "notifications/initialized" => {
                    // Client is ready, nothing to do
                }
                "notifications/cancelled" => {
                    // Request cancelled, nothing to do
                }
                _ => {
                    // Unknown notification, ignore
                    eprintln!("Unknown notification: {}", method);
                }
            }
            continue;
        }

        // This is a request, send a response
        let response = handle_request(&request);
        writeln!(stdout, "{}", response).unwrap();
        stdout.flush().unwrap();
    }
}

fn handle_request(request: &Value) -> Value {
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
                        "name": "mcp-test-server",
                        "version": "0.1.0"
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
                    "tools": [
                        {
                            "name": "echo",
                            "description": "Returns the input message as output",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "message": {
                                        "type": "string",
                                        "description": "Message to echo"
                                    }
                                },
                                "required": ["message"]
                            }
                        },
                        {
                            "name": "add",
                            "description": "Adds two numbers",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "a": {
                                        "type": "number",
                                        "description": "First number"
                                    },
                                    "b": {
                                        "type": "number",
                                        "description": "Second number"
                                    }
                                },
                                "required": ["a", "b"]
                            }
                        },
                        {
                            "name": "slow",
                            "description": "Sleeps for N seconds",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "seconds": {
                                        "type": "number",
                                        "description": "Seconds to sleep"
                                    }
                                },
                                "required": ["seconds"]
                            }
                        },
                        {
                            "name": "fail",
                            "description": "Always returns an error",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "message": {
                                        "type": "string",
                                        "description": "Error message"
                                    }
                                },
                                "required": ["message"]
                            }
                        }
                    ]
                }
            })
        }
        "tools/call" => {
            let params = request.get("params").cloned().unwrap_or(json!({}));
            let tool_name = params.get("name").and_then(|n| n.as_str()).unwrap_or("");
            let arguments = params.get("arguments").cloned().unwrap_or(json!({}));

            let result = match tool_name {
                "echo" => {
                    let message = arguments.get("message")
                        .and_then(|m| m.as_str())
                        .unwrap_or("");
                    json!({
                        "content": [{
                            "type": "text",
                            "text": message
                        }]
                    })
                }
                "add" => {
                    let a = arguments.get("a")
                        .and_then(|n| n.as_f64())
                        .unwrap_or(0.0);
                    let b = arguments.get("b")
                        .and_then(|n| n.as_f64())
                        .unwrap_or(0.0);
                    json!({
                        "content": [{
                            "type": "text",
                            "text": format!("{}", a + b)
                        }]
                    })
                }
                "slow" => {
                    let seconds = arguments.get("seconds")
                        .and_then(|n| n.as_f64())
                        .unwrap_or(1.0);
                    std::thread::sleep(Duration::from_secs_f64(seconds));
                    json!({
                        "content": [{
                            "type": "text",
                            "text": format!("Slept for {} seconds", seconds)
                        }]
                    })
                }
                "fail" => {
                    let message = arguments.get("message")
                        .and_then(|m| m.as_str())
                        .unwrap_or("Tool failed");
                    json!({
                        "content": [{
                            "type": "text",
                            "text": message
                        }],
                        "isError": true
                    })
                }
                _ => {
                    return json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": {
                            "code": -32601,
                            "message": format!("Unknown tool: {}", tool_name)
                        }
                    });
                }
            };

            json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": result
            })
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
