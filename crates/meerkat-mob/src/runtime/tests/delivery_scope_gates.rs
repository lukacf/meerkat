#![cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]
//! Delivery scope correctness gates (a downstream app's numbering 1-6 and 8).
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

/// How long a row waits for its mob's destroy to complete, retries included:
/// a hang guard, not a latency bound. A starved run can archive a member many
/// times slower than the retirement budget, so two minutes; a destroy that
/// never completes still fails the row.
const DESTROY_GUARD: Duration = Duration::from_secs(120);

/// Destroy `handle`'s mob to its terminal report. A member's archive slower
/// than the retirement budget its disposal waits for (2 s in tests, which a
/// loaded run exceeds) leaves the destroy Incomplete while the archive keeps
/// running in its process-owned task. The destroy's contract is to be retried
/// ([`MobHandle::destroy`]), and a retry finds the archive done. Only that
/// Incomplete is retried, within [`DESTROY_GUARD`]; any other answer fails
/// the row.
async fn destroy_to_terminal(handle: &MobHandle) {
    let started = tokio::time::Instant::now();
    let mut attempts = 0_u32;
    loop {
        attempts += 1;
        let Ok(answer) = tokio::time::timeout_at(started + DESTROY_GUARD, handle.destroy()).await
        else {
            panic!("the mob destroy did not finish in {DESTROY_GUARD:?} ({attempts} attempts)");
        };
        match answer {
            Ok(_) => return,
            Err(crate::runtime::handle::MobDestroyError::Incomplete { report })
                if report
                    .errors
                    .iter()
                    .any(|error| error.contains("retirement remains in progress")) =>
            {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            Err(error) => panic!("destroy the mob (attempt {attempts}): {error:?}"),
        }
    }
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
                outcome: meerkat_client::LlmDoneOutcome::success(meerkat_core::StopReason::EndTurn),
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
    service: Arc<Service>,
}

struct Fixture {
    root: tempfile::TempDir,
    owner: Owner,
    identity: AgentIdentity,
    session_id: SessionId,
    client: ScriptedClient,
}

/// The realm's session service over the fixture's SQLite files.
async fn open_service(
    root: &std::path::Path,
    client: &ScriptedClient,
) -> (Arc<Service>, Arc<FaultInjectingRuntimeStore>) {
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
    (service, store)
}

fn owner_definition(label: &str) -> MobDefinition {
    let mut definition = with_unique_mob_id(sample_definition(), label);
    let lead = definition
        .profiles
        .get_mut(&ProfileName::from("lead"))
        .and_then(ProfileBinding::as_inline_mut)
        .expect("inline lead");
    lead.model = "gpt-5.5".to_string();
    lead.runtime_mode = crate::MobRuntimeMode::TurnDriven;
    definition
}

/// The mob's own durable storage: its member history survives a reopen.
fn mob_storage(root: &std::path::Path) -> MobStorage {
    MobStorage::persistent(root.join("mob.db")).expect("persistent mob storage")
}

async fn open_owner(root: &std::path::Path, client: &ScriptedClient, label: &str) -> Owner {
    let (service, store) = open_service(root, client).await;
    let handle = MobBuilder::new(owner_definition(label), mob_storage(root))
        .with_session_service(service.clone())
        .with_default_llm_client(Arc::new(client.clone()))
        .create()
        .await
        .expect("create real turn-driven mob");
    Owner {
        handle,
        store,
        service,
    }
}

/// The same mob, resumed from its durable history over fresh services.
async fn resume_owner(root: &std::path::Path, client: &ScriptedClient) -> Owner {
    let (service, store) = open_service(root, client).await;
    let resume = MobBuilder::for_resume(mob_storage(root))
        .with_session_service(service.clone())
        .with_default_llm_client(Arc::new(client.clone()))
        .notify_orchestrator_on_resume(false)
        .resume();
    let handle = tokio::time::timeout(WAIT, resume)
        .await
        .expect("mob resume finishes")
        .expect("resume the same mob from its durable history");
    Owner {
        handle,
        store,
        service,
    }
}

async fn spawn_member(handle: &MobHandle, identity: &AgentIdentity) -> SessionId {
    handle
        .spawn(ProfileName::from("lead"), identity.clone(), None)
        .await
        .expect("spawn turn-driven member")
        .bridge_session_id()
        .expect("local session member")
        .clone()
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
        // Unique per fixture: the member turn admission test gates are
        // process-wide and keyed by identity, so tests sharing one libtest
        // process must never share a member identity.
        let identity = AgentIdentity::from(format!("scoped-member-{}", Uuid::new_v4().simple()));
        let session_id = spawn_member(&owner.handle, &identity).await;
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

    /// Stop this owner and resume the same mob from its durable history over
    /// fresh services on the same SQLite files.
    async fn reopen(self) -> (Owner, tempfile::TempDir) {
        self.client.release();
        let mob_id = self.owner.handle.mob_id().clone();
        tokio::time::timeout(WAIT, self.owner.handle.shutdown())
            .await
            .expect("mob shutdown finishes")
            .expect("mob shutdown");
        drop(self.owner);
        await_supervisor_route_release(&mob_id).await;
        let root_path = self
            .root
            .path()
            .canonicalize()
            .expect("absolute fixture root");
        let owner = resume_owner(&root_path, &self.client).await;
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

/// A second member B of the fixture's mob with one completed scoped delivery:
/// B's session, B's genuine scope and the delivery whose result B holds.
async fn second_member_with_completed_work(
    fixture: &Fixture,
) -> (SessionId, MemberDeliveryScope, MobDeliveryIdentity) {
    let member_b = AgentIdentity::from("other-member");
    let session_b = spawn_member(fixture.handle(), &member_b).await;
    let scope_b = fixture
        .handle()
        .capture_member_delivery_scope(&member_b)
        .await
        .expect("capture member B's scope");
    let delivery = delivery("member-b-work");
    fixture
        .handle()
        .submit_work_with_mode_and_delivery_identity_bounded(
            &scope_b,
            work("member B's work"),
            HandlingMode::Queue,
            delivery.clone(),
            deadline(),
        )
        .await
        .expect("scoped submit to member B");
    fixture
        .handle()
        .wait_bounded_work_for_identity_with_delivery_identity(
            &member_b,
            &delivery,
            &result_spec(),
            deadline(),
        )
        .await
        .expect("member B's delivery reaches its exact terminal");
    (session_b, scope_b, delivery)
}

/// Member B's genuine scope keeps answering from B's own session.
async fn assert_member_b_recovers(
    fixture: &Fixture,
    scope_b: &MemberDeliveryScope,
    delivery: &MobDeliveryIdentity,
) {
    match fixture.recover(scope_b, delivery).await {
        ScopedWorkState::Terminal { result, .. } => assert!(result.is_ok(), "{result:?}"),
        other => panic!("expected member B's own terminal, got {other:?}"),
    }
}

// Ownership: member A's persisted scope with only its nested session replaced
// by member B's never reports B's work under A. The scope selects; this mob's
// own member history decides what it may read.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_scope_naming_another_members_session_reads_nothing() {
    let fixture = Fixture::new().await;
    let (session_b, scope_b, delivery) = second_member_with_completed_work(&fixture).await;

    let mut persisted = fixture.capture().await.to_json_value();
    persisted["session_id"] = serde_json::to_value(&session_b).expect("session id");
    let substituted =
        MemberDeliveryScope::from_json_value(persisted).expect("a syntactically valid scope");
    assert_eq!(substituted.agent_identity(), &fixture.identity);
    assert_eq!(substituted.session_id(), &session_b);

    match fixture.recover(&substituted, &delivery).await {
        ScopedWorkState::Unresolved {
            cause: ScopedRecoveryUnresolved::ScopeBindingUnproven,
        } => {}
        other => panic!("member A's scope reported member B's session: {other:?}"),
    }
    // Both genuine scopes still answer from their own sessions.
    assert_member_b_recovers(&fixture, &scope_b, &delivery).await;
    let own = fixture.capture().await;
    assert!(matches!(
        fixture.recover(&own, &delivery).await,
        ScopedWorkState::Absent
    ));
    fixture.finish().await;
}

// Ownership: member A's persisted scope with BOTH its runtime incarnation and
// its session replaced by member B's (A's fence left) is not B's scope either.
// The history binds each incarnation and session to one fence.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_scope_naming_another_members_incarnation_and_session_reads_nothing() {
    let fixture = Fixture::new().await;
    let (session_b, scope_b, delivery) = second_member_with_completed_work(&fixture).await;
    let own = fixture.capture().await;
    assert_ne!(
        own.fence_token(),
        scope_b.fence_token(),
        "each member realization holds its own fence"
    );

    let mut persisted = own.to_json_value();
    persisted["runtime_id"] = serde_json::to_value(scope_b.runtime_id()).expect("runtime id");
    persisted["session_id"] = serde_json::to_value(&session_b).expect("session id");
    let substituted =
        MemberDeliveryScope::from_json_value(persisted).expect("a syntactically valid scope");
    assert_eq!(substituted.runtime_id(), scope_b.runtime_id());
    assert_eq!(substituted.session_id(), &session_b);
    assert_eq!(substituted.fence_token(), own.fence_token());

    match fixture.recover(&substituted, &delivery).await {
        ScopedWorkState::Unresolved {
            cause: ScopedRecoveryUnresolved::ScopeBindingUnproven,
        } => {}
        other => panic!("member A's scope reported member B's incarnation and session: {other:?}"),
    }
    assert_member_b_recovers(&fixture, &scope_b, &delivery).await;
    fixture.finish().await;
}

// Ownership: a scope belongs to the mob that bound its session. Another mob
// over the same realm services, with a member of the same identity and
// incarnation, never answers for it; the owning mob still does. The scope
// names the owning mob's incarnation, which the other mob does not host, so
// the other mob reads nothing. Re-stamped with the other mob's own
// incarnation, the scope passes that check and is still refused: the other
// mob's member history never bound the owning mob's session.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn another_mob_never_answers_for_a_scope_it_did_not_bind() {
    let fixture = Fixture::new().await;
    let scope = fixture.capture().await;
    let delivery = delivery("owning-mobs-work");
    fixture
        .submit(&scope, "the owning mob's work", &delivery)
        .await
        .expect("scoped submit");
    fixture.wait_terminal(&delivery).await;

    let foreign = MobBuilder::new(
        owner_definition("delivery-scope-foreign"),
        MobStorage::in_memory(),
    )
    .with_session_service(fixture.owner.service.clone())
    .with_default_llm_client(Arc::new(fixture.client.clone()))
    .create()
    .await
    .expect("create another mob in the same realm");
    let foreign_session = spawn_member(&foreign, &fixture.identity).await;
    assert_ne!(foreign_session, fixture.session_id);
    let foreign_scope = foreign
        .capture_member_delivery_scope(&fixture.identity)
        .await
        .expect("capture the foreign member's scope");
    assert_eq!(
        foreign_scope.runtime_id(),
        scope.runtime_id(),
        "the same identity and incarnation in both mobs"
    );

    match Fixture::recover_with(&foreign, &scope, &delivery).await {
        ScopedWorkState::Unresolved {
            cause: ScopedRecoveryUnresolved::IncarnationNotHosted,
        } => {}
        other => panic!("another mob answered for this scope: {other:?}"),
    }
    let mut persisted = scope.to_json_value();
    persisted["mob_incarnation"] = foreign_scope.to_json_value()["mob_incarnation"].clone();
    let restamped =
        MemberDeliveryScope::from_json_value(persisted).expect("a syntactically valid scope");
    assert_eq!(restamped.session_id(), &fixture.session_id);
    match Fixture::recover_with(&foreign, &restamped, &delivery).await {
        ScopedWorkState::Unresolved {
            cause: ScopedRecoveryUnresolved::ScopeBindingUnproven,
        } => {}
        other => panic!("another mob answered for the owning mob's session: {other:?}"),
    }
    match fixture.recover(&scope, &delivery).await {
        ScopedWorkState::Terminal { result, .. } => assert!(result.is_ok(), "{result:?}"),
        other => panic!("the owning mob must still recover its scope, got {other:?}"),
    }
    let _ = tokio::time::timeout(WAIT, foreign.shutdown()).await;
    fixture.finish().await;
}

