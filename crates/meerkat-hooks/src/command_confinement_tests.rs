//! Source controls for the command owner's closed launch boundary.

#![allow(clippy::expect_used)]

use super::*;
use meerkat_core::confinement::ConfinementRefusal;
use meerkat_core::{HookFailureReason, HookPoint, SessionId};
use std::ffi::{OsStr, OsString};
use std::sync::atomic::AtomicUsize;

async fn finish_background_tasks(engine: &DefaultHookEngine) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(result) = engine.inflight_background.lock().await.join_next().await {
            assert!(result.is_ok(), "background task must finish normally");
        }
    })
    .await
    .expect("owned background tasks must finish");
}

#[tokio::test]
async fn background_refusal_retains_typed_scope_without_a_scheduled_entry()
-> Result<(), Box<dyn std::error::Error>> {
    use meerkat_core::hooks::HookBackgroundResult;
    let custody = Arc::new(LegacyCustody {
        prepare_calls: AtomicUsize::new(0),
    });
    let engine = DefaultHookEngine::new(HooksConfig {
        entries: vec![HookEntryConfig {
            id: HookId::new("background-command"),
            point: HookPoint::PostToolExecution,
            mode: HookExecutionMode::Background,
            capability: HookCapability::Observe,
            runtime: HookAdapterConfig::command("sh", Vec::new(), HashMap::new()),
            ..Default::default()
        }],
        ..Default::default()
    })
    .with_command_process_custody(custody.clone());
    let session_id = SessionId::new();
    let run_id = meerkat_core::RunId::new();
    let mut invocation = HookInvocation::new(HookPoint::PostToolExecution, session_id.clone());
    invocation.run_id = Some(run_id.clone());
    invocation.turn_number = Some(7);
    invocation.tool_result = Some(meerkat_core::HookToolResult::from_tool_result_with_id(
        "exact-call",
        "private-tool",
        &meerkat_core::ToolResult::new(
            "exact-call".to_owned(),
            "private-output-canary".to_owned(),
            false,
        ),
    ));
    let report = engine.execute(invocation, None).await?;
    assert!(report.started.is_empty(), "scheduling does not prove entry");
    assert!(report.outcomes.is_empty());
    assert!(
        report.launch_refusals.is_empty(),
        "completion is still asynchronous"
    );
    finish_background_tasks(&engine).await;
    let completions = engine.take_background_completions(&session_id, Some(&run_id), 8);
    assert_eq!(completions.len(), 1);
    let completion = &completions[0];
    assert_eq!(completion.attribution.session_id, session_id);
    assert_eq!(completion.attribution.run_id.as_ref(), Some(&run_id));
    assert_eq!(completion.attribution.turn_number, Some(7));
    assert_eq!(
        completion.attribution.hook_id,
        HookId::new("background-command")
    );
    assert_eq!(completion.attribution.point, HookPoint::PostToolExecution);
    assert_eq!(
        completion.attribution.tool_use_id.as_deref(),
        Some("exact-call")
    );
    assert!(matches!(
        &completion.result,
        HookBackgroundResult::LaunchRefused(HookFailureReason::ConfinementRefused {
            refusal: ConfinementRefusal::UnsupportedRequirement
        })
    ));
    assert!(!completion.diagnostic_truncated);
    assert!(!serde_json::to_string(completion)?.contains("private-output-canary"));
    assert!(
        engine
            .take_background_completions(&session_id, Some(&run_id), 8)
            .is_empty()
    );
    assert_eq!(custody.prepare_calls.load(Ordering::SeqCst), 0);
    Ok(())
}

