//! Stdio MCP server for Meerkat.

use clap::{Parser, ValueEnum};
use meerkat::surface::{
    RequestAlreadyExists, RequestContext, RequestTerminal, RequestTerminalResolution,
    RequestTransitionError, StdioJsonWriter, SurfaceRequestExecutor, noop_request_action,
    spawn_stdio_json_writer,
};
use meerkat_contracts::ErrorCode;
use meerkat_core::{RealmConfig, RealmSelection, RuntimeBootstrap};
use meerkat_store::RealmBackend;
use serde_json::{Value, json};
use std::future::Future;
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
    terminal: RequestTerminal<Value>,
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
    let request_executor = state.surface_request_executor(tokio::time::Duration::from_secs(5));
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

                        let context =
                            match begin_mcp_tool_request(&request_executor, request_key.clone()) {
                                Ok(context) => context,
                                Err(BeginMcpToolRequestError::AlreadyExists) => {
                                    if writer.send(duplicate_request_response(request_id)).await.is_err() {
                                        break Ok(());
                                    }
                                    continue;
                                }
                                Err(BeginMcpToolRequestError::Lifecycle(err)) => {
                                    if writer
                                        .send(request_lifecycle_authorization_error_response(
                                            Some(request_id),
                                            err,
                                        ))
                                        .await
                                        .is_err()
                                    {
                                        break Ok(());
                                    }
                                    continue;
                                }
                            };
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
                        let handle = spawn_mcp_tool_completion_with_dispatch(
                            completion_tx,
                            request_key.clone(),
                            request_id.clone(),
                            name.clone(),
                            context,
                            move |tool_name, context| async move {
                                meerkat_mcp_server::handle_tools_call_with_notifier(
                                    &state,
                                    &tool_name,
                                    &arguments,
                                    Some(notifier),
                                    Some(context),
                                )
                                .await
                            },
                        );
                        let _ = request_executor.attach_task(&request_key, handle);
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

#[derive(Debug)]
enum BeginMcpToolRequestError {
    AlreadyExists,
    Lifecycle(RequestTransitionError),
}

impl From<RequestAlreadyExists> for BeginMcpToolRequestError {
    fn from(_: RequestAlreadyExists) -> Self {
        Self::AlreadyExists
    }
}

impl From<RequestTransitionError> for BeginMcpToolRequestError {
    fn from(err: RequestTransitionError) -> Self {
        Self::Lifecycle(err)
    }
}

fn begin_mcp_tool_request(
    executor: &SurfaceRequestExecutor,
    request_key: String,
) -> Result<RequestContext, BeginMcpToolRequestError> {
    let context = executor.try_begin_request(request_key, noop_request_action())?;
    context.authorize_cancellable_observation()?;
    Ok(context)
}

fn spawn_mcp_tool_completion_with_dispatch<F, Fut>(
    completion_tx: mpsc::Sender<ToolCompletion>,
    request_key: String,
    request_id: Value,
    tool_name: String,
    context: RequestContext,
    dispatch: F,
) -> tokio::task::JoinHandle<()>
where
    F: FnOnce(String, RequestContext) -> Fut + Send + 'static,
    Fut: Future<Output = Result<Value, meerkat_mcp_server::ToolCallError>> + Send + 'static,
{
    let terminal_context = context.clone();
    tokio::spawn(async move {
        let terminal = match dispatch(tool_name, context).await {
            Ok(result) => {
                let response = json!({
                    "jsonrpc": "2.0",
                    "id": request_id,
                    "result": result
                });
                terminal_context.classify_success_terminal(response)
            }
            Err(err) => {
                let mut error = json!({
                    "code": err.code,
                    "message": err.message
                });
                if let Some(data) = err.data {
                    error["data"] = data;
                }
                terminal_context.classify_failure_terminal(json!({
                    "jsonrpc": "2.0",
                    "id": request_id,
                    "error": error
                }))
            }
        };
        let _ = completion_tx
            .send(ToolCompletion {
                request_key,
                terminal,
            })
            .await;
    })
}

fn duplicate_request_response(id: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": -32600,
            "message": "duplicate in-flight request id"
        }
    })
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
    let cancel_id = completion.terminal.payload().get("id").cloned();
    let to_write = match request_executor
        .resolve_terminal(Some(&completion.request_key), completion.terminal)
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