// Genuine recovery after a rotation and a restart: the member's session moved
// after the original submit, so the original binding is history and no longer
// current. The resumed mob still proves and reads it, and the recorded
// rotation stays provable too.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_rotated_members_original_scope_recovers_after_restart() {
    let fixture = Fixture::new().await;
    let scope = fixture.capture().await;
    let delivery = delivery("rotated-then-restarted");
    fixture
        .submit(&scope, "accepted before the move", &delivery)
        .await
        .expect("scoped submit");
    fixture.wait_terminal(&delivery).await;
    let moved_to = SessionId::new();
    fixture
        .handle()
        .rebind_member_session_for_test(&fixture.identity, moved_to.clone())
        .await
        .expect("move the member's session binding");
    let rotated = fixture.capture().await;
    assert_eq!(
        rotated.session_id(),
        &moved_to,
        "the original binding is no longer current"
    );

    let (owner, _root) = fixture.reopen().await;
    match Fixture::recover_with(&owner.handle, &scope, &delivery).await {
        ScopedWorkState::Terminal { result, .. } => assert!(result.is_ok(), "{result:?}"),
        other => panic!("expected the original terminal after rotation and restart, got {other:?}"),
    }
    match Fixture::recover_with(&owner.handle, &rotated, &delivery).await {
        ScopedWorkState::Unresolved {
            cause: ScopedRecoveryUnresolved::OriginalSessionUnknown,
        } => {}
        other => panic!("expected the rotated binding read and its session unknown, got {other:?}"),
    }
    let _ = tokio::time::timeout(WAIT, owner.handle.shutdown()).await;
}

