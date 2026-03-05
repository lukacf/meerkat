// ═══════════════════════════════════════════════════════════
// force-mcp — Multi-agent MCP server powered by Meerkat mobs
// ═══════════════════════════════════════════════════════════
//
// Exposes two tools to Claude Code (or any MCP client):
//   - `consult`    — single agent, quick opinion
//   - `deliberate` — spawn a mob from a named pack, agents collaborate, structured result
//
// Usage:
//   ANTHROPIC_API_KEY=... cargo run -p force-mcp
//   # Then register in Claude Code via .mcp.json

mod packs;
mod state;
mod tools;

use serde_json::{Value, json};
use state::ForceState;
use tokio::io::{self, AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::OnceCell;

static STATE: OnceCell<ForceState> = OnceCell::const_new();

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "force_mcp=info,warn".parse().unwrap()),
        )
        .with_writer(std::io::stderr)
        .init();

    // Start reading stdin immediately — state is initialized lazily on first tool call.
    // This ensures the MCP handshake (initialize, tools/list) completes instantly.
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    let mut reader = BufReader::new(stdin).lines();

    while let Some(line) = reader.next_line().await? {
        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }

        let request: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                let error = json!({
                    "jsonrpc": "2.0",
                    "id": null,
                    "error": {"code": -32700, "message": format!("Parse error: {e}")}
                });
                stdout
                    .write_all(format!("{error}\n").as_bytes())
                    .await?;
                stdout.flush().await?;
                continue;
            }
        };

        // Notifications (no id) — skip
        if request.get("id").is_none() {
            continue;
        }

        let response = handle_request(&request).await;
        stdout
            .write_all(format!("{response}\n").as_bytes())
            .await?;
        stdout.flush().await?;
    }

    Ok(())
}

/// Get or initialize the server state (lazy — first call creates it).
async fn get_state() -> Result<&'static ForceState, String> {
    STATE
        .get_or_try_init(|| async {
            ForceState::new().map_err(|e| format!("State init failed: {e}"))
        })
        .await
}

/// Dispatch a single MCP JSON-RPC request.
///
/// This is a minimal hand-rolled MCP protocol implementation (no SDK).
/// Targets MCP protocol version 2024-11-05. Supports:
/// - `initialize` / `ping` — handshake (no state needed)
/// - `tools/list` — static tool schema (no state needed)
/// - `tools/call` — lazy-inits ForceState on first use, then dispatches
async fn handle_request(request: &Value) -> Value {
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    let method = request
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or("");

    match method {
        // Handshake — no state needed, responds instantly
        "initialize" => json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "protocolVersion": "2024-11-05",
                "capabilities": { "tools": {} },
                "serverInfo": {
                    "name": "force-mcp",
                    "version": env!("CARGO_PKG_VERSION")
                }
            }
        }),
        "ping" => json!({"jsonrpc": "2.0", "id": id, "result": {}}),
        // Tool listing — no state needed
        "tools/list" => json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": { "tools": tools::tools_list() }
        }),
        // Tool calls — lazy-init state on first use
        "tools/call" => {
            let state = match get_state().await {
                Ok(s) => s,
                Err(e) => {
                    return json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": {"code": -32603, "message": e}
                    });
                }
            };

            let params = request
                .get("params")
                .cloned()
                .unwrap_or_else(|| json!({}));
            let name = params.get("name").and_then(Value::as_str).unwrap_or("");
            let arguments = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            let progress_token = params
                .pointer("/_meta/progressToken")
                .cloned();

            match tools::handle_tool_call(state, name, &arguments, progress_token).await {
                Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
                Err(err) => json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": {"code": err.code, "message": err.message}
                }),
            }
        }
        _ => json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {"code": -32601, "message": format!("Method not found: {method}")}
        }),
    }
}
