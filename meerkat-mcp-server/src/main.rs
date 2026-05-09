//! Stdio MCP server for Meerkat.

use clap::{Parser, ValueEnum};
use meerkat::surface::{
    RequestTerminalResolution, StdioJsonWriter, SurfaceRequestExecutor, SurfaceRequestSemantics,
    noop_request_action, spawn_stdio_json_writer,
};
use meerkat_contracts::{ErrorCode, mcp_tool_request_lifecycle};
use meerkat_core::{RealmConfig, RealmSelection, RuntimeBootstrap};
use meerkat_store::RealmBackend;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::io::{self, AsyncBufReadExt, BufReader};
use tokio::sync::mpsc;

#[derive(Parser, Debug)]
#[command(name = "rkat-mcp", version = env!("CARGO_PKG_VERSION"))]
struct Args {
    /// Explicit realm ID. Reuse to share state across processes/surfaces.
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
    /// Expose resolved filesystem paths in config tool responses.
    #[arg(long, default_value_t = false)]
    expose_paths: bool,
    /// Optional rkat-rpc TCP address to delegate realtime bootstrap through.
    #[arg(long)]
    realtime_rpc_tcp: Option<String>,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum RealmBackendArg {
    Jsonl,
    Sqlite,
}

impl From<RealmBackendArg> for RealmBackend {
    fn from(value: RealmBackendArg) -> Self {
        match value {
            RealmBackendArg::Jsonl => RealmBackend::Jsonl,
            RealmBackendArg::Sqlite => RealmBackend::Sqlite,
        }
    }
}

struct ToolCompletion {
    request_key: String,
    success: bool,
    response: Value,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    let selection =
        RealmConfig::selection_from_inputs(args.realm, args.isolated, RealmSelection::Isolated)?;
    let backend_hint = args
        .realm_backend
        .map(Into::into)
        .map(|b: RealmBackend| b.as_str().to_string());
    #[cfg_attr(not(feature = "mob"), allow(unused_mut))]
    let mut state = meerkat_mcp_server::MeerkatMcpState::new_with_bootstrap_and_options(
        RuntimeBootstrap {
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
        },
        args.expose_paths,
    )
    .await?;
    #[cfg(feature = "mob")]
    state.set_realtime_rpc_tcp_addr(args.realtime_rpc_tcp.clone());
    let state = Arc::new(state);

    eprintln!(
        "rkat-mcp starting (realm={}, backend={})",
        state.realm_id(),
        state.backend()
    );

    let stdin = io::stdin();
    let stdout = io::stdout();
    let (writer, writer_task) = spawn_stdio_json_writer(stdout, 128);
    let mut writer_task = Box::pin(writer_task);
    let request_executor = SurfaceRequestExecutor::new(tokio::time::Duration::from_secs(5));
    let (completion_tx, mut completion_rx) = mpsc::channel::<ToolCompletion>(128);
    let mut reader = BufReader::new(stdin).lines();

