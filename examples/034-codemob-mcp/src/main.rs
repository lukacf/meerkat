// ═══════════════════════════════════════════════════════════
// codemob-mcp — Multi-agent MCP server powered by Meerkat mobs
// ═══════════════════════════════════════════════════════════

mod packs;
mod state;
mod tools;

use meerkat::surface::{
    RequestTerminal, SurfaceRequestExecutor, noop_request_action,
    spawn_stdio_json_writer,
};
use serde_json::{Value, json};
use state::ForceState;
use std::sync::Arc;
use tokio::io::{self, AsyncBufReadExt, BufReader};
use tokio::sync::{OnceCell, mpsc};

static STATE: OnceCell<ForceState> = OnceCell::const_new();

struct ToolCompletion {
    request_key: String,
    terminal: RequestTerminal<Value>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "codemob_mcp=info,warn".parse().unwrap()),
        )
        .with_writer(std::io::stderr)
        .init();

    let stdin = io::stdin();
    let stdout = io::stdout();
    let (writer, writer_task) = spawn_stdio_json_writer(stdout, 128);
    let mut writer_task = Box::pin(writer_task);
    let executor = SurfaceRequestExecutor::new(tokio::time::Duration::from_secs(5));
    let (completion_tx, mut completion_rx) = mpsc::channel::<ToolCompletion>(128);
    let mut reader = BufReader::new(stdin).lines();

    let server_result: Result<(), Box<dyn std::error::Error>> = loop {
        tokio::select! {
            line = reader.next_line() => {
                let Some(line) = line? else {
                    break Ok(());
                };
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
                        if writer.send(error).await.is_err() {
                            break Ok(());
                        }
                        continue;
                    }
                };

                let id = request.get("id").cloned();
                let method = request.get("method").and_then(Value::as_str).unwrap_or("");

                if id.is_none() {
                    if method == "notifications/cancelled" {
                        if let Some(target) = request_cancel_target(request.get("params")) {
                            let _ = executor.cancel_request(&target).await;
                        }
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
                                    "name": "codemob-mcp",
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
                            "result": { "tools": tools::tools_list() }
                        });
                        if writer.send(response).await.is_err() {
                            break Ok(());
                        }
                    }
                    "tools/call" => {
                        let request_id = id.expect("validated request id");
                        let request_key = request_key(&request_id);
                        let params = request.get("params").cloned().unwrap_or_else(|| json!({}));
                        let name = params.get("name").and_then(Value::as_str).unwrap_or("").to_string();
                        let arguments = params
                            .get("arguments")
                            .cloned()
                            .unwrap_or_else(|| json!({}));
                        let progress_token = params.pointer("/_meta/progressToken").cloned();

                        let context = executor.begin_request(request_key.clone(), noop_request_action());
                        let writer_for_progress = writer.clone();
                        let progress_notifier: tools::ProgressNotifier = Arc::new(move |token, progress, total, message| {
                            let writer = writer_for_progress.clone();
                            tokio::spawn(async move {
                                let notification = json!({
                                    "jsonrpc": "2.0",
                                    "method": "notifications/progress",
                                    "params": {
                                        "progressToken": token,
                                        "progress": progress,
                                        "total": total,
                                        "message": message,
                                    }
                                });
                                let _ = writer.send(notification).await;
                            });
                        });

                        let completion_tx = completion_tx.clone();
                        let tool_name = name.clone();
                        let request_id_for_task = request_id.clone();
                        let request_key_for_task = request_key.clone();
                        let handle = tokio::spawn(async move {
                            let terminal = match get_state().await {
                                Ok(state) => match tools::handle_tool_call(
                                    state,
                                    &tool_name,
                                    &arguments,
                                    progress_token,
                                    Some(progress_notifier),
                                    Some(context),
                                ).await {
                                    Ok(result) => RequestTerminal::RespondWithoutPublish(json!({
                                        "jsonrpc": "2.0",
                                        "id": request_id_for_task,
                                        "result": result
                                    })),
                                    Err(err) => RequestTerminal::RespondWithoutPublish(json!({
                                        "jsonrpc": "2.0",
                                        "id": request_id_for_task,
                                        "error": {"code": err.code, "message": err.message}
                                    })),
                                },
                                Err(err) => RequestTerminal::RespondWithoutPublish(json!({
                                    "jsonrpc": "2.0",
                                    "id": request_id_for_task,
                                    "error": {"code": -32603, "message": err}
                                })),
                            };
                            let _ = completion_tx
                                .send(ToolCompletion { request_key: request_key_for_task, terminal })
                                .await;
                        });
                        executor.attach_task(&request_key, handle);
                    }
                    _ => {
                        let response = json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "error": {"code": -32601, "message": format!("Method not found: {method}")}
                        });
                        if writer.send(response).await.is_err() {
                            break Ok(());
                        }
                    }
                }
            }
            Some(completion) = completion_rx.recv() => {
                let terminal = if executor.cancel_requested(&completion.request_key) {
                    RequestTerminal::RespondWithoutPublish(
                        request_cancelled_response(request_id_from_terminal(&completion.terminal))
                    )
                } else {
                    completion.terminal
                };

                match terminal {
                    RequestTerminal::Publish(response) => {
                        if writer.send(response).await.is_ok() {
                            executor.mark_published(&completion.request_key);
                            executor.remove_published(&completion.request_key);
                        } else {
                            executor.finish_unpublished(&completion.request_key).await;
                            break Ok(());
                        }
                    }
                    RequestTerminal::RespondWithoutPublish(response) => {
                        let send_result = writer.send(response).await;
                        executor.finish_unpublished(&completion.request_key).await;
                        if send_result.is_err() {
                            break Ok(());
                        }
                    }
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

    executor.shutdown_and_abort_stragglers().await;
    drop(writer);
    let _ = writer_task.await;
    server_result
}

async fn get_state() -> Result<&'static ForceState, String> {
    STATE
        .get_or_try_init(|| async {
            ForceState::new().map_err(|e| format!("State init failed: {e}"))
        })
        .await
}

fn request_key(id: &Value) -> String {
    serde_json::to_string(id).unwrap_or_else(|_| id.to_string())
}

fn request_cancel_target(params: Option<&Value>) -> Option<String> {
    let request_id = params?.get("requestId")?;
    Some(request_key(request_id))
}

fn request_id_from_terminal(terminal: &RequestTerminal<Value>) -> Option<Value> {
    let response = match terminal {
        RequestTerminal::Publish(response) | RequestTerminal::RespondWithoutPublish(response) => {
            response
        }
    };
    response.get("id").cloned()
}

fn request_cancelled_response(id: Option<Value>) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id.unwrap_or(Value::Null),
        "error": {
            "code": -32005,
            "message": "request cancelled before response publish"
        }
    })
}
