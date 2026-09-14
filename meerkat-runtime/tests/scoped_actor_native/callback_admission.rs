use super::*;
use meerkat_core::IncrementalSessionStore;
use meerkat_core::lifecycle::core_executor::{
    BoundSessionCommit, CommittedSessionBoundaryAuthority, CoreApplyOutput, CoreApplyTerminal,
    CoreExecutor, CoreExecutorError, CoreExecutorPreDequeueHandle, CorePreDequeueOutcome,
};
use meerkat_core::lifecycle::run_primitive::{RunApplyBoundary, RunPrimitive};
use meerkat_core::lifecycle::run_receipt::RunBoundaryReceiptDraft;
use meerkat_runtime::store::{
    CommittedSessionBodyObservation, PreparedRuntimeSessionCommit, RuntimeSessionAuthority,
};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

pub(super) struct Fixture {
    pub machine: Arc<MeerkatMachine>,
    pub store: Arc<dyn RuntimeStore>,
    pub path: PathBuf,
    pub agent: meerkat::DynAgent,
    pub grant: meerkat_runtime::live_grant::LiveExecutionGrant<()>,
    pub source: LiveSourceKey,
    pub channel: meerkat_runtime::live_ledger::transcript_authority::LiveTranscriptChannelIngress,
    pub listener: tokio::net::TcpListener,
    pub stale_body: bool,
    pub resume: bool,
    pub transcript_sibling: bool,
    pub reopen_cancelled_callback: bool,
    pub cancellation: CallbackCancellation,
    pub failure: CallbackFailure,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum CallbackFailure {
    None,
    WithoutAppliedWitness,
    AppliedTerminal,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum CallbackCancellation {
    None,
    BeforeAdmission,
    BeforeStage,
    BeforeStageAppendFailure,
}

struct ColdCallbackRecovery {
    input_id: meerkat_core::InputId,
    ordinary_row: serde_json::Value,
    head: meerkat_runtime::live_ledger::transcript::LiveHeadReference,
    request_id: String,
    disposition: ColdCallbackDisposition,
}

enum ColdCallbackDisposition {
    CancelledContinuation,
    Held {
        callback: meerkat_core::session::CallbackBatchIdentity,
        request_snapshot: serde_json::Value,
    },
}

pub(super) async fn read_body(
    store: &dyn RuntimeStore,
    path: &Path,
    session_id: &meerkat_core::SessionId,
) -> TestResult<CommittedSessionBodyObservation> {
    let runtime = LogicalRuntimeId::for_session(session_id);
    Ok(
        match store
            .load_session_boundary_authority(&runtime)
            .await?
            .ok_or("missing session authority")?
        {
            RuntimeSessionAuthority::WholeBlob(_) => {
                CommittedSessionBodyObservation::from_whole_blob(
                    store
                        .load_committed_whole_blob_snapshot(&runtime)
                        .await?
                        .ok_or("missing WholeBlob")?,
                )
            }
            RuntimeSessionAuthority::HeadCanonical(authority) => {
                let body = meerkat_store::SqliteSessionStore::open(path)?
                    .materialize_head(authority.boundary_head())
                    .await?;
                CommittedSessionBodyObservation::from_head_canonical(authority, body)?
            }
        },
    )
}

async fn boundary(store: &dyn RuntimeStore, session: &Session) -> TestResult<BoundSessionCommit> {
    Ok(
        match store
            .load_session_boundary_authority(&LogicalRuntimeId::for_session(session.id()))
            .await?
            .ok_or("missing session authority")?
        {
            RuntimeSessionAuthority::WholeBlob(_) => {
                BoundSessionCommit::sealed(Arc::new(session.clone()))?
            }
            RuntimeSessionAuthority::HeadCanonical(authority) => {
                BoundSessionCommit::head_canonical_from_session(
                    session,
                    meerkat_core::session_store::PreparedHeadCanonicalMutation::prepare(
                        session,
                        Some(authority.boundary_head().clone()),
                    )?,
                )?
            }
        },
    )
}

struct ContinuationBarrier {
    callback_returned: AtomicBool,
    release: tokio::sync::watch::Sender<bool>,
}

impl Default for ContinuationBarrier {
    fn default() -> Self {
        Self {
            callback_returned: AtomicBool::new(false),
            release: tokio::sync::watch::channel(false).0,
        }
    }
}

#[async_trait::async_trait]
impl CoreExecutorPreDequeueHandle for ContinuationBarrier {
    async fn realize_committed_handoffs_under_turn_finalization_boundary(
        &self,
    ) -> Result<CorePreDequeueOutcome, CoreExecutorError> {
        if self.callback_returned.load(Ordering::SeqCst) {
            self.release
                .subscribe()
                .wait_for(|released| *released)
                .await
                .map_err(fault)?;
        }
        Ok(CorePreDequeueOutcome::NothingPending)
    }
}

struct CallbackActor {
    machine: Arc<MeerkatMachine>,
    store: Arc<dyn RuntimeStore>,
    path: PathBuf,
    agent: meerkat::DynAgent,
    barrier: Arc<ContinuationBarrier>,
    failure: CallbackFailure,
}

fn fault(error: impl std::fmt::Display) -> CoreExecutorError {
    CoreExecutorError::Internal(error.to_string())
}

impl CallbackActor {
    async fn output(
        &self,
        run_id: meerkat_core::RunId,
        primitive: &RunPrimitive,
        terminal: CoreApplyTerminal,
    ) -> Result<CoreApplyOutput, CoreExecutorError> {
        let committed = boundary(self.store.as_ref(), self.agent.session())
            .await
            .map_err(fault)?;
        Ok(CoreApplyOutput::new(
            RunBoundaryReceiptDraft {
                run_id,
                boundary: RunApplyBoundary::RunStart,
                contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                conversation_digest: Some(
                    self.agent
                        .session()
                        .transcript_content_digest()
                        .map_err(fault)?,
                ),
                message_count: self.agent.session().messages().len(),
            },
            Some(terminal),
        )
        .with_bound_session(committed))
    }
}

#[async_trait::async_trait]
impl CoreExecutor for CallbackActor {
    fn pre_dequeue_handle(&self) -> Option<Arc<dyn CoreExecutorPreDequeueHandle>> {
        Some(self.barrier.clone())
    }

    async fn apply(
        &mut self,
        run_id: meerkat_core::RunId,
        primitive: RunPrimitive,
    ) -> Result<CoreApplyOutput, CoreExecutorError> {
        if !primitive.execution_authority().is_session_policy() {
            return Err(fault("ordinary apply cannot consume a scoped primitive"));
        }
        self.agent.set_runtime_execution_kind(Some(
            meerkat_core::lifecycle::RuntimeExecutionKind::ContentTurn,
        ));
        let (events, _receiver) = tokio::sync::mpsc::channel(256);
        let result = self
            .agent
            .run_with_events_and_execution_context(
                primitive.extract_content_input(),
                Vec::new(),
                Vec::new(),
                None,
                RunExecutionContext::SessionPolicy,
                events,
            )
            .await
            .map_err(fault)?;
        self.output(
            run_id,
            &primitive,
            CoreApplyTerminal::RunResult(Box::new(result)),
        )
        .await
    }

    async fn apply_scoped(
        &mut self,
        run_id: meerkat_core::RunId,
        primitive: RunPrimitive,
    ) -> Result<CoreApplyOutput, CoreExecutorError> {
        let RunExecutionAuthority::Scoped(scope) = primitive.execution_authority() else {
            return Err(fault("missing scoped authority"));
        };
        let continuation = scope.record().callback_continuation.is_some();
        let context = self
            .machine
            .scoped_actor_execution_context(scope.clone())
            .await
            .map_err(fault)?;
        self.agent.set_runtime_execution_kind(Some(if continuation {
            meerkat_core::lifecycle::RuntimeExecutionKind::ResumePending
        } else {
            meerkat_core::lifecycle::RuntimeExecutionKind::ContentTurn
        }));
        let (events, _receiver) = tokio::sync::mpsc::channel(256);
        let terminal = if continuation {
            *self.agent.session_mut() =
                read_body(self.store.as_ref(), &self.path, self.agent.session().id())
                    .await
                    .map_err(fault)?
                    .into_session();
            let result = self
                .agent
                .run_pending_with_events_and_execution_context(
                    RunExecutionContext::Scoped(context),
                    events,
                )
                .await;
            match result {
                Ok(result) => CoreApplyTerminal::RunResult(Box::new(result)),
                Err(_) if self.failure == CallbackFailure::AppliedTerminal => {
                    let error = self
                        .agent
                        .take_runtime_terminal_failure_witness()
                        .map_err(fault)?
                        .ok_or_else(|| {
                            fault("actual failure lost its generated terminal witness")
                        })?;
                    if error.outcome != Some(meerkat_core::TurnTerminalOutcome::Failed)
                        || error.kind != meerkat_core::TurnTerminalCauseKind::LlmFailure
                    {
                        return Err(fault(format!("unexpected terminal witness: {error:?}")));
                    }
                    CoreApplyTerminal::MachineTerminalFailure { error }
                }
                Err(error) => return Err(fault(error)),
            }
        } else {
            let result = self
                .agent
                .run_with_events_and_execution_context(
                    primitive.extract_content_input(),
                    Vec::new(),
                    Vec::new(),
                    None,
                    RunExecutionContext::Scoped(context),
                    events,
                )
                .await;
            let Err(meerkat_core::AgentError::CallbackPending {
                tool_use_id,
                tool_name,
                args,
                ..
            }) = result
            else {
                return Err(fault(format!("expected actual callback, got {result:?}")));
            };
            let Some(meerkat_core::session::CallbackBatchObservation::Pending { identity, .. }) =
                self.agent
                    .session()
                    .callback_batch_observation()
                    .map_err(fault)?
            else {
                return Err(fault("actor lost its callback identity"));
            };
            CoreApplyTerminal::CallbackPending {
                tool_use_id,
                tool_name,
                args,
                callback_identity: Some(identity),
            }
        };
        self.barrier.callback_returned.store(true, Ordering::SeqCst);
        self.output(run_id, &primitive, terminal).await
    }

    async fn acknowledge_committed_session_boundary(
        &mut self,
        _: &CommittedSessionBoundaryAuthority,
    ) -> Result<(), CoreExecutorError> {
        *self.agent.session_mut() =
            read_body(self.store.as_ref(), &self.path, self.agent.session().id())
                .await
                .map_err(fault)?
                .into_session();
        Ok(())
    }

    async fn cancel_after_boundary(&mut self, _: String) -> Result<(), CoreExecutorError> {
        Ok(())
    }

    async fn stop_runtime_executor(&mut self, _: String) -> Result<(), CoreExecutorError> {
        Ok(())
    }
}

pub(super) async fn run(fixture: Fixture) -> TestResult {
    static LOGGING: std::sync::Once = std::sync::Once::new();
    LOGGING.call_once(|| {
        tracing_subscriber::fmt()
            .with_test_writer()
            .with_max_level(tracing::Level::DEBUG)
            .init();
    });
    let Fixture {
        machine,
        store,
        path,
        agent,
        grant,
        source,
        mut channel,
        listener,
        stale_body,
        resume,
        transcript_sibling,
        reopen_cancelled_callback,
        cancellation,
        failure,
    } = fixture;
    let session_id = source.session_id().clone();
    let barrier = Arc::new(ContinuationBarrier::default());
    machine
        .register_session_with_executor(
            session_id.clone(),
            Box::new(CallbackActor {
                machine: machine.clone(),
                store: store.clone(),
                path: path.clone(),
                agent,
                barrier: barrier.clone(),
                failure,
            }),
        )
        .await?;
    let requests = Arc::new(AtomicUsize::new(0));
    let request_count = requests.clone();
    let router = axum::Router::new().route(
        "/v1/responses",
        axum::routing::post(move || {
            let index = requests.fetch_add(1, Ordering::SeqCst);
            async move {
            if failure != CallbackFailure::None && index == 2 {
                return (
                    axum::http::StatusCode::BAD_REQUEST,
                    [("content-type", "application/json")],
                    r#"{"error":{"type":"invalid_request_error","message":"explicit callback resume failure"}}"#.to_string(),
                );
            }
            let output = if index == 1 && transcript_sibling {
                serde_json::json!([
                    {"id":"callback-item","type":"function_call","call_id":"fc_0",
                        "name":"allowed_tool","arguments":"{}","status":"completed"},
                    {"id":"sibling-item","type":"function_call","call_id":"fc_sibling",
                        "name":"allowed_tool","arguments":"{}","status":"completed"}
                ])
            } else if index < 2 {
                serde_json::json!([{"id":"callback-item","type":"function_call","call_id":"fc_0",
                    "name":"allowed_tool","arguments":"{}","status":"completed"}])
            } else {
                serde_json::json!([{"id":"resumed-message","type":"message","role":"assistant","status":"completed",
                    "content":[{"type":"output_text","text":"resumed","annotations":[]}]}])
            };
            let event = serde_json::json!({"type":"response.completed","response":{
                "id":"callback-response","status":"completed",
                "output":output,
                "usage":{"input_tokens":1,"output_tokens":1,"total_tokens":2}
            }});
            (
                axum::http::StatusCode::OK,
                [("content-type", "text/event-stream")],
                format!("data: {event}\n\n"),
            )
            }
        }),
    );
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let server = axum::serve(listener, router).with_graceful_shutdown(async {
        let _ = stopped.await;
    });
    let mut cold_recovery = None;
    let assertions = async {
        let (original, completion) = machine
            .commit_live_input_admission(source.clone(), &grant)
            .await?;
        channel.close().await?;
        drop(channel);
        let outcome = tokio::time::timeout(
            std::time::Duration::from_secs(15),
            completion.ok_or("missing completion")?.wait(),
        )
        .await??;
        let meerkat_runtime::completion::CompletionOutcome::CallbackPending {
            callback_identity: Some(identity),
            ..
        } = outcome
        else {
            return Err(format!("expected finalized callback, got {outcome:?}").into());
        };
        let cancelled_callback = if cancellation == CallbackCancellation::BeforeAdmission {
            let runtime = LogicalRuntimeId::for_session(&session_id);
            let original_row = serde_json::to_value(
                store
                    .load_input_state(&runtime, original.record().input_id())
                    .await?
                    .ok_or("original callback owner")?,
            )?;
            let head = store
                .live_ledger_ops()
                .ok_or("ledger")?
                .load_live_head(&session_id)
                .await?
                .ok_or("head")?;
            let state: serde_json::Value = serde_json::from_slice(&head.payload.request_snapshot)?;
            let intent = meerkat_core::live_execution::request::LiveRequestCancelIntent {
                source: source.clone(),
                reason: meerkat_core::live_execution::request::LiveRequestCancellationReason::OperatorRequested,
            };
            assert_eq!(machine.cancel_live_request(intent.clone()).await?, intent);
            assert!(
                machine
                    .reconcile_live_request_cancellations(&session_id)
                    .await?
                    .is_empty(),
                "a held callback must not repeatedly request cancellation of its finalized input"
            );
            Some((original_row, head.reference, state, intent))
        } else {
            None
        };
        let mut body = read_body(store.as_ref(), &path, &session_id)
            .await?
            .into_session();
        let prepared = body.prepare_callback_result_ingress(
            &[meerkat_core::ToolResult::new(
                "fc_0".into(),
                "escaped\0\"\\\n".repeat(200),
                false,
            )],
            Some(&identity),
        )?;
        let mut deferred = meerkat_core::session::SessionDeferredTurnState::default();
        prepared.stage_into(&mut deferred, std::time::SystemTime::UNIX_EPOCH)?;
        body.set_deferred_turn_state(deferred)?;
        store
            .commit_prepared_session_boundary(
                &LogicalRuntimeId::for_session(&session_id),
                PreparedRuntimeSessionCommit::snapshot_only(boundary(store.as_ref(), &body).await?),
            )
            .await?;
        let stale = read_body(store.as_ref(), &path, &session_id)
            .await?
            .observe_callback_results(&identity)?;
        body = read_body(store.as_ref(), &path, &session_id)
            .await?
            .into_session();
        body.set_metadata("unrelated_actor_revision", serde_json::json!(true));
        store
            .commit_prepared_session_boundary(
                &LogicalRuntimeId::for_session(&session_id),
                PreparedRuntimeSessionCommit::snapshot_only(boundary(store.as_ref(), &body).await?),
            )
            .await?;
        let ops = store.live_ledger_ops().ok_or("ledger")?;
        let before = ops.load_live_head(&session_id).await?;
        let source_before = ops.lookup_live_source(&source).await?.ok_or("source")?;
        if stale_body {
            let runtime = LogicalRuntimeId::for_session(&session_id);
            let inputs = serde_json::to_value(store.load_input_states_strict(&runtime).await?)?;
            assert!(
                machine
                    .commit_live_callback_input_admission(source.clone(), stale)
                    .await
                    .is_err()
            );
            assert_eq!(ops.load_live_head(&session_id).await?, before);
            assert_eq!(
                serde_json::to_value(store.load_input_states_strict(&runtime).await?)?,
                inputs
            );
            assert_eq!(
                ops.lookup_live_source(&source)
                    .await?
                    .ok_or("source")?
                    .bytes(),
                source_before.bytes()
            );
            return TestResult::Ok(());
        }
        let observation = read_body(store.as_ref(), &path, &session_id)
            .await?
            .observe_callback_results(&identity)?;
        if let Some((original_row, head_before, state_before, intent)) = cancelled_callback {
            assert!(
                machine
                    .commit_live_callback_input_admission(source.clone(), observation)
                    .await
                    .is_err(),
                "late callback results must not resume a cancelled chain"
            );
            let head = ops.load_live_head(&session_id).await?.ok_or("head")?;
            let state: serde_json::Value = serde_json::from_slice(&head.payload.request_snapshot)?;
            let request = state["source_requests"][serde_json::to_string(&source)?]
                .as_str()
                .ok_or("request")?;
            assert_eq!(state["request_phases"][request], "Suspended");
            assert_eq!(
                state["run_continuation_inputs"][identity.run_id().to_string()],
                ""
            );
            for field in [
                "run_callback_records",
                "run_callback_receipts",
                "claim_phases",
                "claim_credit_records",
                "claim_credit_bytes",
                "claim_credit_spent_records",
                "claim_credit_spent_bytes",
                "request_credit_spent_records",
                "request_credit_spent_bytes",
            ] {
                assert_eq!(state[field], state_before[field], "{field}");
            }
            assert_eq!(head.reference.event_count, head_before.event_count);
            assert_eq!(
                serde_json::to_value(
                    store
                        .load_input_state(
                            &LogicalRuntimeId::for_session(&session_id),
                            original.record().input_id()
                        )
                        .await?
                        .ok_or("original callback owner")?
                )?,
                original_row,
            );
            assert_eq!(request_count.load(Ordering::SeqCst), 2);
            assert_eq!(machine.cancel_live_request(intent.clone()).await?, intent);
            assert!(
                machine
                    .reconcile_live_request_cancellations(&session_id)
                    .await?
                    .is_empty()
            );
            assert!(matches!(
                read_body(store.as_ref(), &path, &session_id)
                    .await?
                    .into_session()
                    .observe_staged_callback_results(&identity)?,
                meerkat_core::session::StagedCallbackResultsObservation::Complete(_)
            ));
            if reopen_cancelled_callback {
                cold_recovery = Some(ColdCallbackRecovery {
                    input_id: original.record().input_id().clone(),
                    ordinary_row: original_row,
                    head: head.reference,
                    request_id: request.to_owned(),
                    disposition: ColdCallbackDisposition::Held {
                        callback: identity,
                        request_snapshot: state,
                    },
                });
            }
            return TestResult::Ok(());
        }
        let (receipt, completion) = machine
            .commit_live_callback_input_admission(source.clone(), observation)
            .await?;
        assert_ne!(receipt.record().input_id(), original.record().input_id());
        assert_ne!(
            receipt.record().admission_commit(),
            original.record().admission_commit()
        );
        assert_eq!(
            ops.lookup_live_source(&source)
                .await?
                .ok_or("source")?
                .bytes(),
            source_before.bytes()
        );
        let head = ops.load_live_head(&session_id).await?;
        let observation = read_body(store.as_ref(), &path, &session_id)
            .await?
            .observe_callback_results(&identity)?;
        let (duplicate, _) = machine
            .commit_live_callback_input_admission(source.clone(), observation)
            .await?;
        assert_eq!(duplicate.record(), receipt.record());
        assert_eq!(ops.load_live_head(&session_id).await?, head);
        if cancellation != CallbackCancellation::None {
            let original_before = serde_json::to_value(
                store
                    .load_input_state(
                        &LogicalRuntimeId::for_session(&session_id),
                        original.record().input_id(),
                    )
                    .await?
                    .ok_or("original callback owner")?,
            )?;
            let intent = meerkat_core::live_execution::request::LiveRequestCancelIntent {
                source: source.clone(),
                reason: meerkat_core::live_execution::request::LiveRequestCancellationReason::OperatorRequested,
            };
            if cancellation == CallbackCancellation::BeforeStageAppendFailure {
                rusqlite::Connection::open(&path)?.execute_batch(
                    "CREATE TRIGGER reject_callback_completion BEFORE INSERT ON runtime_live_events
                     BEGIN SELECT RAISE(ABORT, 'injected callback completion append failure'); END;",
                )?;
            }
            assert_eq!(machine.cancel_live_request(intent.clone()).await?, intent);
            let outcome = tokio::time::timeout(
                std::time::Duration::from_secs(15),
                completion.ok_or("missing continuation completion")?.wait(),
            )
            .await??;
            assert!(
                matches!(outcome,
                    meerkat_runtime::completion::CompletionOutcome::RuntimeTerminated { ref reason, .. }
                        if reason == "Live request cancellation: OperatorRequested"
                ),
                "{outcome:?}"
            );
            let row = store
                .load_input_state(
                    &LogicalRuntimeId::for_session(&session_id),
                    receipt.record().input_id(),
                )
                .await?
                .ok_or("cancelled continuation")?;
            assert!(row.seed.last_run_id.is_none());
            assert_eq!(row.seed.attempt_count, 0);
            assert_eq!(request_count.load(Ordering::SeqCst), 2);
            let head = ops.load_live_head(&session_id).await?.ok_or("head")?;
            let state: serde_json::Value = serde_json::from_slice(&head.payload.request_snapshot)?;
            let request = state["source_requests"][serde_json::to_string(&source)?]
                .as_str()
                .ok_or("request")?;
            assert_eq!(
                state["request_phases"][request],
                if cancellation == CallbackCancellation::BeforeStageAppendFailure {
                    "Suspended"
                } else {
                    "Terminal"
                }
            );
            assert_eq!(
                state["request_runs"][request],
                identity.run_id().to_string()
            );
            assert_eq!(state["run_requests"].as_object().ok_or("runs")?.len(), 1);
            let original_row = store
                .load_input_state(
                    &LogicalRuntimeId::for_session(&session_id),
                    original.record().input_id(),
                )
                .await?
                .ok_or("original callback owner")?;
            assert_eq!(serde_json::to_value(original_row)?, original_before);
            let body = read_body(store.as_ref(), &path, &session_id)
                .await?
                .into_session();
            assert!(
                matches!(
                    body.observe_staged_callback_results(&identity)?,
                    meerkat_core::session::StagedCallbackResultsObservation::Complete(_)
                ),
                "cancellation must not pretend callback results were applied"
            );
            if cancellation == CallbackCancellation::BeforeStageAppendFailure {
                assert!(machine.is_durability_ready(&session_id).await);
                cold_recovery = Some(ColdCallbackRecovery {
                    input_id: receipt.record().input_id().clone(),
                    ordinary_row: serde_json::to_value(row)?,
                    head: head.reference,
                    request_id: request.to_owned(),
                    disposition: ColdCallbackDisposition::CancelledContinuation,
                });
                return TestResult::Ok(());
            }
            assert_eq!(machine.cancel_live_request(intent.clone()).await?, intent);
            assert_eq!(
                ops.load_live_head(&session_id)
                    .await?
                    .ok_or("head")?
                    .reference
                    .event_count,
                head.reference.event_count,
            );
            return TestResult::Ok(());
        }
        if resume {
            barrier.release.send_replace(true);
            let outcome = tokio::time::timeout(
                std::time::Duration::from_secs(15),
                completion.ok_or("missing continuation completion")?.wait(),
            )
            .await??;
            if failure == CallbackFailure::WithoutAppliedWitness {
                assert!(
                    matches!(
                        outcome,
                        meerkat_runtime::completion::CompletionOutcome::AbandonedWithError { .. }
                    ),
                    "failed attempt did not retain its failure observation: {outcome:?}"
                );
                let runtime_id = LogicalRuntimeId::for_session(&session_id);
                let row = store
                    .load_input_state(&runtime_id, receipt.record().input_id())
                    .await?
                    .ok_or("held continuation row")?;
                assert_eq!(row.seed.phase, meerkat_runtime::InputLifecycleState::Staged);
                assert_eq!(row.seed.terminal_outcome, None);
                let run_id = row.seed.last_run_id.as_ref().ok_or("held run")?;
                let held_head = ops.load_live_head(&session_id).await?.ok_or("held head")?;
                let held_state: serde_json::Value =
                    serde_json::from_slice(&held_head.payload.request_snapshot)?;
                assert_eq!(
                    held_state["run_callback_application_claimed"][run_id.to_string()],
                    true
                );
                let body = read_body(store.as_ref(), &path, &session_id)
                    .await?
                    .into_session();
                assert!(
                    matches!(
                        body.observe_staged_callback_results(&identity)?,
                        meerkat_core::session::StagedCallbackResultsObservation::Complete(_)
                    ),
                    "the uncommitted actor application must not become a durable Applied receipt"
                );
                let cold = MeerkatMachine::persistent(
                    store.clone(),
                    Arc::new(meerkat_store::MemoryBlobStore::new()),
                );
                let blocked = cold.prepare_bindings(session_id.clone()).await;
                assert!(
                    matches!(blocked,
                        Err(meerkat_runtime::meerkat_machine::RuntimeBindingsError::RecoveryHeld {
                            evidence_digest: Some(_), ref reason, ..
                        }) if reason.contains("HoldCallbackApplication")
                    ),
                    "{blocked:?}"
                );
                assert_eq!(ops.load_live_head(&session_id).await?, Some(held_head));
                assert_eq!(
                    request_count.load(Ordering::SeqCst),
                    3,
                    "a received scoped failure cannot restart the request"
                );
                assert_ordinary_restoration(&machine, store.as_ref(), &path, &session_id).await?;
                let retained = store
                    .load_input_state(&runtime_id, receipt.record().input_id())
                    .await?
                    .ok_or("held input after ordinary wake")?;
                assert_eq!(
                    retained.seed.phase,
                    meerkat_runtime::InputLifecycleState::Staged
                );
                assert_eq!(retained.seed.last_run_id.as_ref(), Some(run_id));
                assert_eq!(retained.seed.terminal_outcome, None);
                assert_eq!(request_count.load(Ordering::SeqCst), 4);
                return TestResult::Ok(());
            }
            if failure == CallbackFailure::AppliedTerminal {
                let meerkat_runtime::live_source::LiveSourceEntryRecord::Reservation { record } =
                    source_before.record()?
                else {
                    return Err("missing retained request identity".into());
                };
                assert_applied_failure(
                    store.as_ref(),
                    &path,
                    &session_id,
                    receipt.record().input_id(),
                    record.request_id(),
                    &identity,
                    &outcome,
                )
                .await?;
                assert_eq!(request_count.load(Ordering::SeqCst), 3);
                assert_ordinary_restoration(&machine, store.as_ref(), &path, &session_id).await?;
                assert_eq!(request_count.load(Ordering::SeqCst), 4);
                return TestResult::Ok(());
            }
            let meerkat_runtime::completion::CompletionOutcome::Completed(result) = outcome else {
                return Err(format!("continuation did not complete: {outcome:?}").into());
            };
            assert_eq!(result.text, "resumed");
            let body = read_body(store.as_ref(), &path, &session_id)
                .await?
                .into_session();
            assert_eq!(
                body.messages()
                    .iter()
                    .filter(|message| matches!(message, meerkat_core::Message::User(_)))
                    .count(),
                1
            );
            let expected =
                meerkat_core::ToolResult::new("fc_0".into(), "escaped\0\"\\\n".repeat(200), false);
            assert_eq!(
                body.messages()
                    .iter()
                    .flat_map(|message| match message {
                        meerkat_core::Message::ToolResults { results, .. } => results.as_slice(),
                        _ => &[],
                    })
                    .filter(|result| **result == expected)
                    .count(),
                1
            );
            assert!(matches!(
                body.observe_staged_callback_results(&identity)?,
                meerkat_core::session::StagedCallbackResultsObservation::AlreadyApplied {
                    results_digest: Some(_),
                    resume_effects_applied: true,
                }
            ));
            if transcript_sibling {
                let sibling_indices = body
                    .messages()
                    .iter()
                    .enumerate()
                    .filter_map(|(index, message)| {
                        matches!(
                            message,
                            meerkat_core::Message::BlockAssistant(assistant)
                                if assistant.blocks.iter().any(|block| matches!(
                                    block, meerkat_core::AssistantBlock::Text { text, .. }
                                        if text == "retained sibling transcript"
                                ))
                        )
                        .then_some(index)
                    })
                    .collect::<Vec<_>>();
                assert_eq!(
                    sibling_indices.len(),
                    1,
                    "actual transcript: {:?}",
                    body.messages()
                );
                let sibling_index = sibling_indices[0];
                assert!(matches!(&body.messages()[sibling_index],
                    meerkat_core::Message::BlockAssistant(assistant)
                        if assistant.blocks.iter().filter(|block| matches!(
                            block, meerkat_core::AssistantBlock::Image { width: 1, height: 1, .. }
                        )).count() == 1));
                let preceding = sibling_index
                    .checked_sub(1)
                    .ok_or("sibling output has no preceding results")?;
                assert!(matches!(&body.messages()[preceding],
                    meerkat_core::Message::ToolResults { results, .. }
                        if results.len() == 2 && results[0] == expected
                            && results[1].tool_use_id == "fc_sibling"));
            }
            assert_ordinary_restoration(&machine, store.as_ref(), &path, &session_id).await?;
            return TestResult::Ok(());
        }
        let row = store
            .load_input_state(
                &LogicalRuntimeId::for_session(&session_id),
                receipt.record().input_id(),
            )
            .await?
            .ok_or("continuation input")?;
        assert_eq!(
            row.state
                .runtime_semantics
                .ok_or("runtime semantics")?
                .execution_kind(),
            meerkat_core::lifecycle::RuntimeExecutionKind::ResumePending,
        );
        let (_, authority) = machine
            .prepare_next_batch_for_live_scope_authority_test(
                &session_id,
                receipt.record().input_id(),
            )
            .await?;
        let RunExecutionAuthority::Scoped(continuation_scope) = authority else {
            return Err("continuation lost its scope".into());
        };
        assert_eq!(
            continuation_scope
                .record()
                .callback_continuation
                .as_ref()
                .ok_or("callback custody")?
                .target,
            identity,
        );
        assert_eq!(
            &continuation_scope.record().admission_commit,
            receipt.record().admission_commit(),
        );
        let staged = ops
            .load_live_head(&session_id)
            .await?
            .ok_or("staged head")?;
        let admitted = head.ok_or("admitted head")?;
        assert!(
            staged
                .payload
                .used
                .checked_add(staged.payload.reserved)?
                .fits_within(
                    admitted
                        .payload
                        .used
                        .checked_add(admitted.payload.reserved)?
                ),
            "generated continuation staging must fit the already-held charge",
        );
        let permit = machine
            .scoped_effect_host()?
            .claim_callback_application(continuation_scope.clone())
            .await?;
        assert_eq!(permit.scope(), &continuation_scope);
        let claimed = ops
            .load_live_head(&session_id)
            .await?
            .ok_or("claimed head")?;
        let mut before_state: serde_json::Value =
            serde_json::from_slice(&staged.payload.request_snapshot)?;
        let mut after_state: serde_json::Value =
            serde_json::from_slice(&claimed.payload.request_snapshot)?;
        assert_eq!(
            after_state["run_callback_application_claimed"]
                [continuation_scope.record().run_id.to_string()],
            true,
        );
        before_state
            .as_object_mut()
            .ok_or("state")?
            .remove("run_callback_application_claimed");
        after_state
            .as_object_mut()
            .ok_or("state")?
            .remove("run_callback_application_claimed");
        assert_eq!(
            before_state, after_state,
            "application permission cannot charge another effect or token"
        );
        assert!(
            matches!(
                read_body(store.as_ref(), &path, &session_id)
                    .await?
                    .observe_callback_results(&identity)?
                    .results(),
                meerkat_core::session::StagedCallbackResultsObservation::Complete(_),
            ),
            "a permission claim is not an ordinary Applied receipt"
        );
        drop(permit);
        assert!(
            machine
                .claim_live_callback_application(continuation_scope)
                .await
                .is_err()
        );
        assert_eq!(ops.load_live_head(&session_id).await?, Some(claimed));
        TestResult::Ok(())
    };
    let execution = async {
        let result = assertions.await;
        barrier.release.send_replace(true);
        let cleanup = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            if machine
                .durability_reload_required(&session_id)
                .await
                .is_some()
            {
                let witness = machine
                    .current_session_registration_witness(&session_id)
                    .await
                    .ok_or("missing degraded registration")?;
                tracing::debug!("callback fixture: recovering degraded registration");
                machine
                    .recover_or_discard_reload_required_registration_if_current(&witness)
                    .await?;
                tracing::debug!("callback fixture: recovered degraded registration");
                assert!(machine.is_durability_ready(&session_id).await);
            }
            tracing::debug!("callback fixture: unregistering actor");
            machine.unregister_session(&session_id).await?;
            tracing::debug!("callback fixture: actor unregistered");
            TestResult::Ok(())
        })
        .await;
        let stopped = stop.send(()).map_err(|()| "callback server stopped early");
        result?;
        cleanup??;
        stopped?;
        TestResult::Ok(())
    };
    let (execution, server) = tokio::join!(execution, server);
    execution?;
    server?;
    if let Some(cold) = cold_recovery {
        let runtime_id = LogicalRuntimeId::for_session(&session_id);
        let authority = store
            .load_session_boundary_authority(&runtime_id)
            .await?
            .ok_or("cold session authority")?;
        drop(machine);
        drop(grant);
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while Arc::strong_count(&store) != 1 {
                tokio::task::yield_now().await;
            }
        })
        .await?;
        drop(store);
        if matches!(
            cold.disposition,
            ColdCallbackDisposition::CancelledContinuation
        ) {
            rusqlite::Connection::open(&path)?
                .execute_batch("DROP TRIGGER reject_callback_completion;")?;
        }
        let store: Arc<dyn RuntimeStore> = Arc::new(match authority {
            RuntimeSessionAuthority::WholeBlob(_) => {
                meerkat_runtime::store::SqliteRuntimeStore::new_whole_blob(&path)?
            }
            RuntimeSessionAuthority::HeadCanonical(_) => {
                meerkat_runtime::store::SqliteRuntimeStore::new_head_canonical(&path)?
            }
        });
        let machine = MeerkatMachine::persistent(
            store.clone(),
            Arc::new(meerkat_store::MemoryBlobStore::new()),
        );
        machine.prepare_bindings(session_id.clone()).await?;
        let ops = store.live_ledger_ops().ok_or("cold Live ops")?;
        let head = ops
            .load_live_head(&session_id)
            .await?
            .ok_or("cold Live head")?;
        let state: serde_json::Value = serde_json::from_slice(&head.payload.request_snapshot)?;
        match cold.disposition {
            ColdCallbackDisposition::CancelledContinuation => {
                assert_eq!(state["request_phases"][&cold.request_id], "Terminal");
                assert_eq!(head.reference.event_count, cold.head.event_count + 1);
            }
            ColdCallbackDisposition::Held {
                callback,
                request_snapshot,
            } => {
                assert_eq!(state["request_phases"][&cold.request_id], "Suspended");
                assert_eq!(head.reference.event_count, cold.head.event_count);
                for field in [
                    "source_cancellations",
                    "run_callback_records",
                    "run_callback_receipts",
                    "claim_phases",
                    "claim_credit_records",
                    "claim_credit_bytes",
                    "claim_credit_spent_records",
                    "claim_credit_spent_bytes",
                    "request_credit_spent_records",
                    "request_credit_spent_bytes",
                ] {
                    assert_eq!(state[field], request_snapshot[field], "{field}");
                }
                let results = read_body(store.as_ref(), &path, &session_id)
                    .await?
                    .observe_callback_results(&callback)?;
                assert!(matches!(
                    results.results(),
                    meerkat_core::session::StagedCallbackResultsObservation::Complete(_)
                ));
                assert!(
                    machine
                        .commit_live_callback_input_admission(source.clone(), results)
                        .await
                        .is_err(),
                    "cold late results must not escape cancellation"
                );
            }
        }
        assert_eq!(
            serde_json::to_value(
                store
                    .load_input_state(&runtime_id, &cold.input_id)
                    .await?
                    .ok_or("cold retained ordinary result")?
            )?,
            cold.ordinary_row,
        );
        assert!(
            machine
                .reconcile_live_request_cancellations(&session_id)
                .await?
                .is_empty()
        );
        assert_eq!(ops.load_live_head(&session_id).await?, Some(head));
        assert_eq!(request_count.load(Ordering::SeqCst), 2);
    }
    Ok(())
}

