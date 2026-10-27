//! Module process boundary — MCP connections, environment setup, and tool probing.

use std::collections::HashMap;
use std::time::Duration;

use meerkat_core::ContentBlock;
use meerkat_core::McpServerConfig;
use meerkat_mcp::{McpConnection, McpError, McpServerCloseOutcome};

use crate::member_mcp_custody::unsettled_class;
use serde_json::Value;
use tokio::time::timeout;

use super::*;

pub(super) const MODULE_BOUNDARY_KIND_ENV: &str = "META_MODULE_BOUNDARY";
pub(super) const MODULE_BOUNDARY_KIND_MCP: &str = "mcp";
pub(super) const CORE_MODULE_MCP_TIMEOUT: Duration = Duration::from_secs(5);
pub(super) const ROUTER_RESOLVE_MCP_TOOL: &str = "routing.resolve";
pub(super) const DELIVERY_SEND_MCP_TOOL: &str = "delivery.send";
pub(super) const MEMORY_CONFLICT_READ_MCP_TOOL: &str = "memory.conflict_read";

pub(super) fn module_uses_mcp(module: &ModuleConfig, pre_spawn: Option<&PreSpawnData>) -> bool {
    pre_spawn
        .filter(|data| data.module_id == module.id)
        .and_then(|data| {
            data.env
                .iter()
                .find(|(key, _)| key == MODULE_BOUNDARY_KIND_ENV)
                .map(|(_, value)| value)
        })
        .map(|value| value.trim().eq_ignore_ascii_case(MODULE_BOUNDARY_KIND_MCP))
        .unwrap_or(false)
}

pub(super) fn module_env_with_extra(
    module: &ModuleConfig,
    pre_spawn: Option<&PreSpawnData>,
    extra_env: &[(String, String)],
) -> Vec<(String, String)> {
    pre_spawn
        .filter(|data| data.module_id == module.id)
        .map(|data| data.env.clone())
        .unwrap_or_default()
        .into_iter()
        .chain(extra_env.iter().cloned())
        .collect::<Vec<_>>()
}

pub(super) fn mcp_required_error(module_id: &str, flow: &str) -> RuntimeBoundaryError {
    RuntimeBoundaryError::Mcp(McpBoundaryError::McpRequired {
        module_id: module_id.to_string(),
        flow: flow.to_string(),
    })
}

pub(super) fn probe_module_mcp_tools(
    module: &ModuleConfig,
    pre_spawn: Option<&PreSpawnData>,
    timeout_duration: Duration,
) -> Result<Vec<String>, RuntimeBoundaryError> {
    let env = module_env_with_extra(module, pre_spawn, &[]);
    let env_map = env.into_iter().collect::<HashMap<_, _>>();
    let config = McpServerConfig::stdio(
        format!("meta-{}", module.id),
        module.command.clone(),
        module.args.clone(),
        env_map,
    );
    let module_id = module.id.clone();

    run_in_tokio_runtime(async move {
        let connection = connect_within(&module_id, &config, timeout_duration).await?;
        let tools_result = list_tools_with_timeout(&module_id, &connection, timeout_duration).await;
        let close_result = close_settled(&module_id, connection).await;
        let tools = finalize_mcp_operation_with_close(tools_result, close_result)?;
        let mut tool_names = tools
            .into_iter()
            .map(|tool| tool.name.into_string())
            .collect::<Vec<String>>();
        tool_names.sort();
        Ok(tool_names)
    })
}

pub(super) fn call_module_mcp_tool_text(
    module: &ModuleConfig,
    pre_spawn: Option<&PreSpawnData>,
    tool_name: &str,
    args: &Value,
    timeout_duration: Duration,
) -> Result<String, RuntimeBoundaryError> {
    let env = module_env_with_extra(module, pre_spawn, &[]);
    let env_map = env.into_iter().collect::<HashMap<_, _>>();
    let config = McpServerConfig::stdio(
        format!("meta-{}", module.id),
        module.command.clone(),
        module.args.clone(),
        env_map,
    );
    let module_id = module.id.clone();
    let requested_tool = tool_name.to_string();
    let args = args.clone();

    run_in_tokio_runtime(async move {
        let connection = connect_within(&module_id, &config, timeout_duration).await?;
        let operation_result = async {
            let tools = list_tools_with_timeout(&module_id, &connection, timeout_duration).await?;
            let available_tools = tools
                .iter()
                .map(|tool| tool.name.as_str().to_string())
                .collect::<Vec<String>>();
            if !available_tools.iter().any(|name| name == &requested_tool) {
                return Err(RuntimeBoundaryError::Mcp(McpBoundaryError::ToolNotFound {
                    module_id: module_id.clone(),
                    tool: requested_tool.clone(),
                    available_tools,
                }));
            }
            call_tool_with_timeout(
                &module_id,
                &requested_tool,
                &connection,
                &args,
                timeout_duration,
            )
            .await
        }
        .await;

        let close_result = close_settled(&module_id, connection).await;
        finalize_mcp_operation_with_close(operation_result, close_result)
    })
}

