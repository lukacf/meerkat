#![cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]
//! Delivery scope correctness gates (Toolkit numbering 1-6 and 8).
//!
//! A host captures a member's exact delivery scope, persists it, submits
//! against it, and recovers from it after a lost reply. Every test runs a real
//! turn-driven member over a real factory, a real SQLite session store and a
//! real head-canonical SQLite runtime store (behind the fault-injecting
//! decorator only where a gate needs a store fault). Admission is held at
//! deterministic barriers; no assertion waits on a clock.

use super::actor_isolation::FaultInjectingRuntimeStore;
use super::*;
use crate::runtime::{MemberDeliveryScope, ScopedRecoveryUnresolved, ScopedWorkState};
use meerkat_runtime::{LogicalRuntimeId, RuntimeStore};

const WAIT: Duration = Duration::from_secs(30);

fn deadline() -> std::time::Instant {
    std::time::Instant::now() + WAIT
}

fn delivery(label: &str) -> MobDeliveryIdentity {
    MobDeliveryIdentity::new(label, Uuid::new_v4().to_string()).expect("stable delivery identity")
}

fn result_spec() -> BoundedResultSpec {
    BoundedResultSpec::new("delivery-scope-gate", 4096).expect("bounded result spec")
}

fn work(text: &str) -> WorkSpec {
    WorkSpec::new(text, WorkOrigin::Internal)
}

type Service = meerkat_session::PersistentSessionService<meerkat::FactoryAgentBuilder>;

#[derive(Clone)]
struct ScriptedClient {
    released: tokio::sync::watch::Sender<bool>,
}

impl ScriptedClient {
    fn new(released: bool) -> Self {
        let (released, _) = tokio::sync::watch::channel(released);
        Self { released }
    }

    fn release(&self) {
        self.released.send_replace(true);
    }
}

#[async_trait]
impl meerkat_client::LlmClient for ScriptedClient {
    fn project_replay_messages(
        &self,
        messages: &[Message],
    ) -> Result<Vec<Message>, meerkat_client::LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(
        &'a self,
        request: &'a meerkat_client::LlmRequest,
    ) -> meerkat_client::types::LlmStream<'a> {
        let mut released = self.released.subscribe();
        Box::pin(async_stream::try_stream! {
            while !*released.borrow_and_update() {
                if released.changed().await.is_err() {
                    break;
                }
            }
            yield meerkat_client::LlmEvent::TextDelta {
                delta: "scoped-answer".to_string(),
                meta: None,
            };
            yield meerkat_client::LlmEvent::UsageUpdate {
                usage: meerkat_core::TurnUsage::host_declared(
                    Provider::OpenAI,
                    &request.model,
                    Usage::default(),
                ),
            };
            yield meerkat_client::LlmEvent::Done {
                outcome: meerkat_client::LlmDoneOutcome::Success {
                    stop_reason: meerkat_core::StopReason::EndTurn,
                },
            };
        })
    }

    fn provider(&self) -> Provider {
        Provider::OpenAI
    }

    async fn health_check(&self) -> Result<(), meerkat_client::LlmError> {
        Ok(())
    }
}

/// One mob owner over the fixture's SQLite files.
struct Owner {
    handle: MobHandle,
    store: Arc<FaultInjectingRuntimeStore>,
}

struct Fixture {
    root: tempfile::TempDir,
    owner: Owner,
    identity: AgentIdentity,
    session_id: SessionId,
    client: ScriptedClient,
}

