#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Actual composed command hooks and the existing local PreTool refusal path.
//! Positive Required tests require a working backend; they never skip on refusal.

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use meerkat::{AgentBuildConfig, AgentFactory, LlmDoneOutcome, LlmEvent, LlmRequest};
use meerkat_client::LlmClient;
use meerkat_core::confinement::{
    ConfinementRefusal, ConfinementSpec, ExecutionConfinement, FilesystemAccess, IpNetworkAccess,
    PathAccess, PlatformBaseline,
};
use meerkat_core::{
    AgentEvent, AgentToolDispatcher, Config, HookAdapterConfig, HookCapability, HookEntryConfig,
    HookFailureReason, HookId, HookPoint, Message, Provider, ToolCallView, ToolDef,
    ToolDispatchOutcome, ToolError, ToolResult,
};
use serde_json::json;

#[derive(Default)]
struct RecordingClient(Mutex<Vec<Vec<Message>>>);

#[async_trait::async_trait]
impl LlmClient for RecordingClient {
    fn project_replay_messages(
        &self,
        messages: &[Message],
    ) -> Result<Vec<Message>, meerkat_client::LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(&'a self, request: &'a LlmRequest) -> meerkat_llm_core::LlmStream<'a> {
        let first = {
            let mut requests = self.0.lock().unwrap();
            let first = requests.is_empty();
            requests.push(request.messages.clone());
            first
        };
        let (event, stop_reason) = if first {
            assert!(request.tools.iter().any(|tool| tool.name == "guarded_tool"));
            (
                LlmEvent::ToolCallComplete {
                    id: "guarded-call".into(),
                    name: "guarded_tool".into(),
                    args: json!({}),
                    meta: None,
                },
                meerkat_core::StopReason::ToolUse,
            )
        } else {
            (
                LlmEvent::TextDelta {
                    delta: "continued".into(),
                    meta: None,
                },
                meerkat_core::StopReason::EndTurn,
            )
        };
        Box::pin(futures::stream::iter(vec![
            Ok(event),
            Ok(LlmEvent::UsageUpdate {
                usage: meerkat_core::TurnUsage::host_declared(
                    Provider::Other,
                    &request.model,
                    meerkat_core::Usage::default(),
                ),
            }),
            Ok(LlmEvent::Done {
                outcome: LlmDoneOutcome::Success { stop_reason },
            }),
        ]))
    }

    fn provider(&self) -> Provider {
        Provider::Other
    }
    async fn health_check(&self) -> Result<(), meerkat_client::LlmError> {
        Ok(())
    }
}

#[derive(Default)]
struct GuardedTool(AtomicUsize);

#[async_trait::async_trait]
impl AgentToolDispatcher for GuardedTool {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        vec![Arc::new(ToolDef::new(
            "guarded_tool",
            "A tool with a command prerequisite",
            json!({"type":"object"}),
        ))]
        .into()
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(ToolResult::new(call.id.into(), "tool completed".into(), false).into())
    }
}

fn requirement(root: &Path, descendants: bool) -> ExecutionConfinement {
    ConfinementSpec {
        baseline: PlatformBaseline::CommandRuntimeV1,
        read: FilesystemAccess::Paths(vec![PathAccess::Subtree(root.into())]),
        write: FilesystemAccess::Paths(vec![PathAccess::Subtree(root.into())]),
        deny_read: vec![],
        deny_write: vec![],
        network: IpNetworkAccess::Denied,
        unix_connect: vec![],
        require_descendant_termination: descendants,
    }
    .try_into()
    .unwrap()
}

fn config(script: &str, arguments: Vec<String>, environment: HashMap<String, String>) -> Config {
    let mut config = Config::default();
    let mut args = vec!["-c".into(), script.into(), "hook-probe".into()];
    args.extend(arguments);
    config.hooks.entries.push(HookEntryConfig {
        id: HookId::new("required-command-hook"),
        point: HookPoint::PreToolExecution,
        capability: HookCapability::Guardrail,
        timeout_ms: Some(5_000),
        runtime: HookAdapterConfig::command("/bin/sh", args, environment),
        ..Default::default()
    });
    config
}

async fn run_case(factory: AgentFactory, config: Config, refusal: Option<ConfinementRefusal>) {
    let client = Arc::new(RecordingClient::default());
    let tool = Arc::new(GuardedTool::default());
    let (tx, mut rx) = tokio::sync::mpsc::channel(256);
    let mut build = AgentBuildConfig::new("claude-sonnet-4-5");
    build.llm_client_override = Some(client.clone());
    build.external_tools = Some(tool.clone());
    build.event_tx = Some(tx);
    let mut agent =
        tokio::time::timeout(Duration::from_secs(15), factory.build_agent(build, &config))
            .await
            .unwrap()
            .unwrap();
    let result = tokio::time::timeout(
        Duration::from_secs(15),
        agent.run("Attempt the guarded operation".to_string().into()),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(result.text, "continued");
    assert_eq!(
        tool.0.load(Ordering::SeqCst),
        usize::from(refusal.is_none())
    );
    let requests = client.0.lock().unwrap();
    assert_eq!(
        requests.len(),
        2,
        "local launch refusal must reach the next model turn without retry"
    );
    let results = requests[1]
        .iter()
        .filter_map(|message| match message {
            Message::ToolResults { results, .. } => Some(results.as_slice()),
            _ => None,
        })
        .flatten()
        .collect::<Vec<_>>();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].tool_use_id, "guarded-call");
    assert_eq!(results[0].is_error, refusal.is_some());
    if let Some(refusal) = refusal {
        let body: serde_json::Value = serde_json::from_str(&results[0].text_content()).unwrap();
        assert_eq!(body["error"], "confinement_refused");
        assert_eq!(body["data"]["refusal"], json!(refusal));
    } else {
        assert_eq!(results[0].text_content(), "tool completed");
    }
    drop(requests);
    let mut refusals = 0;
    let mut hook_starts = 0;
    let mut completed = 0;
    let mut failures = 0;
    while let Ok(event) = rx.try_recv() {
        match event {
            AgentEvent::HookLaunchRefused {
                hook_id,
                point,
                reason,
                tool_use_id,
            } => {
                assert_eq!(hook_id, HookId::new("required-command-hook"));
                assert_eq!(point, HookPoint::PreToolExecution);
                assert_eq!(tool_use_id.as_deref(), Some("guarded-call"));
                assert_eq!(
                    Some(reason),
                    refusal.map(|refusal| HookFailureReason::ConfinementRefused { refusal })
                );
                refusals += 1;
            }
            AgentEvent::HookStarted { .. } => hook_starts += 1,
            AgentEvent::RunCompleted { .. } => completed += 1,
            AgentEvent::RunFailed { .. } => failures += 1,
            _ => {}
        }
    }
    assert_eq!(refusals, usize::from(refusal.is_some()));
    assert_eq!(hook_starts, usize::from(refusal.is_none()));
    assert_eq!(completed, 1);
    assert_eq!(
        failures, 0,
        "a refused PreTool launch must not fail the run"
    );
}