fn test_principal(name: &str) -> meerkat_core::auth::PrincipalId {
    meerkat_core::auth::PrincipalId::new(name).expect("valid test principal id")
}

/// A clone of `handle` acting as the named non-owner principal.
fn as_principal(handle: &MobHandle, name: &str) -> MobHandle {
    handle
        .clone()
        .with_command_authority(crate::control_policy::CommandAuthority::principal(
            crate::control_policy::MobControlPrincipal::External(test_principal(name)),
        ))
}

async fn recover_as(
    handle: &MobHandle,
    scope: &MemberDeliveryScope,
    delivery: &MobDeliveryIdentity,
) -> Result<ScopedWorkState, MobError> {
    handle
        .recover_bounded_work_at_scope(scope, delivery, &result_spec(), deadline())
        .await
        .map(|recovery| recovery.into_parts().1)
}

fn assert_refused_by_the_history_read_gate(outcome: Result<ScopedWorkState, MobError>, who: &str) {
    match outcome {
        Err(MobError::ScopeDenied(denial)) => assert_eq!(
            denial.required,
            crate::machines::mob_machine::ControlScope::ReadHistory,
            "{who}: refused by the history-read gate"
        ),
        other => panic!("{who} must be refused before any read, got {other:?}"),
    }
}

/// A fixture whose member completed one scoped delivery, for the read-gate
/// controls: the fixture, the genuine scope and the delivery.
async fn completed_scoped_delivery(
    label: &str,
) -> (Fixture, MemberDeliveryScope, MobDeliveryIdentity) {
    let fixture = Fixture::new().await;
    let scope = fixture.capture().await;
    let delivery = delivery(label);
    fixture
        .submit(&scope, "work behind the history-read gate", &delivery)
        .await
        .expect("scoped submit");
    fixture.wait_terminal(&delivery).await;
    (fixture, scope, delivery)
}

