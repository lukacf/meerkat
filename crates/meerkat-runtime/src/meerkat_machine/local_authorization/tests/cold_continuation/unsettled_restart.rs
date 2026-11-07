//! A governed run whose round commit stays unsettled past its retry budget
//! on a host store realm, across a restart.
//!
//! A governed prompt (authenticated fresh, under its own association) runs
//! on a host store realm. Its executor commits the run's first round as the
//! session layer does: a provisional tail whose run cursor binds the prompt.
//! The store then answers the next round's commit `Indeterminate`, applying
//! nothing, and every retry of it and every head read `Unavailable`, so
//! nothing the writer asks settles it. The settle ends at its retry budget
//! and poisons the runtime's scope, and the executor fails the run as the
//! session layer answers that write (the teardown class: the live actor to be
//! reloaded from what the store holds). The process ends as a crash ends it. The restart,
//! once the store answers again and the old lease ran out, marks the run
//! resumable at its registration. Its prompt's controller client did not
//! survive the store, so the runtime asks the host for it again with the
//! prompt's stored association: a host that supplies it resumes the run
//! from its tail once, and a host that has no client for it settles the run
//! as a refused resume before it serves, nothing of it running again.

use super::*;
use crate::store::spi::test_host::TestHost;
use crate::store::spi::{SpiRealm, SpiRealmConfig, SpiRuntimeStore};
use crate::store::{PreparedRuntimeSessionCommit, PreparedWholeBlobProvisionalTail};
use meerkat_core::lifecycle::core_executor::BoundSessionCommit;
use meerkat_core::types::{
    AssistantBlock, BlockAssistantMessage, Message, StopReason, TranscriptMessageIdentity,
    UserMessage,
};

/// The retry budget of the row's settles: a few retries, well inside the
/// writer's 20 s self-fence window, which renews keep open anyway.
const SETTLE_BUDGET: Duration = Duration::from_secs(2);

/// One round's assistant message as the model returned it: one call of a
/// durable tool under `call`.
fn staged_call(run: &RunId, call: &str, message: &str) -> BlockAssistantMessage {
    BlockAssistantMessage {
        blocks: vec![AssistantBlock::ToolUse {
            id: call.to_string(),
            name: "durable_tool".to_string(),
            args: serde_json::value::RawValue::from_string("{}".to_string())
                .expect("raw arguments"),
            meta: None,
        }],
        stop_reason: Some(StopReason::ToolUse),
        identity: TranscriptMessageIdentity::default().with_run_id(run.clone()),
        created_at: meerkat_core::types::message_timestamp_now(),
        assistant_message_id: Some(
            serde_json::from_value(serde_json::Value::String(message.to_string()))
                .expect("assistant message id"),
        ),
    }
}

/// The session layer's stand-in for one governed run: it commits the run's
/// rounds over the store the runtime writes, as the session's checkpointer
/// does. The first round's commit lands; the store leaves the second
/// round's unsettled, and the run fails on it.
struct UnsettledRoundExecutor {
    session: SessionId,
    store: Arc<dyn RuntimeStore>,
    host: Arc<TestHost>,
    entered: tokio::sync::mpsc::UnboundedSender<(RunId, Vec<InputId>)>,
}

impl UnsettledRoundExecutor {
    /// The run's provisional tail at round `round` (zero-based) on the
    /// committed `base`: its cursor binds `inputs`, with the round's call
    /// staged.
    fn round_tail(
        base: &meerkat_core::Session,
        run: &RunId,
        inputs: &[InputId],
        round: u64,
        message: &str,
    ) -> meerkat_core::Session {
        let mut tail = base.clone();
        tail.set_run_cursor(&meerkat_core::SessionRunCursor::new(
            run.clone(),
            inputs.to_vec(),
            round,
            meerkat_core::RunResumePoint::ToolsPending {
                assistant_message: staged_call(run, &format!("round-{round}"), message),
            },
        ))
        .expect("the run cursor binds the inputs");
        tail
    }
}

#[async_trait::async_trait]
impl CoreExecutor for UnsettledRoundExecutor {
    fn supports_work_authorization(&self) -> bool {
        true
    }