async fn open_owner(root: &std::path::Path, client: &ScriptedClient, label: &str) -> Owner {
    let factory = meerkat::AgentFactory::new(root.join("factory-store"))
        .user_config_root(root.join("config"))
        .runtime_root(root.join("runtime"))
        .project_root(root.join("project"))
        .context_root(root.join("context"))
        .builtins(false)
        .comms(false);
    let mut config = meerkat::Config::default();
    config.agent.model = "gpt-5.5".to_string();
    config.compaction.auto_compact_threshold = 1_000_000;
    let mut builder = meerkat::FactoryAgentBuilder::new(factory, config);
    builder.default_llm_client = Some(Arc::new(client.clone()));
    let db = root.join("realm.db");
    let session_store =
        Arc::new(meerkat_store::SqliteSessionStore::open(&db).expect("session store"));
    builder.default_session_store = Some(Arc::new(meerkat_store::StoreAdapter::new(
        session_store.clone(),
    )));
    let sqlite: Arc<dyn RuntimeStore> = Arc::new(
        meerkat_runtime::SqliteRuntimeStore::new_head_canonical(&db)
            .expect("head-canonical runtime store"),
    );
    let store = FaultInjectingRuntimeStore::wrapping(sqlite);
    let runtime_store: Arc<dyn RuntimeStore> = store.clone();
    let service = Arc::new(Service::new(
        builder,
        16,
        session_store,
        runtime_store,
        Arc::new(meerkat_store::MemoryBlobStore::new()),
    ));
    let mut definition = with_unique_mob_id(sample_definition(), label);
    let lead = definition
        .profiles
        .get_mut(&ProfileName::from("lead"))
        .and_then(ProfileBinding::as_inline_mut)
        .expect("inline lead");
    lead.model = "gpt-5.5".to_string();
    lead.runtime_mode = crate::MobRuntimeMode::TurnDriven;
    let handle = MobBuilder::new(definition, MobStorage::in_memory())
        .with_session_service(service)
        .with_default_llm_client(Arc::new(client.clone()))
        .create()
        .await
        .expect("create real turn-driven mob");
    Owner { handle, store }
}

impl Fixture {
    async fn new() -> Self {
        let root = tempfile::Builder::new()
            .prefix("meerkat-delivery-scope-")
            .tempdir()
            .expect("fixture storage in the system temp dir");
        let root_path = root.path().canonicalize().expect("absolute fixture root");
        for directory in ["config", "runtime", "project", "context"] {
            std::fs::create_dir_all(root_path.join(directory)).expect("fixture root");
        }
        let client = ScriptedClient::new(true);
        let owner = open_owner(&root_path, &client, "delivery-scope").await;
        let identity = AgentIdentity::from("scoped-member");
        let session_id = owner
            .handle
            .spawn(ProfileName::from("lead"), identity.clone(), None)
            .await
            .expect("spawn turn-driven member")
            .bridge_session_id()
            .expect("local session member")
            .clone();
        Self {
            root,
            owner,
            identity,
            session_id,
            client,
        }
    }

    fn handle(&self) -> &MobHandle {
        &self.owner.handle
    }

    async fn capture(&self) -> MemberDeliveryScope {
        self.handle()
            .capture_member_delivery_scope(&self.identity)
            .await
            .expect("capture the member's delivery scope")
    }

    async fn submit(
        &self,
        scope: &MemberDeliveryScope,
        text: &str,
        delivery: &MobDeliveryIdentity,
    ) -> Result<WorkDeliveryReceipt, MobError> {
        self.handle()
            .submit_work_with_mode_and_delivery_identity_bounded(
                scope,
                work(text),
                HandlingMode::Queue,
                delivery.clone(),
                deadline(),
            )
            .await
    }

    async fn recover_with(
        handle: &MobHandle,
        scope: &MemberDeliveryScope,
        delivery: &MobDeliveryIdentity,
    ) -> ScopedWorkState {
        handle
            .recover_bounded_work_at_scope(scope, delivery, &result_spec(), deadline())
            .await
            .expect("scoped recovery is observational")
            .into_parts()
            .1
    }

    async fn recover(
        &self,
        scope: &MemberDeliveryScope,
        delivery: &MobDeliveryIdentity,
    ) -> ScopedWorkState {
        Self::recover_with(self.handle(), scope, delivery).await
    }

    /// The store's own row for `delivery` in `session_id`, read directly.
    async fn stored_row(
        &self,
        session_id: &SessionId,
        delivery: &MobDeliveryIdentity,
    ) -> Option<meerkat_runtime::store::ExactInputStateObservation> {
        self.owner
            .store
            .load_input_state_by_idempotency_key(
                &LogicalRuntimeId::for_session(session_id),
                &meerkat_runtime::IdempotencyKey::new(delivery.idempotency_key.clone()),
            )
            .await
            .expect("store read")
    }