async fn grant_reader(
    fixture: &Fixture,
    name: &str,
    scopes: &[crate::machines::mob_machine::ControlScope],
    expires_at_ms: Option<u64>,
) {
    fixture
        .handle()
        .grant_scopes(
            crate::control_policy::MobControlPrincipal::Owner,
            test_principal(name),
            scopes.iter().copied().collect(),
            expires_at_ms,
        )
        .await
        .expect("the owner records the grant");
}

fn assert_recovers_the_genuine_work(outcome: Result<ScopedWorkState, MobError>, who: &str) {
    match outcome {
        Ok(ScopedWorkState::Terminal { result, .. }) => {
            assert!(result.is_ok(), "{who}: {result:?}");
        }
        other => panic!("{who} recovers the genuine work, got {other:?}"),
    }
}

// Read authorization 1: the event-journal scope alone never reaches the
// original session's work. The history-read gate refuses it before any read:
// with the session's reads failing, the refusal still comes first.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_event_journal_scope_alone_cannot_read_scoped_work() {
    use crate::machines::mob_machine::ControlScope;
    let (fixture, scope, delivery) = completed_scoped_delivery("journal-only-authority").await;
    grant_reader(
        &fixture,
        "journal-only",
        &[ControlScope::SubscribeEvents],
        None,
    )
    .await;
    let reader = as_principal(fixture.handle(), "journal-only");
    assert_refused_by_the_history_read_gate(
        recover_as(&reader, &scope, &delivery).await,
        "the event-journal scope alone",
    );
    fixture.owner.store.fail_input_reads(&fixture.session_id);
    assert_refused_by_the_history_read_gate(
        recover_as(&reader, &scope, &delivery).await,
        "the event-journal scope alone, with the session's reads failing",
    );
    fixture.finish().await;
}