#[tokio::test]
async fn required_command_hook_rejects_opaque_engine_override_before_agent_build() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let client = Arc::new(RecordingClient::default());
    let mut build = AgentBuildConfig::new("claude-sonnet-4-5");
    build.llm_client_override = Some(client.clone());
    build.hook_engine_override = Some(Arc::new(meerkat_hooks::DefaultHookEngine::new(
        Default::default(),
    )));
    let factory = AgentFactory::minimal()
        .session_store(Arc::new(meerkat_store::MemoryStore::new()))
        .with_command_hook_confinement(requirement(&root, false), root);
    let result = factory.build_agent(build, &Config::default()).await;
    assert!(
        matches!(result, Err(meerkat::BuildAgentError::Config(message))
        if message == "required command-hook confinement cannot be combined with hook_engine_override")
    );
    assert!(client.0.lock().unwrap().is_empty());
}

#[tokio::test]
async fn required_command_hook_without_runtime_root_refuses_locally_and_continues() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let marker = root.join("must-not-enter");
    let factory = AgentFactory::minimal()
        .session_store(Arc::new(meerkat_store::MemoryStore::new()))
        .with_command_hook_confinement(requirement(&root, false), root.clone());
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    let expected = ConfinementRefusal::BackendUnavailable;
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    let expected = ConfinementRefusal::UnsupportedRequirement;
    run_case(
        factory,
        config(
            "printf entered > \"$1\"; cat >/dev/null; printf '{}'",
            vec![marker.display().to_string()],
            HashMap::new(),
        ),
        Some(expected),
    )
    .await;
    assert!(!marker.exists());
}

#[tokio::test]
async fn unsupported_required_command_hook_never_falls_back_to_trusted_launch() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let marker = root.join("must-not-enter");
    let factory = AgentFactory::minimal()
        .session_store(Arc::new(meerkat_store::MemoryStore::new()))
        .runtime_root(root.join("runtime"))
        .with_command_hook_confinement(requirement(&root, true), root.clone());
    run_case(
        factory,
        config(
            "printf entered > \"$1\"; cat >/dev/null; printf '{}'",
            vec![marker.display().to_string()],
            HashMap::new(),
        ),
        Some(ConfinementRefusal::UnsupportedRequirement),
    )
    .await;
    assert!(!marker.exists());
}

#[tokio::test]
#[cfg(any(target_os = "linux", target_os = "macos"))]
#[cfg_attr(
    target_os = "linux",
    ignore = "Linux positive confinement acceptance lane; requires an eligible host"
)]
async fn required_command_hook_enters_with_exact_argv_cwd_environment_and_custody() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let work = root.join("work");
    std::fs::create_dir(&work).unwrap();
    let outside = root.join("private");
    std::fs::write(&outside, "unchanged").unwrap();
    assert!(
        std::env::var_os("HOME").is_some(),
        "the fixture needs an ambient variable to exclude"
    );
    let literal = "literal ; $(not-a-command)";
    let environment = HashMap::from([
        ("PATH".into(), "/usr/bin:/bin".into()),
        ("HOOK_VALUE".into(), "exact value".into()),
    ]);
    let script = r#"cat >/dev/null || exit 10
test "$HOOK_VALUE" = 'exact value' || exit 11
test -z "${HOME+x}" || exit 12
test "$(pwd -P)" = "$2" || exit 13
if cat "$3" >/dev/null 2>&1; then exit 14; fi
printf '%s' "$1" > entered || exit 15
printf '{}'"#;
    let factory = AgentFactory::minimal()
        .session_store(Arc::new(meerkat_store::MemoryStore::new()))
        .runtime_root(root.join("runtime"))
        .with_command_hook_confinement(requirement(&work, false), work.clone());
    run_case(
        factory,
        config(
            script,
            vec![
                literal.into(),
                work.display().to_string(),
                outside.display().to_string(),
            ],
            environment,
        ),
        None,
    )
    .await;
    assert_eq!(
        std::fs::read_to_string(work.join("entered")).unwrap(),
        literal
    );
    assert_eq!(std::fs::read_to_string(outside).unwrap(), "unchanged");
}

#[tokio::test]
#[cfg(any(target_os = "linux", target_os = "macos"))]
async fn default_command_hook_keeps_legacy_trusted_host_execution() {
    let temp = tempfile::tempdir().unwrap();
    let marker = temp.path().join("entered");
    let factory = AgentFactory::minimal()
        .session_store(Arc::new(meerkat_store::MemoryStore::new()))
        .runtime_root(temp.path().join("runtime"));
    run_case(
        factory,
        config(
            "cat >/dev/null; printf trusted > \"$1\"; printf '{}'",
            vec![marker.display().to_string()],
            HashMap::new(),
        ),
        None,
    )
    .await;
    assert_eq!(std::fs::read_to_string(marker).unwrap(), "trusted");
}