#[tokio::test]
async fn background_ready_transfer_is_bounded_and_exact_session_and_run()
-> Result<(), Box<dyn std::error::Error>> {
    use meerkat_core::hooks::HookBackgroundResult;
    let engine = DefaultHookEngine::new(HooksConfig {
        background_max_concurrency: 4,
        entries: vec![HookEntryConfig {
            id: HookId::new("shared-observer"),
            point: HookPoint::PostToolExecution,
            mode: HookExecutionMode::Background,
            capability: HookCapability::Observe,
            runtime: HookAdapterConfig::in_process("held-observer"),
            ..Default::default()
        }],
        ..Default::default()
    });
    let session_a = SessionId::new();
    let session_b = SessionId::new();
    let run = meerkat_core::RunId::new();
    let old_run = meerkat_core::RunId::new();
    let release_a = Arc::new(tokio::sync::Notify::new());
    let release_b = Arc::new(tokio::sync::Notify::new());
    let (entered_tx, mut entered_rx) = tokio::sync::mpsc::unbounded_channel();
    let owner_a = session_a.clone();
    let handler_a = Arc::clone(&release_a);
    let handler_b = Arc::clone(&release_b);
    engine
        .register_in_process_handler(
            "held-observer",
            Arc::new(move |invocation| {
                let release = if invocation.session_id == owner_a {
                    Arc::clone(&handler_a)
                } else {
                    Arc::clone(&handler_b)
                };
                let entered = entered_tx.clone();
                Box::pin(async move {
                    let ready = release.notified();
                    tokio::pin!(ready);
                    ready.as_mut().enable();
                    entered.send(()).map_err(|error| error.to_string())?;
                    ready.await;
                    Ok(RuntimeHookResponse { decision: None })
                })
            }),
        )
        .await?;
    for (session, original_run) in [
        (&session_a, &run),
        (&session_a, &run),
        (&session_a, &old_run),
        (&session_b, &run),
    ] {
        let mut invocation = HookInvocation::new(HookPoint::PostToolExecution, session.clone());
        invocation.run_id = Some(original_run.clone());
        let report = engine.execute(invocation, None).await?;
        assert!(report.started.is_empty());
        tokio::time::timeout(Duration::from_secs(5), entered_rx.recv())
            .await?
            .ok_or("missing entry witness")?;
    }
    assert!(
        engine
            .take_background_completions(&session_a, Some(&run), 8)
            .is_empty(),
        "a ready read never waits for held hooks"
    );
    // Cancelling a registered readiness waiter is non-consuming and leaves
    // held tasks unchanged. Repeated readiness calls also cannot consume facts.
    {
        let cancelled = engine
            .background_dispatch_ledger()
            .wait_for_completion(&session_a, Some(&run));
        tokio::pin!(cancelled);
        assert!(futures::poll!(cancelled.as_mut()).is_pending());
    }
    release_b.notify_waiters();
    tokio::time::timeout(
        Duration::from_secs(5),
        engine
            .background_dispatch_ledger()
            .wait_for_completion(&session_b, Some(&run)),
    )
    .await?;
    assert!(
        engine
            .take_background_completions(&session_a, Some(&run), 8)
            .is_empty()
    );
    tokio::time::timeout(
        Duration::from_secs(5),
        engine
            .background_dispatch_ledger()
            .wait_for_completion(&session_b, Some(&run)),
    )
    .await?;
    let b = engine.take_background_completions(&session_b, Some(&run), 8);
    assert_eq!(b.len(), 1);
    assert!(
        matches!(&b[0].result, HookBackgroundResult::Completed(outcome)
        if outcome.failure_reason.is_none() && outcome.decision.is_none())
    );
    release_a.notify_waiters();
    finish_background_tasks(&engine).await;
    let first = engine.take_background_completions(&session_a, Some(&run), 1);
    let second = engine.take_background_completions(&session_a, Some(&run), 1);
    assert_eq!(first.len(), 1);
    assert_eq!(second.len(), 1);
    assert_ne!(
        first[0].ordinal, second[0].ordinal,
        "repeated invocations are distinct process-local tasks"
    );
    assert!(
        engine
            .take_background_completions(&session_a, Some(&run), 8)
            .is_empty()
    );
    assert!(
        engine
            .take_background_completions(&session_a, None, 8)
            .is_empty(),
        "None is not a wildcard"
    );
    assert_eq!(
        engine
            .take_background_completions(&session_a, Some(&old_run), 8)
            .len(),
        1
    );
    assert!(
        engine
            .background_dispatch_ledger()
            .completion_snapshot()
            .await
            .is_empty()
    );
    Ok(())
}

