//! MCP Test Server for Meerkat testing
//!
//! Provides simple tools for testing MCP integration:
//! - echo: Returns input as output
//! - add: Adds two numbers
//! - slow: Sleeps for N seconds (for timeout testing)
//! - fail: Always returns an error
//! - env_digest: with `--env-digest NAME`, the FNV-1a digest of the server's
//!   own `NAME` environment value (`absent` without one), so a test can
//!   check what a launch received without the value crossing the protocol
//! - cwd_report: with `--cwd-report FILE`, the server's working directory,
//!   its process id and the contents of `FILE` read relative to that
//!   directory (`null` when it cannot be read), as one JSON text, so a test
//!   can check where a launch runs and which relative file it sees

use schemars::JsonSchema;
use serde_json::{Map, Value, json};
use std::io::{BufRead, BufReader, Write};
use std::time::Duration;

fn schema_for<T: JsonSchema>() -> Value {
    let schema = schemars::schema_for!(T);
    let mut value = serde_json::to_value(&schema).unwrap_or(Value::Null);

    // Some generators omit empty `properties`/`required` for `{}`.
    // Our tool schema contract expects explicit presence of both keys.
    if let Value::Object(ref mut obj) = value
        && obj.get("type").and_then(Value::as_str) == Some("object")
    {
        obj.entry("properties".to_string())
            .or_insert_with(|| Value::Object(Map::new()));
        obj.entry("required".to_string())
            .or_insert_with(|| Value::Array(Vec::new()));
    }

    value
}

#[derive(JsonSchema)]
#[allow(dead_code)]
struct EchoArgs {
    /// Message to echo
    message: String,
}

#[derive(JsonSchema)]
#[allow(dead_code)]
struct AddArgs {
    /// First number
    a: f64,
    /// Second number
    b: f64,
}

#[derive(JsonSchema)]
#[allow(dead_code)]
struct SlowArgs {
    /// Seconds to sleep
    seconds: f64,
}

#[derive(JsonSchema)]
#[allow(dead_code)]
struct EnvDigestArgs {}

#[derive(JsonSchema)]
#[allow(dead_code)]
struct CwdReportArgs {}

#[derive(JsonSchema)]
#[allow(dead_code)]
struct FailArgs {
    /// Error message
    message: String,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::args().nth(1).as_deref() == Some("--form-elicitation") {
        if let Some(path) = std::env::args().nth(2) {
            std::fs::write(path, b"started")?;
        }
        return tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(mcp_test_server::serve_forms_stdio());
    }
    let env_digest = std::env::args()
        .skip_while(|arg| arg != "--env-digest")
        .nth(1);
    let cwd_report = std::env::args()
        .skip_while(|arg| arg != "--cwd-report")
        .nth(1);
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    let reader = BufReader::new(stdin.lock());

    for line in reader.lines() {
        let line = line?;

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
                        "message": format!("Parse error: {e}")
                    }
                });
                writeln!(stdout, "{error}")?;
                stdout.flush()?;
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
                "notifications/initialized" | "notifications/cancelled" => {
                    // Client is ready or request cancelled, nothing to do
                }
                _ => {
                    // Unknown notification, ignore
                    tracing::debug!("Unknown notification: {method}");
                }
            }
            continue;
        }

        // This is a request, send a response
        let response = handle_request(&request, env_digest.as_deref(), cwd_report.as_deref());
        writeln!(stdout, "{response}")?;
        stdout.flush()?;
    }

    Ok(())
}

/// FNV-1a (64-bit) of `value`, as 16 lowercase hex digits.
fn fnv1a(value: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in value.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

fn handle_request(request: &Value, env_digest: Option<&str>, cwd_report: Option<&str>) -> Value {
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
            let mut listed = json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "tools": [
                        {
                            "name": "echo",
                            "description": "Returns the input message as output",
                            "inputSchema": schema_for::<EchoArgs>()
                        },
                        {
                            "name": "add",
                            "description": "Adds two numbers",
                            "inputSchema": schema_for::<AddArgs>()
                        },
                        {
                            "name": "slow",
                            "description": "Sleeps for N seconds",
                            "inputSchema": schema_for::<SlowArgs>()
                        },
                        {
                            "name": "fail",
                            "description": "Always returns an error",
                            "inputSchema": schema_for::<FailArgs>()
                        }
                    ]
                }
            });
            if env_digest.is_some()
                && let Some(tools) = listed["result"]["tools"].as_array_mut()
            {
                tools.push(json!({
                    "name": "env_digest",
                    "description": "Digest of this server's configured environment value",
                    "inputSchema": schema_for::<EnvDigestArgs>()
                }));
            }
            if cwd_report.is_some()
                && let Some(tools) = listed["result"]["tools"].as_array_mut()
            {
                tools.push(json!({
                    "name": "cwd_report",
                    "description": "This server's working directory and the file it reads there",
                    "inputSchema": schema_for::<CwdReportArgs>()
                }));
            }
            listed
        }
        "tools/call" => {
            let params = request.get("params").cloned().unwrap_or(json!({}));
            let tool_name = params.get("name").and_then(|n| n.as_str()).unwrap_or("");
            let arguments = params.get("arguments").cloned().unwrap_or(json!({}));

            let result = match tool_name {
                "echo" => {
                    let message = arguments
                        .get("message")
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
                    let a = arguments
                        .get("a")
                        .and_then(serde_json::Value::as_f64)
                        .unwrap_or(0.0);
                    let b = arguments
                        .get("b")
                        .and_then(serde_json::Value::as_f64)
                        .unwrap_or(0.0);
                    json!({
                        "content": [{
                            "type": "text",
                            "text": format!("{}", a + b)
                        }]
                    })
                }
                "slow" => {
                    let seconds = arguments
                        .get("seconds")
                        .and_then(serde_json::Value::as_f64)
                        .unwrap_or(1.0);
                    std::thread::sleep(Duration::from_secs_f64(seconds));
                    json!({
                        "content": [{
                            "type": "text",
                            "text": format!("Slept for {} seconds", seconds)
                        }]
                    })
                }
                "env_digest" if env_digest.is_some() => {
                    let digest = env_digest
                        .and_then(|name| std::env::var(name).ok())
                        .map_or_else(|| "absent".to_owned(), |value| fnv1a(&value));
                    json!({
                        "content": [{
                            "type": "text",
                            "text": digest
                        }]
                    })
                }
                "cwd_report" if cwd_report.is_some() => {
                    let cwd = std::env::current_dir()
                        .map(|dir| dir.display().to_string())
                        .unwrap_or_default();
                    // Read relative to the working directory, as a server
                    // reading a relative configuration file does.
                    let file = cwd_report.and_then(|name| std::fs::read_to_string(name).ok());
                    json!({
                        "content": [{
                            "type": "text",
                            "text": json!({
                                "cwd": cwd,
                                "pid": std::process::id(),
                                "file": file,
                            })
                            .to_string()
                        }]
                    })
                }
                "fail" => {
                    let message = arguments
                        .get("message")
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
                            "message": format!("Unknown tool: {tool_name}")
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
                    "message": format!("Method not found: {method}")
                }
            })
        }
    }
}