// These controls start at the existing closed command-custody refusal seam.
// They exercise DefaultHookEngine and the real factory/Agent, not an OS backend
// or native admitted controller. Guardrail denial is checked at report level;
// its final local-controller/run disposition is a separate contract.
mod observe_refusal_locality {
    use super::*;
    use meerkat_core::{HookDecision, HookEngine, HookInvocation, HookReasonCode, SessionId};
    use meerkat_hooks::{
        CommandHookCustodyError, CommandHookCustodySpawn, CommandHookProcessCustody,
        DefaultHookEngine, RuntimeHookResponse,
    };
    use std::ffi::{OsStr, OsString};

    struct RefusingCommandCustody {
        order: Arc<Mutex<Vec<&'static str>>>,
    }

    #[async_trait::async_trait]
    impl CommandHookProcessCustody for RefusingCommandCustody {
        async fn spawn(
            &self,
            hook_id: &HookId,
            _run_id: Option<&meerkat_core::RunId>,
            _command: &meerkat_core::config::CommandRuntimeConfig,
        ) -> Result<meerkat_sandbox::ProcessChild, HookFailureReason> {
            let label = if hook_id == &HookId::new("refused-observer") {
                "observer-refused"
            } else {
                assert_eq!(hook_id, &HookId::new("refused-guardrail"));
                "guardrail-refused"
            };
            self.order.lock().unwrap().push(label);
            Err(HookFailureReason::ConfinementRefused {
                refusal: ConfinementRefusal::UnsupportedRequirement,
            })
        }

        async fn prepare(
            &self,
            _hook_id: &HookId,
            _run_id: Option<&meerkat_core::RunId>,
            _program: &OsStr,
            _args: &[OsString],
        ) -> Result<
            (Box<dyn CommandHookCustodySpawn>, tokio::process::Command),
            CommandHookCustodyError,
        > {
            self.order.lock().unwrap().push("unexpected-legacy-prepare");
            Err(CommandHookCustodyError {
                reason: "a no-entry refusal reached mutable command preparation".into(),
            })
        }
    }

    async fn engine(
        point: HookPoint,
        deny: bool,
        marker: &Path,
    ) -> (DefaultHookEngine, Arc<Mutex<Vec<&'static str>>>) {
        let mut hooks = config(
            "printf entered > \"$1\"; cat >/dev/null; printf '{}'",
            vec![marker.display().to_string()],
            HashMap::new(),
        )
        .hooks;
        hooks.entries[0].id = HookId::new("refused-observer");
        hooks.entries[0].point = point;
        hooks.entries[0].capability = HookCapability::Observe;
        hooks.entries[0].priority = 0;
        hooks.entries.push(HookEntryConfig {
            id: HookId::new("mandatory-guardrail"),
            point,
            capability: HookCapability::Guardrail,
            priority: 10,
            runtime: HookAdapterConfig::in_process("mandatory-guardrail"),
            ..Default::default()
        });
        let order = Arc::new(Mutex::new(Vec::new()));
        let engine = DefaultHookEngine::new(hooks).with_command_process_custody(Arc::new(
            RefusingCommandCustody {
                order: order.clone(),
            },
        ));
        let guardrail_order = order.clone();
        engine
            .register_in_process_handler(
                "mandatory-guardrail",
                Arc::new(move |invocation| {
                    assert_eq!(invocation.point, point);
                    guardrail_order.lock().unwrap().push("guardrail");
                    Box::pin(async move {
                        Ok(RuntimeHookResponse {
                            decision: Some(if deny {
                                HookDecision::deny(
                                    HookId::new("mandatory-guardrail"),
                                    HookReasonCode::PolicyViolation,
                                    "mandatory prerequisite denied",
                                    None,
                                )
                            } else {
                                HookDecision::Allow
                            }),
                        })
                    })
                }),
            )
            .await
            .unwrap();
        (engine, order)
    }

    async fn allowing_guardrail_completes(point: HookPoint, expected_invocations: usize) {
        let temp = tempfile::tempdir().unwrap();
        let marker = temp.path().join("observer-must-not-enter");
        let (engine, order) = engine(point, false, &marker).await;
        let client = Arc::new(RecordingClient::default());
        let tool = Arc::new(GuardedTool::default());
        let (tx, mut rx) = tokio::sync::mpsc::channel(256);
        let mut build = AgentBuildConfig::new("claude-sonnet-4-5");
        build.llm_client_override = Some(client.clone());
        build.external_tools = Some(tool.clone());
        build.event_tx = Some(tx);
        // The ordinary factory override composes this real engine. Required
        // factory-profile propagation is covered by the separate cases above.
        build.hook_engine_override = Some(Arc::new(engine));
        let factory =
            AgentFactory::minimal().session_store(Arc::new(meerkat_store::MemoryStore::new()));
        let mut agent = tokio::time::timeout(
            Duration::from_secs(15),
            factory.build_agent(build, &Config::default()),
        )
        .await
        .unwrap()
        .unwrap();
        let result = tokio::time::timeout(
            Duration::from_secs(15),
            agent.run(
                "Complete the guarded operation despite an unavailable observer"
                    .to_string()
                    .into(),
            ),
        )
        .await
        .unwrap();

        assert!(!marker.exists(), "the refused observer never entered");
        assert_eq!(
            *order.lock().unwrap(),
            ["observer-refused", "guardrail"].repeat(expected_invocations),
            "each refused Observe launch must still reach the later mandatory Guardrail",
        );
        let result = result.expect("an Observe launch refusal must remain local");
        assert_eq!(result.text, "continued");
        assert_eq!(
            tool.0.load(Ordering::SeqCst),
            1,
            "preserve the actual tool effect"
        );
        let requests = client.0.lock().unwrap();
        assert_eq!(
            requests.len(),
            2,
            "one tool call and one model continuation, without retry"
        );
        assert!(
            requests[1].iter().any(|message| matches!(
                message, Message::ToolResults { results, .. }
                    if results.iter().any(|result| result.tool_use_id == "guarded-call"
                        && !result.is_error && result.text_content() == "tool completed")
            )),
            "RunCompleted observation cannot erase the already-entered tool outcome"
        );
        drop(requests);

        let mut refusals = 0;
        let mut guardrail_starts = 0;
        let mut guardrail_completions = 0;
        let mut completions = 0;
        let mut failures = 0;
        let mut hook_failures = 0;
        let mut hook_denials = 0;
        while let Ok(event) = rx.try_recv() {
            match event {
                AgentEvent::HookLaunchRefused {
                    hook_id,
                    point: actual,
                    reason,
                    tool_use_id,
                } => {
                    assert_eq!(hook_id, HookId::new("refused-observer"));
                    assert_eq!(actual, point);
                    assert_eq!(
                        reason,
                        HookFailureReason::ConfinementRefused {
                            refusal: ConfinementRefusal::UnsupportedRequirement,
                        }
                    );
                    assert!(tool_use_id.is_none());
                    refusals += 1;
                }
                AgentEvent::HookStarted {
                    hook_id,
                    point: actual,
                } => {
                    assert_eq!(
                        hook_id,
                        HookId::new("mandatory-guardrail"),
                        "no fabricated observer entry"
                    );
                    assert_eq!(actual, point);
                    guardrail_starts += 1;
                }
                AgentEvent::HookCompleted {
                    hook_id,
                    point: actual,
                    ..
                } => {
                    assert_eq!(
                        hook_id,
                        HookId::new("mandatory-guardrail"),
                        "no fabricated observer completion"
                    );
                    assert_eq!(actual, point);
                    guardrail_completions += 1;
                }
                AgentEvent::HookFailed { .. } => hook_failures += 1,
                AgentEvent::HookDenied { .. } => hook_denials += 1,
                AgentEvent::RunCompleted { .. } => completions += 1,
                AgentEvent::RunFailed { .. } => failures += 1,
                _ => {}
            }
        }
        assert_eq!(refusals, expected_invocations);
        assert_eq!(guardrail_starts, expected_invocations);
        assert_eq!(guardrail_completions, expected_invocations);
        assert_eq!(completions, 1);
        assert_eq!(
            failures, 0,
            "Observe refusal cannot turn successful work into RunFailed"
        );
        assert_eq!(hook_failures, 0, "no hook body failed after entry");
        assert_eq!(hook_denials, 0, "the mandatory Guardrail allowed");
    }