#[tokio::test]
async fn background_retention_pressure_refuses_scheduling_without_losing_ready_facts()
-> Result<(), Box<dyn std::error::Error>> {
    use meerkat_core::hooks::HookBackgroundSkipReason;
    let calls = Arc::new(AtomicUsize::new(0));
    let engine = DefaultHookEngine::new(HooksConfig {
        entries: vec![HookEntryConfig {
            id: HookId::new("bounded-observer"),
            point: HookPoint::PostToolExecution,
            mode: HookExecutionMode::Background,
            runtime: HookAdapterConfig::in_process("bounded"),
            ..Default::default()
        }],
        ..Default::default()
    });
    let entered = Arc::clone(&calls);
    engine
        .register_in_process_handler(
            "bounded",
            Arc::new(move |_| {
                let entered = Arc::clone(&entered);
                Box::pin(async move {
                    entered.fetch_add(1, Ordering::SeqCst);
                    Err("private diagnostic ".repeat(BACKGROUND_DIAGNOSTIC_BYTES))
                })
            }),
        )
        .await?;
    let session_id = SessionId::new();
    for _ in 0..BACKGROUND_COMPLETION_CAPACITY {
        let report = engine
            .execute(
                HookInvocation::new(HookPoint::PostToolExecution, session_id.clone()),
                None,
            )
            .await?;
        assert!(report.background_skips.is_empty());
        finish_background_tasks(&engine).await;
    }
    let before = engine
        .background_dispatch_ledger()
        .completion_snapshot()
        .await;
    assert_eq!(before.len(), BACKGROUND_COMPLETION_CAPACITY);
    assert!(before.iter().all(|item| item.diagnostic_truncated));
    for completion in &before {
        assert!(
            matches!(&completion.result, HookBackgroundResult::Failed(
            HookFailureReason::ExecutionFailed { message }
        ) if message.len() <= BACKGROUND_DIAGNOSTIC_BYTES
            && message.capacity() <= BACKGROUND_DIAGNOSTIC_BYTES),
            "truncated diagnostics must release the oversized original allocation"
        );
    }
    assert!(
        serde_json::to_vec(&before)?.len()
            < BACKGROUND_COMPLETION_CAPACITY * BACKGROUND_RECORD_BYTES
    );
    let report = engine
        .execute(
            HookInvocation::new(HookPoint::PostToolExecution, session_id.clone()),
            None,
        )
        .await?;
    assert!(report.started.is_empty());
    assert!(report.outcomes.is_empty());
    assert_eq!(report.background_skips.len(), 1);
    assert_eq!(
        report.background_skips[0].reason,
        HookBackgroundSkipReason::RetentionFull
    );
    assert_eq!(
        engine
            .background_dispatch_ledger()
            .completion_snapshot()
            .await,
        before
    );
    assert_eq!(calls.load(Ordering::SeqCst), BACKGROUND_COMPLETION_CAPACITY);
    assert_eq!(
        engine
            .take_background_completions(&session_id, None, 1)
            .len(),
        1
    );
    let report = engine
        .execute(
            HookInvocation::new(HookPoint::PostToolExecution, session_id.clone()),
            None,
        )
        .await?;
    assert!(
        report.background_skips.is_empty(),
        "transfer releases retained capacity"
    );
    finish_background_tasks(&engine).await;
    assert_eq!(
        calls.load(Ordering::SeqCst),
        BACKGROUND_COMPLETION_CAPACITY + 1
    );
    let mut oversized = HookInvocation::new(HookPoint::PostToolExecution, session_id.clone());
    oversized.tool_result = Some(meerkat_core::HookToolResult::from_tool_result_with_id(
        "x".repeat(BACKGROUND_ATTRIBUTION_BYTES + 1),
        "tool",
        &meerkat_core::ToolResult::new("call".to_owned(), "ok".to_owned(), false),
    ));
    let report = engine.execute(oversized, None).await?;
    assert_eq!(
        report.background_skips[0].reason,
        HookBackgroundSkipReason::AttributionTooLarge
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        BACKGROUND_COMPLETION_CAPACITY + 1
    );
    Ok(())
}

struct LegacyCustody {
    prepare_calls: AtomicUsize,
}

#[async_trait::async_trait]
impl CommandHookProcessCustody for LegacyCustody {
    async fn prepare(
        &self,
        _hook_id: &HookId,
        _run_id: Option<&meerkat_core::RunId>,
        _program: &OsStr,
        _args: &[OsString],
    ) -> Result<(Box<dyn CommandHookCustodySpawn>, Command), CommandHookCustodyError> {
        self.prepare_calls.fetch_add(1, Ordering::SeqCst);
        Err(CommandHookCustodyError {
            reason: "legacy mutable-command preparation was reached".to_owned(),
        })
    }
}