    async fn input_count(&self, session_id: &SessionId) -> usize {
        self.owner
            .store
            .load_input_states(&LogicalRuntimeId::for_session(session_id))
            .await
            .expect("store read")
            .len()
    }

    /// Wait for the delivery's exact terminal in the member's current session
    /// (an actual committed boundary, not a clock).
    async fn wait_terminal(&self, delivery: &MobDeliveryIdentity) {
        self.handle()
            .wait_bounded_work_for_identity_with_delivery_identity(
                &self.identity,
                delivery,
                &result_spec(),
                deadline(),
            )
            .await
            .expect("the delivery reaches its exact terminal");
    }

    /// Stop this owner and open a fresh one over the same SQLite files.
    async fn reopen(self) -> (Owner, tempfile::TempDir) {
        self.client.release();
        tokio::time::timeout(WAIT, self.owner.handle.shutdown())
            .await
            .expect("mob shutdown finishes")
            .expect("mob shutdown");
        drop(self.owner);
        let root_path = self
            .root
            .path()
            .canonicalize()
            .expect("absolute fixture root");
        let owner = open_owner(&root_path, &self.client, "delivery-scope-reopened").await;
        (owner, self.root)
    }

    async fn finish(self) {
        self.client.release();
        let _ = tokio::time::timeout(WAIT, self.owner.handle.shutdown()).await;
    }
}

fn is_present(state: &ScopedWorkState) -> bool {
    matches!(
        state,
        ScopedWorkState::InFlight { .. } | ScopedWorkState::Terminal { .. }
    )
}

// Gate 1 and 4 (before submit): capture is observational, the persisted scope
// survives decoding, and a moved session binding refuses it typed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn capture_is_observational_and_a_moved_session_refuses_the_saved_scope() {
    let fixture = Fixture::new().await;
    let inputs_before = fixture.input_count(&fixture.session_id).await;
    let scope = fixture.capture().await;
    assert_eq!(scope.session_id(), &fixture.session_id);
    assert_eq!(scope.agent_identity(), &fixture.identity);
    assert_eq!(
        fixture.input_count(&fixture.session_id).await,
        inputs_before,
        "capture has zero work effects"
    );

    // The host persists the scope; only the persisted bytes survive.
    let persisted = serde_json::to_string(&scope).expect("persist scope");
    drop(scope);
    let saved: MemberDeliveryScope = serde_json::from_str(&persisted).expect("decode saved scope");

    let moved_to = SessionId::new();
    fixture
        .handle()
        .rebind_member_session_for_test(&fixture.identity, moved_to.clone())
        .await
        .expect("move the member's session binding under the same runtime");

    let delivery = delivery("moved-before-submit");
    let refused = fixture
        .submit(&saved, "must not land anywhere", &delivery)
        .await
        .expect_err("a moved session binding refuses the saved scope");
    match refused {
        MobError::StaleDeliveryScope {
            agent_identity,
            expected_session,
            actual_session,
        } => {
            assert_eq!(agent_identity, fixture.identity);
            assert_eq!(expected_session, fixture.session_id);
            assert_eq!(actual_session, Some(moved_to.clone()));
        }
        other => panic!("expected StaleDeliveryScope, got {other:?}"),
    }
    assert_eq!(
        fixture.input_count(&fixture.session_id).await,
        inputs_before,
        "the refused submit admitted nothing in the original session"
    );
    assert!(
        fixture.stored_row(&moved_to, &delivery).await.is_none(),
        "the refused submit was never retargeted to the current session"
    );
    fixture.finish().await;
}