    #[tokio::test]
    async fn pre_llm_observe_launch_refusal_keeps_guardrail_and_model_tool_model() {
        allowing_guardrail_completes(HookPoint::PreLlmRequest, 2).await;
    }

    #[tokio::test]
    async fn run_completed_observe_launch_refusal_preserves_guardrail_effect_and_success() {
        allowing_guardrail_completes(HookPoint::RunCompleted, 1).await;
    }

    #[tokio::test]
    async fn observe_launch_refusal_cannot_remove_later_mandatory_denial() {
        for point in [HookPoint::PreLlmRequest, HookPoint::RunCompleted] {
            let temp = tempfile::tempdir().unwrap();
            let marker = temp.path().join("observer-must-not-enter");
            let (engine, order) = engine(point, true, &marker).await;
            let mut invocation = HookInvocation::run_completed(SessionId::new(), 1);
            invocation.point = point;
            let report = engine.execute(invocation, None).await;

            assert!(!marker.exists());
            assert_eq!(
                *order.lock().unwrap(),
                vec!["observer-refused", "guardrail"]
            );
            let report =
                report.expect("Observe refusal must not discard the later Guardrail result");
            assert_eq!(report.started, vec![HookId::new("mandatory-guardrail")]);
            assert_eq!(
                report.decision,
                Some(HookDecision::deny(
                    HookId::new("mandatory-guardrail"),
                    HookReasonCode::PolicyViolation,
                    "mandatory prerequisite denied",
                    None,
                ))
            );
            assert!(
                report.denial_error(point).is_some(),
                "the dependent operation remains denied"
            );
            // No assertion about Agent terminalization: local Guardrail feedback
            // and retained-controller handling are intentionally still open.
        }
    }