pub(super) fn call_module_mcp_tool_json(
    module: &ModuleConfig,
    pre_spawn: Option<&PreSpawnData>,
    tool_name: &str,
    args: &Value,
    timeout_duration: Duration,
) -> Result<Value, RuntimeBoundaryError> {
    let response = call_module_mcp_tool_text(module, pre_spawn, tool_name, args, timeout_duration)?;
    serde_json::from_str::<Value>(&response).map_err(|_| {
        RuntimeBoundaryError::Mcp(McpBoundaryError::InvalidJsonResponse {
            module_id: module.id.clone(),
            tool: tool_name.to_string(),
            response,
        })
    })
}

fn run_in_tokio_runtime<F, T>(future: F) -> Result<T, RuntimeBoundaryError>
where
    F: std::future::Future<Output = Result<T, RuntimeBoundaryError>>,
{
    if tokio::runtime::Handle::try_current().is_ok() {
        return Err(RuntimeBoundaryError::Mcp(
            McpBoundaryError::RuntimeUnavailable(
                "cannot execute blocking MCP boundary call inside an active tokio runtime"
                    .to_string(),
            ),
        ));
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| {
            RuntimeBoundaryError::Mcp(McpBoundaryError::RuntimeUnavailable(error.to_string()))
        })?;
    runtime.block_on(future)
}

/// Connect within `timeout_duration`. Meerkat owns the bound, so a server it
/// spawned for a failed or timed-out connect is closed through its custody,
/// and that close is joined before this returns, inside this call's runtime;
/// a close that did not settle joins the connect failure. A call's total bound
/// is its connect, list and call timeouts plus one stdio close bound.
async fn connect_within(
    module_id: &str,
    config: &McpServerConfig,
    timeout_duration: Duration,
) -> Result<McpConnection, RuntimeBoundaryError> {
    match McpConnection::connect_within(config, timeout_duration).await {
        Ok(connection) => Ok(connection),
        Err(failure) => {
            let primary = if failure.timed_out {
                mcp_timeout_error(module_id, "connect", timeout_duration)
            } else {
                mcp_connect_error(module_id, failure.error)
            };
            match failure
                .close
                .as_ref()
                .and_then(|close| close_failure(module_id, close))
            {
                Some(close) => Err(attach_mcp_close_failure(primary, close)),
                None => Err(primary),
            }
        }
    }
}

async fn list_tools_with_timeout(
    module_id: &str,
    connection: &McpConnection,
    timeout_duration: Duration,
) -> Result<Vec<meerkat_core::ToolDef>, RuntimeBoundaryError> {
    timeout(timeout_duration, connection.list_tools(module_id))
        .await
        .map_err(|_| mcp_timeout_error(module_id, "list_tools", timeout_duration))?
        .map_err(|error| mcp_list_tools_error(module_id, error))
}

/// Flatten `Vec<ContentBlock>` to a single text string for Meta's JSON pipeline.
///
/// MCP module tools return text in practice. If the result contains only
/// text blocks, concatenate them. If non-text blocks are present, serialize
/// the full block list to JSON and log a warning — this is lossy and will
/// be replaced when the pipeline supports multimodal content natively.
fn content_blocks_to_text(blocks: Vec<ContentBlock>) -> String {
    let all_text = blocks
        .iter()
        .all(|b| matches!(b, ContentBlock::Text { .. }));
    if all_text {
        blocks
            .into_iter()
            .filter_map(|b| match b {
                ContentBlock::Text { text } => Some(text),
                _ => None,
            })
            .collect::<String>()
    } else {
        eprintln!(
            "WARN meta: MCP tool returned multimodal content blocks; \
             serializing to JSON string (lossy) until pipeline supports native multimodal"
        );
        serde_json::to_string(&blocks).unwrap_or_default()
    }
}