async fn assert_applied_failure(
    store: &dyn RuntimeStore,
    path: &Path,
    session_id: &meerkat_core::SessionId,
    input_id: &meerkat_core::lifecycle::InputId,
    request_id: &meerkat_core::ops::OperationId,
    callback: &meerkat_core::session::CallbackBatchIdentity,
    outcome: &meerkat_runtime::completion::CompletionOutcome,
) -> TestResult {
    use meerkat_runtime::completion::CompletionOutcome;
    use meerkat_runtime::live_ledger::completion::LiveRequestCompletionFact;
    use meerkat_runtime::store::live_history::LiveHistoryReadRequest;

    let CompletionOutcome::AbandonedWithError { error, .. } = outcome else {
        return Err(format!("failed-but-applied outcome lost its error: {outcome:?}").into());
    };
    assert_eq!(
        error.outcome,
        Some(meerkat_core::TurnTerminalOutcome::Failed)
    );
    assert_eq!(error.kind, meerkat_core::TurnTerminalCauseKind::LlmFailure);
    assert!(error.terminal);
    assert!(
        error
            .detail
            .as_deref()
            .is_some_and(|detail| detail.contains("explicit callback resume failure")),
        "{error:?}"
    );
    let runtime_id = LogicalRuntimeId::for_session(session_id);
    let row = store
        .load_input_state(&runtime_id, input_id)
        .await?
        .ok_or("terminal input")?;
    assert_eq!(
        row.seed.phase,
        meerkat_runtime::InputLifecycleState::Consumed
    );
    assert_eq!(
        row.seed.terminal_outcome,
        Some(meerkat_runtime::InputTerminalOutcome::Consumed)
    );
    let run_id = row.seed.last_run_id.as_ref().ok_or("terminal run")?;
    let boundary = row.seed.last_boundary_sequence.ok_or("terminal boundary")?;
    assert!(
        store
            .load_boundary_receipt(&runtime_id, run_id, boundary)
            .await?
            .is_some(),
        "failed-but-applied input must retain its actual ordinary boundary receipt"
    );
    let image = serde_json::to_value(&row)?;
    let completion = image.get("terminal_completion").ok_or("completion owner")?;
    assert_eq!(completion["phase"]["phase"], "finalized");
    let persisted: CompletionOutcome = serde_json::from_value(
        completion
            .get("outcome")
            .ok_or("retained ordinary outcome")?
            .clone(),
    )?;
    assert_eq!(
        serde_json::to_value(persisted)?,
        serde_json::to_value(outcome)?
    );
    let body = read_body(store, path, session_id).await?.into_session();
    assert!(matches!(
        body.observe_staged_callback_results(callback)?,
        meerkat_core::session::StagedCallbackResultsObservation::AlreadyApplied {
            results_digest: Some(_),
            resume_effects_applied: true,
        }
    ));
    assert_eq!(
        body.messages()
            .iter()
            .filter(|message| matches!(message, meerkat_core::Message::User(_)))
            .count(),
        1
    );
    let expected =
        meerkat_core::ToolResult::new("fc_0".into(), "escaped\0\"\\\n".repeat(200), false);
    assert_eq!(
        body.messages()
            .iter()
            .flat_map(|message| match message {
                meerkat_core::Message::ToolResults { results, .. } => results.as_slice(),
                _ => &[],
            })
            .filter(|result| **result == expected)
            .count(),
        1
    );
    let ops = store.live_ledger_ops().ok_or("ledger")?;
    let head = tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            let head = ops.load_live_head(session_id).await?.ok_or("Live head")?;
            let state: serde_json::Value = serde_json::from_slice(&head.payload.request_snapshot)?;
            if state["request_phases"][request_id.to_string()] == "Terminal" {
                break TestResult::Ok(head);
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await??;
    let history = ops
        .read_live_history(&LiveHistoryReadRequest::new(
            head.reference.clone(),
            None,
            0,
            64,
        )?)
        .await?;
    assert_eq!(
        u64::try_from(history.records().len())?,
        head.reference.event_count
    );
    let outcomes = history
        .records()
        .iter()
        .filter_map(|record| match record {
            LiveLedgerRecord::Completion(record) => match &record.event {
                LiveCompletionEvent::RequestOutcome {
                    request_id: observed,
                    outcome,
                } if observed == request_id => Some(outcome),
                _ => None,
            },
            _ => None,
        })
        .collect::<Vec<_>>();
    let [
        LiveRequestCompletionFact::OrdinaryTerminal {
            input_id: observed_input,
            run_id: observed_run,
            receipt_digest,
        },
    ] = outcomes.as_slice()
    else {
        return Err(format!("expected one ordinary terminal reference: {outcomes:?}").into());
    };
    assert_eq!(observed_input, input_id);
    assert_eq!(observed_run, run_id);
    assert_eq!(
        receipt_digest.as_str(),
        completion["phase"]["receipt_digest"]
            .as_str()
            .ok_or("receipt digest")?
    );
    Ok(())
}

async fn assert_ordinary_restoration(
    machine: &MeerkatMachine,
    store: &dyn RuntimeStore,
    path: &Path,
    session_id: &meerkat_core::SessionId,
) -> TestResult {
    let ops = store.live_ledger_ops().ok_or("ledger")?;
    let before = ops.load_live_head(session_id).await?.ok_or("Live head")?;
    let before_state: serde_json::Value = serde_json::from_slice(&before.payload.request_snapshot)?;
    let (_, completion) = machine
        .accept_input_with_completion(
            session_id,
            meerkat_runtime::Input::Prompt(meerkat_runtime::input::PromptInput::new(
                "unrelated ordinary turn after callback",
                None,
            )),
        )
        .await?;
    let ordinary = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        completion.ok_or("ordinary completion")?.wait(),
    )
    .await??;
    assert!(
        matches!(ordinary,
        meerkat_runtime::completion::CompletionOutcome::Completed(ref result)
            if result.text == "resumed"),
        "{ordinary:?}"
    );
    let after = ops.load_live_head(session_id).await?.ok_or("Live head")?;
    let after_state: serde_json::Value = serde_json::from_slice(&after.payload.request_snapshot)?;
    assert_eq!(before_state["claim_records"], after_state["claim_records"]);
    let body = read_body(store, path, session_id).await?.into_session();
    assert_eq!(
        body.messages()
            .iter()
            .filter(|message| matches!(message, meerkat_core::Message::User(_)))
            .count(),
        2
    );
    Ok(())
}