    async fn apply(
        &mut self,
        run: RunId,
        primitive: RunPrimitive,
    ) -> Result<CoreApplyOutput, CoreExecutorError> {
        let inputs = primitive.contributing_input_ids().to_vec();
        let _ = self.entered.send((run.clone(), inputs.clone()));
        let runtime = LogicalRuntimeId::for_session(&self.session);
        let mut committed = meerkat_core::Session::with_id(self.session.clone());
        committed.push(Message::User(UserMessage::text("the governed prompt")));
        let base = self
            .store
            .commit_prepared_session_boundary(
                &runtime,
                PreparedRuntimeSessionCommit::snapshot_only(
                    BoundSessionCommit::sealed(Arc::new(committed.clone()))
                        .expect("a sealed boundary"),
                ),
            )
            .await
            .expect("the session's committed document")
            .authority()
            .and_then(|authority| authority.whole_blob())
            .cloned()
            .expect("a WholeBlob store authority");
        // The first round commits.
        let first = Self::round_tail(
            &committed,
            &run,
            &inputs,
            0,
            "0190b5f0-0000-7000-8000-00000000a001",
        );
        self.store
            .write_prepared_whole_blob_provisional_tail(
                &runtime,
                PreparedWholeBlobProvisionalTail::prepare(
                    base.clone(),
                    run.clone(),
                    1,
                    &BoundSessionCommit::sealed(Arc::new(first)).expect("a sealed round"),
                )
                .expect("the first round's tail"),
            )
            .await
            .expect("the first round commits");
        // The second round's commit: `Indeterminate` with nothing applied,
        // then `Unavailable` to every retry and every head read, until the
        // settle's budget ends.
        {
            let mut faults = self.host.faults.lock().unwrap();
            faults.indeterminate_without_apply = 1;
            faults.unavailable = 1000;
            faults.load_head_unavailable = 1000;
        }
        let second = Self::round_tail(
            &committed,
            &run,
            &inputs,
            1,
            "0190b5f0-0000-7000-8000-00000000a002",
        );
        let written = self
            .store
            .write_prepared_whole_blob_provisional_tail(
                &runtime,
                PreparedWholeBlobProvisionalTail::prepare(
                    base,
                    run.clone(),
                    2,
                    &BoundSessionCommit::sealed(Arc::new(second)).expect("a sealed round"),
                )
                .expect("the second round's tail"),
            )
            .await;
        {
            let mut faults = self.host.faults.lock().unwrap();
            faults.unavailable = 0;
            faults.load_head_unavailable = 0;
        }
        let error = written.expect_err("the second round's commit never settles");
        // As the session's checkpointer answers a WholeBlob checkpoint write
        // whose batch stayed unsettled: the teardown class, the live actor to
        // be reloaded from what the store holds.
        Err(CoreExecutorError::apply_failed_from_session_error(
            meerkat_core::service::SessionError::Agent(
                meerkat_core::AgentError::session_durable_projection_authority_unknown(format!(
                    "session {}: its checkpoint may or may not have landed ({error}); live \
                     actor reload required",
                    self.session
                )),
            ),
        ))
    }

    async fn cancel_after_boundary(&mut self, _: String) -> Result<(), CoreExecutorError> {
        Ok(())
    }

    async fn stop_runtime_executor(&mut self, _: String) -> Result<(), CoreExecutorError> {
        Ok(())
    }
}

/// The session layer's stand-in for the restart's resumed run: a resumed
/// dispatch continues the run's provisional tail, as the session service's
/// does, so its boundary promotes that tail with the receipt the store
/// derives for it, and the store's authority for the boundary is
/// acknowledged. Each run it is asked for is reported with its inputs and
/// whether its work authorization carries a controller client.
struct ResumingExecutor {
    session: SessionId,
    store: Arc<dyn RuntimeStore>,
    entered: tokio::sync::mpsc::UnboundedSender<(RunId, Vec<InputId>, bool)>,
}

#[async_trait::async_trait]
impl CoreExecutor for ResumingExecutor {
    fn supports_work_authorization(&self) -> bool {
        true
    }