// Gate 2: the reply is lost after the runtime admitted the input; the owner is
// destroyed and reopened; recovery from the saved scope finds the original
// input and its exact terminal. Exactly one input, no repair, no resubmit.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_reply_lost_after_admission_recovers_the_original_input_after_reopen() {
    let fixture = Fixture::new().await;
    let scope = fixture.capture().await;
    let delivery = delivery("lost-after-admission");
    let inputs_before = fixture.input_count(&fixture.session_id).await;
    let (entered, release) = MobHandle::arm_member_turn_admission_test_gate(
        fixture.identity.clone(),
        crate::MemberTurnAdmissionTestStage::AfterRuntimeAdmission,
    );
    let submit = fixture
        .handle()
        .submit_work_with_mode_and_delivery_identity_bounded(
            &scope,
            work("lost reply work"),
            HandlingMode::Queue,
            delivery.clone(),
            std::time::Instant::now() + Duration::from_secs(2),
        );
    let (lost, ()) = tokio::join!(submit, async {
        entered
            .await
            .expect("admission reached the post-admission gate");
    });
    // The runtime admitted the input, but the caller's deadline elapsed at
    // the gate: the reply is lost.
    assert!(
        matches!(lost, Err(MobError::ActorCommandTimedOut { .. })),
        "got {lost:?}"
    );
    let _ = release.send(());
    fixture.wait_terminal(&delivery).await;

    let (owner, _root) = fixture.reopen().await;
    let recovered = Fixture::recover_with(&owner.handle, &scope, &delivery).await;
    match recovered {
        ScopedWorkState::Terminal { result, .. } => {
            assert!(result.is_ok(), "the original turn completed: {result:?}");
        }
        other => panic!("expected the original terminal, got {other:?}"),
    }
    let count = owner
        .store
        .load_input_states(&LogicalRuntimeId::for_session(scope.session_id()))
        .await
        .expect("store read")
        .len();
    assert_eq!(
        count,
        inputs_before + 1,
        "exactly one input for the delivery"
    );
    let _ = tokio::time::timeout(WAIT, owner.handle.shutdown()).await;
}

// Gate 3: the reply is lost while admission is still blocked before the
// runtime input exists. Recovery reports an authoritative miss, which is not
// retry permission, then sees the late original admission.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_reply_lost_before_admission_reports_absent_then_the_late_original() {
    let fixture = Fixture::new().await;
    let scope = fixture.capture().await;
    let delivery = delivery("lost-before-admission");
    let (entered, release) = MobHandle::arm_member_turn_admission_test_gate(
        fixture.identity.clone(),
        crate::MemberTurnAdmissionTestStage::BeforeRuntimeAdmission,
    );
    let (admitted, release_admitted) = MobHandle::arm_member_turn_admission_test_gate(
        fixture.identity.clone(),
        crate::MemberTurnAdmissionTestStage::AfterRuntimeAdmission,
    );
    let submit = fixture
        .handle()
        .submit_work_with_mode_and_delivery_identity_bounded(
            &scope,
            work("late admission work"),
            HandlingMode::Queue,
            delivery.clone(),
            std::time::Instant::now() + Duration::from_secs(2),
        );
    let (lost, ()) = tokio::join!(submit, async {
        entered
            .await
            .expect("admission reached the pre-admission gate");
    });
    assert!(
        matches!(lost, Err(MobError::ActorCommandTimedOut { .. })),
        "got {lost:?}"
    );

    assert!(
        matches!(
            fixture.recover(&scope, &delivery).await,
            ScopedWorkState::Absent
        ),
        "no runtime input exists yet"
    );
    assert!(
        fixture
            .stored_row(&fixture.session_id, &delivery)
            .await
            .is_none(),
        "recovery never resubmitted"
    );

    let _ = release.send(());
    admitted.await.expect("the late original admission lands");
    let late = fixture.recover(&scope, &delivery).await;
    assert!(is_present(&late), "the late original is visible: {late:?}");
    let _ = release_admitted.send(());
    fixture.wait_terminal(&delivery).await;
    fixture.finish().await;
}