// Read authorization 2: the history-read scope alone, without the
// event-journal scope, reaches otherwise-permitted recovery, and its read is
// real: with the session's reads failing it reaches the outage.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_read_history_scope_alone_recovers_genuine_work() {
    use crate::machines::mob_machine::ControlScope;
    let (fixture, scope, delivery) = completed_scoped_delivery("history-only-authority").await;
    grant_reader(&fixture, "history-only", &[ControlScope::ReadHistory], None).await;
    let reader = as_principal(fixture.handle(), "history-only");
    assert_recovers_the_genuine_work(
        recover_as(&reader, &scope, &delivery).await,
        "the ReadHistory scope alone",
    );
    fixture.owner.store.fail_input_reads(&fixture.session_id);
    match recover_as(&reader, &scope, &delivery).await {
        Ok(ScopedWorkState::Unresolved {
            cause: ScopedRecoveryUnresolved::OriginalOwnerUnavailable { .. },
        }) => {}
        other => panic!("the authorized read reaches the failing store, got {other:?}"),
    }
    fixture.finish().await;
}

// Read authorization 3: the gate reads the caller's CURRENT grant. An expired
// grant is refused; a revoked grant is refused after it had read.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_expired_or_revoked_history_reader_is_refused() {
    use crate::machines::mob_machine::ControlScope;
    let (fixture, scope, delivery) = completed_scoped_delivery("expired-or-revoked").await;
    grant_reader(
        &fixture,
        "expired-reader",
        &[ControlScope::ReadHistory],
        Some(1),
    )
    .await;
    assert_refused_by_the_history_read_gate(
        recover_as(
            &as_principal(fixture.handle(), "expired-reader"),
            &scope,
            &delivery,
        )
        .await,
        "an expired ReadHistory grant",
    );
    grant_reader(
        &fixture,
        "revoked-reader",
        &[ControlScope::ReadHistory],
        None,
    )
    .await;
    let revoked = as_principal(fixture.handle(), "revoked-reader");
    assert_recovers_the_genuine_work(
        recover_as(&revoked, &scope, &delivery).await,
        "a current ReadHistory grant",
    );
    fixture
        .handle()
        .revoke_scopes(
            crate::control_policy::MobControlPrincipal::Owner,
            test_principal("revoked-reader"),
            None,
        )
        .await
        .expect("the owner revokes the grant");
    assert_refused_by_the_history_read_gate(
        recover_as(&revoked, &scope, &delivery).await,
        "a revoked ReadHistory grant",
    );
    fixture.finish().await;
}

// Read authorization 4a: the gate never consults the member's current
// binding. After the member's session rotates, a history reader still
// recovers the genuine old scope.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_history_reader_recovers_a_genuine_scope_after_its_session_rotated() {
    use crate::machines::mob_machine::ControlScope;
    let (fixture, scope, delivery) = completed_scoped_delivery("reader-after-rotation").await;
    grant_reader(
        &fixture,
        "history-reader",
        &[ControlScope::ReadHistory],
        None,
    )
    .await;
    fixture
        .handle()
        .rebind_member_session_for_test(&fixture.identity, SessionId::new())
        .await
        .expect("move the member's session binding");
    assert_recovers_the_genuine_work(
        recover_as(
            &as_principal(fixture.handle(), "history-reader"),
            &scope,
            &delivery,
        )
        .await,
        "a history reader after the rotation",
    );
    fixture.finish().await;
}

// Read authorization 4b: retiring the member ends its binding; a history
// reader still recovers the genuine old scope.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_history_reader_recovers_a_genuine_scope_after_its_member_retired() {
    use crate::machines::mob_machine::ControlScope;
    let (fixture, scope, delivery) = completed_scoped_delivery("reader-after-retirement").await;
    grant_reader(
        &fixture,
        "history-reader",
        &[ControlScope::ReadHistory],
        None,
    )
    .await;
    // A plain `retire` answers typed `MemberRetirementInProgress` once its
    // 2 s test budget elapses while the saga keeps running, which a loaded
    // in-process run reaches; join the saga to its terminal reply instead.
    retire_to_terminal(fixture.handle(), &fixture.identity)
        .await
        .expect("retire the member to its terminal");
    assert!(
        fixture
            .handle()
            .capture_member_delivery_scope(&fixture.identity)
            .await
            .is_err(),
        "the retired member has no current scope"
    );
    assert_recovers_the_genuine_work(
        recover_as(
            &as_principal(fixture.handle(), "history-reader"),
            &scope,
            &delivery,
        )
        .await,
        "a history reader after the retirement",
    );
    fixture.finish().await;
}