    async fn apply(
        &mut self,
        run: RunId,
        primitive: RunPrimitive,
    ) -> Result<CoreApplyOutput, CoreExecutorError> {
        let inputs = primitive.contributing_input_ids().to_vec();
        let metadata = primitive.turn_metadata();
        let on_client = metadata
            .and_then(|metadata| metadata.work_authorization.as_ref())
            .and_then(|context| context.controller_client())
            .is_some();
        let _ = self.entered.send((run.clone(), inputs.clone(), on_client));
        if metadata.and_then(|metadata| metadata.execution_kind)
            != Some(meerkat_core::lifecycle::RuntimeExecutionKind::ResumeRun)
        {
            return Err(CoreExecutorError::apply_failed_runtime_turn(
                "only the kept run resumes on this executor",
            ));
        }
        let continuation =
            crate::recovery::observe_open_run_continuation(self.store.as_ref(), &self.session)
                .await
                .ok()
                .flatten()
                .filter(|continuation| continuation.run_id == run)
                .ok_or_else(|| {
                    CoreExecutorError::apply_failed_runtime_turn(format!(
                        "resumed run {run} has no provisional tail to continue from"
                    ))
                })?;
        let receipt = continuation.receipt;
        Ok(CoreApplyOutput::new(
            RunBoundaryReceiptDraft {
                run_id: run,
                boundary: primitive.apply_boundary(),
                contributing_input_ids: inputs,
                conversation_digest: Some(receipt.conversation_digest().to_string()),
                message_count: usize::try_from(receipt.message_count())
                    .expect("a receipt's message count"),
            },
            None,
        )
        .with_bound_session(BoundSessionCommit::provisional_promotion(receipt)))
    }

    async fn acknowledge_committed_session_boundary(
        &mut self,
        _: &meerkat_core::CommittedSessionBoundaryAuthority,
    ) -> Result<(), CoreExecutorError> {
        Ok(())
    }

    async fn cancel_after_boundary(&mut self, _: String) -> Result<(), CoreExecutorError> {
        Ok(())
    }

    async fn stop_runtime_executor(&mut self, _: String) -> Result<(), CoreExecutorError> {
        Ok(())
    }
}

/// A governed persistent machine over `store`, with the host's `owners`,
/// once the owners of a machine the crash dropped are released.
async fn governed_machine(
    store: &Arc<dyn RuntimeStore>,
    blobs: &Arc<MemoryBlobStore>,
    owners: &HostOwners,
) -> Arc<MeerkatMachine> {
    tokio::time::timeout(BOUND, async {
        loop {
            match MeerkatMachine::persistent_with_local_grant_authorization(
                Arc::clone(store),
                Some(Arc::clone(blobs) as Arc<dyn meerkat_core::BlobStore>),
                owners.configuration(),
            ) {
                Ok(machine) => return Arc::new(machine),
                Err(RuntimeDriverError::ControllerReadinessUnavailable {
                    reason: ControllerReadinessFailure::Busy,
                }) => tokio::task::yield_now().await,
                Err(error) => panic!("governed persistent machine: {error:?}"),
            }
        }
    })
    .await
    .expect("the crashed machine's owners released")
}

/// What the first process did before it ended.
#[derive(Debug)]
struct FirstProcess {
    run: RunId,
    bound: Vec<InputId>,
    prompt: InputId,
    /// Each poisoned scope, and why.
    poisoned_scopes: Vec<(String, String)>,
    realm_poisoned: Option<String>,
}