#[tokio::test]
async fn legacy_custody_without_closed_launch_support_refuses_before_prepare() {
    let custody = Arc::new(LegacyCustody {
        prepare_calls: AtomicUsize::new(0),
    });
    let hook_id = HookId::new("required-command-hook");
    let engine = DefaultHookEngine::new(HooksConfig {
        entries: vec![HookEntryConfig {
            id: hook_id.clone(),
            point: HookPoint::PreToolExecution,
            capability: HookCapability::Guardrail,
            runtime: HookAdapterConfig::command("sh", Vec::new(), HashMap::new()),
            ..Default::default()
        }],
        ..Default::default()
    })
    .with_command_process_custody(custody.clone());

    let result = engine
        .execute(
            HookInvocation {
                run_id: None,
                point: HookPoint::PreToolExecution,
                session_id: SessionId::new(),
                turn_number: Some(1),
                prompt_input: None,
                error_report: None,
                error_class: None,
                llm_request: None,
                llm_response: None,
                tool_call: None,
                tool_result: None,
                observation: None,
            },
            None,
        )
        .await;

    assert!(matches!(result, Err(HookEngineError::LaunchRefused {
        hook_id: actual,
        reason: HookFailureReason::ConfinementRefused {
            refusal: ConfinementRefusal::UnsupportedRequirement,
        },
    }) if actual == hook_id));
    assert_eq!(custody.prepare_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn observe_confinement_refusal_retains_prior_and_later_entered_outcomes()
-> Result<(), Box<dyn std::error::Error>> {
    let custody = Arc::new(LegacyCustody {
        prepare_calls: AtomicUsize::new(0),
    });
    let engine = DefaultHookEngine::new(HooksConfig {
        entries: vec![
            HookEntryConfig {
                id: HookId::new("before-observer"),
                point: HookPoint::PreLlmRequest,
                priority: -1,
                runtime: HookAdapterConfig::in_process("before-observer"),
                ..Default::default()
            },
            HookEntryConfig {
                id: HookId::new("refused-observer"),
                point: HookPoint::PreLlmRequest,
                priority: 0,
                runtime: HookAdapterConfig::command("sh", Vec::new(), HashMap::new()),
                capability: HookCapability::Observe,
                ..Default::default()
            },
            HookEntryConfig {
                id: HookId::new("after-guardrail"),
                point: HookPoint::PreLlmRequest,
                priority: 1,
                runtime: HookAdapterConfig::in_process("after-guardrail"),
                capability: HookCapability::Guardrail,
                ..Default::default()
            },
        ],
        ..Default::default()
    })
    .with_command_process_custody(custody.clone());
    for handler in ["before-observer", "after-guardrail"] {
        engine
            .register_in_process_handler(
                handler,
                Arc::new(|_| {
                    Box::pin(async {
                        Ok(RuntimeHookResponse {
                            decision: Some(HookDecision::Allow),
                        })
                    })
                }),
            )
            .await?;
    }
    let report = engine
        .execute(
            HookInvocation::new(HookPoint::PreLlmRequest, SessionId::new()),
            None,
        )
        .await?;
    assert_eq!(
        report.started,
        vec![
            HookId::new("before-observer"),
            HookId::new("after-guardrail")
        ]
    );
    assert_eq!(
        report
            .outcomes
            .iter()
            .map(|outcome| outcome.hook_id.clone())
            .collect::<Vec<_>>(),
        report.started
    );
    assert!(
        report
            .outcomes
            .iter()
            .all(|outcome| outcome.failure_reason.is_none())
    );
    assert_eq!(report.decision, Some(HookDecision::Allow));
    assert_eq!(report.launch_refusals.len(), 1);
    assert_eq!(
        report.launch_refusals[0].hook_id,
        HookId::new("refused-observer")
    );
    assert_eq!(report.launch_refusals[0].point, HookPoint::PreLlmRequest);
    assert_eq!(
        report.launch_refusals[0].refusal,
        ConfinementRefusal::UnsupportedRequirement
    );
    assert_eq!(custody.prepare_calls.load(Ordering::SeqCst), 0);
    Ok(())
}

#[tokio::test]
async fn post_commit_refusal_runs_later_observer() -> Result<(), Box<dyn std::error::Error>> {
    let custody = Arc::new(LegacyCustody {
        prepare_calls: AtomicUsize::new(0),
    });
    let refused_id = HookId::new("post-commit-refused");
    let later_id = HookId::new("post-commit-later");
    let point = HookPoint::RuntimeInputAccepted;
    let engine = DefaultHookEngine::new(HooksConfig {
        entries: vec![
            // Reverse registration order so the configured priority decides
            // which observer follows the refused command.
            HookEntryConfig {
                id: later_id.clone(),
                point,
                priority: 1,
                capability: HookCapability::Observe,
                runtime: HookAdapterConfig::in_process("post-commit-later"),
                ..Default::default()
            },
            HookEntryConfig {
                id: refused_id.clone(),
                point,
                priority: 0,
                capability: HookCapability::Observe,
                runtime: HookAdapterConfig::command("sh", Vec::new(), HashMap::new()),
                ..Default::default()
            },
        ],
        ..Default::default()
    })
    .with_command_process_custody(custody.clone());

    let observed = Arc::new(Mutex::new(Vec::<HookInvocation>::new()));
    let observed_by_handler = Arc::clone(&observed);
    engine
        .register_in_process_handler(
            "post-commit-later",
            Arc::new(move |invocation| {
                let observed = Arc::clone(&observed_by_handler);
                Box::pin(async move {
                    observed.lock().await.push(invocation);
                    Ok(RuntimeHookResponse { decision: None })
                })
            }),
        )
        .await?;

    // This fixture supplies the already committed fact to the real post-commit
    // engine entry. It does not manufacture a storage commit or test rollback.
    let invocation = HookInvocation::committed(
        SessionId::new(),
        meerkat_core::HookObservation::RuntimeInputAccepted(
            meerkat_core::HookRuntimeInputAccepted {
                input_id: meerkat_core::lifecycle::InputId::new(),
                input_kind: meerkat_core::HookRuntimeInputKind::Prompt,
                handling_mode: meerkat_core::HandlingMode::Queue,
            },
        ),
    );
    let report = engine.execute_post_commit(invocation.clone(), None).await?;

    assert_eq!(
        report.launch_refusals,
        vec![HookLaunchRefusal {
            hook_id: refused_id,
            point,
            refusal: ConfinementRefusal::UnsupportedRequirement,
        }]
    );
    assert_eq!(report.started, vec![later_id.clone()]);
    assert_eq!(report.outcomes.len(), 1);
    assert_eq!(report.outcomes[0].hook_id, later_id);
    assert_eq!(report.outcomes[0].point, point);
    assert_eq!(report.outcomes[0].failure_reason, None);
    assert_eq!(report.outcomes[0].decision, None);
    assert_eq!(report.decision, None);
    assert_eq!(observed.lock().await.as_slice(), &[invocation]);
    assert_eq!(custody.prepare_calls.load(Ordering::SeqCst), 0);
    Ok(())
}

#[tokio::test]
async fn post_commit_later_error_retains_prior_refusal() -> Result<(), Box<dyn std::error::Error>> {
    let custody = Arc::new(LegacyCustody {
        prepare_calls: AtomicUsize::new(0),
    });
    let refused_id = HookId::new("post-commit-refused-before-error");
    let later_id = HookId::new("post-commit-error");
    let point = HookPoint::RuntimeInputAccepted;
    let engine = DefaultHookEngine::new(HooksConfig {
        entries: vec![
            HookEntryConfig {
                id: refused_id.clone(),
                point,
                priority: 0,
                capability: HookCapability::Observe,
                runtime: HookAdapterConfig::command("sh", Vec::new(), HashMap::new()),
                ..Default::default()
            },
            HookEntryConfig {
                id: later_id.clone(),
                point,
                priority: 1,
                capability: HookCapability::Observe,
                runtime: HookAdapterConfig::in_process("post-commit-error"),
                ..Default::default()
            },
        ],
        ..Default::default()
    })
    .with_command_process_custody(custody.clone());

    let observed = Arc::new(Mutex::new(Vec::<HookInvocation>::new()));
    let observed_by_handler = Arc::clone(&observed);
    engine
        .register_in_process_handler(
            "post-commit-error",
            Arc::new(move |invocation| {
                let observed = Arc::clone(&observed_by_handler);
                Box::pin(async move {
                    observed.lock().await.push(invocation);
                    Err("later observer failed".to_owned())
                })
            }),
        )
        .await?;

    let invocation = HookInvocation::committed(
        SessionId::new(),
        meerkat_core::HookObservation::RuntimeInputAccepted(
            meerkat_core::HookRuntimeInputAccepted {
                input_id: meerkat_core::lifecycle::InputId::new(),
                input_kind: meerkat_core::HookRuntimeInputKind::Prompt,
                handling_mode: meerkat_core::HandlingMode::Queue,
            },
        ),
    );
    let (report, error) = match engine.execute_post_commit(invocation.clone(), None).await {
        Err(HookEngineError::WithReport { report, error }) => (report, error),
        other => {
            return Err(std::io::Error::other(format!(
                "expected prior refusal plus later observer error, got {other:?}"
            ))
            .into());
        }
    };

    assert_eq!(
        report.launch_refusals,
        vec![HookLaunchRefusal {
            hook_id: refused_id,
            point,
            refusal: ConfinementRefusal::UnsupportedRequirement,
        }]
    );
    // WithReport contains only prior facts. The later failing entry is carried
    // once by the original error, not duplicated as an entered report outcome.
    assert!(report.started.is_empty());
    assert!(report.outcomes.is_empty());
    assert_eq!(report.decision, None);
    assert!(matches!(
        error.as_ref(),
        HookEngineError::ExecutionFailed { hook_id, reason }
            if hook_id == &later_id && reason == "later observer failed"
    ));
    assert_eq!(observed.lock().await.as_slice(), &[invocation]);
    assert_eq!(custody.prepare_calls.load(Ordering::SeqCst), 0);
    Ok(())
}

struct InfrastructureRefusingCustody;

#[async_trait::async_trait]
impl CommandHookProcessCustody for InfrastructureRefusingCustody {
    async fn spawn(
        &self,
        _hook_id: &HookId,
        _run_id: Option<&meerkat_core::RunId>,
        _command: &meerkat_core::config::CommandRuntimeConfig,
    ) -> Result<meerkat_sandbox::ProcessChild, HookFailureReason> {
        Err(HookFailureReason::ExecutionFailed {
            message: "ordinary custody IO".into(),
        })
    }

    async fn prepare(
        &self,
        _hook_id: &HookId,
        _run_id: Option<&meerkat_core::RunId>,
        _program: &OsStr,
        _args: &[OsString],
    ) -> Result<(Box<dyn CommandHookCustodySpawn>, Command), CommandHookCustodyError> {
        Err(CommandHookCustodyError {
            reason: "unexpected legacy preparation".into(),
        })
    }
}

#[tokio::test]
async fn observe_infrastructure_launch_error_still_stops_before_later_hooks()
-> Result<(), Box<dyn std::error::Error>> {
    let later_calls = Arc::new(AtomicUsize::new(0));
    let engine = DefaultHookEngine::new(HooksConfig {
        entries: vec![
            HookEntryConfig {
                id: HookId::new("observer-io"),
                point: HookPoint::PreLlmRequest,
                priority: 0,
                capability: HookCapability::Observe,
                runtime: HookAdapterConfig::command("sh", Vec::new(), HashMap::new()),
                ..Default::default()
            },
            HookEntryConfig {
                id: HookId::new("later-guardrail"),
                point: HookPoint::PreLlmRequest,
                priority: 1,
                capability: HookCapability::Guardrail,
                runtime: HookAdapterConfig::in_process("later-guardrail"),
                ..Default::default()
            },
        ],
        ..Default::default()
    })
    .with_command_process_custody(Arc::new(InfrastructureRefusingCustody));
    let calls = later_calls.clone();
    engine
        .register_in_process_handler(
            "later-guardrail",
            Arc::new(move |_| {
                calls.fetch_add(1, Ordering::SeqCst);
                Box::pin(async {
                    Ok(RuntimeHookResponse {
                        decision: Some(HookDecision::Allow),
                    })
                })
            }),
        )
        .await?;
    let result = engine
        .execute(
            HookInvocation::new(HookPoint::PreLlmRequest, SessionId::new()),
            None,
        )
        .await;
    assert!(matches!(result, Err(HookEngineError::LaunchRefused {
        hook_id, reason: HookFailureReason::ExecutionFailed { message },
    }) if hook_id == HookId::new("observer-io") && message == "ordinary custody IO"));
    assert_eq!(later_calls.load(Ordering::SeqCst), 0);
    Ok(())
}