    #[tokio::test]
    async fn later_guardrail_failure_preserves_prior_facts_without_duplicate_entry_events() {
        use meerkat_core::error::AgentError;

        for no_entry in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let marker = temp.path().join("no-command-entered");
            let mut hooks = config(
                "printf entered > \"$1\"",
                vec![marker.display().to_string()],
                HashMap::new(),
            )
            .hooks;
            hooks.entries[0].id = HookId::new("refused-observer");
            hooks.entries[0].point = HookPoint::PreLlmRequest;
            hooks.entries[0].capability = HookCapability::Observe;
            hooks.entries[0].priority = 0;
            hooks.entries.push(HookEntryConfig {
                id: HookId::new("before-observer"),
                point: HookPoint::PreLlmRequest,
                priority: -1,
                runtime: HookAdapterConfig::in_process("before-observer"),
                ..Default::default()
            });
            let failed_id = if no_entry {
                "refused-guardrail"
            } else {
                "failed-guardrail"
            };
            let later_runtime = if no_entry {
                hooks.entries[0].runtime.clone()
            } else {
                HookAdapterConfig::in_process("failed-guardrail")
            };
            hooks.entries.push(HookEntryConfig {
                id: HookId::new(failed_id),
                point: HookPoint::PreLlmRequest,
                capability: HookCapability::Guardrail,
                priority: 1,
                runtime: later_runtime,
                ..Default::default()
            });
            let order = Arc::new(Mutex::new(Vec::new()));
            let engine = DefaultHookEngine::new(hooks).with_command_process_custody(Arc::new(
                RefusingCommandCustody {
                    order: order.clone(),
                },
            ));
            let before_order = order.clone();
            engine
                .register_in_process_handler(
                    "before-observer",
                    Arc::new(move |_| {
                        before_order.lock().unwrap().push("before");
                        Box::pin(async {
                            Ok(RuntimeHookResponse {
                                decision: Some(HookDecision::Allow),
                            })
                        })
                    }),
                )
                .await
                .unwrap();
            if !no_entry {
                let failure_order = order.clone();
                engine
                    .register_in_process_handler(
                        "failed-guardrail",
                        Arc::new(move |_| {
                            failure_order.lock().unwrap().push("guardrail-failed");
                            Box::pin(async { Err("ordinary later handler failure".to_string()) })
                        }),
                    )
                    .await
                    .unwrap();
            }
            let client = Arc::new(RecordingClient::default());
            let tool = Arc::new(GuardedTool::default());
            let (tx, mut rx) = tokio::sync::mpsc::channel(256);
            let mut build = AgentBuildConfig::new("claude-sonnet-4-5");
            build.llm_client_override = Some(client.clone());
            build.external_tools = Some(tool.clone());
            build.event_tx = Some(tx);
            build.hook_engine_override = Some(Arc::new(engine));
            let factory =
                AgentFactory::minimal().session_store(Arc::new(meerkat_store::MemoryStore::new()));
            let mut agent = tokio::time::timeout(
                Duration::from_secs(15),
                factory.build_agent(build, &Config::default()),
            )
            .await
            .unwrap()
            .unwrap();
            let result = tokio::time::timeout(
                Duration::from_secs(15),
                agent.run(
                    "Keep the mandatory Guardrail authoritative"
                        .to_string()
                        .into(),
                ),
            )
            .await
            .unwrap();
            if no_entry {
                assert!(matches!(result, Err(AgentError::HookLaunchRefused {
                    hook_id, reason: HookFailureReason::ConfinementRefused {
                        refusal: ConfinementRefusal::UnsupportedRequirement,
                    },
                }) if hook_id == HookId::new(failed_id)));
            } else {
                assert!(
                    matches!(result, Err(AgentError::HookExecutionFailed { hook_id, reason })
                    if hook_id == HookId::new(failed_id) && reason == "ordinary later handler failure")
                );
            }
            assert!(!marker.exists());
            assert!(client.0.lock().unwrap().is_empty());
            assert_eq!(tool.0.load(Ordering::SeqCst), 0);
            assert_eq!(
                *order.lock().unwrap(),
                vec![
                    "before",
                    "observer-refused",
                    if no_entry {
                        "guardrail-refused"
                    } else {
                        "guardrail-failed"
                    }
                ]
            );
            let mut events = Vec::new();
            while let Ok(event) = rx.try_recv() {
                events.push(event);
            }
            for (id, starts, completions, failures, refusals) in [
                ("before-observer", 1, 1, 0, 0),
                ("refused-observer", 0, 0, 0, 1),
                (
                    failed_id,
                    usize::from(!no_entry),
                    0,
                    usize::from(!no_entry),
                    usize::from(no_entry),
                ),
            ] {
                let id = HookId::new(id);
                assert_eq!(
                    events
                        .iter()
                        .filter(|event| matches!(event,
                    AgentEvent::HookStarted { hook_id, .. } if hook_id == &id))
                        .count(),
                    starts
                );
                assert_eq!(
                    events
                        .iter()
                        .filter(|event| matches!(event,
                    AgentEvent::HookCompleted { hook_id, .. } if hook_id == &id))
                        .count(),
                    completions
                );
                assert_eq!(
                    events
                        .iter()
                        .filter(|event| matches!(event,
                    AgentEvent::HookFailed { hook_id, .. } if hook_id == &id))
                        .count(),
                    failures
                );
                assert_eq!(
                    events
                        .iter()
                        .filter(|event| matches!(event,
                    AgentEvent::HookLaunchRefused { hook_id, .. } if hook_id == &id))
                        .count(),
                    refusals
                );
            }
        }
    }

    // This companion exercises the ordinary factory/Agent transfer from the
    // real background engine. The custody refusal is injected before entry;
    // it is not OS, native admitted-work, persistence, or restart evidence.
    mod background_completion {
        use super::*;
        use meerkat_core::hooks::{HookBackgroundCompletion, HookBackgroundResult};
        use meerkat_hooks::BackgroundDispatchLedger;
        use tokio::sync::Notify;

        struct NotifyingRefusingCustody {
            inner: RefusingCommandCustody,
            attempted: Notify,
            original_run: Mutex<Option<meerkat_core::RunId>>,
        }

        #[async_trait::async_trait]
        impl CommandHookProcessCustody for NotifyingRefusingCustody {
            async fn spawn(
                &self,
                hook_id: &HookId,
                run_id: Option<&meerkat_core::RunId>,
                command: &meerkat_core::config::CommandRuntimeConfig,
            ) -> Result<meerkat_sandbox::ProcessChild, HookFailureReason> {
                *self.original_run.lock().unwrap() = run_id.cloned();
                let result = self.inner.spawn(hook_id, run_id, command).await;
                self.attempted.notify_one();
                result
            }

            async fn prepare(
                &self,
                _hook_id: &HookId,
                _run_id: Option<&meerkat_core::RunId>,
                _program: &OsStr,
                _args: &[OsString],
            ) -> Result<
                (Box<dyn CommandHookCustodySpawn>, tokio::process::Command),
                CommandHookCustodyError,
            > {
                panic!("a refused background hook must not use legacy command preparation")
            }
        }

        struct CompletionAwaitingTool {
            inner: Arc<GuardedTool>,
            custody: Arc<NotifyingRefusingCustody>,
            ledger: BackgroundDispatchLedger,
            session_id: Mutex<Option<SessionId>>,
            retained: Mutex<Vec<HookBackgroundCompletion>>,
        }

        #[async_trait::async_trait]
        impl AgentToolDispatcher for CompletionAwaitingTool {
            fn tools(&self) -> Arc<[Arc<ToolDef>]> {
                self.inner.tools()
            }

            async fn dispatch(
                &self,
                call: ToolCallView<'_>,
            ) -> Result<ToolDispatchOutcome, ToolError> {
                assert_eq!(call.id, "guarded-call");
                self.custody.attempted.notified().await;
                let session_id = self.session_id.lock().unwrap().clone().unwrap();
                let run_id = self.custody.original_run.lock().unwrap().clone();
                // Observe retention without taking the result. Only the Agent
                // transfers it to a notice at its next ordinary model boundary.
                self.ledger
                    .wait_for_completion(&session_id, run_id.as_ref())
                    .await;
                let retained = self.ledger.completion_snapshot().await;
                *self.retained.lock().unwrap() = retained;
                self.inner.dispatch(call).await
            }
        }

        fn notice_payloads<'a>(
            messages: &'a [Message],
            wanted: &str,
        ) -> Vec<&'a serde_json::Value> {
            messages
                .iter()
                .filter_map(|message| match message {
                    Message::SystemNotice(notice) => Some(notice),
                    _ => None,
                })
                .flat_map(|notice| notice.blocks.iter())
                .filter_map(|block| match block {
                    meerkat_core::types::SystemNoticeBlock::RuntimeNotice {
                        category,
                        payload,
                        ..
                    } if category == wanted => payload.as_ref(),
                    _ => None,
                })
                .collect()
        }

        fn completion_payloads(messages: &[Message]) -> Vec<&serde_json::Value> {
            notice_payloads(messages, "background_hook_completion")
        }

        fn assert_provider_notice_text(messages: &[Message], wanted: &str) {
            let mut projected = 0;
            for message in messages {
                let Message::SystemNotice(notice) = message else {
                    continue;
                };
                for block in &notice.blocks {
                    let meerkat_core::types::SystemNoticeBlock::RuntimeNotice {
                        category,
                        payload: Some(payload),
                        ..
                    } = block
                    else {
                        continue;
                    };
                    if category != wanted {
                        continue;
                    }
                    // Built-in providers consume this text projection, not the
                    // structured payload. The detail must render the same safe
                    // enum/attribution data, with labels JSON-escaped.
                    let block_text = block.model_projection_text();
                    assert_eq!(
                        serde_json::from_str::<serde_json::Value>(&block_text).unwrap(),
                        *payload
                    );
                    assert_eq!(block_text, serde_json::to_string(payload).unwrap());
                    let provider_text = notice.model_projection_text();
                    assert!(provider_text.contains(&block_text));
                    assert!(provider_text.len() < 4096);
                    projected += 1;
                }
            }
            assert_eq!(projected, 1);
        }

        fn assert_successful_tool_result(messages: &[Message]) {
            let results: Vec<_> = messages
                .iter()
                .filter_map(|message| match message {
                    Message::ToolResults { results, .. } => Some(results),
                    _ => None,
                })
                .flatten()
                .filter(|result| result.tool_use_id == "guarded-call")
                .collect();
            assert_eq!(results.len(), 1);
            assert!(!results[0].is_error);
            assert_eq!(results[0].text_content(), "tool completed");
        }

        #[tokio::test]
        async fn background_observe_refusal_reaches_next_factory_request_once_with_original_scope()
        {
            let temp = tempfile::tempdir().unwrap();
            let marker = temp.path().join("background-observer-must-not-enter");
            let private_canary = "private-background-command-canary";
            let mut hooks = config(
                "printf entered > \"$1\"; cat >/dev/null; printf '{}'; # private-background-command-canary",
                vec![marker.display().to_string()],
                HashMap::new(),
            )
            .hooks;
            hooks.entries[0].id = HookId::new("refused-observer");
            hooks.entries[0].point = HookPoint::PreToolExecution;
            hooks.entries[0].capability = HookCapability::Observe;
            hooks.entries[0].mode = meerkat_core::HookExecutionMode::Background;
            let order = Arc::new(Mutex::new(Vec::new()));
            let custody = Arc::new(NotifyingRefusingCustody {
                inner: RefusingCommandCustody {
                    order: order.clone(),
                },
                attempted: Notify::new(),
                original_run: Mutex::new(None),
            });
            let engine = Arc::new(
                DefaultHookEngine::new(hooks).with_command_process_custody(custody.clone()),
            );
            let ledger = engine.background_dispatch_ledger().clone();
            let tool = Arc::new(GuardedTool::default());
            let awaiting_tool = Arc::new(CompletionAwaitingTool {
                inner: tool.clone(),
                custody,
                ledger: ledger.clone(),
                session_id: Mutex::new(None),
                retained: Mutex::new(Vec::new()),
            });
            let client = Arc::new(RecordingClient::default());
            let (tx, mut rx) = tokio::sync::mpsc::channel(256);
            let mut build = AgentBuildConfig::new("claude-sonnet-4-5");
            build.llm_client_override = Some(client.clone());
            build.external_tools = Some(awaiting_tool.clone());
            build.event_tx = Some(tx);
            build.hook_engine_override = Some(engine);
            let factory =
                AgentFactory::minimal().session_store(Arc::new(meerkat_store::MemoryStore::new()));
            let mut agent = tokio::time::timeout(
                Duration::from_secs(15),
                factory.build_agent(build, &Config::default()),
            )
            .await
            .unwrap()
            .unwrap();
            let session_id = agent.session().id().clone();
            *awaiting_tool.session_id.lock().unwrap() = Some(session_id.clone());
            let result = tokio::time::timeout(
                Duration::from_secs(15),
                agent.run(
                    "Complete the permitted tool despite an unavailable background observer"
                        .to_string()
                        .into(),
                ),
            )
            .await
            .expect("the scoped completion barrier and ordinary run must settle")
            .expect("an Observe launch refusal cannot fail the run");

            assert_eq!(result.text, "continued");
            assert!(!marker.exists(), "the refused hook target never entered");
            assert_eq!(*order.lock().unwrap(), vec!["observer-refused"]);
            assert_eq!(tool.0.load(Ordering::SeqCst), 1);
            let completion = {
                let retained = awaiting_tool.retained.lock().unwrap();
                assert_eq!(
                    retained.len(),
                    1,
                    "the engine retained one actual completion"
                );
                retained[0].clone()
            };
            assert_eq!(completion.attribution.session_id, session_id);
            assert!(
                completion.attribution.run_id.is_some(),
                "the real Agent selected this run"
            );
            assert_eq!(
                completion.attribution.hook_id,
                HookId::new("refused-observer")
            );
            assert_eq!(completion.attribution.point, HookPoint::PreToolExecution);
            // The first model/tool turn is zero-based. Delivery to the next
            // model request must retain that original turn number.
            assert_eq!(completion.attribution.turn_number, Some(0));
            assert_eq!(
                completion.attribution.tool_use_id.as_deref(),
                Some("guarded-call")
            );
            assert!(completion.attribution.observation.is_none());
            assert!(!completion.diagnostic_truncated);
            assert!(matches!(
                completion.result,
                HookBackgroundResult::LaunchRefused(HookFailureReason::ConfinementRefused {
                    refusal: ConfinementRefusal::UnsupportedRequirement,
                })
            ));
            {
                let requests = client.0.lock().unwrap();
                assert_eq!(
                    requests.len(),
                    2,
                    "no extra feedback-only model call or retry"
                );
                assert!(completion_payloads(&requests[0]).is_empty());
                let payloads = completion_payloads(&requests[1]);
                assert_eq!(payloads.len(), 1);
                let payload = payloads[0];
                assert_eq!(payload["ordinal"], json!(completion.ordinal));
                assert_eq!(
                    payload["attribution"],
                    serde_json::to_value(&completion.attribution).unwrap()
                );
                assert_eq!(payload["disposition"], "launch_refused");
                assert_eq!(
                    payload["refusal"],
                    json!(ConfinementRefusal::UnsupportedRequirement)
                );
                let notice_bytes = serde_json::to_string(payload).unwrap();
                assert!(notice_bytes.len() < 4096, "feedback is bounded metadata");
                let projected_notices: Vec<_> = requests[1]
                    .iter()
                    .filter(|message| {
                        matches!(message, Message::SystemNotice(notice)
                        if notice.blocks.iter().any(|block| matches!(block,
                            meerkat_core::types::SystemNoticeBlock::RuntimeNotice { category, .. }
                                if category == "background_hook_completion")))
                    })
                    .collect();
                let notice_projection = serde_json::to_string(&projected_notices).unwrap();
                assert!(
                    notice_projection.len() < 4096,
                    "the complete projected notice is bounded"
                );
                assert!(!notice_projection.contains(private_canary));
                assert!(!notice_projection.contains(&marker.display().to_string()));
                assert_successful_tool_result(&requests[1]);
                assert_provider_notice_text(&requests[1], "background_hook_completion");
            }
            assert_successful_tool_result(agent.session().messages());
            assert_eq!(completion_payloads(agent.session().messages()).len(), 1);
            assert!(
                ledger.completion_snapshot().await.is_empty(),
                "Agent transferred the retained result once"
            );

            let mut starts = Vec::new();
            let mut refusals = 0;
            let mut completions = 0;
            let mut failures = 0;
            while let Ok(event) = rx.try_recv() {
                match event {
                    AgentEvent::RunStarted {
                        session_id,
                        identity,
                        ..
                    } => {
                        starts.push((session_id, identity.run_id));
                    }
                    AgentEvent::HookLaunchRefused {
                        hook_id,
                        point,
                        reason,
                        tool_use_id,
                    } => {
                        assert_eq!(hook_id, completion.attribution.hook_id);
                        assert_eq!(point, completion.attribution.point);
                        assert_eq!(tool_use_id, completion.attribution.tool_use_id);
                        assert_eq!(
                            reason,
                            HookFailureReason::ConfinementRefused {
                                refusal: ConfinementRefusal::UnsupportedRequirement,
                            }
                        );
                        refusals += 1;
                    }
                    AgentEvent::HookStarted { .. }
                    | AgentEvent::HookCompleted { .. }
                    | AgentEvent::HookFailed { .. }
                    | AgentEvent::HookDenied { .. } => {
                        panic!("scheduling or a no-entry refusal must not fabricate hook execution")
                    }
                    AgentEvent::RunCompleted { result, .. } => {
                        assert_eq!(result, "continued");
                        completions += 1;
                    }
                    AgentEvent::RunFailed { .. } => failures += 1,
                    _ => {}
                }
            }
            assert_eq!(starts, vec![(session_id, completion.attribution.run_id)]);
            assert_eq!(refusals, 1);
            assert_eq!(completions, 1);
            assert_eq!(failures, 0);
        }

        #[tokio::test]
        async fn background_pressure_reaches_next_factory_request_without_inventing_entry() {
            let temp = tempfile::tempdir().unwrap();
            let marker = temp.path().join("unscheduled-command-must-not-enter");
            let skipped_id = HookId::new(r#"not-scheduled-"observer"\label"#);
            let mut hooks = config(
                "printf entered > \"$1\"; # private-unscheduled-command-canary",
                vec![marker.display().to_string()],
                HashMap::new(),
            )
            .hooks;
            hooks.background_max_concurrency = 1;
            hooks.entries[0].id = skipped_id.clone();
            hooks.entries[0].point = HookPoint::PreToolExecution;
            hooks.entries[0].capability = HookCapability::Observe;
            hooks.entries[0].mode = meerkat_core::HookExecutionMode::Background;
            hooks.entries.insert(
                0,
                HookEntryConfig {
                    id: HookId::new("held-observer"),
                    point: HookPoint::PreToolExecution,
                    mode: meerkat_core::HookExecutionMode::Background,
                    capability: HookCapability::Observe,
                    timeout_ms: Some(60_000),
                    runtime: HookAdapterConfig::in_process("held-observer"),
                    ..Default::default()
                },
            );
            let custody_order = Arc::new(Mutex::new(Vec::new()));
            let custody = Arc::new(NotifyingRefusingCustody {
                inner: RefusingCommandCustody {
                    order: custody_order.clone(),
                },
                attempted: Notify::new(),
                original_run: Mutex::new(None),
            });
            let engine = Arc::new(
                DefaultHookEngine::new(hooks).with_command_process_custody(custody.clone()),
            );
            let (release_tx, release_rx) = tokio::sync::oneshot::channel();
            let release = Arc::new(tokio::sync::Mutex::new(Some(release_rx)));
            let held_run = Arc::new(Mutex::new(None));
            let held_calls = Arc::new(AtomicUsize::new(0));
            let held_entered = Arc::new(Notify::new());
            let handler_entered = held_entered.clone();
            let handler_run = held_run.clone();
            let handler_calls = held_calls.clone();
            engine
                .register_in_process_handler(
                    "held-observer",
                    Arc::new(move |invocation| {
                        let release = release.clone();
                        let handler_run = handler_run.clone();
                        let handler_calls = handler_calls.clone();
                        let handler_entered = handler_entered.clone();
                        Box::pin(async move {
                            *handler_run.lock().unwrap() = invocation.run_id.clone();
                            handler_calls.fetch_add(1, Ordering::SeqCst);
                            handler_entered.notify_one();
                            let receiver = release
                                .lock()
                                .await
                                .take()
                                .ok_or("duplicate held observer")?;
                            receiver.await.map_err(|error| error.to_string())?;
                            Ok(RuntimeHookResponse { decision: None })
                        })
                    }),
                )
                .await
                .unwrap();
            let ledger = engine.background_dispatch_ledger().clone();
            let client = Arc::new(RecordingClient::default());
            let tool = Arc::new(GuardedTool::default());
            let (tx, mut rx) = tokio::sync::mpsc::channel(256);
            let mut build = AgentBuildConfig::new("claude-sonnet-4-5");
            build.llm_client_override = Some(client.clone());
            build.external_tools = Some(tool.clone());
            build.event_tx = Some(tx);
            build.hook_engine_override = Some(engine.clone());
            let factory =
                AgentFactory::minimal().session_store(Arc::new(meerkat_store::MemoryStore::new()));
            let mut agent = tokio::time::timeout(
                Duration::from_secs(15),
                factory.build_agent(build, &Config::default()),
            )
            .await
            .unwrap()
            .unwrap();
            let session_id = agent.session().id().clone();
            let run_result = tokio::time::timeout(
                Duration::from_secs(15),
                agent.run(
                    "Complete the permitted tool despite a full background observer slot"
                        .to_string()
                        .into(),
                ),
            )
            .await;

            // Release and settle the actual held task before asserting the run
            // result, including an early failure. No additional run/model call
            // is started to consume the now-late completion.
            let before_release = ledger.completion_snapshot().await;
            let released = release_tx.send(());
            tokio::time::timeout(Duration::from_secs(5), held_entered.notified())
                .await
                .expect("the scheduled task must expose its actual original run");
            let original_run = held_run.lock().unwrap().clone();
            tokio::time::timeout(
                Duration::from_secs(5),
                ledger.wait_for_completion(&session_id, original_run.as_ref()),
            )
            .await
            .expect("released observer must settle through its owner");
            assert!(released.is_ok());
            assert!(
                before_release.is_empty(),
                "the first slot remained held during model continuation"
            );
            let result = run_result
                .expect("ordinary run must be bounded")
                .expect("background scheduling pressure cannot fail the run");
            assert_eq!(result.text, "continued");
            assert!(original_run.is_some());
            assert_eq!(held_calls.load(Ordering::SeqCst), 1);
            assert_eq!(tool.0.load(Ordering::SeqCst), 1);
            assert!(
                custody.original_run.lock().unwrap().is_none(),
                "the unscheduled command never reached custody"
            );
            assert!(custody_order.lock().unwrap().is_empty());
            assert!(!marker.exists());
            let completed = ledger.completion_snapshot().await;
            assert_eq!(completed.len(), 1);
            assert_eq!(
                completed[0].attribution.hook_id,
                HookId::new("held-observer")
            );
            assert_eq!(completed[0].attribution.run_id, original_run);
            assert!(
                matches!(&completed[0].result, HookBackgroundResult::Completed(outcome)
                if outcome.decision.is_none() && outcome.failure_reason.is_none())
            );
            {
                let requests = client.0.lock().unwrap();
                assert_eq!(requests.len(), 2);
                assert!(notice_payloads(&requests[0], "background_hook_not_scheduled").is_empty());
                let payloads = notice_payloads(&requests[1], "background_hook_not_scheduled");
                assert_eq!(payloads.len(), 1);
                assert_eq!(
                    payloads[0],
                    &json!({
                        "attribution": {
                            "session_id": session_id,
                            "run_id": original_run,
                            "turn_number": 0,
                            "hook_id": skipped_id,
                            "point": HookPoint::PreToolExecution,
                            "tool_use_id": "guarded-call",
                            "observation": null,
                        },
                        "disposition": "not_scheduled",
                        "reason": meerkat_core::hooks::HookBackgroundSkipReason::ConcurrencyFull,
                        "attribution_truncated": false,
                    })
                );
                assert_provider_notice_text(&requests[1], "background_hook_not_scheduled");
                assert_successful_tool_result(&requests[1]);
                assert!(completion_payloads(&requests[1]).is_empty());
                let request_bytes = serde_json::to_string(&requests[1]).unwrap();
                assert!(!request_bytes.contains("private-unscheduled-command-canary"));
                assert!(!request_bytes.contains(&marker.display().to_string()));
            }
            assert_successful_tool_result(agent.session().messages());
            assert_eq!(
                notice_payloads(agent.session().messages(), "background_hook_not_scheduled").len(),
                1
            );
            assert!(completion_payloads(agent.session().messages()).is_empty());
            let mut starts = Vec::new();
            let mut completions = 0;
            let mut failures = 0;
            while let Ok(event) = rx.try_recv() {
                match event {
                    AgentEvent::RunStarted {
                        session_id,
                        identity,
                        ..
                    } => starts.push((session_id, identity.run_id)),
                    AgentEvent::RunCompleted { result, .. } => {
                        assert_eq!(result, "continued");
                        completions += 1;
                    }
                    AgentEvent::RunFailed { .. } => failures += 1,
                    AgentEvent::HookStarted { .. }
                    | AgentEvent::HookCompleted { .. }
                    | AgentEvent::HookFailed { .. }
                    | AgentEvent::HookLaunchRefused { .. }
                    | AgentEvent::HookDenied { .. } => {
                        panic!("scheduling pressure is not hook entry or a completed disposition")
                    }
                    _ => {}
                }
            }
            assert_eq!(starts, vec![(session_id, original_run)]);
            assert_eq!(completions, 1);
            assert_eq!(failures, 0);
        }
    }
}