/// The first process, on a runtime of its own: the governed prompt runs and
/// fails on its unsettled round commit. It reports once the runtime's scope
/// is poisoned, then lives on until its runtime is shut down.
async fn first_process(
    host: Arc<TestHost>,
    blobs: Arc<MemoryBlobStore>,
    owners: HostOwners,
    template: Input,
    session: SessionId,
    report: tokio::sync::oneshot::Sender<FirstProcess>,
) {
    let realm = SpiRealm::open(
        Arc::clone(&host) as _,
        SpiRealmConfig {
            commit_retry_budget: SETTLE_BUDGET,
            ..SpiRealmConfig::default()
        },
    )
    .await
    .expect("the realm opens");
    let store: Arc<dyn RuntimeStore> = Arc::new(
        SpiRuntimeStore::open(Arc::clone(&realm))
            .await
            .expect("the runtime store opens"),
    );
    let machine = governed_machine(&store, &blobs, &owners).await;
    machine
        .register_session(session.clone())
        .await
        .expect("actual registration");
    machine
        .prepare_bindings(session.clone())
        .await
        .expect("actual runtime binding");
    let (entered, mut applied) = tokio::sync::mpsc::unbounded_channel();
    machine
        .register_session_with_executor(
            session.clone(),
            Box::new(UnsettledRoundExecutor {
                session: session.clone(),
                store: Arc::clone(&store),
                host: Arc::clone(&host),
                entered,
            }),
        )
        .await
        .expect("actual executor attachment");
    install_credential(
        &machine,
        &EphemeralTokenStore::new(),
        &controller_selection(&template),
    )
    .await;
    let runtime = LogicalRuntimeId::for_session(&session);
    let original = original_prompt(&template, &runtime);
    owners.custody.hold(&original);
    bind_destination(&machine, &session, &original).await;
    let prompt = original.id().clone();
    let outcome = machine
        .ingest(&runtime, original)
        .await
        .expect("governed original admitted");
    assert!(
        matches!(outcome, AcceptOutcome::Accepted { .. }),
        "{outcome:?}"
    );
    let (run, bound) = tokio::time::timeout(BOUND, applied.recv())
        .await
        .expect("the run starts within bound")
        .expect("the executor reports its run");
    let poisoned_by = tokio::time::Instant::now() + BOUND;
    while realm.poisoned_scopes().is_empty() && tokio::time::Instant::now() < poisoned_by {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    // The runtime loop meets the run's failure on the poisoned scope.
    tokio::time::sleep(Duration::from_secs(1)).await;
    let _ = report.send(FirstProcess {
        run,
        bound,
        prompt,
        poisoned_scopes: realm
            .poisoned_scopes()
            .into_iter()
            .map(|scope| (scope.id.to_string(), scope.cause))
            .collect(),
        realm_poisoned: realm.realm_poisoned(),
    });
    std::future::pending::<()>().await;
}

/// What the restart starts from: the first process ran the governed prompt,
/// failed on its unsettled round commit and crashed; then the store answers
/// again and the crashed process's lease ran out, and the restart's realm and
/// runtime store are open over it.
struct Restart {
    owners: HostOwners,
    template: Input,
    blobs: Arc<MemoryBlobStore>,
    session: SessionId,
    first: FirstProcess,
    store: Arc<dyn RuntimeStore>,
}

async fn restart_after_an_unsettled_round_commit() -> Restart {
    let (configuration, template, _account, _publication) = mutable_controller_configuration(true);
    let owners = owners_of(&configuration, &template);
    drop(configuration);
    let host = Arc::new(TestHost::new());
    let blobs = Arc::new(MemoryBlobStore::new());
    let session = SessionId::new();
    let runtime = LogicalRuntimeId::for_session(&session);

    let first_runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("the first process's runtime");
    let (report, reported) = tokio::sync::oneshot::channel();
    drop(first_runtime.spawn(first_process(
        Arc::clone(&host),
        Arc::clone(&blobs),
        owners.clone(),
        template.clone(),
        session.clone(),
        report,
    )));
    let first = tokio::time::timeout(BOUND * 3, reported)
        .await
        .expect("the first process reports")
        .expect("the first process sends its report");
    // The crash: every task of the first process ends where it is.
    first_runtime.shutdown_background();
    eprintln!("first process: {first:#?}");
    assert_eq!(
        first.bound,
        vec![first.prompt.clone()],
        "the run binds the prompt"
    );
    assert!(
        matches!(first.poisoned_scopes.as_slice(), [(scope, cause)]
            if scope.starts_with("rt:") && cause.contains("unavailable")),
        "the runtime's scope alone is poisoned, by the unsettled batch: {first:#?}"
    );
    assert!(
        first.realm_poisoned.is_none(),
        "the realm keeps its fence: {first:#?}"
    );

    // The store answers again, and the crashed process's lease ran out.
    host.expire_lease();
    let realm = SpiRealm::open(Arc::clone(&host) as _, SpiRealmConfig::default())
        .await
        .expect("the restart's realm opens");
    let store: Arc<dyn RuntimeStore> = Arc::new(
        SpiRuntimeStore::open(Arc::clone(&realm))
            .await
            .expect("the restart's runtime store opens"),
    );
    let tail = store
        .load_whole_blob_provisional_tail(&runtime)
        .await
        .expect("the restart reads the run's tail")
        .map(|tail| tail.authority().run_id().clone());
    assert_eq!(
        tail,
        Some(first.run.clone()),
        "the store holds the run's tail from its first round"
    );
    let kept = store
        .load_input_state(&runtime, &first.prompt)
        .await
        .expect("row read")
        .expect("the prompt's row");
    eprintln!(
        "the store at the restart: prompt {:?}, terminal {:?}, last run {:?}",
        kept.seed.phase, kept.seed.terminal_outcome, kept.seed.last_run_id
    );
    assert!(
        kept.seed.terminal_outcome.is_none(),
        "the store holds the prompt pending: {:?}",
        kept.seed.phase
    );
    Restart {
        owners,
        template,
        blobs,
        session,
        first,
        store,
    }
}

/// The restart's host has no client for the prompt's selection any more:
/// the attachment settles the run as a refused resume before it serves, and
/// nothing of it runs again.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_governed_run_whose_round_commit_was_unsettled_settles_as_a_refused_resume_at_the_restart()
 {
    let Restart {
        owners,
        blobs,
        session,
        first,
        store,
        ..
    } = restart_after_an_unsettled_round_commit().await;
    owners.controllers.withdraw_clients();
    let machine = governed_machine(&store, &blobs, &owners).await;
    machine
        .register_session(session.clone())
        .await
        .expect("the restart's registration");
    machine
        .prepare_bindings(session.clone())
        .await
        .expect("actual runtime binding");
    assert_eq!(
        machine.resumable_run_id(&session).await,
        Some(first.run.clone()),
        "the registration marks the run resumable"
    );
    let (effects, mut applied) =
        attach_registered_holding(&machine, &session, "never runs", Arc::default()).await;
    let row = terminal_row(&machine, &session, &first.prompt).await;
    assert_eq!(
        row.seed.terminal_outcome,
        Some(crate::input_state::InputTerminalOutcome::Abandoned {
            reason: crate::input_state::InputAbandonReason::ResumeRefused,
        }),
        "the governed run settles as a refused resume"
    );
    assert_eq!(
        machine.resumable_run_id(&session).await,
        None,
        "the run is no longer marked"
    );
    assert_eq!(
        effects.load(Ordering::SeqCst),
        0,
        "nothing of it runs again"
    );
    assert!(
        applied.try_recv().is_err(),
        "the executor was never entered"
    );
}

/// The acceptance check of a kept governed prompt's controller: the
/// restart's host supplies the prompt's client again (its controller owner
/// has one for the stored selection, and the custody of the original, the
/// invocation and the controller admission all hold under the current
/// grants). The governed run resumes from its tail under its own id, on
/// that client, and completes once; its prompt is consumed and the run is
/// no longer marked. RED before: the attachment settled the run as a
/// refused resume, and nothing ran.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_governed_run_whose_round_commit_was_unsettled_resumes_once_when_its_host_supplies_the_client()
 {
    let Restart {
        owners,
        template,
        blobs,
        session,
        first,
        store,
    } = restart_after_an_unsettled_round_commit().await;
    let runtime = LogicalRuntimeId::for_session(&session);
    let machine = governed_machine(&store, &blobs, &owners).await;
    machine
        .register_session(session.clone())
        .await
        .expect("the restart's registration");
    machine
        .prepare_bindings(session.clone())
        .await
        .expect("actual runtime binding");
    assert_eq!(
        machine.resumable_run_id(&session).await,
        Some(first.run.clone()),
        "the registration marks the run resumable"
    );
    // A resumed run does not stage again: its destination is bound before
    // anything is attached.
    let destination = original_prompt(&template, &runtime)
        .header()
        .authority_association
        .as_ref()
        .expect("claims")
        .candidate()
        .target
        .clone();
    bind_when_settled(&machine, &session, &destination).await;
    let (entered, mut applied) = tokio::sync::mpsc::unbounded_channel();
    machine
        .register_session_with_executor(
            session.clone(),
            Box::new(ResumingExecutor {
                session: session.clone(),
                store: Arc::clone(&store),
                entered,
            }),
        )
        .await
        .expect("actual executor attachment");

    let (run, inputs, on_client) = tokio::time::timeout(BOUND, applied.recv())
        .await
        .expect("the run resumes within bound")
        .expect("the executor reports the run");
    assert_eq!(run, first.run, "the run resumes under its own id");
    assert_eq!(inputs, vec![first.prompt.clone()], "with its prompt");
    assert!(on_client, "on the controller client its host supplied");
    let row = terminal_row(&machine, &session, &first.prompt).await;
    assert_eq!(
        row.seed.terminal_outcome,
        Some(crate::input_state::InputTerminalOutcome::Consumed),
        "the resumed run completes, and its prompt is consumed"
    );
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(applied.try_recv().is_err(), "the run resumed once");
    assert_eq!(
        machine.resumable_run_id(&session).await,
        None,
        "the run is no longer marked"
    );
}
