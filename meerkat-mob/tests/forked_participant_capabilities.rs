//! Source-owned forked-participant capability service conformance.
//!
//! Every scenario runs against BOTH durable backends (in-memory and SQLite) so
//! the lifecycle, idempotency, and crash-safety guarantees are properties of
//! the contract rather than of one implementation.

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use meerkat_core::SessionId;
use meerkat_core::service::SessionError;
use meerkat_mob::forked_participant::{
    ForkedParticipantAttachmentId, ForkedParticipantCapabilityId,
    ForkedParticipantCleanupAttemptId, ForkedParticipantCleanupClaim,
    ForkedParticipantCleanupClaimOutcome, ForkedParticipantCleanupPublish, ForkedParticipantError,
    ForkedParticipantOperationScope, ForkedParticipantOwnerRoute, ForkedParticipantRef,
    ForkedParticipantReleaseOutcome, ForkedParticipantRequest, ForkedParticipantRequestId,
    ForkedParticipantReusePolicy, ForkedParticipantRevocationOutcome, ForkedParticipantService,
    ForkedParticipantSourceRuntime, MAX_FORKED_PARTICIPANT_TTL, MAX_FORKED_PARTICIPANT_USES,
    PlannedChildEvidence, PlannedForkOutcome, PlannedForkRequest, SessionExecutionEvidence,
};
use meerkat_mob::ids::AgentIdentity;
use meerkat_mob::store::{ForkedParticipantStore, InMemoryForkedParticipantStore, SqliteMobStores};

const PREFIX_DIGEST: &str = "sha256:selected-prefix";
const PREFIX_COUNT: usize = 4;
const SOURCE_MEMBER: &str = "researcher";

// ---------------------------------------------------------------------------
// Fake source runtime
// ---------------------------------------------------------------------------

#[derive(Default)]
struct FakeRuntimeState {
    sources: HashMap<String, SessionExecutionEvidence>,
    children: HashMap<String, PlannedChildEvidence>,
    fork_calls: Vec<SessionId>,
    fail_next_fork: Option<String>,
    archive_failures: HashMap<String, String>,
    archive_not_found: std::collections::HashSet<String>,
    /// Sessions whose archive blocks until the test releases it. The barrier
    /// makes "a sweeper is mid-archive, holding its claim" a deterministic
    /// state instead of a sleep-and-hope race.
    archive_gates: HashMap<String, Arc<ArchiveGate>>,
    archived: Vec<SessionId>,
    /// When set, the fork saves the child but reports failure — the exact
    /// crash window between "child is durable" and "activation is recorded".
    crash_after_child_save: bool,
}

/// A two-way barrier around one archive call: the test waits for `entered`
/// and then opens `release`.
#[derive(Debug)]
struct ArchiveGate {
    entered: tokio::sync::Semaphore,
    release: tokio::sync::Semaphore,
}

impl ArchiveGate {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            entered: tokio::sync::Semaphore::new(0),
            release: tokio::sync::Semaphore::new(0),
        })
    }

    /// Block until an archive call has entered the gate.
    async fn wait_until_entered(&self) {
        let permit = self.entered.acquire().await.expect("gate open");
        permit.forget();
    }

    /// Let the blocked archive call proceed.
    fn release(&self) {
        self.release.add_permits(1);
    }
}

#[derive(Default, Clone)]
struct FakeSourceRuntime {
    state: Arc<Mutex<FakeRuntimeState>>,
}

impl FakeSourceRuntime {
    fn lock(&self) -> std::sync::MutexGuard<'_, FakeRuntimeState> {
        self.state.lock().unwrap_or_else(|error| error.into_inner())
    }

    fn register_source(&self, session_id: &SessionId, evidence: SessionExecutionEvidence) {
        self.lock().sources.insert(session_id.to_string(), evidence);
    }

    fn fail_next_fork(&self, detail: &str) {
        self.lock().fail_next_fork = Some(detail.to_string());
    }

    fn crash_after_child_save(&self, crash: bool) {
        self.lock().crash_after_child_save = crash;
    }

    /// Install a one-shot barrier so the NEXT archive of `session_id` blocks
    /// inside the sweeper, with its cleanup claim held. Later archives of the
    /// same session are not gated.
    fn gate_archive(&self, session_id: &SessionId) -> Arc<ArchiveGate> {
        let gate = ArchiveGate::new();
        self.lock()
            .archive_gates
            .insert(session_id.to_string(), Arc::clone(&gate));
        gate
    }

    fn fail_archive_not_found(&self, session_id: &SessionId) {
        self.lock().archive_not_found.insert(session_id.to_string());
    }

    fn fail_archive(&self, session_id: &SessionId, detail: &str) {
        self.lock()
            .archive_failures
            .insert(session_id.to_string(), detail.to_string());
    }

    fn clear_archive_failure(&self, session_id: &SessionId) {
        self.lock().archive_failures.remove(&session_id.to_string());
    }

    fn set_child_policy(
        &self,
        session_id: &SessionId,
        policy: Option<meerkat_core::ops::ToolAccessPolicy>,
    ) {
        if let Some(child) = self.lock().children.get_mut(&session_id.to_string()) {
            child.execution.tool_access_policy = policy;
        }
    }

    fn set_child_realm(&self, session_id: &SessionId, realm: Option<meerkat_core::RealmId>) {
        if let Some(child) = self.lock().children.get_mut(&session_id.to_string()) {
            child.execution.realm_id = realm;
        }
    }

    fn set_child_auth_binding(
        &self,
        session_id: &SessionId,
        binding: Option<meerkat_core::AuthBindingRef>,
    ) {
        if let Some(child) = self.lock().children.get_mut(&session_id.to_string()) {
            child.execution.auth_binding = binding;
        }
    }

    fn fork_calls(&self) -> usize {
        self.lock().fork_calls.len()
    }

    fn archived(&self) -> Vec<SessionId> {
        self.lock().archived.clone()
    }
}

