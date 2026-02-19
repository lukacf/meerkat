use clap::{Parser, ValueEnum};
use meerkat_core::{RealmConfig, RealmSelection, RuntimeBootstrap};
use meerkat_store::RealmBackend;
use serde_json::{Value, json};
use std::path::PathBuf;
use tokio::io::{self, AsyncBufReadExt, AsyncWriteExt, BufReader};

#[derive(Parser, Debug)]
#[command(name = "rkat-mob-mcp")]
struct Args {
    /// Explicit realm ID. Reuse to share state across surfaces.
    #[arg(long)]
    realm: Option<String>,

    /// Start in isolated mode (new generated realm).
    #[arg(long)]
    isolated: bool,

    /// Optional instance ID for this server process.
    #[arg(long)]
    instance: Option<String>,

    /// Realm backend when creating a new realm.
    #[arg(long, value_enum)]
    realm_backend: Option<RealmBackendArg>,

    /// Optional override for realm state root.
    #[arg(long)]
    state_root: Option<PathBuf>,

    /// Optional context root for filesystem conventions.
    #[arg(long)]
    context_root: Option<PathBuf>,

    /// Optional user-level config root for additive conventions.
    #[arg(long)]
    user_config_root: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum RealmBackendArg {
    Redb,
}

impl From<RealmBackendArg> for RealmBackend {
    fn from(value: RealmBackendArg) -> Self {
        match value {
            RealmBackendArg::Redb => RealmBackend::Redb,
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    let selection =
        RealmConfig::selection_from_inputs(args.realm, args.isolated, RealmSelection::Isolated)?;

    let backend_hint = args
        .realm_backend
        .map(Into::into)
        .map(|backend: RealmBackend| backend.as_str().to_string());

    let bootstrap = RuntimeBootstrap {
        realm: RealmConfig {
            selection,
            instance_id: args.instance,
            backend_hint,
            state_root: args.state_root,
        },
        context: meerkat_core::ContextConfig {
            context_root: args.context_root,
            user_config_root: args.user_config_root,
        },
    };

    let state = meerkat_mob_mcp::MobMcpState::new_with_bootstrap(bootstrap).await?;
    eprintln!(
        "rkat-mob-mcp starting (realm={}, backend={})",
        state.realm_id(),
        state.backend(),
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
            Ok(value) => value,
            Err(err) => {
                let response = json!({
                    "jsonrpc": "2.0",
                    "id": null,
                    "error": {
                        "code": -32700,
                        "message": format!("Parse error: {err}"),
                    }
                });
                stdout.write_all(format!("{response}\n").as_bytes()).await?;
                stdout.flush().await?;
                continue;
            }
        };

        if request.get("id").is_none() {
            continue;
        }

        let response = handle_request(&state, &request).await;
        stdout.write_all(format!("{response}\n").as_bytes()).await?;
        stdout.flush().await?;
    }

    Ok(())
}

async fn handle_request(state: &meerkat_mob_mcp::MobMcpState, request: &Value) -> Value {
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    let method = request.get("method").and_then(Value::as_str).unwrap_or("");

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
        "tools/list" => json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "tools": meerkat_mob_mcp::tools_list(),
            }
        }),
        "tools/call" => {
            let params = request.get("params").cloned().unwrap_or_else(|| json!({}));
            let name = params.get("name").and_then(Value::as_str).unwrap_or("");
            let arguments = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));

            match meerkat_mob_mcp::handle_tools_call(state, name, &arguments).await {
                Ok(result) => json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": result,
                }),
                Err(err) => {
                    let mut error = json!({
                        "code": err.code,
                        "message": err.message,
                    });
                    if let Some(data) = err.data {
                        error["data"] = data;
                    }

                    json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": error,
                    })
                }
            }
        }
        _ => json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {
                "code": -32601,
                "message": format!("Method not found: {method}"),
            }
        }),
    }
}