fn request_lifecycle_authorization_error_response(
    id: Option<Value>,
    err: RequestTransitionError,
) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id.unwrap_or(Value::Null),
        "error": {
            "code": -32603,
            "message": format!("request lifecycle rejected request authorization: {err}")
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn raw_mcp_tool_name_alone_does_not_grant_publish_authority() {
        let executor = SurfaceRequestExecutor::new_standalone(std::time::Duration::from_millis(1));
        let request_id = json!(1);
        let request_key = request_key(&request_id);
        let context = begin_mcp_tool_request(&executor, request_key.clone())
            .expect("first tool request admission should succeed");
        let (completion_tx, mut completion_rx) = mpsc::channel(1);

        let handle = spawn_mcp_tool_completion_with_dispatch(
            completion_tx,
            request_key.clone(),
            request_id,
            "meerkat_run".to_string(),
            context,
            |tool_name, _context| async move {
                assert_eq!(tool_name, "meerkat_run");
                Ok(json!({"ok": true}))
            },
        );

        let completion =
            tokio::time::timeout(std::time::Duration::from_secs(2), completion_rx.recv())
                .await
                .expect("tool completion should arrive")
                .expect("tool completion channel should stay open");
        handle.await.expect("synthetic tool task should complete");
        assert_eq!(completion.request_key, request_key);
        assert!(
            !completion.terminal.is_publish(),
            "raw meerkat_run tool name must not grant committed publish authority before the typed handler marks the request"
        );
    }

    #[test]
    fn mcp_request_context_grants_publish_authority_after_handler_marks_it() {
        let executor = SurfaceRequestExecutor::new_standalone(std::time::Duration::from_millis(1));
        let context = executor
            .try_begin_request("mcp-marked", noop_request_action())
            .expect("test request key should be unique");
        context
            .authorize_publish_on_success()
            .expect("runtime lifecycle authority should accept publish authorization");

        assert!(context.classify_success_terminal(()).is_publish());
        assert!(
            context
                .classify_failure_terminal(())
                .is_respond_without_publish()
        );
    }

    #[test]
    fn mcp_admission_does_not_call_raw_lifecycle_helper() {
        let source = include_str!("main.rs");
        assert!(
            !source.contains(concat!("mcp", "_tool", "_request", "_lifecycle")),
            "MCP tool admission must use machine-owned request classification, not the legacy raw tool-name lifecycle helper"
        );
    }

    #[test]
    fn mcp_tool_admission_rejects_duplicate_in_flight_request_id() {
        let executor = SurfaceRequestExecutor::new_standalone(std::time::Duration::from_millis(1));
        let request_key = request_key(&json!(7));
        let _existing = begin_mcp_tool_request(&executor, request_key.clone())
            .expect("first tool request admission should succeed");
        let duplicate = begin_mcp_tool_request(&executor, request_key.clone());

        assert!(
            duplicate.is_err(),
            "duplicate MCP tool request ids must be rejected at admission"
        );
        assert_eq!(
            executor.phase(&request_key),
            Some(meerkat::surface::SurfaceRequestPhase::Pending),
            "duplicate admission must not overwrite the existing lifecycle entry"
        );
        let response = duplicate_request_response(json!(7));
        assert_eq!(response["error"]["code"], -32600);
    }

    #[tokio::test]
    async fn publish_completion_after_cancel_writes_cancel_response() {
        use tokio::io::AsyncReadExt;

        let (transport, mut output) = tokio::io::duplex(4096);
        let (writer, writer_task) = spawn_stdio_json_writer(transport, 8);
        let request_executor =
            SurfaceRequestExecutor::new_standalone(tokio::time::Duration::from_millis(1));
        let request_id = json!(42);
        let request_key = request_key(&request_id);
        let context = request_executor
            .try_begin_request(request_key.clone(), noop_request_action())
            .expect("test request key should be unique");
        context
            .authorize_publish_on_success()
            .expect("runtime lifecycle authority should accept publish authorization");

        assert_eq!(
            request_executor.cancel_request(&request_key).await,
            meerkat::surface::CancelOutcome::Cancelled
        );
        let terminal = context.classify_success_terminal(json!({
            "jsonrpc": "2.0",
            "id": request_id,
            "result": {"ok": true}
        }));

        assert!(
            write_tool_completion(
                &writer,
                &request_executor,
                ToolCompletion {
                    request_key: request_key.clone(),
                    terminal,
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