// Gate 4 (mid-admission): the session binding moves between MobMachine
// admission and runtime admission. The input lands only in the captured
// session, or the submit is refused typed; never in the replacement.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_rotation_mid_admission_admits_only_to_the_captured_session() {
    let fixture = Fixture::new().await;
    let scope = fixture.capture().await;
    let delivery = delivery("rotated-mid-admission");
    let (entered, release) = MobHandle::arm_member_turn_admission_test_gate(
        fixture.identity.clone(),
        crate::MemberTurnAdmissionTestStage::BeforeRuntimeAdmission,
    );
    let moved_to = SessionId::new();
    let submit = fixture
        .handle()
        .submit_work_with_mode_and_delivery_identity_bounded(
            &scope,
            work("mid-admission work"),
            HandlingMode::Queue,
            delivery.clone(),
            deadline(),
        );
    let (outcome, ()) = tokio::join!(submit, async {
        entered
            .await
            .expect("admission reached the pre-admission gate");
        fixture
            .handle()
            .rebind_member_session_for_test(&fixture.identity, moved_to.clone())
            .await
            .expect("move the binding while admission is held");
        let _ = release.send(());
    });
    match outcome {
        Ok(receipt) => {
            assert_eq!(receipt.session_id.as_ref(), Some(&fixture.session_id));
            assert!(
                fixture
                    .stored_row(&fixture.session_id, &delivery)
                    .await
                    .is_some(),
                "the admitted input is in the captured session"
            );
        }
        Err(error) => assert!(
            matches!(
                error,
                MobError::StaleDeliveryScope { .. } | MobError::StaleFenceToken { .. }
            ),
            "only a typed stale-scope refusal is acceptable, got {error:?}"
        ),
    }
    assert!(
        fixture.stored_row(&moved_to, &delivery).await.is_none(),
        "nothing was admitted to the replacement session"
    );
    fixture.finish().await;
}

// Gate 5: the binding moves after acceptance. Recovery reads the original
// session even though the current one lacks the key; an unknown original
// session is unresolved, never absent.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_rotation_after_acceptance_recovers_from_the_original_session() {
    let fixture = Fixture::new().await;
    let scope = fixture.capture().await;
    let delivery = delivery("rotated-after-acceptance");
    let receipt = fixture
        .submit(&scope, "accepted before the move", &delivery)
        .await
        .expect("scoped submit");
    assert_eq!(receipt.stage, crate::WorkAdmissionStage::IngressAccepted);
    assert_eq!(receipt.session_id.as_ref(), Some(&fixture.session_id));
    fixture.wait_terminal(&delivery).await;

    let moved_to = SessionId::new();
    fixture
        .handle()
        .rebind_member_session_for_test(&fixture.identity, moved_to.clone())
        .await
        .expect("move the member's session binding");

    match fixture.recover(&scope, &delivery).await {
        ScopedWorkState::Terminal { result, .. } => assert!(result.is_ok(), "{result:?}"),
        other => panic!("expected the original terminal, got {other:?}"),
    }
    // Today's session has never seen this key; the identity-wide recovery
    // reads it and cannot see the original delivery.
    let current = fixture
        .handle()
        .recover_bounded_work_for_identity_with_delivery_identity(
            &fixture.identity,
            &delivery,
            &result_spec(),
        )
        .await
        .expect("identity-wide recovery");
    assert!(
        !matches!(current.work(), DurableBoundedWorkState::Terminal { .. }),
        "the current session lacks the original key"
    );

    // A scope naming a session the store never knew stays unresolved.
    let unknown = fixture.capture().await;
    assert_eq!(unknown.session_id(), &moved_to);
    match fixture.recover(&unknown, &delivery).await {
        ScopedWorkState::Unresolved {
            cause: ScopedRecoveryUnresolved::OriginalSessionUnknown,
        } => {}
        other => panic!("expected an unresolved unknown session, got {other:?}"),
    }
    fixture.finish().await;
}