async fn call_tool_with_timeout(
    module_id: &str,
    tool_name: &str,
    connection: &McpConnection,
    args: &Value,
    timeout_duration: Duration,
) -> Result<String, RuntimeBoundaryError> {
    let blocks = timeout(timeout_duration, connection.call_tool(tool_name, args))
        .await
        .map_err(|_| {
            mcp_timeout_error(
                module_id,
                &format!("call_tool:{tool_name}"),
                timeout_duration,
            )
        })?
        .map_err(|error| mcp_tool_call_error(module_id, tool_name, error))?;
    Ok(content_blocks_to_text(blocks))
}

/// Close the module server and wait for its close to report, with no bound
/// of our own: Meerkat bounds every stdio close (EOF and TERM graces, then the
/// kill backstop), and this call's runtime must outlive it, or dropping that
/// runtime would cancel the close. A close that did not settle is a cleanup
/// failure next to the operation's own result.
async fn close_settled(
    module_id: &str,
    connection: McpConnection,
) -> Result<(), RuntimeBoundaryError> {
    match close_failure(module_id, &connection.close_with_outcome().await) {
        None => Ok(()),
        Some(close) => Err(close),
    }
}

/// The cleanup failure a close outcome is, if any: the native close error
/// itself for a failed close, otherwise the class it did not settle in.
fn close_failure(module_id: &str, outcome: &McpServerCloseOutcome) -> Option<RuntimeBoundaryError> {
    let class = unsettled_class(outcome)?;
    Some(RuntimeBoundaryError::Mcp(match outcome {
        McpServerCloseOutcome::CloseFailed(error) => McpBoundaryError::CloseFailed {
            module_id: module_id.to_string(),
            reason: error.to_string(),
        },
        _ => McpBoundaryError::CloseUnsettled {
            module_id: module_id.to_string(),
            class,
        },
    }))
}

fn mcp_timeout_error(
    module_id: &str,
    operation: &str,
    timeout_duration: Duration,
) -> RuntimeBoundaryError {
    RuntimeBoundaryError::Mcp(McpBoundaryError::Timeout {
        module_id: module_id.to_string(),
        operation: operation.to_string(),
        timeout_ms: timeout_duration.as_millis() as u64,
    })
}

fn mcp_connect_error(module_id: &str, error: McpError) -> RuntimeBoundaryError {
    RuntimeBoundaryError::Mcp(McpBoundaryError::ConnectionFailed {
        module_id: module_id.to_string(),
        reason: error.to_string(),
    })
}

fn mcp_list_tools_error(module_id: &str, error: McpError) -> RuntimeBoundaryError {
    RuntimeBoundaryError::Mcp(McpBoundaryError::ToolListFailed {
        module_id: module_id.to_string(),
        reason: error.to_string(),
    })
}

fn mcp_tool_call_error(module_id: &str, tool_name: &str, error: McpError) -> RuntimeBoundaryError {
    RuntimeBoundaryError::Mcp(McpBoundaryError::ToolCallFailed {
        module_id: module_id.to_string(),
        tool: tool_name.to_string(),
        reason: error.to_string(),
    })
}

fn finalize_mcp_operation_with_close<T>(
    operation_result: Result<T, RuntimeBoundaryError>,
    close_result: Result<(), RuntimeBoundaryError>,
) -> Result<T, RuntimeBoundaryError> {
    match (operation_result, close_result) {
        (Ok(value), Ok(())) => Ok(value),
        (Ok(_), Err(close_error)) => Err(close_error),
        (Err(operation_error), Ok(())) => Err(operation_error),
        (Err(operation_error), Err(close_error)) => {
            Err(attach_mcp_close_failure(operation_error, close_error))
        }
    }
}

fn attach_mcp_close_failure(
    operation_error: RuntimeBoundaryError,
    close_error: RuntimeBoundaryError,
) -> RuntimeBoundaryError {
    match (operation_error, close_error) {
        (RuntimeBoundaryError::Mcp(primary), RuntimeBoundaryError::Mcp(close)) => {
            RuntimeBoundaryError::Mcp(McpBoundaryError::OperationFailedWithCloseFailure {
                primary: Box::new(primary),
                close: Box::new(close),
            })
        }
        (operation_error, _) => operation_error,
    }
}

