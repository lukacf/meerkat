//! Stdio MCP server for Meerkat Mob orchestration.
//!
//! Exposes mob tools (spec CRUD, activation, runs, meerkats, reconcile,
//! events, capabilities) via the MCP protocol over JSON-RPC on stdin/stdout.

use clap::Parser;
use serde_json::{Value, json};
use tokio::io::{self, AsyncBufReadExt, AsyncWriteExt, BufReader};

#[derive(Parser, Debug)]
#[command(name = "rkat-mob-mcp", about = "MCP server for Meerkat Mob orchestration")]
struct Args {
    /// Realm ID for namespace isolation.
    #[arg(long, default_value = "default")]
    realm: String,
    /// Mob ID for this server instance.
    #[arg(long, default_value = "default")]
    mob_id: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .with_writer(std::io::stderr)
        .init();

    let args = Args::parse();
    let state = meerkat_mob_mcp::MobMcpState::new(&args.realm, &args.mob_id)?;
    eprintln!(
        "rkat-mob-mcp starting (realm={}, mob_id={})",
        args.realm, args.mob_id
    );

    let stdin = io::stdin();
    let mut stdout = io::stdout();
    let mut reader = BufReader::new(stdin).lines();

    while let Some(line) = reader.next_line().await? {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let request: Value = match serde_json::from_str(line) {
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
                stdout
                    .write_all(format!("{error}\n").as_bytes())
                    .await?;
                stdout.flush().await?;
                continue;
            }
        };

        // Notifications don't get responses.
        if request.get("id").is_none() {
            continue;
        }

        let response = handle_request(&state, &request).await;
        stdout
            .write_all(format!("{response}\n").as_bytes())
            .await?;
        stdout.flush().await?;
    }

    Ok(())
}

async fn handle_request(state: &meerkat_mob_mcp::MobMcpState, request: &Value) -> Value {
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    let method = request
        .get("method")
        .and_then(|m| m.as_str())
        .unwrap_or("");

    match method {
        "initialize" => json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "protocolVersion": "2024-11-05",
                "capabilities": { "tools": {} },
                "serverInfo": {
                    "name": "rkat-mob-mcp",
                    "version": env!("CARGO_PKG_VERSION")
                }
            }
        }),
        "ping" => json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {}
        }),
        "tools/list" => {
            let tools = meerkat_mob_mcp::tools_list();
            json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": { "tools": tools }
            })
        }
        "tools/call" => {
            let params = request
                .get("params")
                .cloned()
                .unwrap_or_else(|| json!({}));
            let name = params
                .get("name")
                .and_then(|n| n.as_str())
                .unwrap_or("");
            let arguments = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));

            match meerkat_mob_mcp::handle_tools_call(state, name, &arguments).await {
                Ok(result) => json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": result
                }),
                Err(err) => {
                    let mut error = json!({
                        "code": err.code,
                        "message": err.message
                    });
                    if let Some(data) = err.data {
                        error["data"] = data;
                    }
                    json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": error
                    })
                }
            }
        }
        _ => json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {
                "code": -32601,
                "message": format!("Method not found: {method}")
            }
        }),
    }
}