#[async_trait]
impl ForkedParticipantSourceRuntime for FakeSourceRuntime {
    async fn session_execution_evidence(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<SessionExecutionEvidence>, SessionError> {
        Ok(self.lock().sources.get(&session_id.to_string()).cloned())
    }

    async fn fork_planned_child(
        &self,
        request: PlannedForkRequest,
    ) -> Result<PlannedForkOutcome, SessionError> {
        let mut state = self.lock();
        state
            .fork_calls
            .push(request.planned_child_session_id.clone());
        if let Some(detail) = state.fail_next_fork.take() {
            return Err(SessionError::Agent(
                meerkat_core::error::AgentError::InternalError(detail),
            ));
        }
        // The fake mirrors the real seam: the child inherits the source's
        // execution evidence verbatim, never a caller-supplied replacement.
        let source = state
            .sources
            .get(&request.source_session_id.to_string())
            .cloned()
            .unwrap_or(SessionExecutionEvidence {
                agent_identity: None,
                realm_id: None,
                tool_access_policy: None,
                auth_binding: None,
            });
        if source.agent_identity.as_ref() != Some(&request.source_identity)
            || source.realm_id.as_ref() != Some(&request.owner_realm)
        {
            return Err(SessionError::Agent(
                meerkat_core::error::AgentError::ConfigError(
                    "source session does not belong to the requested member/realm".to_string(),
                ),
            ));
        }
        let evidence = PlannedChildEvidence {
            prefix_digest: PREFIX_DIGEST.to_string(),
            prefix_message_count: request.prefix_message_count.unwrap_or(PREFIX_COUNT),
            execution: source,
        };
        state.children.insert(
            request.planned_child_session_id.to_string(),
            evidence.clone(),
        );
        if state.crash_after_child_save {
            state.crash_after_child_save = false;
            return Err(SessionError::Agent(
                meerkat_core::error::AgentError::InternalError(
                    "crashed after the child was saved".to_string(),
                ),
            ));
        }
        Ok(PlannedForkOutcome {
            child_session_id: request.planned_child_session_id,
            prefix_message_count: evidence.prefix_message_count,
            prefix_digest: evidence.prefix_digest,
        })
    }

    async fn planned_child_evidence(
        &self,
        child_session_id: &SessionId,
    ) -> Result<Option<PlannedChildEvidence>, SessionError> {
        Ok(self
            .lock()
            .children
            .get(&child_session_id.to_string())
            .cloned())
    }

    async fn archive_fork_session(&self, child_session_id: &SessionId) -> Result<(), SessionError> {
        // Take the one-shot gate (if any) WITHOUT holding the std mutex across
        // an await. It blocks exactly the FIRST archive of this session, so a
        // takeover sweeper is never blocked by the barrier meant for the
        // sweeper it superseded.
        let gate = self
            .lock()
            .archive_gates
            .remove(&child_session_id.to_string());
        if let Some(gate) = gate {
            gate.entered.add_permits(1);
            let permit = gate.release.acquire().await.expect("gate open");
            permit.forget();
        }

        let mut state = self.lock();
        if state
            .archive_not_found
            .contains(&child_session_id.to_string())
        {
            return Err(SessionError::NotFound {
                id: child_session_id.clone(),
            });
        }
        if let Some(detail) = state.archive_failures.get(&child_session_id.to_string()) {
            return Err(SessionError::Agent(
                meerkat_core::error::AgentError::InternalError(detail.clone()),
            ));
        }
        state.archived.push(child_session_id.clone());
        state.children.remove(&child_session_id.to_string());
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

struct Harness {
    service: ForkedParticipantService,
    runtime: FakeSourceRuntime,
    store: Arc<dyn ForkedParticipantStore>,
    _dir: Option<tempfile::TempDir>,
}

impl Harness {
    fn in_memory() -> Self {
        let store: Arc<dyn ForkedParticipantStore> =
            Arc::new(InMemoryForkedParticipantStore::new());
        Self::compose(store, None)
    }

    fn sqlite() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("mob.db");
        let stores = SqliteMobStores::open(&path).expect("open");
        let store: Arc<dyn ForkedParticipantStore> = Arc::new(stores.forked_participant_store());
        Self::compose(store, Some(dir))
    }

    fn compose(store: Arc<dyn ForkedParticipantStore>, dir: Option<tempfile::TempDir>) -> Self {
        let runtime = FakeSourceRuntime::default();
        let service = ForkedParticipantService::new(
            owned_route(),
            Arc::clone(&store),
            Arc::new(runtime.clone()),
        )
        .expect("service");
        Self {
            service,
            runtime,
            store,
            _dir: dir,
        }
    }

    /// Rebuild the service over the SAME durable store, as a restarted process
    /// would. A fresh service means a fresh sweeper identity.
    fn reopen(&self) -> ForkedParticipantService {
        ForkedParticipantService::new(
            owned_route(),
            Arc::clone(&self.store),
            Arc::new(self.runtime.clone()),
        )
        .expect("service")
    }

    /// Register a source session whose execution evidence the fork inherits.
    fn register_source(&self, request: &ForkedParticipantRequest) {
        self.runtime
            .register_source(&request.source_session_id, source_evidence());
    }
}

fn owned_route() -> ForkedParticipantOwnerRoute {
    ForkedParticipantOwnerRoute::Local { realm_id: realm() }
}

fn source_evidence() -> SessionExecutionEvidence {
    SessionExecutionEvidence {
        agent_identity: Some(AgentIdentity::from(SOURCE_MEMBER)),
        realm_id: Some(realm()),
        tool_access_policy: Some(meerkat_core::ops::ToolAccessPolicy::AllowList(
            ["read_file".to_string()].into_iter().collect(),
        )),
        auth_binding: Some(meerkat_core::AuthBindingRef {
            realm: realm(),
            binding: meerkat_core::connection::BindingId::parse("anthropic").expect("binding"),
            profile: None,
            origin: meerkat_core::connection::BindingOrigin::default(),
        }),
    }
}

fn realm() -> meerkat_core::RealmId {
    meerkat_core::RealmId::parse("global").expect("realm")
}

fn request(id: &str) -> ForkedParticipantRequest {
    ForkedParticipantRequest {
        request_id: ForkedParticipantRequestId::new(id).expect("request id"),
        source_identity: AgentIdentity::from(SOURCE_MEMBER),
        source_session_id: SessionId::new(),
        owner_route: owned_route(),
        prefix_message_count: Some(PREFIX_COUNT),
        scope: ForkedParticipantOperationScope::InvokeAndObserve,
        reuse: ForkedParticipantReusePolicy::OneShot,
        ttl: Duration::from_secs(600),
    }
}

fn attachment(id: &str) -> ForkedParticipantAttachmentId {
    ForkedParticipantAttachmentId::new(id).expect("attachment id")
}

/// Re-parse a capability with one serialized field replaced, exactly as a
/// tampering holder would present it. Private fields make in-memory mutation
/// impossible, so the wire is the only tampering surface.
fn tampered(
    capability: &ForkedParticipantRef,
    field: &str,
    value: serde_json::Value,
) -> ForkedParticipantRef {
    let mut encoded = serde_json::to_value(capability).expect("serialize");
    encoded
        .as_object_mut()
        .expect("capability object")
        .insert(field.to_string(), value);
    serde_json::from_value(encoded).expect("tampered capability is still well typed")
}

fn now() -> DateTime<Utc> {
    Utc::now()
}

macro_rules! backend_cases {
    ($($name:ident),+ $(,)?) => {
        $(
            mod $name {
                use super::*;

                #[tokio::test]
                async fn in_memory() {
                    super::$name(Harness::in_memory()).await;
                }

                #[tokio::test]
                async fn sqlite() {
                    super::$name(Harness::sqlite()).await;
                }
            }
        )+
    };
}

backend_cases!(
    reserve_is_idempotent_and_conflict_is_typed,
    concurrent_same_request_converges_on_one_capability,
    create_records_exact_prefix_provenance,
    create_replay_returns_the_same_capability_without_forking_again,
    crash_after_child_save_retries_onto_the_same_planned_child,
    create_failure_is_retryable_by_the_same_request,
    tampered_reference_never_resolves,
    attach_replay_does_not_increment_and_concurrent_attach_is_denied,
    older_attachment_identity_can_never_consume_a_second_use,
    release_then_bounded_reuse_then_exhaustion,
    revoke_while_attached_waits_for_the_exact_release,
    expiry_while_attached_waits_for_the_exact_release,
    cleanup_failure_is_retained_while_other_records_continue,
    lifecycle_truth_survives_reopen,
    invalid_budget_and_ttl_are_refused_before_the_machine,
    capability_reference_carries_no_credentials_or_session_body,
);

// ---------------------------------------------------------------------------
// Scenarios
// ---------------------------------------------------------------------------

async fn reserve_is_idempotent_and_conflict_is_typed(harness: Harness) {
    let request = request("req-reserve");
    harness.register_source(&request);
    let first = harness
        .service
        .reserve(&request, now())
        .await
        .expect("reserve");
    assert!(!first.replayed);

    let replay = harness
        .service
        .reserve(&request, now())
        .await
        .expect("replay");
    assert!(replay.replayed);
    assert_eq!(replay.capability_id, first.capability_id);
    assert_eq!(
        replay.planned_child_session_id, first.planned_child_session_id,
        "a replayed reservation must keep the exact planned child"
    );

    let mut conflicting = request.clone();
    conflicting.scope = ForkedParticipantOperationScope::Observe;
    let error = harness
        .service
        .reserve(&conflicting, now())
        .await
        .expect_err("a different request shape must not take a bound identity");
    assert!(
        matches!(error, ForkedParticipantError::ReservationRejected { .. }),
        "expected a typed reservation rejection, got {error:?}"
    );
}

async fn concurrent_same_request_converges_on_one_capability(harness: Harness) {
    let request = request("req-concurrent");
    harness.register_source(&request);
    let request = Arc::new(request);
    let service = Arc::new(harness.reopen());

    let mut handles = Vec::new();
    for _ in 0..4 {
        let service = Arc::clone(&service);
        let request = Arc::clone(&request);
        handles.push(tokio::spawn(async move {
            service.reserve(&request, now()).await
        }));
    }
    let mut capability_ids = Vec::new();
    for handle in handles {
        let reservation = handle.await.expect("join").expect("reserve");
        capability_ids.push(reservation.capability_id);
    }
    let first = capability_ids.first().expect("one reservation").clone();
    assert!(
        capability_ids.iter().all(|id| id == &first),
        "concurrent reserves for one request must converge on a single capability"
    );

    let all = harness.store.list_all().await.expect("list");
    assert_eq!(all.len(), 1, "exactly one durable record may exist");
}

async fn create_records_exact_prefix_provenance(harness: Harness) {
    let request = request("req-provenance");
    harness.register_source(&request);
    let capability = harness
        .service
        .create(&request, now())
        .await
        .expect("create");

    assert_eq!(
        capability.provenance().source_session_id,
        request.source_session_id
    );
    assert_eq!(capability.provenance().prefix_message_count, PREFIX_COUNT);
    assert_eq!(capability.provenance().prefix_digest, PREFIX_DIGEST);
    assert_eq!(capability.source_identity(), &request.source_identity);
    assert_eq!(capability.owner_route(), &request.owner_route);
    assert_eq!(capability.scope(), request.scope);
    assert_eq!(capability.reuse(), request.reuse);

    let record = harness
        .store
        .load_by_request_id(&request.request_id)
        .await
        .expect("load")
        .expect("present");
    assert_eq!(
        &record.planned_child_session_id,
        capability.fork_session_id(),
        "the activated fork must be the child reserved before the fork"
    );
}

async fn create_replay_returns_the_same_capability_without_forking_again(harness: Harness) {
    let request = request("req-create-replay");
    harness.register_source(&request);
    let first = harness
        .service
        .create(&request, now())
        .await
        .expect("create");
    let second = harness
        .service
        .create(&request, now())
        .await
        .expect("create replay");
    assert_eq!(first, second);
    assert_eq!(
        harness.runtime.fork_calls(),
        1,
        "a replayed create must never fork a second child"
    );
}

async fn crash_after_child_save_retries_onto_the_same_planned_child(harness: Harness) {
    let request = request("req-crash");
    harness.register_source(&request);
    harness.runtime.crash_after_child_save(true);

    let error = harness
        .service
        .create(&request, now())
        .await
        .expect_err("the crashed create must fail");
    assert!(matches!(error, ForkedParticipantError::Session(_)));

    let record = harness
        .store
        .load_by_request_id(&request.request_id)
        .await
        .expect("load")
        .expect("present");
    let planned_child = record.planned_child_session_id.clone();
    assert!(
        record.sidecar.capability_ref.is_none(),
        "a crashed create must not publish a capability"
    );

    // Reopen, as a restarted owner process would, and retry the exact request.
    let reopened = harness.reopen();
    let capability = reopened.create(&request, now()).await.expect("retry");
    assert_eq!(
        capability.fork_session_id(),
        &planned_child,
        "the retry must activate the durable child that already exists"
    );
    assert_eq!(
        harness.runtime.fork_calls(),
        1,
        "the retry must discover the saved child instead of forking again"
    );
    assert_eq!(
        harness.store.list_all().await.expect("list").len(),
        1,
        "a retry must never create a second capability record"
    );
}

async fn create_failure_is_retryable_by_the_same_request(harness: Harness) {
    let request = request("req-fork-failure");
    harness.register_source(&request);
    harness.runtime.fail_next_fork("source refused the fork");

    let error = harness
        .service
        .create(&request, now())
        .await
        .expect_err("the failed fork must surface");
    assert!(matches!(error, ForkedParticipantError::Session(_)));

    // A different request may not steal the identity while it is retryable.
    let mut foreign = request.clone();
    foreign.scope = ForkedParticipantOperationScope::Invoke;
    let error = harness
        .service
        .reserve(&foreign, now())
        .await
        .expect_err("a different request must not steal a failed reservation");
    assert!(matches!(
        error,
        ForkedParticipantError::ReservationRejected { .. }
    ));

    // The same exact request retries and succeeds.
    let capability = harness
        .service
        .create(&request, now())
        .await
        .expect("retry succeeds");
    assert_eq!(capability.provenance().prefix_digest, PREFIX_DIGEST);
    assert_eq!(harness.store.list_all().await.expect("list").len(), 1);
}

async fn tampered_reference_never_resolves(harness: Harness) {
    let request = request("req-tamper");
    harness.register_source(&request);
    let capability = harness
        .service
        .create(&request, now())
        .await
        .expect("create");

    let widened = tampered(
        &capability,
        "reuse",
        serde_json::json!({"kind": "bounded_reuse", "max_uses": 9}),
    );
    let error = harness
        .service
        .attach(&widened, &attachment("a"), true, now())
        .await
        .expect_err("a widened reuse policy must not resolve");
    assert!(
        matches!(error, ForkedParticipantError::Store(_)),
        "expected a store-level full-reference refusal, got {error:?}"
    );

    let extended = tampered(
        &capability,
        "expires_at",
        serde_json::json!(capability.expires_at() + ChronoDuration::hours(1)),
    );
    assert!(
        harness
            .service
            .attach(&extended, &attachment("a"), true, now())
            .await
            .is_err(),
        "an extended expiry must not resolve"
    );

    let rerouted = tampered(
        &capability,
        "owner_route",
        serde_json::json!({"kind": "host", "realm_id": "global", "host_id": "host-x"}),
    );
    assert!(
        harness
            .service
            .attach(&rerouted, &attachment("a"), true, now())
            .await
            .is_err(),
        "a re-pointed route must not resolve"
    );

    // The untampered reference still works, so the refusals were about the
    // tampering rather than a broken record.
    harness
        .service
        .attach(&capability, &attachment("a"), true, now())
        .await
        .expect("the exact reference still attaches");
}

async fn attach_replay_does_not_increment_and_concurrent_attach_is_denied(harness: Harness) {
    let mut request = request("req-attach");
    harness.register_source(&request);
    request.reuse = ForkedParticipantReusePolicy::BoundedReuse { max_uses: 3 };
    let capability = harness
        .service
        .create(&request, now())
        .await
        .expect("create");

    let granted = harness
        .service
        .attach(&capability, &attachment("a"), true, now())
        .await
        .expect("attach");
    assert!(!granted.replayed);
    assert_eq!(granted.use_index, 1);
    assert_eq!(granted.remaining_uses, 2);
    assert_eq!(&granted.fork_session_id, capability.fork_session_id());

    let replay = harness
        .service
        .attach(&capability, &attachment("a"), true, now())
        .await
        .expect("attach replay");
    assert!(replay.replayed);
    assert_eq!(replay.use_index, 1, "a replay must not consume a use");

    let error = harness
        .service
        .attach(&capability, &attachment("b"), true, now())
        .await
        .expect_err("a concurrent attachment must be denied");
    assert!(
        matches!(error, ForkedParticipantError::AttachDenied { .. }),
        "expected a typed attach denial, got {error:?}"
    );

    let unauthorized = harness
        .service
        .attach(&capability, &attachment("a"), false, now())
        .await
        .expect_err("an unauthorized caller must be denied");
    assert!(matches!(
        unauthorized,
        ForkedParticipantError::AttachDenied { .. }
    ));

    let record = harness
        .service
        .load_record(capability.capability_id())
        .await
        .expect("load")
        .expect("present");
    assert_eq!(
        record.machine_state.use_count, 1,
        "replays and denials must never move the use count"
    );
    assert_ne!(
        record.machine_state.lifecycle_phase,
        meerkat_mob::machines::forked_participant_lifecycle::ForkedParticipantLifecycleState::Exhausted,
        "a successful attach must not archive or exhaust the capability"
    );
}

async fn older_attachment_identity_can_never_consume_a_second_use(harness: Harness) {
    let mut request = request("req-dedup");
    harness.register_source(&request);
    request.reuse = ForkedParticipantReusePolicy::BoundedReuse { max_uses: 3 };
    let capability = harness
        .service
        .create(&request, now())
        .await
        .expect("create");

    harness
        .service
        .attach(&capability, &attachment("a"), true, now())
        .await
        .expect("attach a");
    harness
        .service
        .release(&capability, &attachment("a"))
        .await
        .expect("release a");
    harness
        .service
        .attach(&capability, &attachment("b"), true, now())
        .await
        .expect("attach b");
    harness
        .service
        .release(&capability, &attachment("b"))
        .await
        .expect("release b");

    let error = harness
        .service
        .attach(&capability, &attachment("a"), true, now())
        .await
        .expect_err("an older identity must never consume a second use");
    assert!(matches!(error, ForkedParticipantError::AttachDenied { .. }));

    let record = harness
        .service
        .load_record(capability.capability_id())
        .await
        .expect("load")
        .expect("present");
    assert_eq!(record.machine_state.use_count, 2);
    assert_eq!(record.machine_state.granted_attachment_ids.len(), 2);
}

async fn release_then_bounded_reuse_then_exhaustion(harness: Harness) {
    let mut request = request("req-reuse");
    harness.register_source(&request);
    request.reuse = ForkedParticipantReusePolicy::BoundedReuse { max_uses: 2 };
    let capability = harness
        .service
        .create(&request, now())
        .await
        .expect("create");

    harness
        .service
        .attach(&capability, &attachment("a"), true, now())
        .await
        .expect("attach a");
    let outcome = harness
        .service
        .release(&capability, &attachment("a"))
        .await
        .expect("release a");
    assert_eq!(outcome, ForkedParticipantReleaseOutcome::Reusable);

    let second = harness
        .service
        .attach(&capability, &attachment("b"), true, now())
        .await
        .expect("attach b");
    assert_eq!(second.use_index, 2);
    assert_eq!(second.remaining_uses, 0);

    let outcome = harness
        .service
        .release(&capability, &attachment("b"))
        .await
        .expect("release b");
    assert_eq!(outcome, ForkedParticipantReleaseOutcome::Exhausted);

    // Duplicate release converges instead of failing ambiguously.
    let duplicate = harness
        .service
        .release(&capability, &attachment("b"))
        .await
        .expect("duplicate release converges");
    assert_eq!(duplicate, ForkedParticipantReleaseOutcome::Replayed);

    // Nothing was archived on exhaustion; only the cleanup sweep archives.
    assert!(harness.runtime.archived().is_empty());
}

async fn revoke_while_attached_waits_for_the_exact_release(harness: Harness) {
    let mut request = request("req-revoke");
    harness.register_source(&request);
    request.reuse = ForkedParticipantReusePolicy::BoundedReuse { max_uses: 3 };
    let capability = harness
        .service
        .create(&request, now())
        .await
        .expect("create");
    harness
        .service
        .attach(&capability, &attachment("a"), true, now())
        .await
        .expect("attach");

    let unauthorized = harness
        .service
        .revoke(capability.capability_id(), false)
        .await
        .expect_err("an unauthorized revoke must be denied");
    assert!(matches!(
        unauthorized,
        ForkedParticipantError::RevocationDenied { .. }
    ));

    let outcome = harness
        .service
        .revoke(capability.capability_id(), true)
        .await
        .expect("revoke");
    assert_eq!(
        outcome,
        ForkedParticipantRevocationOutcome::PendingAttachedRelease
    );

    // No new work while revocation is pending, and cleanup is not actionable.
    assert!(
        harness
            .service
            .attach(&capability, &attachment("b"), true, now())
            .await
            .is_err()
    );
    let report = harness.service.sweep_cleanup(now()).await.expect("sweep");
    assert!(report.completed.is_empty());
    assert!(report.retained.is_empty());

    let outcome = harness
        .service
        .release(&capability, &attachment("a"))
        .await
        .expect("release");
    assert_eq!(outcome, ForkedParticipantReleaseOutcome::Revoked);

    let report = harness.service.sweep_cleanup(now()).await.expect("sweep");
    assert_eq!(report.completed.len(), 1);
    assert_eq!(
        harness.runtime.archived(),
        vec![capability.fork_session_id().clone()]
    );
}

async fn expiry_while_attached_waits_for_the_exact_release(harness: Harness) {
    let mut request = request("req-expiry");
    harness.register_source(&request);
    request.reuse = ForkedParticipantReusePolicy::BoundedReuse { max_uses: 3 };
    request.ttl = Duration::from_secs(60);
    let created_at = now();
    let capability = harness
        .service
        .create(&request, created_at)
        .await
        .expect("create");
    harness
        .service
        .attach(&capability, &attachment("a"), true, created_at)
        .await
        .expect("attach");

    let after_expiry = capability.expires_at() + ChronoDuration::seconds(1);
    let report = harness
        .service
        .sweep_expiry(after_expiry)
        .await
        .expect("expiry sweep");
    assert_eq!(report.expiry_pending_attached.len(), 1);
    assert!(report.expired.is_empty());

    // Expiry blocks new work but waits for the exact release before cleanup.
    assert!(
        harness
            .service
            .attach(&capability, &attachment("b"), true, after_expiry)
            .await
            .is_err()
    );
    let sweep = harness.service.sweep_cleanup(now()).await.expect("sweep");
    assert!(sweep.completed.is_empty());

    let outcome = harness
        .service
        .release(&capability, &attachment("a"))
        .await
        .expect("release");
    assert_eq!(outcome, ForkedParticipantReleaseOutcome::Expired);

    let sweep = harness.service.sweep_cleanup(now()).await.expect("sweep");
    assert_eq!(sweep.completed.len(), 1);
}

async fn cleanup_failure_is_retained_while_other_records_continue(harness: Harness) {
    let failing_request = request("req-cleanup-failing");
    let healthy_request = request("req-cleanup-healthy");
    harness.register_source(&failing_request);
    harness.register_source(&healthy_request);
    let failing = harness
        .service
        .create(&failing_request, now())
        .await
        .expect("create failing");
    let healthy = harness
        .service
        .create(&healthy_request, now())
        .await
        .expect("create healthy");

    harness
        .service
        .revoke(failing.capability_id(), true)
        .await
        .expect("revoke failing");
    harness
        .service
        .revoke(healthy.capability_id(), true)
        .await
        .expect("revoke healthy");

    harness
        .runtime
        .fail_archive(failing.fork_session_id(), "archive refused");

    let report = harness.service.sweep_cleanup(now()).await.expect("sweep");
    assert_eq!(
        report.retained.len(),
        1,
        "the failing record must retain typed debt"
    );
    assert_eq!(
        report.completed.len(),
        1,
        "a failing record must not abort the sweep for later records"
    );
    assert_eq!(
        harness.runtime.archived(),
        vec![healthy.fork_session_id().clone()]
    );

    let record = harness
        .service
        .load_record(failing.capability_id())
        .await
        .expect("load")
        .expect("present");
    let debt = record.cleanup_debt.expect("typed debt is persisted");
    assert_eq!(debt.attempts, 1);
    assert!(
        debt.last_error.contains("source session failure"),
        "typed session failure detail must be retained: {}",
        debt.last_error
    );

    // A retried sweep raises the attempt count while the failure persists.
    let report = harness.service.sweep_cleanup(now()).await.expect("sweep");
    assert_eq!(report.retained.len(), 1);
    let record = harness
        .service
        .load_record(failing.capability_id())
        .await
        .expect("load")
        .expect("present");
    assert_eq!(record.cleanup_debt.expect("debt").attempts, 2);

    // Once the archive succeeds, the debt is discharged through the machine.
    harness
        .runtime
        .clear_archive_failure(failing.fork_session_id());
    let report = harness.service.sweep_cleanup(now()).await.expect("sweep");
    assert_eq!(report.completed.len(), 1);
    let record = harness
        .service
        .load_record(failing.capability_id())
        .await
        .expect("load")
        .expect("present");
    assert!(record.cleanup_debt.is_none());
    assert_eq!(
        record.machine_state.cleanup_state,
        meerkat_mob::machines::forked_participant_lifecycle::ForkedParticipantCleanupState::Complete
    );
}

async fn lifecycle_truth_survives_reopen(harness: Harness) {
    let mut request = request("req-reopen");
    harness.register_source(&request);
    request.reuse = ForkedParticipantReusePolicy::BoundedReuse { max_uses: 2 };
    let capability = harness
        .service
        .create(&request, now())
        .await
        .expect("create");
    harness
        .service
        .attach(&capability, &attachment("a"), true, now())
        .await
        .expect("attach");

    // A restarted owner sees the same lifecycle and idempotency truth.
    let reopened = harness.reopen();
    let replay = reopened
        .attach(&capability, &attachment("a"), true, now())
        .await
        .expect("replay after reopen");
    assert!(replay.replayed);
    assert_eq!(replay.use_index, 1);

    assert!(
        reopened
            .attach(&capability, &attachment("b"), true, now())
            .await
            .is_err(),
        "the attachment is still held after reopen"
    );

    reopened
        .release(&capability, &attachment("a"))
        .await
        .expect("release after reopen");

    let error = reopened
        .attach(&capability, &attachment("a"), true, now())
        .await
        .expect_err("dedup survives reopen");
    assert!(matches!(error, ForkedParticipantError::AttachDenied { .. }));

    let created_again = reopened
        .create(&request, now())
        .await
        .expect("create replay");
    assert_eq!(
        created_again, capability,
        "create replay after reopen must return the same capability"
    );
}

async fn invalid_budget_and_ttl_are_refused_before_the_machine(harness: Harness) {
    let mut over_budget = request("req-budget");
    over_budget.reuse = ForkedParticipantReusePolicy::BoundedReuse {
        max_uses: MAX_FORKED_PARTICIPANT_USES + 1,
    };
    let error = harness
        .service
        .reserve(&over_budget, now())
        .await
        .expect_err("an unbounded budget must be refused");
    assert!(matches!(
        error,
        ForkedParticipantError::InvalidRequest { .. }
    ));

    let mut zero_budget = request("req-budget-zero");
    harness.register_source(&zero_budget);
    zero_budget.reuse = ForkedParticipantReusePolicy::BoundedReuse { max_uses: 0 };
    assert!(matches!(
        harness
            .service
            .reserve(&zero_budget, now())
            .await
            .expect_err("a zero budget must be refused"),
        ForkedParticipantError::InvalidRequest { .. }
    ));

    let mut zero_ttl = request("req-ttl-zero");
    harness.register_source(&zero_ttl);
    zero_ttl.ttl = Duration::ZERO;
    assert!(matches!(
        harness
            .service
            .reserve(&zero_ttl, now())
            .await
            .expect_err("a zero ttl must be refused"),
        ForkedParticipantError::InvalidRequest { .. }
    ));

    let mut long_ttl = request("req-ttl-long");
    harness.register_source(&long_ttl);
    long_ttl.ttl = MAX_FORKED_PARTICIPANT_TTL + Duration::from_secs(1);
    assert!(matches!(
        harness
            .service
            .reserve(&long_ttl, now())
            .await
            .expect_err("an uncapped ttl must be refused"),
        ForkedParticipantError::InvalidRequest { .. }
    ));

    // Nothing was persisted for any refused request.
    assert!(harness.store.list_all().await.expect("list").is_empty());

    // A capped budget and ttl are admitted.
    let mut capped = request("req-capped");
    harness.register_source(&capped);
    capped.reuse = ForkedParticipantReusePolicy::BoundedReuse {
        max_uses: MAX_FORKED_PARTICIPANT_USES,
    };
    capped.ttl = MAX_FORKED_PARTICIPANT_TTL;
    harness
        .service
        .reserve(&capped, now())
        .await
        .expect("capped request is admitted");
}

async fn capability_reference_carries_no_credentials_or_session_body(harness: Harness) {
    let request = request("req-redaction");
    harness.register_source(&request);
    let capability = harness
        .service
        .create(&request, now())
        .await
        .expect("create");

    let encoded = serde_json::to_value(&capability).expect("serialize");
    let object = encoded.as_object().expect("capability is a JSON object");
    let mut keys: Vec<_> = object.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        vec![
            "capability_id",
            "cleanup_id",
            "expires_at",
            "fork_session_id",
            "owner_route",
            "provenance",
            "reuse",
            "revocation_id",
            "scope",
            "source_identity",
        ],
        "the capability reference must bind identities, route, provenance, scope, \
         expiry and reuse — and nothing else"
    );

    let rendered = serde_json::to_string(&encoded).expect("render");
    for forbidden in [
        "api_key",
        "apiKey",
        "authorization",
        "Bearer ",
        "credential",
        "messages",
        "transcript",
        "content",
    ] {
        assert!(
            !rendered.contains(forbidden),
            "capability reference must not carry `{forbidden}`: {rendered}"
        );
    }

    // Debug output never reveals the bearer token.
    let debug = format!("{capability:?}");
    assert!(debug.contains("[REDACTED]"));
    assert!(!debug.contains(capability.capability_id().expose_bearer_token()));

    // The reference binds identities and provenance only.
    assert_eq!(capability.provenance().prefix_digest, PREFIX_DIGEST);
}

// ---------------------------------------------------------------------------
// Inherited execution policy (source-owned fork session metadata)
// ---------------------------------------------------------------------------

/// The capability layer must not become a second policy truth: the concrete
/// tool policy and realm auth binding live on the forked child session, the
/// source is unmodified, and neither reaches the capability reference.
/// The planned-identity fork keeps the caller's child identity and the exact
/// selected prefix, leaves the source untouched, and — like every core fork —
/// deliberately strips session-authority metadata so the durable fork path is
/// the only thing that re-applies inherited execution configuration.
#[test]
fn planned_identity_fork_selects_the_exact_prefix_and_leaves_the_source_unmodified() {
    use meerkat_core::{Message, Provider, Session, SessionMetadata, SessionTooling, UserMessage};

    fn user(text: &str) -> Message {
        Message::User(UserMessage::text(text))
    }

    let realm = meerkat_core::RealmId::parse("global").expect("realm");
    let binding = meerkat_core::connection::BindingId::parse("anthropic").expect("binding");
    let concrete_policy = meerkat_core::ops::ToolAccessPolicy::AllowList(
        ["read_file".to_string()].into_iter().collect(),
    );
    let auth_binding = meerkat_core::AuthBindingRef {
        realm: realm.clone(),
        binding,
        profile: None,
        origin: meerkat_core::connection::BindingOrigin::default(),
    };

    let mut source = Session::new();
    source.push(user("first"));
    source.push(user("second"));

    let tooling = SessionTooling {
        tool_access_policy: Some(concrete_policy.clone()),
        ..SessionTooling::default()
    };
    let metadata = SessionMetadata {
        schema_version: meerkat_core::session_metadata_schema_version(),
        model: "claude-sonnet-4-6".to_string(),
        max_tokens: 4096,
        structured_output_retries: 2,
        provider: Provider::Other,
        self_hosted_server_id: None,
        provider_params: None,
        tooling,
        keep_alive: false,
        comms_name: None,
        peer_meta: None,
        realm_id: Some(realm.clone()),
        instance_id: None,
        backend: None,
        config_generation: None,
        auth_binding: Some(auth_binding.clone()),
        mob_member_binding: None,
    };
    source
        .set_session_metadata(metadata)
        .expect("source metadata");

    let planned_child = SessionId::new();
    let child = source
        .fork_at_complete_boundary_with_identity(1, planned_child.clone())
        .expect("planned fork");

    assert_eq!(
        child.id(),
        &planned_child,
        "the child must carry the planned identity"
    );
    assert_eq!(child.messages().len(), 1, "the selected prefix is exact");
    assert!(
        child
            .try_session_metadata()
            .expect("child metadata decodes")
            .is_none(),
        "a core fork strips session-authority metadata; only the durable fork \
         path re-applies inherited execution configuration"
    );

    // The source is untouched: same transcript, same identity, same policy.
    assert_eq!(source.messages().len(), 2);
    assert_ne!(source.id(), child.id());
    let source_metadata = source
        .try_session_metadata()
        .expect("source metadata decodes")
        .expect("source metadata present");
    assert_eq!(
        source_metadata.tooling.tool_access_policy,
        Some(concrete_policy)
    );
    assert_eq!(source_metadata.auth_binding, Some(auth_binding));
    assert_eq!(source_metadata.realm_id, Some(realm));
}

/// A retry must verify the durable child's inherited execution metadata
/// against the SOURCE's own evidence before recording activation. A child that
/// does not inherit the source tool policy, auth binding, or realm fails typed
/// and never activates.
#[tokio::test]
async fn retry_verifies_inherited_execution_metadata_against_source_evidence() {
    for harness in [Harness::in_memory(), Harness::sqlite()] {
        let request = request("req-inherited-policy");
        harness.register_source(&request);

        // Crash after the child was saved, then corrupt the durable child's
        // inherited execution metadata before each retry.
        harness.runtime.crash_after_child_save(true);
        harness
            .service
            .create(&request, now())
            .await
            .expect_err("the crashed create fails");
        let record = harness
            .store
            .load_by_request_id(&request.request_id)
            .await
            .expect("load")
            .expect("present");
        let planned_child = record.planned_child_session_id.clone();
        let source = source_evidence();

        harness.runtime.set_child_policy(
            &planned_child,
            Some(meerkat_core::ops::ToolAccessPolicy::Inherit),
        );
        assert!(
            matches!(
                harness
                    .service
                    .create(&request, now())
                    .await
                    .expect_err("a child that lost the source tool policy must not activate"),
                ForkedParticipantError::PlannedChildConflict { .. }
            ),
            "a broadened tool policy must be refused"
        );
        harness
            .runtime
            .set_child_policy(&planned_child, source.tool_access_policy.clone());

        harness.runtime.set_child_auth_binding(&planned_child, None);
        assert!(matches!(
            harness
                .service
                .create(&request, now())
                .await
                .expect_err("a child that lost the source auth binding must not activate"),
            ForkedParticipantError::PlannedChildConflict { .. }
        ));
        harness
            .runtime
            .set_child_auth_binding(&planned_child, source.auth_binding.clone());

        harness.runtime.set_child_realm(
            &planned_child,
            Some(meerkat_core::RealmId::parse("other").expect("realm")),
        );
        assert!(matches!(
            harness
                .service
                .create(&request, now())
                .await
                .expect_err("a foreign-realm child must not activate"),
            ForkedParticipantError::PlannedChildConflict { .. }
        ));
        harness
            .runtime
            .set_child_realm(&planned_child, source.realm_id.clone());

        // With inheritance intact, the retry activates the exact planned child.
        let capability = harness
            .service
            .create(&request, now())
            .await
            .expect("retry activates");
        assert_eq!(capability.fork_session_id(), &planned_child);
        assert_eq!(harness.runtime.fork_calls(), 1);
    }
}

/// The source session must belong to the claimed member and the owned realm,
/// and the service must refuse routes it does not own.
#[tokio::test]
async fn foreign_source_and_foreign_route_are_refused() {
    for harness in [Harness::in_memory(), Harness::sqlite()] {
        // A source with no registered evidence cannot be forked.
        let unknown = request("req-unknown-source");
        assert!(matches!(
            harness
                .service
                .reserve(&unknown, now())
                .await
                .expect_err("an unproven source must be refused"),
            ForkedParticipantError::SourceOwnershipRejected { .. }
        ));

        // A source that belongs to a different member is refused.
        let mut foreign_member = request("req-foreign-member");
        foreign_member.source_identity = AgentIdentity::from("impostor");
        harness
            .runtime
            .register_source(&foreign_member.source_session_id, source_evidence());
        assert!(matches!(
            harness
                .service
                .reserve(&foreign_member, now())
                .await
                .expect_err("a foreign member must be refused"),
            ForkedParticipantError::SourceOwnershipRejected { .. }
        ));

        // A request naming a route this owner does not serve is refused.
        let mut foreign_route = request("req-foreign-route");
        harness.register_source(&foreign_route);
        foreign_route.owner_route = ForkedParticipantOwnerRoute::Host {
            realm_id: realm(),
            host_id: meerkat_mob::machines::mob_machine::HostId::from("host-x"),
        };
        assert!(matches!(
            harness
                .service
                .reserve(&foreign_route, now())
                .await
                .expect_err("a foreign route must be refused"),
            ForkedParticipantError::ForeignRoute { .. }
        ));

        assert!(harness.store.list_all().await.expect("list").is_empty());
    }
}

/// Two sweepers must not both archive one fork, and neither may record false
/// debt for the other's work.
#[tokio::test]
async fn concurrent_cleanup_sweeps_archive_each_fork_once() {
    for harness in [Harness::in_memory(), Harness::sqlite()] {
        let request = request("req-cleanup-concurrent");
        harness.register_source(&request);
        let capability = harness
            .service
            .create(&request, now())
            .await
            .expect("create");
        harness
            .service
            .revoke(capability.capability_id(), true)
            .await
            .expect("revoke");

        // A second, independent sweeper over the same durable store.
        let other = harness.reopen();
        let sampled = now();
        let (first, second) = tokio::join!(
            harness.service.sweep_cleanup(sampled),
            other.sweep_cleanup(sampled)
        );
        let first = first.expect("first sweep");
        let second = second.expect("second sweep");

        let completed = first.completed.len() + second.completed.len();
        assert_eq!(completed, 1, "exactly one sweeper may complete the cleanup");
        assert!(
            first.retained.is_empty() && second.retained.is_empty(),
            "a sweeper that lost the claim must not record debt for the winner's work"
        );
        assert_eq!(
            harness.runtime.archived(),
            vec![capability.fork_session_id().clone()],
            "the fork must be archived exactly once"
        );

        let record = harness
            .service
            .load_record(capability.capability_id())
            .await
            .expect("load")
            .expect("present");
        assert!(
            record.cleanup_claim.is_none(),
            "a finished claim is released"
        );
        assert_eq!(
            record.machine_state.cleanup_state,
            meerkat_mob::machines::forked_participant_lifecycle::ForkedParticipantCleanupState::Complete
        );
    }
}

/// A sweeper that died holding a claim must not park the record forever: the
/// claim is reclaimable after its TTL, and the restarted sweep finishes the
/// cleanup through the machine.
#[tokio::test]
async fn a_stale_cleanup_claim_is_recovered_after_restart() {
    for harness in [Harness::in_memory(), Harness::sqlite()] {
        let request = request("req-cleanup-stale-claim");
        harness.register_source(&request);
        let capability = harness
            .service
            .create(&request, now())
            .await
            .expect("create");
        harness
            .service
            .revoke(capability.capability_id(), true)
            .await
            .expect("revoke");

        // Simulate a sweeper that claimed the record and then died.
        let mut record = harness
            .service
            .load_record(capability.capability_id())
            .await
            .expect("load")
            .expect("present");
        let claimed_at = now();
        record.cleanup_claim = Some(ForkedParticipantCleanupClaim {
            attempt_id: ForkedParticipantCleanupAttemptId::mint().expect("attempt id"),
            claimed_at,
        });
        harness.store.commit(&record).await.expect("record claim");

        // A restarted sweeper honors the live claim...
        let restarted = harness.reopen();
        let report = restarted
            .sweep_cleanup(claimed_at + ChronoDuration::seconds(1))
            .await
            .expect("sweep");
        assert_eq!(report.claimed_elsewhere.len(), 1);
        assert!(report.completed.is_empty());
        assert!(harness.runtime.archived().is_empty());

        // ...and reclaims it once the claim has gone stale.
        let report = restarted
            .sweep_cleanup(claimed_at + ChronoDuration::seconds(3600))
            .await
            .expect("sweep");
        assert_eq!(report.completed.len(), 1);
        assert_eq!(
            harness.runtime.archived(),
            vec![capability.fork_session_id().clone()]
        );
    }
}

/// An already-absent fork session converges as cleanup success rather than
/// accruing permanent debt.
#[tokio::test]
async fn cleanup_converges_when_the_fork_session_is_already_gone() {
    for harness in [Harness::in_memory(), Harness::sqlite()] {
        let request = request("req-cleanup-absent");
        harness.register_source(&request);
        let capability = harness
            .service
            .create(&request, now())
            .await
            .expect("create");
        harness
            .service
            .revoke(capability.capability_id(), true)
            .await
            .expect("revoke");
        harness
            .runtime
            .fail_archive_not_found(capability.fork_session_id());

        let report = harness.service.sweep_cleanup(now()).await.expect("sweep");
        assert_eq!(
            report.completed.len(),
            1,
            "an absent session is the state cleanup wanted"
        );
        assert!(report.retained.is_empty());
        let record = harness
            .service
            .load_record(capability.capability_id())
            .await
            .expect("load")
            .expect("present");
        assert!(record.cleanup_debt.is_none());
    }
}

/// A malformed bearer token must never resolve to a typed capability id.
#[test]
fn capability_bearer_tokens_are_validated_at_the_boundary() {
    assert!(ForkedParticipantCapabilityId::parse_bearer_token("not-a-token").is_err());
    assert!(
        serde_json::from_value::<ForkedParticipantCapabilityId>(serde_json::json!("deadbeef"))
            .is_err()
    );
    assert!(ForkedParticipantRequestId::new("  ").is_err());
    assert!(
        ForkedParticipantAttachmentId::new(
            "with
control"
        )
        .is_err()
    );
}

/// Concurrent duplicate commands must converge on the machine's own replay
/// verdict instead of leaking a storage compare-and-swap conflict, while a
/// genuinely different attachment still gets the machine's typed denial.
#[tokio::test]
async fn concurrent_duplicate_commands_converge_and_conflicting_ones_stay_typed() {
    for harness in [Harness::in_memory(), Harness::sqlite()] {
        let mut request = request("req-cas-convergence");
        request.reuse = ForkedParticipantReusePolicy::BoundedReuse { max_uses: 4 };
        harness.register_source(&request);
        let capability = harness
            .service
            .create(&request, now())
            .await
            .expect("create");

        // Four writers, one attachment identity: exactly one grant, three
        // replays, and no storage conflict escapes.
        let first = Arc::new(harness.reopen());
        let second = Arc::new(harness.reopen());
        let attachment_a = attachment("a");
        let sampled = now();
        let mut handles = Vec::new();
        for index in 0..4 {
            let service = if index % 2 == 0 {
                Arc::clone(&first)
            } else {
                Arc::clone(&second)
            };
            let capability = capability.clone();
            let attachment = attachment_a.clone();
            handles.push(tokio::spawn(async move {
                service
                    .attach(&capability, &attachment, true, sampled)
                    .await
            }));
        }
        let mut grants = Vec::new();
        for handle in handles {
            grants.push(
                handle
                    .await
                    .expect("join")
                    .expect("a duplicate attach must converge, not surface a CAS conflict"),
            );
        }
        assert_eq!(grants.iter().filter(|grant| !grant.replayed).count(), 1);
        assert_eq!(grants.iter().filter(|grant| grant.replayed).count(), 3);
        assert!(grants.iter().all(|grant| grant.use_index == 1));

        let record = harness
            .service
            .load_record(capability.capability_id())
            .await
            .expect("load")
            .expect("present");
        assert_eq!(record.machine_state.use_count, 1);

        // A different attachment racing the same record is typed Busy.
        let attachment_b = attachment("b");
        let denied = harness
            .service
            .attach(&capability, &attachment_b, true, sampled)
            .await
            .expect_err("a different concurrent attachment must be denied");
        assert!(
            matches!(denied, ForkedParticipantError::AttachDenied { .. }),
            "expected a typed attach denial, got {denied:?}"
        );

        // Duplicate releases and revokes converge the same way.
        let releases = tokio::join!(
            first.release(&capability, &attachment_a),
            second.release(&capability, &attachment_a)
        );
        releases.0.expect("first release");
        releases.1.expect("duplicate release converges");

        let revokes = tokio::join!(
            first.revoke(capability.capability_id(), true),
            second.revoke(capability.capability_id(), true)
        );
        revokes.0.expect("first revoke");
        revokes.1.expect("duplicate revoke converges");
    }
}

// ---------------------------------------------------------------------------
// Deterministic cleanup-claim fencing
// ---------------------------------------------------------------------------

/// Build one revoked capability whose cleanup debt is Pending.
async fn pending_cleanup_record(harness: &Harness, request_id: &str) -> ForkedParticipantRef {
    let request = request(request_id);
    harness.register_source(&request);
    let capability = harness
        .service
        .create(&request, now())
        .await
        .expect("create");
    harness
        .service
        .revoke(capability.capability_id(), true)
        .await
        .expect("revoke");
    capability
}

/// While one sweeper is mid-archive holding its claim, a second sweeper must
/// see the record as claimed elsewhere — with no sleeps and no polling.
#[tokio::test]
async fn a_second_sweeper_sees_a_held_claim_as_claimed_elsewhere() {
    for harness in [Harness::in_memory(), Harness::sqlite()] {
        let capability = pending_cleanup_record(&harness, "req-claim-held").await;
        let gate = harness.runtime.gate_archive(capability.fork_session_id());

        let first = Arc::new(harness.reopen());
        let sampled = now();
        let sweeping = {
            let first = Arc::clone(&first);
            tokio::spawn(async move { first.sweep_cleanup(sampled).await })
        };

        // Deterministic: the first sweeper is now inside archive, claim held.
        gate.wait_until_entered().await;

        let blocked = harness
            .reopen()
            .sweep_cleanup(sampled)
            .await
            .expect("second sweep");
        assert_eq!(
            blocked.claimed_elsewhere.len(),
            1,
            "a live claim must be visible to every other sweeper"
        );
        assert!(blocked.completed.is_empty());
        assert!(blocked.retained.is_empty());

        gate.release();
        let finished = sweeping.await.expect("join").expect("first sweep");
        assert_eq!(finished.completed.len(), 1);
        assert_eq!(
            harness.runtime.archived(),
            vec![capability.fork_session_id().clone()],
            "the fork is archived exactly once"
        );
    }
}

/// After a TTL takeover, the superseded sweeper's late outcome must not touch
/// the record: it may neither complete it nor record debt on it.
#[tokio::test]
async fn a_superseded_sweeper_cannot_publish_a_late_outcome() {
    for harness in [Harness::in_memory(), Harness::sqlite()] {
        let capability = pending_cleanup_record(&harness, "req-claim-takeover").await;
        let fork_session_id = capability.fork_session_id().clone();
        let gate = harness.runtime.gate_archive(&fork_session_id);

        // The first sweeper claims and then stalls inside archive.
        let first = Arc::new(harness.reopen());
        let claimed_at = now();
        let stalled = {
            let first = Arc::clone(&first);
            tokio::spawn(async move { first.sweep_cleanup(claimed_at).await })
        };
        gate.wait_until_entered().await;

        // A later sweeper takes the claim over once it goes stale. Its archive
        // is not gated, so it finishes and completes the record.
        let takeover_at = claimed_at + ChronoDuration::seconds(3600);
        let taker = harness
            .reopen()
            .sweep_cleanup(takeover_at)
            .await
            .expect("takeover sweep");
        assert_eq!(taker.completed.len(), 1, "the taker completes the cleanup");

        let after_takeover = harness
            .service
            .load_record(capability.capability_id())
            .await
            .expect("load")
            .expect("present");
        assert_eq!(
            after_takeover.machine_state.cleanup_state,
            meerkat_mob::machines::forked_participant_lifecycle::ForkedParticipantCleanupState::Complete
        );

        // The stalled sweeper now returns. Its archive may well have run twice
        // (session archive is idempotent), but it must publish nothing.
        gate.release();
        let late = stalled.await.expect("join").expect("late sweep");
        assert!(
            late.completed.is_empty(),
            "a superseded attempt must not complete a cleanup it no longer owns"
        );
        assert!(
            late.retained.is_empty(),
            "a superseded attempt must not record false debt"
        );
        assert_eq!(
            late.claimed_elsewhere.len(),
            1,
            "the superseded attempt reports the takeover"
        );

        let final_record = harness
            .service
            .load_record(capability.capability_id())
            .await
            .expect("load")
            .expect("present");
        assert_eq!(
            final_record.machine_state.cleanup_state,
            meerkat_mob::machines::forked_participant_lifecycle::ForkedParticipantCleanupState::Complete,
            "the record must still read as completed by the claim owner"
        );
        assert!(
            final_record.cleanup_debt.is_none(),
            "a superseded attempt must not leave debt behind"
        );
        assert!(final_record.cleanup_claim.is_none());
    }
}

/// A lease whose claim was taken over publishes nothing, and a record that is
/// no longer Pending cannot be claimed at all.
#[tokio::test]
async fn leases_fence_publication_and_completed_records_are_unclaimable() {
    for harness in [Harness::in_memory(), Harness::sqlite()] {
        let capability = pending_cleanup_record(&harness, "req-claim-lease").await;
        let sampled = now();

        let lease = match harness
            .service
            .claim_cleanup(capability.capability_id(), sampled)
            .await
            .expect("claim")
        {
            ForkedParticipantCleanupClaimOutcome::Claimed(lease) => lease,
            other => panic!("expected a claim, got {other:?}"),
        };
        assert_eq!(lease.capability_id(), capability.capability_id());

        // A live claim blocks another attempt.
        assert_eq!(
            harness
                .service
                .claim_cleanup(capability.capability_id(), sampled)
                .await
                .expect("second claim"),
            ForkedParticipantCleanupClaimOutcome::ClaimedElsewhere
        );

        // Take the claim over, then prove the first lease is inert.
        let takeover = match harness
            .service
            .claim_cleanup(
                capability.capability_id(),
                sampled + ChronoDuration::seconds(3600),
            )
            .await
            .expect("takeover claim")
        {
            ForkedParticipantCleanupClaimOutcome::Claimed(lease) => lease,
            other => panic!("expected a takeover claim, got {other:?}"),
        };
        assert_ne!(takeover.attempt_id(), lease.attempt_id());

        assert_eq!(
            harness
                .service
                .publish_cleanup_failure(&lease, "late failure".to_string(), sampled)
                .await
                .expect("stale failure publish"),
            ForkedParticipantCleanupPublish::ClaimLost
        );
        assert_eq!(
            harness
                .service
                .publish_cleanup_success(&lease)
                .await
                .expect("stale success publish"),
            ForkedParticipantCleanupPublish::ClaimLost
        );
        let untouched = harness
            .service
            .load_record(capability.capability_id())
            .await
            .expect("load")
            .expect("present");
        assert!(untouched.cleanup_debt.is_none());
        assert_eq!(
            untouched.machine_state.cleanup_state,
            meerkat_mob::machines::forked_participant_lifecycle::ForkedParticipantCleanupState::Pending
        );

        // The current claimant publishes, and the record stops being claimable.
        assert_eq!(
            harness
                .service
                .publish_cleanup_success(&takeover)
                .await
                .expect("current publish"),
            ForkedParticipantCleanupPublish::Published(())
        );
        assert_eq!(
            harness
                .service
                .claim_cleanup(
                    capability.capability_id(),
                    sampled + ChronoDuration::seconds(7200)
                )
                .await
                .expect("post-completion claim"),
            ForkedParticipantCleanupClaimOutcome::NotPending,
            "a completed record must never be claimable again"
        );
    }
}

/// Two concurrent sweeps issued by the SAME service must fence each other:
/// per-attempt claim identity, not a service-wide one.
#[tokio::test]
async fn same_service_concurrent_sweeps_complete_each_record_once() {
    for harness in [Harness::in_memory(), Harness::sqlite()] {
        let capability = pending_cleanup_record(&harness, "req-claim-same-service").await;
        let service = Arc::new(harness.reopen());
        let sampled = now();

        let (left, right) = tokio::join!(
            service.sweep_cleanup(sampled),
            service.sweep_cleanup(sampled)
        );
        let left = left.expect("left sweep");
        let right = right.expect("right sweep");

        assert_eq!(
            left.completed.len() + right.completed.len(),
            1,
            "one record may be completed exactly once, even by one service"
        );
        assert!(
            left.retained.is_empty() && right.retained.is_empty(),
            "no attempt may record debt for another attempt's work"
        );
        assert_eq!(
            harness.runtime.archived(),
            vec![capability.fork_session_id().clone()]
        );
    }
}

/// The concurrent-sweep guarantee is a property, not a lucky interleaving:
/// stress it repeatedly on both backends.
#[tokio::test]
async fn concurrent_cleanup_sweeps_are_exactly_once_under_stress() {
    for backend in ["in_memory", "sqlite"] {
        let harness = if backend == "in_memory" {
            Harness::in_memory()
        } else {
            Harness::sqlite()
        };
        let first = Arc::new(harness.reopen());
        let second = Arc::new(harness.reopen());

        for iteration in 0..50 {
            let capability =
                pending_cleanup_record(&harness, &format!("req-stress-{backend}-{iteration}"))
                    .await;
            let sampled = now();
            let (left, right) = tokio::join!(
                {
                    let first = Arc::clone(&first);
                    async move { first.sweep_cleanup(sampled).await }
                },
                {
                    let second = Arc::clone(&second);
                    async move { second.sweep_cleanup(sampled).await }
                }
            );
            let left = left.expect("left sweep");
            let right = right.expect("right sweep");

            assert_eq!(
                left.completed.len() + right.completed.len(),
                1,
                "{backend} iteration {iteration}: exactly one completion"
            );
            assert!(
                left.retained.is_empty() && right.retained.is_empty(),
                "{backend} iteration {iteration}: no false debt"
            );
            assert!(
                left.failed.is_empty() && right.failed.is_empty(),
                "{backend} iteration {iteration}: no sweep failures: {:?} {:?}",
                left.failed,
                right.failed
            );

            let archived = harness.runtime.archived();
            assert_eq!(
                archived.len(),
                iteration + 1,
                "{backend} iteration {iteration}: each fork is archived exactly once"
            );
            assert_eq!(
                archived.last(),
                Some(capability.fork_session_id()),
                "{backend} iteration {iteration}: this fork was archived"
            );

            let record = harness
                .service
                .load_record(capability.capability_id())
                .await
                .expect("load")
                .expect("present");
            assert!(record.cleanup_claim.is_none());
            assert!(record.cleanup_debt.is_none());
            assert_eq!(
                record.machine_state.cleanup_state,
                meerkat_mob::machines::forked_participant_lifecycle::ForkedParticipantCleanupState::Complete
            );
        }
    }
}