// Gate 6: one key at one scope is one input. Reusing the key with changed
// bytes never creates a second effect and never overwrites the original row.
// Conflict rejection on changed bytes is the exact-replay witness's job
// (part 2); this only proves no duplicate and no fabricated witness.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_key_at_one_scope_is_one_input_and_changed_bytes_never_overwrite() {
    let fixture = Fixture::new().await;
    let scope = fixture.capture().await;
    let delivery = delivery("reused-key");
    let inputs_before = fixture.input_count(&fixture.session_id).await;
    fixture
        .submit(&scope, "original bytes", &delivery)
        .await
        .expect("first scoped submit");
    fixture.wait_terminal(&delivery).await;
    let original = fixture
        .stored_row(&fixture.session_id, &delivery)
        .await
        .expect("the original input row");

    let _ = fixture.submit(&scope, "original bytes", &delivery).await;
    let _ = fixture.submit(&scope, "changed bytes", &delivery).await;

    assert_eq!(
        fixture.input_count(&fixture.session_id).await,
        inputs_before + 1,
        "reusing the key never creates a second input"
    );
    let after = fixture
        .stored_row(&fixture.session_id, &delivery)
        .await
        .expect("the original input row survives");
    assert_eq!(
        after.state().state.input_id,
        original.state().state.input_id
    );
    assert_eq!(
        after.exact_row_digest(),
        original.exact_row_digest(),
        "the original row is byte-identical: never overwritten"
    );
    assert_eq!(
        after.state().state.prompt_replay_identity,
        original.state().state.prompt_replay_identity,
        "no exact replay witness was fabricated or replaced"
    );
    fixture.finish().await;
}

// Gate 8: store failure, late timeout, caller cancellation and restart
// without a live waiter. None of them claims durability or absence it cannot
// prove, and the saved scope stays usable.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn store_failure_timeout_cancellation_and_restart_keep_the_scope_and_claim_nothing() {
    let fixture = Fixture::new().await;
    let scope = fixture.capture().await;

    // A submit whose deadline already passed is refused before admission.
    let expired = delivery("expired-deadline");
    let refused = fixture
        .handle()
        .submit_work_with_mode_and_delivery_identity_bounded(
            &scope,
            work("never admitted"),
            HandlingMode::Queue,
            expired.clone(),
            std::time::Instant::now(),
        )
        .await;
    assert!(
        matches!(refused, Err(MobError::ActorCommandTimedOut { .. })),
        "got {refused:?}"
    );
    assert!(matches!(
        fixture.recover(&scope, &expired).await,
        ScopedWorkState::Absent
    ));

    // Caller cancellation does not cancel an admission already on its lane.
    let cancelled = delivery("cancelled-caller");
    let (entered, release) = MobHandle::arm_member_turn_admission_test_gate(
        fixture.identity.clone(),
        crate::MemberTurnAdmissionTestStage::BeforeRuntimeAdmission,
    );
    let (admitted, release_admitted) = MobHandle::arm_member_turn_admission_test_gate(
        fixture.identity.clone(),
        crate::MemberTurnAdmissionTestStage::AfterRuntimeAdmission,
    );
    let task = tokio::spawn({
        let handle = fixture.handle().clone();
        let scope = scope.clone();
        let cancelled = cancelled.clone();
        async move {
            handle
                .submit_work_with_mode_and_delivery_identity_bounded(
                    &scope,
                    work("caller leaves"),
                    HandlingMode::Queue,
                    cancelled,
                    deadline(),
                )
                .await
        }
    });
    entered
        .await
        .expect("admission reached the pre-admission gate");
    task.abort();
    let _ = task.await;
    let _ = release.send(());
    admitted
        .await
        .expect("the admission proceeds without its caller");
    assert!(is_present(&fixture.recover(&scope, &cancelled).await));
    let _ = release_admitted.send(());
    fixture.wait_terminal(&cancelled).await;

    // A store outage is unresolved, never absent.
    fixture.owner.store.fail_input_reads(&fixture.session_id);
    match fixture.recover(&scope, &cancelled).await {
        ScopedWorkState::Unresolved {
            cause: ScopedRecoveryUnresolved::OriginalOwnerUnavailable { .. },
        } => {}
        other => panic!("expected an unresolved store outage, got {other:?}"),
    }

    // Restart without a live waiter: the reopened owner still answers from
    // the saved scope.
    let (owner, _root) = fixture.reopen().await;
    assert!(is_present(
        &Fixture::recover_with(&owner.handle, &scope, &cancelled).await
    ));
    assert!(matches!(
        Fixture::recover_with(&owner.handle, &scope, &expired).await,
        ScopedWorkState::Absent
    ));
    let _ = tokio::time::timeout(WAIT, owner.handle.shutdown()).await;
}