// A scope belongs to the incarnation of the mob it was captured at. The mob is
// destroyed and created again under the same id on the same store and realm,
// and the member is spawned again at the same generation: the old scope is
// refused typed before anything is admitted (never StaleDeliveryScope, whose
// recovery is a re-capture that would address the new mob's member), scoped
// recovery with it reads no session, and the same submit with a scope
// captured at the new incarnation is admitted.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_scope_from_a_destroyed_incarnation_never_reaches_the_mob_created_again() {
    let fixture = Fixture::new().await;
    let old_scope = fixture.capture().await;
    let destroyed = fixture.handle().incarnation();
    let definition = fixture.handle().definition().clone();
    let Fixture {
        root,
        owner,
        identity,
        session_id: old_session,
        client,
    } = fixture;
    destroy_to_terminal(&owner.handle).await;
    let Owner {
        handle,
        store,
        service,
    } = owner;
    drop(handle);
    await_supervisor_route_release(&definition.id).await;

    let root_path = root.path().canonicalize().expect("absolute fixture root");
    let again = MobBuilder::new(definition, mob_storage(&root_path))
        .with_session_service(service.clone())
        .with_default_llm_client(Arc::new(client.clone()))
        .create()
        .await
        .expect("create the mob again under its id on the same store");
    let current = again.incarnation();
    assert_ne!(current, destroyed, "a create mints a new incarnation");
    let new_session = spawn_member(&again, &identity).await;
    assert_ne!(new_session, old_session);
    let new_scope = again
        .capture_member_delivery_scope(&identity)
        .await
        .expect("capture the member's scope at the new incarnation");
    assert_eq!(
        new_scope.runtime_id(),
        old_scope.runtime_id(),
        "the member runs at the same generation in both incarnations"
    );

    let inputs = |session: SessionId| {
        let store = store.clone();
        async move {
            store
                .load_input_states(&LogicalRuntimeId::for_session(&session))
                .await
                .expect("store read")
                .len()
        }
    };
    let inputs_before = inputs(new_session.clone()).await;
    let delivery = delivery("scope-of-a-destroyed-incarnation");
    let text = "one scoped delivery";
    let refused = again
        .submit_work_with_mode_and_delivery_identity_bounded(
            &old_scope,
            work(text),
            HandlingMode::Queue,
            delivery.clone(),
            deadline(),
        )
        .await
        .expect_err("a scope from the destroyed incarnation is refused");
    match refused {
        MobError::IncarnationMismatch {
            mob_id,
            expected,
            current: running,
        } => {
            assert_eq!(&mob_id, again.mob_id());
            assert_eq!(expected, Some(destroyed));
            assert_eq!(running, Some(current));
        }
        other => panic!("expected IncarnationMismatch, got {other:?}"),
    }
    assert_eq!(
        inputs(new_session.clone()).await,
        inputs_before,
        "the refused submit admitted nothing in the new member's session"
    );
    for session in [&old_session, &new_session] {
        let row = store
            .load_input_state_by_idempotency_key(
                &LogicalRuntimeId::for_session(session),
                &meerkat_runtime::IdempotencyKey::new(delivery.idempotency_key.clone()),
            )
            .await
            .expect("store read");
        assert!(
            row.is_none(),
            "the refused submit claimed nothing in {session:?}"
        );
    }
    match Fixture::recover_with(&again, &old_scope, &delivery).await {
        ScopedWorkState::Unresolved {
            cause: ScopedRecoveryUnresolved::IncarnationNotHosted,
        } => {}
        other => panic!("expected IncarnationNotHosted, got {other:?}"),
    }

    // Control: the same submit with the scope captured at the new incarnation.
    let receipt = again
        .submit_work_with_mode_and_delivery_identity_bounded(
            &new_scope,
            work(text),
            HandlingMode::Queue,
            delivery.clone(),
            deadline(),
        )
        .await
        .expect("a scope captured at the new incarnation is admitted");
    assert_eq!(receipt.stage, crate::WorkAdmissionStage::IngressAccepted);
    assert_eq!(receipt.session_id.as_ref(), Some(&new_session));
    client.release();
    let _ = tokio::time::timeout(WAIT, again.shutdown()).await;
}