    let server_result: Result<(), Box<dyn std::error::Error>> = loop {
        tokio::select! {
            line = reader.next_line() => {
                let Some(line) = line? else {
                    break Ok(());
                };
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
                        if writer.send(error).await.is_err() {
                            break Ok(());
                        }
                        continue;
                    }
                };

                let id = request.get("id").cloned();
                let method = request.get("method").and_then(|m| m.as_str()).unwrap_or("");

                if id.is_none() {
                    if method == "notifications/cancelled"
                        && let Some(target) = request_cancel_target(request.get("params"))
                    {
                        let _ = request_executor.cancel_request(&target).await;
                    }
                    continue;
                }

                match method {
                    "initialize" => {
                        let response = json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "result": {
                                "protocolVersion": "2024-11-05",
                                "capabilities": { "tools": {} },
                                "serverInfo": {
                                    "name": "rkat-mcp",
                                    "version": env!("CARGO_PKG_VERSION")
                                }
                            }
                        });
                        if writer.send(response).await.is_err() {
                            break Ok(());
                        }
                    }
                    "ping" => {
                        if writer.send(json!({"jsonrpc": "2.0", "id": id, "result": {}})).await.is_err() {
                            break Ok(());
                        }
                    }
                    "tools/list" => {
                        let response = json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "result": { "tools": meerkat_mcp_server::tools_list() }
                        });
                        if writer.send(response).await.is_err() {
                            break Ok(());
                        }
                    }
                    "tools/call" => {
                        let Some(request_id) = id else {
                            continue;
                        };
                        let request_key = request_key(&request_id);
                        let params = request.get("params").cloned().unwrap_or_else(|| json!({}));
                        let name = params.get("name").and_then(|n| n.as_str()).unwrap_or("").to_string();
                        let arguments = params
                            .get("arguments")
                            .cloned()
                            .unwrap_or_else(|| json!({}));

                        let semantics =
                            SurfaceRequestSemantics::from(mcp_tool_request_lifecycle(&name));
                        let context = request_executor.begin_request_with_semantics(
                            request_key.clone(),
                            noop_request_action(),
                            semantics,
                        );

                        let notifier_writer = writer.clone();
                        let notifier: meerkat_mcp_server::EventNotifier =
                            Arc::new(move |session_id, event| {
                                let session_id = session_id.to_string();
                                let event_value = serde_json::to_value(event).unwrap_or_else(|_| {
                                    json!({
                                        "error": "failed_to_serialize_event"
                                    })
                                });
                                let notification = json!({
                                    "jsonrpc": "2.0",
                                    "method": "notifications/message",
                                    "params": {
                                        "level": "info",
                                        "data": {
                                            "source": "meerkat",
                                            "session_id": session_id,
                                            "event": event_value
                                        }
                                    }
                                });
                                let writer = notifier_writer.clone();
                                tokio::spawn(async move {
                                    let _ = writer.send(notification).await;
                                });
                            });

                        let state = Arc::clone(&state);
                        let completion_tx = completion_tx.clone();
                        let tool_name = name.clone();
                        let request_id_for_task = request_id.clone();
                        let request_key_for_task = request_key.clone();
                        let handle = tokio::spawn(async move {
                            let (success, response) = match meerkat_mcp_server::handle_tools_call_with_notifier(
                                &state,
                                &tool_name,
                                &arguments,
                                Some(notifier),
                                Some(context),
                            ).await {
                                Ok(result) => {
                                    let response = json!({
                                        "jsonrpc": "2.0",
                                        "id": request_id_for_task,
                                        "result": result
                                    });
                                    (true, response)
                                }
                                Err(err) => {
                                    let mut error = json!({
                                        "code": err.code,
                                        "message": err.message
                                    });
                                    if let Some(data) = err.data {
                                        error["data"] = data;
                                    }
                                    (false, json!({
                                        "jsonrpc": "2.0",
                                        "id": request_id_for_task,
                                        "error": error
                                    }))
                                }
                            };
                            let _ = completion_tx
                                .send(ToolCompletion {
                                    request_key: request_key_for_task,
                                    success,
                                    response,
                                })
                                .await;
                        });
                        request_executor.attach_task(&request_key, handle);
                    }
                    _ => {
                        let response = json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "error": {
                                "code": -32601,
                                "message": format!("Method not found: {method}")
                            }
                        });
                        if writer.send(response).await.is_err() {
                            break Ok(());
                        }
                    }
                }
            }
            Some(completion) = completion_rx.recv() => {
                if !write_tool_completion(&writer, &request_executor, completion).await {
                    break Ok(());
                }
            }
            writer_result = &mut writer_task => {
                match writer_result {
                    Ok(Ok(())) => break Ok(()),
                    Ok(Err(_)) | Err(_) => break Ok(()),
                }
            }
        }
    };

    request_executor.shutdown_and_abort_stragglers().await;
    drop(writer);
    let _ = writer_task.await;
    server_result
}

fn request_key(id: &Value) -> String {
    serde_json::to_string(id).unwrap_or_else(|_| id.to_string())
}

fn request_cancel_target(params: Option<&Value>) -> Option<String> {
    let request_id = params?.get("requestId")?;
    Some(request_key(request_id))
}