#[cfg(all(test, target_os = "linux"))]
#[allow(clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::test_mcp_fixture::{
        FixtureMode, McpFixture, ProcessState, process_state, recorded_pid,
    };
    use meerkat_mcp::{McpStdioLeaderOutcome, McpStdioWitnessOutcome};

    fn module(fixture: &McpFixture, mode: FixtureMode) -> ModuleConfig {
        let (command, args) = fixture.command(mode);
        ModuleConfig {
            id: "router".to_string(),
            command,
            args,
            restart_policy: crate::RestartPolicy::Never,
        }
    }

    /// Control: a server that exits on stdin EOF settles at once.
    #[test]
    fn a_module_call_with_a_cooperative_server_returns_its_result() {
        let fixture = McpFixture::new();
        let response = call_module_mcp_tool_text(
            &module(&fixture, FixtureMode::Cooperative),
            None,
            "echo",
            &serde_json::json!({}),
            Duration::from_secs(5),
        )
        .expect("the call and its close succeed");
        assert_eq!(response, "{\"ok\":true}");
        let pid = recorded_pid(&fixture.pid_path()).expect("the server ran");
        assert_eq!(process_state(pid), ProcessState::Gone);
    }

    /// A server spawned that never initializes: the connect times out, and the
    /// original server is closed through its custody and reaped before the
    /// call returns. The call's total bound is its connect timeout plus that
    /// close.
    #[test]
    fn a_connect_timeout_returns_after_its_server_is_reaped() {
        let fixture = McpFixture::new();
        let error = call_module_mcp_tool_text(
            &module(&fixture, FixtureMode::Silent),
            None,
            "echo",
            &serde_json::json!({}),
            Duration::from_secs(2),
        )
        .expect_err("a server that never initializes times out");
        match error {
            RuntimeBoundaryError::Mcp(McpBoundaryError::Timeout {
                module_id,
                operation,
                ..
            }) => {
                assert_eq!(module_id, "router");
                assert_eq!(operation, "connect");
            }
            other => panic!("expected a connect timeout alone: {other:?}"),
        }
        let pid = recorded_pid(&fixture.pid_path()).expect("the server ran");
        assert_eq!(process_state(pid), ProcessState::Gone);
    }

    /// The same timeout with a stdout holder outside the server's group: the
    /// timeout stays primary, the unsettled close stands beside it.
    #[test]
    fn a_connect_timeout_keeps_an_unsettled_close_beside_it() {
        let fixture = McpFixture::new();
        let error = call_module_mcp_tool_text(
            &module(&fixture, FixtureMode::SilentEscaping),
            None,
            "echo",
            &serde_json::json!({}),
            Duration::from_secs(2),
        )
        .expect_err("a server that never initializes times out");
        match error {
            RuntimeBoundaryError::Mcp(McpBoundaryError::OperationFailedWithCloseFailure {
                primary,
                close,
            }) => {
                assert!(
                    matches!(&*primary, McpBoundaryError::Timeout { operation, .. } if operation == "connect"),
                    "{primary:?}"
                );
                assert!(
                    matches!(
                        &*close,
                        McpBoundaryError::CloseUnsettled {
                            class: crate::McpCloseUnsettledClass::StdoutOpen,
                            ..
                        }
                    ),
                    "{close:?}"
                );
            }
            other => panic!("expected the timeout with the close failure: {other:?}"),
        }
        let pid = recorded_pid(&fixture.pid_path()).expect("the server ran");
        assert_eq!(process_state(pid), ProcessState::Gone);
    }

    /// Meerkat's bounded connect returns, with a connect that timed out, the
    /// final facts of the close it ran on the server it spawned: the leader
    /// reaped and its stdout closed. The facts are joined before the connect
    /// returns, not only implied by a reaped pid.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_timed_out_connect_returns_its_servers_final_close_facts() {
        let fixture = McpFixture::new();
        let Err(failure) = McpConnection::connect_within(
            &fixture.server("router", FixtureMode::Silent),
            Duration::from_secs(2),
        )
        .await
        else {
            panic!("a server that never initializes times out");
        };
        assert!(failure.timed_out, "{failure:?}");
        let Some(McpServerCloseOutcome::Stdio(receipt)) = &failure.close else {
            panic!("the spawned server's close facts come with the failure: {failure:?}");
        };
        assert!(
            matches!(receipt.leader, McpStdioLeaderOutcome::Reaped(_)),
            "{receipt:?}"
        );
        assert!(
            matches!(receipt.witness, McpStdioWitnessOutcome::Closed(_)),
            "{receipt:?}"
        );
        let pid = recorded_pid(&fixture.pid_path()).expect("the server ran");
        assert_eq!(process_state(pid), ProcessState::Gone);
    }

    /// With a stdout holder outside the server's group, the returned facts
    /// are a reaped leader whose stdout is still open. The boundary projects
    /// that as `CloseUnsettled { class: stdout_open }`; `CloseFailed` is kept
    /// for an actual close error.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_timed_out_connect_returns_an_open_stdout_as_unsettled() {
        let fixture = McpFixture::new();
        let Err(failure) = McpConnection::connect_within(
            &fixture.server("router", FixtureMode::SilentEscaping),
            Duration::from_secs(2),
        )
        .await
        else {
            panic!("a server that never initializes times out");
        };
        assert!(failure.timed_out, "{failure:?}");
        let Some(close) = &failure.close else {
            panic!("the spawned server's close facts come with the failure: {failure:?}");
        };
        let McpServerCloseOutcome::Stdio(receipt) = close else {
            panic!("a stdio close reports its receipt: {close:?}");
        };
        assert!(
            matches!(receipt.leader, McpStdioLeaderOutcome::Reaped(_)),
            "{receipt:?}"
        );
        assert!(
            matches!(receipt.witness, McpStdioWitnessOutcome::StillOpen),
            "{receipt:?}"
        );
        assert!(
            matches!(
                close_failure("router", close),
                Some(RuntimeBoundaryError::Mcp(
                    McpBoundaryError::CloseUnsettled {
                        class: crate::McpCloseUnsettledClass::StdoutOpen,
                        ..
                    }
                ))
            ),
            "{close:?}"
        );
    }

    /// The module server ignores stdin EOF and SIGTERM, so its close takes
    /// the full escalation, past the call's own 5 s timeout. The call keeps
    /// its runtime until that close reports, and returns its result with the
    /// server reaped.
    #[test]
    fn a_module_call_returns_once_its_stubborn_server_is_reaped() {
        let fixture = McpFixture::new();
        let response = call_module_mcp_tool_text(
            &module(&fixture, FixtureMode::Stubborn),
            None,
            "echo",
            &serde_json::json!({}),
            Duration::from_secs(5),
        )
        .expect("the call and its close succeed");
        assert_eq!(response, "{\"ok\":true}");
        let pid = recorded_pid(&fixture.pid_path()).expect("the server ran");
        assert_eq!(process_state(pid), ProcessState::Gone);
    }

    /// A holder of the server's stdout outside its group keeps the close from
    /// settling: that is a cleanup failure, never a clean call.
    #[test]
    fn a_module_close_that_cannot_settle_fails_the_call_as_cleanup() {
        let fixture = McpFixture::new();
        let error = call_module_mcp_tool_text(
            &module(&fixture, FixtureMode::Escaping),
            None,
            "echo",
            &serde_json::json!({}),
            Duration::from_secs(5),
        )
        .expect_err("an unsettled close fails the call");
        match error {
            RuntimeBoundaryError::Mcp(McpBoundaryError::CloseUnsettled { module_id, class }) => {
                assert_eq!(module_id, "router");
                assert_eq!(class, crate::McpCloseUnsettledClass::StdoutOpen);
            }
            other => panic!("expected an unsettled close: {other:?}"),
        }
        let pid = recorded_pid(&fixture.pid_path()).expect("the server ran");
        assert_eq!(process_state(pid), ProcessState::Gone);
        let escaped = recorded_pid(&fixture.escaped_pid_path()).expect("the holder ran");
        assert!(matches!(process_state(escaped), ProcessState::Live(_)));
    }

    /// A failed operation keeps its own error as primary, with the unsettled
    /// close beside it, and nothing is called again.
    #[test]
    fn an_operation_failure_keeps_its_primary_error_beside_an_unsettled_close() {
        let fixture = McpFixture::new();
        let error = call_module_mcp_tool_text(
            &module(&fixture, FixtureMode::Escaping),
            None,
            "missing",
            &serde_json::json!({}),
            Duration::from_secs(5),
        )
        .expect_err("an unknown tool fails the call");
        match error {
            RuntimeBoundaryError::Mcp(McpBoundaryError::OperationFailedWithCloseFailure {
                primary,
                close,
            }) => {
                assert!(
                    matches!(*primary, McpBoundaryError::ToolNotFound { .. }),
                    "{primary:?}"
                );
                assert!(
                    matches!(
                        &*close,
                        McpBoundaryError::CloseUnsettled {
                            class: crate::McpCloseUnsettledClass::StdoutOpen,
                            ..
                        }
                    ),
                    "{close:?}"
                );
            }
            other => panic!("expected the primary error with the close failure: {other:?}"),
        }
    }
}