async fn write_tool_completion(
    writer: &StdioJsonWriter,
    request_executor: &SurfaceRequestExecutor,
    completion: ToolCompletion,
) -> bool {
    let cancel_id = completion.response.get("id").cloned();
    let to_write = match request_executor
        .resolve_admitted_terminal(
            Some(&completion.request_key),
            completion.success,
            completion.response,
        )
        .await
    {
        RequestTerminalResolution::Emit(response) => response,
        RequestTerminalResolution::Cancelled => request_cancelled_response(cancel_id),
        RequestTerminalResolution::LifecycleError(err) => {
            tracing::warn!(
                request_key = %completion.request_key,
                error = %err,
                "request lifecycle rejected publish response"
            );
            request_lifecycle_error_response(cancel_id, err)
        }
    };

    writer.send(to_write).await.is_ok()
}

fn request_cancelled_response(id: Option<Value>) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id.unwrap_or(Value::Null),
        "error": {
            "code": ErrorCode::RequestCancelled.jsonrpc_code(),
            "message": "request cancelled before response publish"
        }
    })
}

fn request_lifecycle_error_response(
    id: Option<Value>,
    err: meerkat::surface::RequestTransitionError,
) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id.unwrap_or(Value::Null),
        "error": {
            "code": -32603,
            "message": format!("request lifecycle rejected publish response: {err}")
        }
    })
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn meerkat_run_and_resume_publish_on_success() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        for (tool_name, expected_cleanup_count) in [
            ("meerkat_run", 0),
            ("meerkat_resume", 0),
            ("meerkat_sessions", 1),
        ] {
            let executor = SurfaceRequestExecutor::new(tokio::time::Duration::from_millis(1));
            let cleanup_count = Arc::new(AtomicUsize::new(0));
            let request_key = format!("{tool_name}-request");
            let context = executor.begin_request_with_semantics(
                request_key.clone(),
                noop_request_action(),
                SurfaceRequestSemantics::from(mcp_tool_request_lifecycle(tool_name)),
            );
            context.set_unpublished_cleanup(meerkat::surface::request_action({
                let cleanup_count = Arc::clone(&cleanup_count);
                move || {
                    let cleanup_count = Arc::clone(&cleanup_count);
                    async move {
                        cleanup_count.fetch_add(1, Ordering::SeqCst);
                    }
                }
            }));

            assert!(matches!(
                executor
                    .resolve_admitted_terminal(Some(context.key()), true, ())
                    .await,
                RequestTerminalResolution::Emit(())
            ));
            assert_eq!(
                cleanup_count.load(Ordering::SeqCst),
                expected_cleanup_count,
                "{tool_name} should let the executor choose publish vs observation"
            );
            assert_eq!(executor.phase(&request_key), None);
        }
    }

    #[tokio::test]
    async fn publish_completion_after_cancel_writes_cancel_response() {
        use tokio::io::AsyncReadExt;

        let (transport, mut output) = tokio::io::duplex(4096);
        let (writer, writer_task) = spawn_stdio_json_writer(transport, 8);
        let request_executor = SurfaceRequestExecutor::new(tokio::time::Duration::from_millis(1));
        let request_id = json!(42);
        let request_key = request_key(&request_id);
        let _ctx = request_executor.begin_request_with_semantics(
            request_key.clone(),
            noop_request_action(),
            SurfaceRequestSemantics::long_running_publish_on_success(),
        );

        assert_eq!(
            request_executor.cancel_request(&request_key).await,
            meerkat::surface::CancelOutcome::Cancelled
        );

        assert!(
            write_tool_completion(
                &writer,
                &request_executor,
                ToolCompletion {
                    request_key: request_key.clone(),
                    success: true,
                    response: json!({
                        "jsonrpc": "2.0",
                        "id": request_id,
                        "result": {"ok": true}
                    }),
                },
            )
            .await
        );

        assert_eq!(request_executor.phase(&request_key), None);
        drop(writer);
        writer_task
            .await
            .expect("writer task should join")
            .expect("writer task should succeed");

        let mut buf = String::new();
        output
            .read_to_string(&mut buf)
            .await
            .expect("output should read");
        let response: Value = serde_json::from_str(buf.trim()).expect("response should parse");
        assert_eq!(response["id"], 42);
        assert_eq!(
            response["error"]["code"],
            ErrorCode::RequestCancelled.jsonrpc_code()
        );
        assert!(response.get("result").is_none());
    }
}
