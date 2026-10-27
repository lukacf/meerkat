//! The identity runtime is a view over Core, not a second authority.
//!
//! Meta keeps no durable identity state. A member's identity, generation,
//! fence and session binding are MobMachine's; its history (which session an
//! identity was last bound to) is the mob journal's. These tests drive the
//! identity runtime over a real persistent mob and pin the lean contract:
//!
//! - a restart with only Core's stores (mob and session) resumes every roster
//!   identity onto its committed session at its Core generation;
//! - a wake after retire resumes the same session, and the generation the
//!   identity reports is the one Core minted for the woken member;
//! - delete starts a fresh life at once and keeps liveness: a live
//!   identity stays live on a fresh session at a higher Core generation, a
//!   retired one stays retired with a fresh last binding;
//! - the boot roster compile retires a durable member the roster no longer
//!   lists (the durable-identity marker);
//! - a wake after a mob reset is fresh;
//! - a wake whose last session Core holds nothing for spawns fresh and says
//!   so with a typed marker;
//! - an identity a host adopted (Core holds its identity intent row), or one
//!   whose intent row Core cannot read, is read-only to every Meta verb and
//!   to the roster compile;
//! - nothing reads the continuity store or the lease provider any more (the
//!   two config fields stay only until they are deleted);
//! - the first-boot manifest `storage-migrate-native` leaves is carried
//!   out by the roster compile before anything wakes a listed identity: a
//!   `Resume` row resumes the migrated session with its transcript, a
//!   `Fresh` row starts a fresh life and leaves the old session in Core,
//!   bound to nothing; a boot that stops part-way finishes on the next one;
//!   a row Core's history contradicts refuses that identity alone; a row
//!   whose identity the roster does not list waits for the roster; a
//!   `Resume` row naming a session Core holds no durable copy of is refused
//!   before any resume, so no fresh life starts in its place; a pending row
//!   refuses reset and delete with its way out; a manifest the reader
//!   refuses refuses every identity, wakes none, and leaves the runtime up
//!   with the refusal in its status;
//! - a member Meta 0.9.3 spawned without the durable-identity marker, which
//!   Core restores live across the upgrade, is respawned by the first boot
//!   whose roster lists its identity: the same session and transcript at a
//!   higher Core generation, now marked; such a member of an identity the
//!   roster dropped is retired; a host-adopted member is neither respawned
//!   nor retired, and a second boot changes nothing.
//!
//! Every assertion reads Core directly (the mob handle's roster), so no test
//! can pass by agreeing with a Meta copy of the same fact.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::collections::BTreeMap;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use meerkat_client::{LlmError, LlmEvent, LlmRequest};
use meerkat_core::types::{SessionId, StopReason};
use meerkat_meta::UnifiedRuntimeBuilder;
use meerkat_meta::identity_first::contracts::{
    AgentCustomizer, ContinuityIncrementalSessions, RosterProvider, TopologyProvider,
};
use meerkat_meta::identity_first::{
    AgentAddressability, AgentBuildContext, AgentBuildDraft, AgentIdentity, CheckpointVersion,
    ContinuityGeneration, ContinuityRecord, ContinuityResolveState, ContinuityStore,
    ContinuityStoreError, CustomizerError, DurabilityPolicy, DurableAgentSpec, FencingToken,
    FirstBootManifest, FirstBootRow, FirstBootRowKind, FirstBootWait, IdentityFirstRuntimeContext,
    IdentityRuntime, IdentityRuntimeConfig, IdentityRuntimeError, LeaseAcquireResult, LeaseError,
    LeaseGrant, LeaseProvider, LeaseRenewResult, ManagedPeerEdge, ReplacedSession,
    ReplacedSessionReason, RestoreFlowResult, RestoreOutcome, RosterContext, RosterError,
    SessionBridge, SessionSnapshot, TopologyContext, TopologyError,
};
use meerkat_meta::mob_composition_manifest::persistent_mob_storage;
use meerkat_meta::mob_handle_runtime::SessionCreatedContext;
use meerkat_mob::{MobDefinition, ProfileName};
use tokio::time::sleep;

#[path = "support/llm_usage.rs"]
mod llm_usage;

const MOB_TOML: &str = r#"
[mob]
id = "MOB_ID_PLACEHOLDER"

[profiles.personal]
model = "gpt-5.5"
external_addressable = true
runtime_mode = "turn_driven"

[profiles.personal.tools]
comms = true
"#;

fn definition(mob_id: &str) -> MobDefinition {
    MobDefinition::from_toml(&MOB_TOML.replace("MOB_ID_PLACEHOLDER", mob_id))
        .expect("parse the core-authority mob definition")
}

/// Core's mob store under a state directory (the mob journal and identity
/// intent rows). The session and runtime stores are the rest of the
/// directory.
const MOB_STORE_FILE: &str = "mob.sqlite3";

/// The topology controller's state under a state directory. Like the mob
/// store it is bound to its mob (the topology authority).
const TOPOLOGY_STATE_FILE: &str = "topology-control.json";

/// Move `from`'s mob store (with its WAL files) and the topology state bound
/// to the same mob into `to`, replacing `to`'s: `to` then holds one mob's
/// journal over another directory's session and runtime stores.
fn transplant_mob_store(from: &Path, to: &Path) {
    let is_mob_store = |name: &std::ffi::OsStr| {
        name.to_str().is_some_and(|name| {
            name.starts_with(MOB_STORE_FILE) || name.starts_with(TOPOLOGY_STATE_FILE)
        })
    };
    for entry in std::fs::read_dir(to).expect("read the target state directory") {
        let entry = entry.expect("a target state entry");
        if is_mob_store(&entry.file_name()) {
            std::fs::remove_file(entry.path()).expect("remove the target's mob store");
        }
    }
    for entry in std::fs::read_dir(from).expect("read the source state directory") {
        let entry = entry.expect("a source state entry");
        if is_mob_store(&entry.file_name()) {
            std::fs::copy(entry.path(), to.join(entry.file_name()))
                .expect("copy the source's mob store");
        }
    }
}

/// Remove `state`'s mob store (with its WAL files), as a 0.9.3 gateway left
/// it: that release kept the mob journal in memory, so the migrated state
/// directory holds the sessions but no member binding.
fn remove_mob_store(state: &Path) {
    for entry in std::fs::read_dir(state).expect("read the state directory") {
        let entry = entry.expect("a state entry");
        if entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.starts_with(MOB_STORE_FILE))
        {
            std::fs::remove_file(entry.path()).expect("remove the mob store");
        }
    }
}

/// `outcome` must be `identity`'s typed first-boot refusal: Core's history
/// contradicts its row, the storage-resolution class.
fn assert_first_boot_row_refused(identity: &AgentIdentity, outcome: Option<&RestoreOutcome>) {
    match outcome {
        Some(RestoreOutcome::Refused(error)) => assert!(
            matches!(
                error.as_ref(),
                IdentityRuntimeError::FirstBootRowRefused { identity: refused, .. }
                    if refused == identity
            ),
            "{identity}'s first-boot row must be refused typed, got {error:?}"
        ),
        other => panic!("{identity}'s first-boot row must be refused typed, got {other:?}"),
    }
}

/// `result` must be `identity`'s typed host-adoption refusal.
fn assert_host_adopted<T: std::fmt::Debug>(
    verb: &str,
    identity: &AgentIdentity,
    result: Result<T, IdentityRuntimeError>,
) {
    match result {
        Err(IdentityRuntimeError::IdentityHostAdopted { identity: refused }) => {
            assert_eq!(&refused, identity, "{verb}");
        }
        other => panic!("{verb} must refuse the host-adopted {identity} typed: {other:?}"),
    }
}

/// `result` must be the typed history-unavailable refusal an unreadable
/// intent row gives (-32052).
fn assert_history_unavailable<T: std::fmt::Debug>(
    verb: &str,
    identity: &AgentIdentity,
    result: Result<T, IdentityRuntimeError>,
) {
    match result {
        Err(IdentityRuntimeError::IdentityHistoryUnavailable {
            identity: refused, ..
        }) => {
            assert_eq!(&refused, identity, "{verb}");
        }
        other => panic!(
            "{verb} must refuse {identity}, whose intent row Core cannot read, as history \
             unavailable: {other:?}"
        ),
    }
}

/// Unique across tests (concurrent tests must not share a supervisor route),
/// stable within one test, whose restarts re-open the same mob.
fn unique_mob_id(test: &str) -> String {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    format!(
        "core-authority-{test}-{}",
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

fn id(name: &str) -> AgentIdentity {
    AgentIdentity::parse(name).unwrap()
}

fn spec(name: &str) -> DurableAgentSpec {
    DurableAgentSpec {
        identity: id(name),
        profile: ProfileName::from("personal"),
        addressability: AgentAddressability::Addressable,
        display_name: None,
        labels: BTreeMap::new(),
        context: None,
        additional_instructions: Vec::new(),
        initial_message: None,
        runtime_mode_override: None,
        backend: None,
        binding: None,
        placement: None,
    }
}

struct StaticRoster(Vec<DurableAgentSpec>);

#[async_trait]
impl RosterProvider for StaticRoster {
    async fn roster(&self, _context: &RosterContext) -> Result<Vec<DurableAgentSpec>, RosterError> {
        Ok(self.0.clone())
    }
}

struct EmptyTopology;

#[async_trait]
impl TopologyProvider for EmptyTopology {
    async fn compute_edges(
        &self,
        _target_identities: &[AgentIdentity],
        _context: &TopologyContext,
    ) -> Result<Vec<ManagedPeerEdge>, TopologyError> {
        Ok(Vec::new())
    }
}

struct NoopCustomizer;

#[async_trait]
impl AgentCustomizer for NoopCustomizer {
    async fn customize_build(
        &self,
        _context: &AgentBuildContext,
        _spec: &DurableAgentSpec,
        _draft: &mut AgentBuildDraft,
    ) -> Result<(), CustomizerError> {
        Ok(())
    }

    async fn after_create(
        &self,
        _identity: &AgentIdentity,
        _session_id: &SessionId,
        _context: &SessionCreatedContext,
    ) -> Result<(), CustomizerError> {
        Ok(())
    }
}

/// Records every serialized LLM request and answers "ok", so turns complete
/// without a provider and a later request shows which transcript it replays.
#[derive(Clone, Default)]
struct CaptureClient {
    requests: Arc<std::sync::Mutex<Vec<String>>>,
}

impl CaptureClient {
    fn count(&self) -> usize {
        self.requests.lock().unwrap().len()
    }

    fn last(&self) -> Option<String> {
        self.requests.lock().unwrap().last().cloned()
    }

    async fn wait_for_requests(&self, at_least: usize, what: &str) {
        let deadline = Instant::now() + Duration::from_secs(30);
        while self.count() < at_least {
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {what} to reach the LLM"
            );
            sleep(Duration::from_millis(50)).await;
        }
        // Let the turn's answer commit before the next step reads or restarts.
        sleep(Duration::from_millis(500)).await;
    }
}

impl meerkat_client::LlmClient for CaptureClient {
    fn project_replay_messages(
        &self,
        messages: &[meerkat_core::Message],
    ) -> Result<Vec<meerkat_core::Message>, LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
    ) -> Pin<Box<dyn futures::Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>> {
        self.requests
            .lock()
            .unwrap()
            .push(serde_json::to_string(request).unwrap_or_default());
        let [usage, done] =
            llm_usage::usage_then_done(request, meerkat::Provider::OpenAI, StopReason::EndTurn);
        Box::pin(async_stream::stream! {
            yield Ok(LlmEvent::TextDelta { delta: "ok".to_string(), meta: None });
            yield Ok(usage);
            yield Ok(done);
        })
    }

    fn provider(&self) -> meerkat::Provider {
        meerkat::Provider::OpenAI
    }

    fn health_check<'life0, 'async_trait>(
        &'life0 self,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<(), LlmError>> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async { Ok(()) })
    }
}

/// Counts every call into the two config fields the lean runtime must never
/// read. Each call is also refused, so a reader cannot get any further on a
/// value this store invented.
#[derive(Default)]
struct UnreadContinuityStore {
    calls: AtomicUsize,
}

impl UnreadContinuityStore {
    fn refuse(&self, what: &'static str) -> ContinuityStoreError {
        self.calls.fetch_add(1, Ordering::SeqCst);
        ContinuityStoreError::Io(format!(
            "the identity runtime read the continuity store ({what})"
        ))
    }
}

#[async_trait]
impl ContinuityStore for UnreadContinuityStore {
    async fn resolve_many(
        &self,
        _identities: &[AgentIdentity],
    ) -> Result<BTreeMap<AgentIdentity, ContinuityResolveState>, ContinuityStoreError> {
        Err(self.refuse("resolve_many"))
    }

    async fn resolve_record_by_session(
        &self,
        _session_id: &SessionId,
    ) -> Result<Option<(ContinuityRecord, FencingToken, CheckpointVersion)>, ContinuityStoreError>
    {
        Err(self.refuse("resolve_record_by_session"))
    }

    async fn session_owner(
        &self,
        _session_id: &SessionId,
    ) -> Result<Option<AgentIdentity>, ContinuityStoreError> {
        Err(self.refuse("session_owner"))
    }

    async fn load_session_snapshot(
        &self,
        _session_id: &SessionId,
    ) -> Result<Option<SessionSnapshot>, ContinuityStoreError> {
        Err(self.refuse("load_session_snapshot"))
    }

    async fn delete_session_snapshot_if_current_revision(
        &self,
        _session_id: &SessionId,
        _expected_current_revision: &str,
    ) -> Result<bool, ContinuityStoreError> {
        Err(self.refuse("delete_session_snapshot_if_current_revision"))
    }

    async fn save_session_snapshot(
        &self,
        _identity: &AgentIdentity,
        _session_id: &SessionId,
        _generation: ContinuityGeneration,
        _version: CheckpointVersion,
        _fencing_token: FencingToken,
        _snapshot: &SessionSnapshot,
    ) -> Result<(), ContinuityStoreError> {
        Err(self.refuse("save_session_snapshot"))
    }

    async fn upsert_continuity_record(
        &self,
        _record: &ContinuityRecord,
        _fencing_token: FencingToken,
    ) -> Result<(), ContinuityStoreError> {
        Err(self.refuse("upsert_continuity_record"))
    }

    async fn rollback_continuity_record(
        &self,
        _expected_attempt: &ContinuityRecord,
        _previous: Option<&ContinuityRecord>,
        _fencing_token: FencingToken,
    ) -> Result<(), ContinuityStoreError> {
        Err(self.refuse("rollback_continuity_record"))
    }

    async fn delete_continuity_record(
        &self,
        _identity: &AgentIdentity,
        _fencing_token: FencingToken,
    ) -> Result<(), ContinuityStoreError> {
        Err(self.refuse("delete_continuity_record"))
    }

    fn as_incremental_sessions(&self) -> Option<Arc<dyn ContinuityIncrementalSessions>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        None
    }
}

#[derive(Default)]
struct UnreadLeaseProvider {
    calls: AtomicUsize,
}

impl UnreadLeaseProvider {
    fn refuse(&self, what: &'static str) -> LeaseError {
        self.calls.fetch_add(1, Ordering::SeqCst);
        LeaseError::ProviderUnavailable(format!(
            "the identity runtime called the lease provider ({what})"
        ))
    }
}

#[async_trait]
impl LeaseProvider for UnreadLeaseProvider {
    async fn acquire_leases(
        &self,
        _identities: &[AgentIdentity],
        _holder: &str,
    ) -> Result<BTreeMap<AgentIdentity, LeaseAcquireResult>, LeaseError> {
        Err(self.refuse("acquire_leases"))
    }

    async fn renew_leases(
        &self,
        _grants: &[LeaseGrant],
    ) -> Result<BTreeMap<AgentIdentity, LeaseRenewResult>, LeaseError> {
        Err(self.refuse("renew_leases"))
    }

    async fn release_leases(&self, _grants: &[LeaseGrant]) -> Result<(), LeaseError> {
        Err(self.refuse("release_leases"))
    }
}

/// One process lifetime over `state`: a persistent mob (its own mob and
/// session stores) and an identity runtime composed over its session bridge.
/// The two continuity-era config fields get stores that record and refuse
/// every call, so the lean runtime runs on Core alone.
struct Boot {
    unified: meerkat_meta::UnifiedRuntime,
    context: Arc<IdentityFirstRuntimeContext>,
    capture: CaptureClient,
    continuity: Arc<UnreadContinuityStore>,
    leases: Arc<UnreadLeaseProvider>,
    mob_id: String,
    mob_store: std::path::PathBuf,
}

/// Core's last binding of an identity, as the tests compare it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct CoreBinding {
    generation: u64,
    session: Option<SessionId>,
    /// The binding carries the durable-identity marker keyed by the identity.
    marked: bool,
    live: bool,
}

impl Boot {
    async fn start(state: &Path, mob_id: &str, roster: &[DurableAgentSpec]) -> Self {
        let capture = CaptureClient::default();
        // `persistent_state` alone keeps the mob journal in memory; the
        // restart contract needs Core's mob store on disk too.
        let mob_store = state.join(MOB_STORE_FILE);
        let (mob_storage, mob_provenance) =
            persistent_mob_storage(mob_store.clone()).expect("open persistent mob storage");
        let unified = UnifiedRuntimeBuilder::default()
            .definition(definition(mob_id))
            .mob_storage(mob_storage, mob_provenance)
            .persistent_state(state)
            .comms(true)
            .default_llm_client(Arc::new(capture.clone()))
            .build()
            .await
            .expect("build the persistent unified runtime");
        let bridge: Arc<dyn SessionBridge> = unified
            .session_bridge()
            .map(|bridge| Arc::new(bridge) as Arc<dyn SessionBridge>)
            .expect("the persistent unified runtime exposes a session bridge");
        let continuity = Arc::new(UnreadContinuityStore::default());
        let leases = Arc::new(UnreadLeaseProvider::default());
        let runtime = Arc::new(IdentityRuntime::new(IdentityRuntimeConfig {
            continuity_store: continuity.clone() as Arc<dyn ContinuityStore>,
            lease_provider: leases.clone() as Arc<dyn LeaseProvider>,
            runtime_instance_id: format!("{mob_id}-instance"),
            has_runtime_store: true,
            durability_policy: DurabilityPolicy::SyncWriteThrough,
            bridge: Some(bridge),
            default_timeout: None,
        }));
        // As the gateway builder does: the state directory's first-boot
        // manifest, if `storage-migrate-native` left one.
        runtime.set_first_boot_state_dir(Some(state.to_path_buf()));
        let context = Arc::new(IdentityFirstRuntimeContext::new(
            runtime,
            Arc::new(StaticRoster(roster.to_vec())),
            Some(Arc::new(EmptyTopology) as Arc<dyn TopologyProvider>),
            Some(Arc::new(NoopCustomizer) as Arc<dyn AgentCustomizer>),
            Some(definition(mob_id)),
        ));
        Self {
            unified,
            context,
            capture,
            continuity,
            leases,
            mob_id: mob_id.to_string(),
            mob_store,
        }
    }

    fn runtime(&self) -> &IdentityRuntime {
        self.context.runtime()
    }

    async fn compile(&self, roster: &[DurableAgentSpec]) {
        self.compile_outcomes(roster).await;
    }

    /// The roster compile, with each identity's outcome.
    async fn compile_outcomes(&self, roster: &[DurableAgentSpec]) -> RestoreFlowResult {
        self.context
            .bootstrap_roster(roster)
            .await
            .unwrap_or_else(|error| {
                panic!("the roster compile must succeed on Core's stores alone: {error}")
            })
    }

    /// The roster compile's refusal, for a roster the compile must refuse.
    async fn compile_refusal(&self, roster: &[DurableAgentSpec]) -> IdentityRuntimeError {
        match self.context.bootstrap_roster(roster).await {
            Ok(compiled) => panic!("the roster compile must refuse: {compiled:?}"),
            Err(error) => error,
        }
    }

    /// A host adopts `identity`'s live member through Core, exactly as it
    /// runs now: Core then holds an identity intent row for it and its
    /// reconciler drives it.
    async fn adopt(&self, identity: &AgentIdentity, session_id: &SessionId) {
        self.adopt_as(identity, session_id, "core-authority").await;
    }

    /// [`Self::adopt`] under the adoption request `{key}-adoption` and the
    /// declaration scope `{key}-host`, for a test that adopts more than one
    /// identity.
    async fn adopt_as(&self, identity: &AgentIdentity, session_id: &SessionId, key: &str) {
        let handle = self.unified.mob_handle();
        let core_identity =
            meerkat_meta::member_comms_id::roster_member_id_for_identity(identity.as_str());
        let entry = handle
            .get_member(&core_identity)
            .await
            .expect("Core's roster read")
            .expect("a live member to adopt");
        let request = meerkat_mob::AdoptMemberIdentityDeclaration {
            mob_id: meerkat_mob::MobId::from(self.mob_id.as_str()),
            agent_identity: core_identity,
            request_id: meerkat_mob::IdentityAdoptionId::new(format!("{key}-adoption"))
                .expect("valid adoption id"),
            precondition: meerkat_mob::IdentityAdoptionPrecondition::ExpectedAbsent,
            declaration_scope: meerkat_mob::IdentityDeclarationScopeId::new(format!("{key}-host"))
                .expect("valid declaration scope"),
            declaration_revision: 1,
            session: meerkat_mob::DesiredSessionTarget {
                session_id: session_id.clone(),
                lineage_id: meerkat_core::SessionLineageId::for_session(session_id),
                lineage_generation: meerkat_core::SessionGeneration::INITIAL,
                authority_policy: meerkat_mob::DesiredSessionAuthorityPolicy::RequireExisting,
            },
            member: meerkat_mob::IdentityProfileMemberDeclaration {
                profile_name: entry.role.clone(),
                profile_override: None,
                model_override: None,
                external_addressable_override: None,
                context: None,
                labels: Some(entry.labels.clone()),
                additional_instructions: None,
                system_prompt_override: None,
                tool_access_policy: None,
                auth_binding: None,
                budget_limits: None,
                runtime_mode: None,
                required_env_keys: Vec::new(),
                required_local_callback_tools: Vec::new(),
                execution: meerkat_mob::DesiredExecution::ControllingSession,
            },
            wiring_custody: Default::default(),
            owned_wiring: Default::default(),
            convergence: meerkat_mob::IdentityConvergenceMode::Drain { max_wait_ms: 1_000 },
        };
        let adopted = handle
            .adopt_member_identity_declaration(request)
            .await
            .expect("Core adopts the live member");
        assert!(
            matches!(
                adopted.adoption,
                meerkat_mob::IdentityAdoptionOutcome::Adopted { .. }
            ),
            "{:?}",
            adopted.adoption
        );
        self.wait_converged(identity).await;
    }

    /// Wait (bounded) until Core's reconciler reports `identity` converged:
    /// until then Core keeps the member's admission closed while it seats
    /// the adopted material.
    async fn wait_converged(&self, identity: &AgentIdentity) {
        let handle = self.unified.mob_handle();
        let core_identity =
            meerkat_meta::member_comms_id::roster_member_id_for_identity(identity.as_str());
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        loop {
            let observation = handle
                .identity_convergence_status(&core_identity)
                .await
                .expect("Core's convergence status read");
            if let meerkat_mob::IdentityStoredObservation::Valid(status) = &observation
                && status.condition() == meerkat_mob::IdentityConvergenceCondition::Converged
            {
                return;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "Core did not converge the adopted identity: {observation:?}"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// Write an identity intent row for `identity` that no build can read
    /// into Core's mob store, beside the running mob (the
    /// `IntentUnreadable`).
    fn write_unreadable_intent_row(&self, identity: &AgentIdentity) {
        let core_identity =
            meerkat_meta::member_comms_id::roster_member_id_for_identity(identity.as_str());
        let connection =
            rusqlite::Connection::open(&self.mob_store).expect("open Core's mob store");
        connection
            .busy_timeout(Duration::from_secs(10))
            .expect("set a busy timeout");
        connection
            .execute(
                "INSERT INTO mob_identity_intents (mob_id, agent_identity, record_json)
                 VALUES (?1, ?2, ?3)",
                rusqlite::params![
                    self.mob_id,
                    core_identity.as_str(),
                    b"not an identity intent record".to_vec()
                ],
            )
            .expect("write the unreadable intent row");
    }

    async fn send(&self, identity: &AgentIdentity, text: &str) {
        let before = self.capture.count();
        let sent = self
            .runtime()
            .send(
                identity,
                &meerkat_core::ContentInput::Text(text.to_string()),
            )
            .await;
        assert!(
            sent.is_ok(),
            "send to {identity} must be admitted: {sent:?}"
        );
        self.capture
            .wait_for_requests(before + 1, &format!("the turn sent to {identity}"))
            .await;
    }

    /// Core's own answer: the member MobMachine holds for `identity` now,
    /// with its generation. `None` when Core holds no member (retired or
    /// never spawned).
    async fn core_member_generation(&self, identity: &AgentIdentity) -> Option<u64> {
        let handle = self.unified.mob_handle();
        let roster_id =
            meerkat_meta::member_comms_id::roster_member_id_for_identity(identity.as_str());
        handle
            .get_member(&roster_id)
            .await
            .expect("Core's roster read")
            .map(|entry| entry.agent_runtime_id.generation.get())
    }

    /// The transcript Core's session service still serves for `session`,
    /// serialized. Core keeps a retired or replaced session's body archived:
    /// hidden from listings, but its history stays readable by id.
    async fn core_session_history(&self, session: &SessionId) -> String {
        let page = self
            .unified
            .mob_runtime()
            .read_session_history(&session.to_string(), 0, None)
            .await
            .unwrap_or_else(|error| {
                panic!("Core must still serve the history of session {session}: {error}")
            });
        serde_json::to_string(&page.messages).expect("serialize the history page")
    }

    /// Core's member history, read from the mob directly.
    async fn core_history(&self) -> meerkat_mob::MemberBindingHistory {
        self.unified
            .mob_handle()
            .member_binding_history()
            .await
            .expect("Core's member history")
    }

    /// Core lists `identity`'s live member as a durable identity: the spawn of
    /// its current incarnation carried the marker, keyed by the identity.
    async fn assert_durable_identity_marker(&self, identity: &AgentIdentity, after: &str) {
        let core_identity =
            meerkat_meta::member_comms_id::roster_member_id_for_identity(identity.as_str());
        let history = self.core_history().await;
        let binding = history
            .durable_identity_members()
            .find(|binding| *binding.identity() == core_identity)
            .unwrap_or_else(|| {
                panic!("Core must list {identity} as a durable identity after {after}")
            });
        assert_eq!(
            binding.continuity_intent(),
            &meerkat_mob::SpawnContinuityIntent::DurableIdentity {
                continuity_key: identity.as_str().to_string(),
            },
            "the marker is keyed by the identity after {after}"
        );
    }

    async fn status_session(
        &self,
        identity: &AgentIdentity,
    ) -> (String, Option<SessionId>, Option<u64>) {
        let status = self
            .runtime()
            .status(identity)
            .await
            .unwrap_or_else(|error| panic!("status of {identity}: {error}"));
        (
            status.state.wire_str().to_string(),
            status.session_id,
            status.generation.map(meerkat_mob::Generation::get),
        )
    }

    fn assert_continuity_and_leases_unread(&self) {
        assert_eq!(
            self.continuity.calls.load(Ordering::SeqCst),
            0,
            "the identity runtime must never read the continuity store (Meta keeps no \
             durable identity state)"
        );
        assert_eq!(
            self.leases.calls.load(Ordering::SeqCst),
            0,
            "the identity runtime must never call the lease provider (Core's realm lock \
             and hosting claim are the exclusivity)"
        );
    }

    /// Spawn `identity`'s member the way Meta 0.9.3 did: through Core's mob,
    /// under the identity's roster member id with Meta's identity labels, but
    /// without the durable-identity marker (0.9.3 had none). Waits for its
    /// first turn, which asks the model to remember `token`, and returns its
    /// session.
    async fn spawn_unmarked(&self, identity: &AgentIdentity, token: &str) -> SessionId {
        self.spawn_core_member(
            identity,
            token,
            BTreeMap::from([
                ("agent_identity".to_string(), identity.as_str().to_string()),
                ("profile_name".to_string(), "personal".to_string()),
            ]),
        )
        .await
    }

    /// Spawn a member through Core's mob under `identity`'s roster member id
    /// with exactly `labels` and no durable-identity marker, the way a host
    /// makes one directly on the mob when `labels` carry no identity-first
    /// label. Waits for its first turn, which asks the model to remember
    /// `token`, and returns its session.
    async fn spawn_core_member(
        &self,
        identity: &AgentIdentity,
        token: &str,
        labels: BTreeMap<String, String>,
    ) -> SessionId {
        let core_identity =
            meerkat_meta::member_comms_id::roster_member_id_for_identity(identity.as_str());
        let before = self.capture.count();
        self.unified
            .mob_handle()
            .spawn_spec(
                meerkat_mob::SpawnMemberSpec::host_root("personal", core_identity)
                    .with_labels(labels)
                    .with_initial_message(meerkat_core::ContentInput::Text(format!(
                        "Remember this token: {token}"
                    ))),
            )
            .await
            .unwrap_or_else(|error| panic!("Core spawns {identity}'s unmarked member: {error}"));
        self.capture
            .wait_for_requests(before + 1, &format!("{identity}'s first turn"))
            .await;
        let binding = self.core_last_binding(identity).await;
        assert!(
            !binding.marked,
            "{identity}'s seeded member carries no durable-identity marker"
        );
        binding
            .session
            .unwrap_or_else(|| panic!("{identity}'s unmarked member has a session"))
    }

    /// Retire `identity`'s member through Core's mob, the way a host retires
    /// a member it made itself.
    async fn retire_core_member(&self, identity: &AgentIdentity) {
        let core_identity =
            meerkat_meta::member_comms_id::roster_member_id_for_identity(identity.as_str());
        self.unified
            .mob_handle()
            .retire(core_identity)
            .await
            .unwrap_or_else(|error| panic!("Core retires {identity}'s member: {error}"));
    }

    /// Give `identity`'s live member new spawn `labels` the way a host does
    /// with a member it made itself: retire it and spawn it again onto
    /// `session`, so its transcript carries over.
    async fn relabel_core_member(
        &self,
        identity: &AgentIdentity,
        session: &SessionId,
        labels: BTreeMap<String, String>,
    ) {
        self.retire_core_member(identity).await;
        let core_identity =
            meerkat_meta::member_comms_id::roster_member_id_for_identity(identity.as_str());
        self.unified
            .mob_handle()
            .spawn_spec(
                meerkat_mob::SpawnMemberSpec::host_root("personal", core_identity)
                    .with_labels(labels)
                    .with_launch_mode(meerkat_mob::MemberLaunchMode::Resume {
                        bridge_session_id: session.clone(),
                        resume_from_role: None,
                    }),
            )
            .await
            .unwrap_or_else(|error| {
                panic!("Core spawns {identity}'s member again onto its session: {error}")
            });
    }

    /// Core's last binding of `identity` since the latest mob reset.
    async fn core_last_binding(&self, identity: &AgentIdentity) -> CoreBinding {
        let core_identity =
            meerkat_meta::member_comms_id::roster_member_id_for_identity(identity.as_str());
        let history = self.core_history().await;
        let binding = history
            .last_binding(&core_identity)
            .unwrap_or_else(|| panic!("Core holds a binding of {identity}"));
        CoreBinding {
            generation: binding.generation().get(),
            session: binding.session_id().cloned(),
            marked: *binding.continuity_intent()
                == meerkat_mob::SpawnContinuityIntent::DurableIdentity {
                    continuity_key: identity.as_str().to_string(),
                },
            live: binding.is_live(),
        }
    }

    async fn stop(self) {
        let Self {
            unified, context, ..
        } = self;
        drop(context);
        let _report = unified.shutdown().await;
    }
}

/// A restart with only Core's stores resumes each roster
/// identity onto its committed session at its Core generation, with its
/// transcript. In 0.9.3 the second boot found no continuity record and the
/// identity was Uninitialized.
#[tokio::test(flavor = "multi_thread")]
async fn restart_with_only_core_stores_resumes_each_identity_on_its_committed_session() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let state = temp.path().join("state");
    let mob_id = unique_mob_id("restart");
    let roster = vec![spec("personal:alice"), spec("personal:bob")];
    let tokens = [
        (id("personal:alice"), "MARKER-ALICE-4-QUEBEC"),
        (id("personal:bob"), "MARKER-BOB-7-SIERRA"),
    ];

    let mut committed = BTreeMap::new();
    {
        let boot = Boot::start(&state, &mob_id, &roster).await;
        boot.compile(&roster).await;
        for (identity, token) in &tokens {
            boot.send(identity, &format!("Remember this token: {token}"))
                .await;
            let (_, session, _) = boot.status_session(identity).await;
            committed.insert(
                identity.clone(),
                session.expect("a live identity reports its session"),
            );
        }
        boot.assert_continuity_and_leases_unread();
        boot.stop().await;
    }

    let boot = Boot::start(&state, &mob_id, &roster).await;
    boot.compile(&roster).await;
    for (identity, token) in &tokens {
        let (state_name, session, generation) = boot.status_session(identity).await;
        assert_eq!(
            state_name, "active",
            "{identity} must be live after the second boot's eager compile"
        );
        assert_eq!(
            session.as_ref(),
            committed.get(identity),
            "{identity} must resume onto the session Core committed before the restart"
        );
        let core_generation = boot.core_member_generation(identity).await;
        assert!(
            core_generation.is_some(),
            "Core must hold a member for {identity} after the second boot"
        );
        assert_eq!(
            generation, core_generation,
            "{identity}'s reported generation must be Core's, not a Meta count"
        );
        boot.send(identity, "Which token did I give you?").await;
        let request = boot.capture.last().expect("a post-restart request");
        assert!(
            request.contains(token),
            "{identity}'s post-restart turn must replay its committed transcript ({token})"
        );
    }
    boot.assert_continuity_and_leases_unread();
    boot.stop().await;
}

/// A wake after retire keeps the transcript, and the
/// identity reports the generation Core minted for the woken member.
#[tokio::test(flavor = "multi_thread")]
async fn wake_after_retire_resumes_the_same_session_at_cores_generation() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let state = temp.path().join("state");
    let mob_id = unique_mob_id("retire-wake");
    let alice = id("personal:alice");
    let roster = vec![spec("personal:alice")];
    const TOKEN: &str = "MARKER-RETIRE-2-TANGO";

    let boot = Boot::start(&state, &mob_id, &roster).await;
    boot.compile(&roster).await;
    boot.send(&alice, &format!("Remember this token: {TOKEN}"))
        .await;
    let (_, before_session, _) = boot.status_session(&alice).await;
    let before_session = before_session.expect("a live identity reports its session");

    let retired = boot.runtime().retire(&alice).await;
    assert!(retired.is_ok(), "retire of a live identity: {retired:?}");
    assert_eq!(
        boot.core_member_generation(&alice).await,
        None,
        "retire must leave Core without a live member for the identity"
    );
    let (state_name, _, _) = boot.status_session(&alice).await;
    assert_ne!(state_name, "active", "a retired identity is not live");

    boot.send(&alice, "Which token did I give you?").await;
    let (state_name, session, generation) = boot.status_session(&alice).await;
    assert_eq!(state_name, "active", "the send woke the identity");
    assert_eq!(
        session,
        Some(before_session),
        "the wake must resume the session the identity was retired on"
    );
    assert_eq!(
        generation,
        boot.core_member_generation(&alice).await,
        "the woken identity's generation is the one Core minted for the woken member"
    );
    let request = boot.capture.last().expect("a post-wake request");
    assert!(
        request.contains(TOKEN),
        "the woken member must replay the transcript it was retired with"
    );
    boot.assert_continuity_and_leases_unread();
    boot.stop().await;
}

/// `delete_identity` starts a fresh life at once. A
/// live identity stays live, on a fresh session, at a higher Core generation,
/// and its old transcript is gone from the new life. Meta deletes nothing:
/// Core still serves the old session's history by its id.
#[tokio::test(flavor = "multi_thread")]
async fn delete_of_a_live_identity_starts_a_fresh_life_at_a_higher_generation() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let state = temp.path().join("state");
    let mob_id = unique_mob_id("delete-live");
    let alice = id("personal:alice");
    let roster = vec![spec("personal:alice")];
    const TOKEN: &str = "MARKER-DELETE-9-UNIFORM";

    let boot = Boot::start(&state, &mob_id, &roster).await;
    boot.compile(&roster).await;
    boot.send(&alice, &format!("Remember this token: {TOKEN}"))
        .await;
    let (_, old_session, old_generation) = boot.status_session(&alice).await;
    let old_generation = old_generation.expect("a live identity reports its generation");

    let deleted = boot.runtime().delete_identity(&alice).await;
    assert!(deleted.is_ok(), "delete of a live identity: {deleted:?}");

    let (state_name, session, generation) = boot.status_session(&alice).await;
    assert_eq!(state_name, "active", "delete keeps a live identity live");
    assert!(session.is_some(), "the fresh life has a session");
    assert_ne!(session, old_session, "delete must start a fresh session");
    let generation = generation.expect("the fresh life reports its generation");
    assert!(
        generation > old_generation,
        "the fresh life's Core generation ({generation}) must be above the old one \
         ({old_generation}); Meta no longer re-mints generation 0"
    );
    assert_eq!(
        Some(generation),
        boot.core_member_generation(&alice).await,
        "the reported generation is Core's"
    );

    boot.send(&alice, "Which token did I give you?").await;
    let request = boot.capture.last().expect("a post-delete request");
    assert!(
        !request.contains(TOKEN),
        "the fresh life must not carry the deleted life's transcript"
    );
    let old_session = old_session.expect("the deleted life had a session");
    assert!(
        boot.core_session_history(&old_session)
            .await
            .contains(TOKEN),
        "delete must leave the old session in Core: its history is still served by id"
    );
    boot.assert_continuity_and_leases_unread();
    boot.stop().await;
}

/// Delete of a retired identity starts the fresh life without making it
/// live: it stays retired, and its last binding is the fresh session.
#[tokio::test(flavor = "multi_thread")]
async fn delete_of_a_retired_identity_keeps_it_retired_with_a_fresh_last_binding() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let state = temp.path().join("state");
    let mob_id = unique_mob_id("delete-retired");
    let alice = id("personal:alice");
    let roster = vec![spec("personal:alice")];
    const TOKEN: &str = "MARKER-RETIRED-5-VICTOR";

    let boot = Boot::start(&state, &mob_id, &roster).await;
    boot.compile(&roster).await;
    boot.send(&alice, &format!("Remember this token: {TOKEN}"))
        .await;
    let (_, old_session, _) = boot.status_session(&alice).await;
    let retired = boot.runtime().retire(&alice).await;
    assert!(retired.is_ok(), "retire of a live identity: {retired:?}");

    let deleted = boot.runtime().delete_identity(&alice).await;
    assert!(deleted.is_ok(), "delete of a retired identity: {deleted:?}");
    assert_eq!(
        boot.core_member_generation(&alice).await,
        None,
        "delete must never make a retired identity live"
    );
    let (state_name, session, _) = boot.status_session(&alice).await;
    assert_ne!(state_name, "active", "the identity stays retired");
    assert!(
        session.is_some(),
        "the retired identity's last binding is reported"
    );
    assert_ne!(
        session, old_session,
        "the retired identity's last binding is the fresh life's session"
    );

    boot.send(&alice, "Which token did I give you?").await;
    let request = boot.capture.last().expect("a post-delete request");
    assert!(
        !request.contains(TOKEN),
        "waking after delete resumes the fresh life, not the deleted transcript"
    );
    let old_session = old_session.expect("the retired life had a session");
    assert!(
        boot.core_session_history(&old_session)
            .await
            .contains(TOKEN),
        "delete must leave the retired life's session in Core: its history is still served \
         by id"
    );
    boot.assert_continuity_and_leases_unread();
    boot.stop().await;
}

/// The boot roster compile retires a durable-identity member the
/// roster no longer lists. Core restores every member it persisted; only
/// Meta's roster policy says the dropped identity is gone.
#[tokio::test(flavor = "multi_thread")]
async fn boot_compile_retires_a_durable_member_the_roster_no_longer_lists() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let state = temp.path().join("state");
    let mob_id = unique_mob_id("compile-retire");
    let alice = id("personal:alice");
    let bob = id("personal:bob");
    let first_roster = vec![spec("personal:alice"), spec("personal:bob")];
    let second_roster = vec![spec("personal:alice")];

    {
        let boot = Boot::start(&state, &mob_id, &first_roster).await;
        boot.compile(&first_roster).await;
        assert!(
            boot.core_member_generation(&bob).await.is_some(),
            "the first boot spawns bob"
        );
        boot.stop().await;
    }

    let boot = Boot::start(&state, &mob_id, &second_roster).await;
    assert!(
        boot.core_member_generation(&bob).await.is_some(),
        "control: Core restores bob from its own store before any Meta compile"
    );
    boot.compile(&second_roster).await;
    assert_eq!(
        boot.core_member_generation(&bob).await,
        None,
        "the compile must retire a durable member the roster no longer lists"
    );
    assert!(
        boot.core_member_generation(&alice).await.is_some(),
        "a listed identity stays live"
    );
    boot.assert_continuity_and_leases_unread();
    boot.stop().await;
}

/// Every Meta spawn of an identity carries Core's durable-identity
/// marker, keyed by the identity: the first create, a reset's successor and
/// a wake that resumes the retired session. A retired identity has no live
/// member, so Core lists nothing for it.
#[tokio::test(flavor = "multi_thread")]
async fn every_meta_spawn_of_an_identity_carries_the_durable_identity_marker() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let state = temp.path().join("state");
    let mob_id = unique_mob_id("durable-marker");
    let alice = id("personal:alice");
    let roster = vec![spec("personal:alice")];

    let boot = Boot::start(&state, &mob_id, &roster).await;
    boot.compile(&roster).await;
    boot.send(&alice, "A first turn.").await;
    boot.assert_durable_identity_marker(&alice, "the first create")
        .await;

    let reset = boot.runtime().reset(&alice).await;
    assert!(reset.is_ok(), "reset of a live identity: {reset:?}");
    boot.assert_durable_identity_marker(&alice, "the reset successor")
        .await;

    let retired = boot.runtime().retire(&alice).await;
    assert!(retired.is_ok(), "retire of a live identity: {retired:?}");
    let alice_core = meerkat_meta::member_comms_id::roster_member_id_for_identity(alice.as_str());
    assert!(
        boot.core_history()
            .await
            .durable_identity_members()
            .all(|binding| *binding.identity() != alice_core),
        "a retired identity has no live member to list"
    );

    boot.send(&alice, "Wake up.").await;
    boot.assert_durable_identity_marker(&alice, "the resume wake")
        .await;
    boot.assert_continuity_and_leases_unread();
    boot.stop().await;
}

/// A wake after a mob reset is fresh. MobReset keeps the journal, but the
/// identity's last binding is scoped to the current reset epoch.
#[tokio::test(flavor = "multi_thread")]
async fn wake_after_a_mob_reset_is_fresh() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let state = temp.path().join("state");
    let mob_id = unique_mob_id("mob-reset");
    let alice = id("personal:alice");
    let roster = vec![spec("personal:alice")];
    const TOKEN: &str = "MARKER-RESET-3-WHISKEY";

    let boot = Boot::start(&state, &mob_id, &roster).await;
    boot.compile(&roster).await;
    boot.send(&alice, &format!("Remember this token: {TOKEN}"))
        .await;
    let (_, old_session, _) = boot.status_session(&alice).await;

    boot.unified
        .mob_handle()
        .reset()
        .await
        .expect("Core's mob reset");
    assert_eq!(
        boot.core_member_generation(&alice).await,
        None,
        "control: a mob reset leaves no live member"
    );

    boot.send(&alice, "Which token did I give you?").await;
    let (state_name, session, _) = boot.status_session(&alice).await;
    assert_eq!(state_name, "active", "the send woke the identity");
    assert_ne!(
        session, old_session,
        "a wake after a mob reset must not resume the pre-reset session"
    );
    let request = boot.capture.last().expect("a post-reset request");
    assert!(
        !request.contains(TOKEN),
        "the post-reset life must not replay the pre-reset transcript"
    );
    boot.assert_continuity_and_leases_unread();
    boot.stop().await;
}

/// A wake whose last session Core holds
/// nothing for (Core's `Absent`: no body, no authority, no catalog entry, no
/// lifecycle row) spawns the identity fresh, and says so: the wake's outcome
/// and the identity's status name the replaced session with the reason
/// `absent`. The identity was retired on a session whose stores are then
/// replaced by another directory's (which holds other sessions, so the
/// whole-store check stays quiet).
#[tokio::test(flavor = "multi_thread")]
async fn a_wake_whose_last_session_core_holds_nothing_for_spawns_fresh_with_a_typed_marker() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let state = temp.path().join("state");
    let foreign_state = temp.path().join("foreign-state");
    let mob_id = unique_mob_id("absent");
    let foreign_mob_id = unique_mob_id("absent-foreign");
    let alice = id("personal:alice");
    let bob = id("personal:bob");
    let roster = vec![spec("personal:alice")];
    let foreign_roster = vec![spec("personal:bob")];
    const TOKEN: &str = "MARKER-ABSENT-6-XRAY";

    let replaced = {
        let boot = Boot::start(&state, &mob_id, &roster).await;
        boot.compile(&roster).await;
        boot.send(&alice, &format!("Remember this token: {TOKEN}"))
            .await;
        let (_, session, _) = boot.status_session(&alice).await;
        let retired = boot.runtime().retire(&alice).await;
        assert!(retired.is_ok(), "retire of a live identity: {retired:?}");
        boot.stop().await;
        session.expect("a live identity reports its session")
    };
    {
        let boot = Boot::start(&foreign_state, &foreign_mob_id, &foreign_roster).await;
        boot.compile(&foreign_roster).await;
        boot.send(&bob, "A session of another mob.").await;
        boot.stop().await;
    }
    transplant_mob_store(&state, &foreign_state);

    let boot = Boot::start(&foreign_state, &mob_id, &roster).await;
    let compiled = boot.compile_outcomes(&roster).await;
    match compiled.outcomes.get(&alice) {
        Some(RestoreOutcome::Created {
            binding,
            replaced_session,
            ..
        }) => {
            assert_eq!(
                replaced_session.as_ref(),
                Some(&replaced),
                "the wake's outcome names the session it replaced"
            );
            assert_ne!(
                binding.session_id.as_ref(),
                Some(&replaced),
                "the fresh life runs on a new session"
            );
        }
        other => panic!("the wake must spawn alice fresh: {other:?}"),
    }
    let status = boot
        .runtime()
        .status(&alice)
        .await
        .expect("status of the fresh identity");
    assert_eq!(status.state.wire_str(), "active");
    assert_ne!(status.session_id.as_ref(), Some(&replaced));
    assert_eq!(
        status.replaced_session,
        Some(ReplacedSession {
            session_id: replaced.clone(),
            reason: ReplacedSessionReason::Absent,
        }),
        "the status carries the typed marker until the next wake"
    );
    assert!(
        status.wake_refusal.is_none(),
        "an absent session is replaced, not refused"
    );

    boot.send(&alice, "Which token did I give you?").await;
    let request = boot.capture.last().expect("a post-wake request");
    assert!(
        !request.contains(TOKEN),
        "the fresh life cannot replay a transcript Core does not hold"
    );
    boot.assert_continuity_and_leases_unread();
    boot.stop().await;
}

/// An identity a host adopted (Core holds its identity intent row) is
/// read-only to every Meta verb (the wake door, retire, respawn, reset,
/// delete_identity, reload). Each refuses typed and moves nothing in Core; a
/// roster entry naming it is a typed conflict in the compile report; a
/// roster that drops it never retires it; its status says it is
/// intent-managed. A delivery to its live member is not a lifecycle verb.
#[tokio::test(flavor = "multi_thread")]
async fn a_host_adopted_identity_is_read_only_to_every_meta_verb() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let state = temp.path().join("state");
    let mob_id = unique_mob_id("host-adopted");
    let alice = id("personal:alice");
    let roster = vec![spec("personal:alice")];

    let boot = Boot::start(&state, &mob_id, &roster).await;
    boot.compile(&roster).await;
    boot.send(&alice, "A turn before the host adopts this identity.")
        .await;
    let (_, session, _) = boot.status_session(&alice).await;
    let session = session.expect("a live identity reports its session");
    boot.adopt(&alice, &session).await;
    let generation = boot
        .core_member_generation(&alice)
        .await
        .expect("Core holds the adopted member");

    // A delivery to the live member is not a lifecycle verb: Meta hands the
    // member input and moves nothing. The wake door itself refuses.
    boot.send(&alice, "A Meta send after adoption.").await;
    let runtime = boot.runtime();
    assert_host_adopted("materialize", &alice, runtime.materialize(&alice).await);
    assert_host_adopted("retire", &alice, runtime.retire(&alice).await);
    assert_host_adopted("respawn", &alice, runtime.respawn(&alice).await);
    assert_host_adopted("reset", &alice, runtime.reset(&alice).await);
    assert_host_adopted(
        "delete_identity",
        &alice,
        runtime.delete_identity(&alice).await,
    );
    assert_host_adopted("reload_member", &alice, runtime.reload_member(&alice).await);

    let status = runtime.status(&alice).await.expect("status of alice");
    assert!(
        status.intent_managed,
        "the status names the identity intent-managed"
    );
    assert_eq!(status.session_id, Some(session), "the session stays Core's");

    let compiled = boot.compile_outcomes(&roster).await;
    match compiled.outcomes.get(&alice) {
        Some(RestoreOutcome::Refused(error)) => assert!(
            matches!(
                error.as_ref(),
                IdentityRuntimeError::IdentityHostAdopted { identity } if *identity == alice
            ),
            "{error:?}"
        ),
        other => {
            panic!("a roster entry naming the adopted identity is a typed conflict: {other:?}")
        }
    }
    boot.compile(&[]).await;
    assert_eq!(
        boot.core_member_generation(&alice).await,
        Some(generation),
        "no Meta verb and no roster compile may move Core's member of an adopted identity"
    );
    boot.assert_continuity_and_leases_unread();
    boot.stop().await;
}

/// The offline identity reset refuses an identity a host adopted, as every
/// Meta lifecycle verb does, and writes no first-boot row: the roster compile
/// refuses the adopted identity before its carry-out, so the row would stay
/// pending for good.
#[tokio::test(flavor = "multi_thread")]
async fn the_offline_identity_reset_refuses_a_host_adopted_identity() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let state = temp.path().join("state");
    let mob_id = unique_mob_id("reset-adopted");
    let alice = id("personal:alice");
    let roster = vec![spec("personal:alice")];

    let boot = Boot::start(&state, &mob_id, &roster).await;
    boot.compile(&roster).await;
    boot.send(&alice, "A turn before the host adopts this identity.")
        .await;
    let (_, session, _) = boot.status_session(&alice).await;
    let session = session.expect("a live identity reports its session");
    boot.adopt(&alice, &session).await;
    boot.stop().await;

    let report = meerkat_meta::storage_migrate_native::reset_identity(
        &meerkat_meta::storage_migrate_native::IdentityResetRequest::new(&state, alice.as_str())
            .with_mob_store(state.join(MOB_STORE_FILE))
            .with_apply(),
    )
    .await;
    assert!(
        matches!(
            report.refusal,
            Some(meerkat_meta::storage_migrate_native::IdentityResetRefusal::IdentityHostAdopted)
        ),
        "the reset refuses the adopted identity: {report:?}"
    );
    assert!(!report.written, "a refused reset writes nothing");
    assert!(
        !FirstBootManifest::path(&state).exists(),
        "a refused reset writes no first-boot row"
    );
}

/// An identity whose intent row Core cannot read (an unsupported or
/// malformed record) may declare anything, so Meta acts on it no more than on
/// an adopted one: every verb refuses as history unavailable (-32052,
/// retryable once the row reads) and the compile never retires it.
#[tokio::test(flavor = "multi_thread")]
async fn an_identity_whose_intent_row_core_cannot_read_is_refused_and_never_retired() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let state = temp.path().join("state");
    let mob_id = unique_mob_id("intent-unreadable");
    let alice = id("personal:alice");
    let roster = vec![spec("personal:alice")];

    let boot = Boot::start(&state, &mob_id, &roster).await;
    boot.compile(&roster).await;
    let generation = boot
        .core_member_generation(&alice)
        .await
        .expect("Core holds alice");
    boot.write_unreadable_intent_row(&alice);

    let runtime = boot.runtime();
    assert_history_unavailable("materialize", &alice, runtime.materialize(&alice).await);
    assert_history_unavailable("retire", &alice, runtime.retire(&alice).await);
    assert_history_unavailable("respawn", &alice, runtime.respawn(&alice).await);
    assert_history_unavailable("reset", &alice, runtime.reset(&alice).await);
    assert_history_unavailable(
        "delete_identity",
        &alice,
        runtime.delete_identity(&alice).await,
    );
    assert_history_unavailable("reload_member", &alice, runtime.reload_member(&alice).await);

    boot.compile(&[]).await;
    assert_eq!(
        boot.core_member_generation(&alice).await,
        Some(generation),
        "the compile must never retire an identity whose intent row it cannot read"
    );
    boot.assert_continuity_and_leases_unread();
    boot.stop().await;
}

/// First boot, `Resume`: a migrated identity whose session Core's
/// session store holds, but Core's mob never bound (0.9.3 kept the mob
/// journal in memory), resumes that exact session on the first boot, with
/// its transcript. The manifest is removed once its last row is done.
/// Control: an identity with the same history and no row starts fresh.
#[tokio::test(flavor = "multi_thread")]
async fn a_first_boot_resume_row_resumes_the_migrated_session_with_its_transcript() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let state = temp.path().join("state");
    let mob_id = unique_mob_id("first-boot-resume");
    let alice = id("personal:alice");
    let bob = id("personal:bob");
    let roster = vec![spec("personal:alice"), spec("personal:bob")];
    const TOKEN_A: &str = "MARKER-FIRSTBOOT-RESUME-7-KILO";
    const TOKEN_B: &str = "MARKER-FIRSTBOOT-CONTROL-8-LIMA";

    let boot = Boot::start(&state, &mob_id, &roster).await;
    boot.compile(&roster).await;
    boot.send(&alice, &format!("Remember this token: {TOKEN_A}"))
        .await;
    boot.send(&bob, &format!("Remember this token: {TOKEN_B}"))
        .await;
    let (_, alice_session, _) = boot.status_session(&alice).await;
    let alice_session = alice_session.expect("alice's session");
    boot.stop().await;
    remove_mob_store(&state);
    FirstBootManifest::write_new(
        &state,
        vec![FirstBootRow::Resume {
            identity: alice.clone(),
            session: alice_session.clone(),
        }],
    )
    .expect("storage-migrate-native writes the first-boot manifest");

    let boot = Boot::start(&state, &mob_id, &roster).await;
    boot.compile(&roster).await;
    let (state_name, session, _) = boot.status_session(&alice).await;
    assert_eq!(state_name, "active", "the first boot woke alice");
    assert_eq!(
        session,
        Some(alice_session.clone()),
        "the first boot resumes the session the migration names"
    );
    boot.send(&alice, "Which token did I give you?").await;
    let request = boot.capture.last().expect("alice's first-boot request");
    assert!(
        request.contains(TOKEN_A),
        "the resumed session keeps its transcript"
    );
    boot.send(&bob, "Which token did I give you?").await;
    let request = boot.capture.last().expect("bob's first-boot request");
    assert!(
        !request.contains(TOKEN_B),
        "control: without a row, an identity Core holds no binding of starts fresh"
    );
    assert!(
        !FirstBootManifest::path(&state).exists(),
        "the manifest is removed once its last row is done"
    );
    assert!(
        boot.runtime()
            .identity_bootstrap_status()
            .first_boot_pending
            .is_empty(),
        "no row is pending"
    );
    boot.assert_continuity_and_leases_unread();
    boot.stop().await;
}

/// First boot, `Fresh`: an identity whose 0.9.3 continuity was
/// deleted starts a fresh life on the first boot, although Core's last
/// binding still names its retired session. The old session stays in Core,
/// bound to nothing. Control: an identity with the same history and no row
/// resumes its retired session.
#[tokio::test(flavor = "multi_thread")]
async fn a_first_boot_fresh_row_starts_fresh_and_leaves_the_old_session_in_core_unbound() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let state = temp.path().join("state");
    let mob_id = unique_mob_id("first-boot-fresh");
    let alice = id("personal:alice");
    let bob = id("personal:bob");
    let roster = vec![spec("personal:alice"), spec("personal:bob")];
    const TOKEN_A: &str = "MARKER-FIRSTBOOT-FRESH-5-MIKE";
    const TOKEN_B: &str = "MARKER-FIRSTBOOT-RETIRED-6-NOVEMBER";

    let boot = Boot::start(&state, &mob_id, &roster).await;
    boot.compile(&roster).await;
    boot.send(&alice, &format!("Remember this token: {TOKEN_A}"))
        .await;
    boot.send(&bob, &format!("Remember this token: {TOKEN_B}"))
        .await;
    let (_, old_session, _) = boot.status_session(&alice).await;
    let old_session = old_session.expect("alice's session");
    for identity in [&alice, &bob] {
        boot.runtime()
            .retire(identity)
            .await
            .unwrap_or_else(|error| panic!("retire {identity}: {error}"));
    }
    boot.stop().await;
    FirstBootManifest::write_new(
        &state,
        vec![FirstBootRow::Fresh {
            identity: alice.clone(),
            retired_session: old_session.clone(),
        }],
    )
    .expect("storage-migrate-native writes the first-boot manifest");

    let boot = Boot::start(&state, &mob_id, &roster).await;
    boot.compile(&roster).await;
    let (_, session, _) = boot.status_session(&alice).await;
    let fresh_session = session.expect("alice's first-boot session");
    assert_ne!(
        fresh_session, old_session,
        "the first boot starts a fresh life, never the deleted session"
    );
    boot.send(&alice, "Which token did I give you?").await;
    let request = boot.capture.last().expect("alice's first-boot request");
    assert!(
        !request.contains(TOKEN_A),
        "the fresh life must not replay the deleted transcript"
    );
    assert!(
        boot.core_session_history(&old_session)
            .await
            .contains(TOKEN_A),
        "Core keeps the old session"
    );
    let history = boot.core_history().await;
    let core_alice = meerkat_meta::member_comms_id::roster_member_id_for_identity(alice.as_str());
    assert_eq!(
        history
            .last_binding(&core_alice)
            .and_then(|binding| binding.session_id().cloned()),
        Some(fresh_session),
        "Core's last binding of alice is the fresh session"
    );
    assert_eq!(
        history
            .binding_for_session(&old_session)
            .map(meerkat_mob::MemberBinding::state),
        Some(meerkat_mob::MemberBindingState::Retired),
        "the old session is bound to nothing"
    );
    boot.send(&bob, "Which token did I give you?").await;
    let request = boot.capture.last().expect("bob's first-boot request");
    assert!(
        request.contains(TOKEN_B),
        "control: without a row, the wake resumes the retired session"
    );
    assert!(!FirstBootManifest::path(&state).exists());
    boot.assert_continuity_and_leases_unread();
    boot.stop().await;
}

/// First boot, `Fresh` over a live binding: an identity whose life was
/// reset while the directory was stopped, live at its last shutdown. The
/// first boot replaces its live member by a successor on a fresh session,
/// so it stays live; the old session stays in Core, bound to nothing.
/// Control: a live identity without a row resumes its session.
#[tokio::test(flavor = "multi_thread")]
async fn a_first_boot_fresh_row_over_a_live_binding_gives_the_identity_its_successor() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let state = temp.path().join("state");
    let mob_id = unique_mob_id("first-boot-fresh-live");
    let alice = id("personal:alice");
    let bob = id("personal:bob");
    let roster = vec![spec("personal:alice"), spec("personal:bob")];
    const TOKEN_A: &str = "MARKER-FIRSTBOOT-FRESH-LIVE-7-OSCAR";
    const TOKEN_B: &str = "MARKER-FIRSTBOOT-LIVE-8-ROMEO";

    let boot = Boot::start(&state, &mob_id, &roster).await;
    boot.compile(&roster).await;
    boot.send(&alice, &format!("Remember this token: {TOKEN_A}"))
        .await;
    boot.send(&bob, &format!("Remember this token: {TOKEN_B}"))
        .await;
    let (_, old_session, _) = boot.status_session(&alice).await;
    let old_session = old_session.expect("alice's session");
    // Both stay live through the shutdown.
    boot.stop().await;
    FirstBootManifest::write_new(
        &state,
        vec![FirstBootRow::Fresh {
            identity: alice.clone(),
            retired_session: old_session.clone(),
        }],
    )
    .expect("the offline reset writes the first-boot manifest");

    let boot = Boot::start(&state, &mob_id, &roster).await;
    boot.compile(&roster).await;
    let (state_name, session, _) = boot.status_session(&alice).await;
    assert_eq!(state_name, "active", "the fresh life keeps alice live");
    let fresh_session = session.expect("alice's first-boot session");
    assert_ne!(
        fresh_session, old_session,
        "the first boot gives alice a fresh session, never the reset one"
    );
    let history = boot.core_history().await;
    let core_alice = meerkat_meta::member_comms_id::roster_member_id_for_identity(alice.as_str());
    let last = history
        .last_binding(&core_alice)
        .expect("Core's last binding of alice");
    assert_eq!(
        last.session_id().cloned(),
        Some(fresh_session),
        "Core's last binding of alice is the fresh session"
    );
    assert_eq!(
        last.state(),
        meerkat_mob::MemberBindingState::Live,
        "the successor is live"
    );
    assert_eq!(
        history
            .binding_for_session(&old_session)
            .map(meerkat_mob::MemberBinding::state),
        Some(meerkat_mob::MemberBindingState::Retired),
        "the old session is bound to nothing"
    );
    boot.send(&alice, "Which token did I give you?").await;
    let request = boot.capture.last().expect("alice's first-boot request");
    assert!(
        !request.contains(TOKEN_A),
        "the fresh life must not replay the reset transcript"
    );
    assert!(
        boot.core_session_history(&old_session)
            .await
            .contains(TOKEN_A),
        "Core keeps the old session"
    );
    boot.send(&bob, "Which token did I give you?").await;
    let request = boot.capture.last().expect("bob's first-boot request");
    assert!(
        request.contains(TOKEN_B),
        "control: without a row, a live identity resumes its session"
    );
    assert!(!FirstBootManifest::path(&state).exists());
    boot.assert_continuity_and_leases_unread();
    boot.stop().await;
}

/// A `Fresh` row over a live binding that the compile did not carry out
/// (the manifest was unreadable then) is carried out by the next delivery
/// to the live member, and that delivery goes to the successor: never to
/// the member the row retires.
#[tokio::test(flavor = "multi_thread")]
async fn a_delivery_carries_out_a_pending_fresh_row_and_goes_to_the_successor() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let state = temp.path().join("state");
    let mob_id = unique_mob_id("first-boot-fresh-delivery");
    let alice = id("personal:alice");
    let roster = vec![spec("personal:alice")];
    const TOKEN_A: &str = "MARKER-FIRSTBOOT-FRESH-DELIVERY-9-SIERRA";

    let boot = Boot::start(&state, &mob_id, &roster).await;
    boot.compile(&roster).await;
    boot.send(&alice, &format!("Remember this token: {TOKEN_A}"))
        .await;
    let (_, old_session, _) = boot.status_session(&alice).await;
    let old_session = old_session.expect("alice's session");
    boot.stop().await;
    let manifest_path = FirstBootManifest::path(&state);
    std::fs::write(&manifest_path, b"not a first-boot manifest")
        .expect("write an unreadable manifest");

    // Core resumes alice's live member. The compile cannot read the
    // manifest, so it carries nothing out, and a delivery is refused.
    let boot = Boot::start(&state, &mob_id, &roster).await;
    boot.compile_outcomes(&roster).await;
    assert!(
        boot.core_member_generation(&alice).await.is_some(),
        "Core holds alice's live member"
    );
    let send = boot
        .runtime()
        .send(
            &alice,
            &meerkat_core::ContentInput::Text("Are you there?".to_string()),
        )
        .await;
    assert!(
        matches!(
            send,
            Err(IdentityRuntimeError::FirstBootManifestUnreadable { .. })
        ),
        "a delivery to the live member is refused while the manifest cannot be read: {send:?}"
    );

    std::fs::remove_file(&manifest_path).expect("set the unreadable manifest aside");
    FirstBootManifest::write_new(
        &state,
        vec![FirstBootRow::Fresh {
            identity: alice.clone(),
            retired_session: old_session.clone(),
        }],
    )
    .expect("restore a readable manifest");
    boot.send(&alice, "Which token did I give you?").await;
    let request = boot.capture.last().expect("alice's request after the row");
    assert!(
        !request.contains(TOKEN_A),
        "the delivery goes to the successor's fresh session, never to the reset one"
    );
    let (state_name, session, generation) = boot.status_session(&alice).await;
    assert_eq!(state_name, "active", "the successor is live");
    assert_ne!(
        session.as_ref(),
        Some(&old_session),
        "alice is on a fresh session"
    );
    assert_eq!(
        generation,
        boot.core_member_generation(&alice).await,
        "alice reports the successor's generation"
    );
    assert_eq!(
        boot.core_history()
            .await
            .binding_for_session(&old_session)
            .map(meerkat_mob::MemberBinding::state),
        Some(meerkat_mob::MemberBindingState::Retired),
        "the old session is bound to nothing"
    );
    assert!(
        !manifest_path.exists(),
        "the carried-out manifest is removed"
    );
    boot.assert_continuity_and_leases_unread();
    boot.stop().await;
}

/// First boot: a boot that stops after carrying a row out but
/// before the manifest says so finishes on the next boot. Each row is read
/// against Core's history again: the done row is not carried out twice
/// (alice keeps the session of her one fresh life), and the rest is.
#[tokio::test(flavor = "multi_thread")]
async fn a_boot_that_stops_part_way_through_the_manifest_finishes_on_the_next_boot() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let state = temp.path().join("state");
    let mob_id = unique_mob_id("first-boot-resume-stop");
    let alice = id("personal:alice");
    let bob = id("personal:bob");
    let roster = vec![spec("personal:alice"), spec("personal:bob")];

    let boot = Boot::start(&state, &mob_id, &roster).await;
    boot.compile(&roster).await;
    let mut retired = BTreeMap::new();
    for identity in [&alice, &bob] {
        boot.send(identity, "A turn before the migration.").await;
        let (_, session, _) = boot.status_session(identity).await;
        retired.insert(identity.clone(), session.expect("a session"));
        boot.runtime()
            .retire(identity)
            .await
            .unwrap_or_else(|error| panic!("retire {identity}: {error}"));
    }
    boot.stop().await;
    let rows = [&alice, &bob]
        .into_iter()
        .map(|identity| FirstBootRow::Fresh {
            identity: identity.clone(),
            retired_session: retired[identity].clone(),
        })
        .collect();
    FirstBootManifest::write_new(&state, rows).expect("write the manifest");
    let manifest_path = FirstBootManifest::path(&state);
    let original = std::fs::read(&manifest_path).expect("the manifest bytes");

    // The first boot lists alice alone: her row is carried out, bob's waits.
    let alice_roster = vec![spec("personal:alice")];
    let boot = Boot::start(&state, &mob_id, &alice_roster).await;
    boot.compile(&alice_roster).await;
    let (_, alice_fresh, _) = boot.status_session(&alice).await;
    let alice_fresh = alice_fresh.expect("alice's fresh session");
    assert_ne!(alice_fresh, retired[&alice]);
    boot.stop().await;
    // Stopped before the file said so: it still lists both rows.
    std::fs::write(&manifest_path, &original).expect("restore the two-row manifest");

    let boot = Boot::start(&state, &mob_id, &roster).await;
    boot.compile(&roster).await;
    let (_, alice_session, _) = boot.status_session(&alice).await;
    assert_eq!(
        alice_session,
        Some(alice_fresh),
        "a done row is not carried out twice"
    );
    let (_, bob_session, _) = boot.status_session(&bob).await;
    let bob_session = bob_session.expect("bob's session");
    assert_ne!(
        bob_session, retired[&bob],
        "the rest of the manifest is carried out"
    );
    assert!(!manifest_path.exists(), "the finished manifest is removed");
    boot.assert_continuity_and_leases_unread();
    boot.stop().await;
}

/// First boot: a row Core's history contradicts (Core's last
/// binding names another session: its mob store is not the one the
/// migration read) refuses that identity's wake typed, in the
/// storage-resolution class, and keeps the manifest. It does not block the
/// rest of the roster.
#[tokio::test(flavor = "multi_thread")]
async fn a_first_boot_row_core_contradicts_refuses_that_identity_and_keeps_the_manifest() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let state = temp.path().join("state");
    let mob_id = unique_mob_id("first-boot-mismatch");
    let alice = id("personal:alice");
    let bob = id("personal:bob");
    let roster = vec![spec("personal:alice"), spec("personal:bob")];

    let boot = Boot::start(&state, &mob_id, &roster).await;
    boot.compile(&roster).await;
    boot.send(&alice, "A turn on alice's own session.").await;
    let alice_core = meerkat_meta::member_comms_id::roster_member_id_for_identity(alice.as_str());
    let core_binding = |history: &meerkat_mob::MemberBindingHistory| {
        history
            .last_binding(&alice_core)
            .map(|binding| (binding.session_id().cloned(), binding.generation()))
    };
    let alice_binding = core_binding(&boot.core_history().await);
    assert!(
        alice_binding.is_some(),
        "Core binds alice on the first boot"
    );
    boot.stop().await;
    let migrated_elsewhere = SessionId::new();
    FirstBootManifest::write_new(
        &state,
        vec![FirstBootRow::Resume {
            identity: alice.clone(),
            session: migrated_elsewhere,
        }],
    )
    .expect("write the manifest");
    let manifest_path = FirstBootManifest::path(&state);
    let written = std::fs::read(&manifest_path).expect("the manifest bytes");

    let boot = Boot::start(&state, &mob_id, &roster).await;
    let outcomes = boot.compile_outcomes(&roster).await;
    assert_first_boot_row_refused(&alice, outcomes.outcomes.get(&alice));
    assert!(
        !matches!(
            outcomes.outcomes.get(&bob),
            Some(RestoreOutcome::Refused(_))
        ),
        "the rest of the roster boots: {:?}",
        outcomes.outcomes.get(&bob)
    );
    let send = boot
        .runtime()
        .send(
            &alice,
            &meerkat_core::ContentInput::Text("Are you there?".to_string()),
        )
        .await;
    assert!(
        matches!(
            send,
            Err(IdentityRuntimeError::FirstBootRowRefused {
                kind: FirstBootRowKind::Resume,
                ..
            })
        ),
        "a send to alice is refused typed, nothing submitted: {send:?}"
    );
    assert_eq!(
        std::fs::read(&manifest_path).expect("the manifest is kept"),
        written,
        "a refused row keeps the manifest unchanged"
    );
    assert_eq!(
        core_binding(&boot.core_history().await),
        alice_binding,
        "a refused row never overwrites Core's binding of the identity"
    );
    let pending = boot
        .runtime()
        .identity_bootstrap_status()
        .first_boot_pending;
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].identity, alice);
    assert_eq!(pending[0].waiting_for, FirstBootWait::CarryOut);
    boot.assert_continuity_and_leases_unread();
    boot.stop().await;
}

/// First boot: a `Resume` row naming a session Core holds but has
/// moved past (here a reset bound the identity to a fresh session; for a
/// builder app, a mob store other than the one the migration read) refuses
/// that identity typed and keeps the manifest. Core's binding is never
/// overwritten: the row's session is not resumed.
#[tokio::test(flavor = "multi_thread")]
async fn a_resume_row_core_has_moved_past_refuses_and_never_overwrites_the_binding() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let state = temp.path().join("state");
    let mob_id = unique_mob_id("first-boot-moved-past");
    let alice = id("personal:alice");
    let roster = vec![spec("personal:alice")];

    let boot = Boot::start(&state, &mob_id, &roster).await;
    boot.compile(&roster).await;
    boot.send(&alice, "A turn on alice's first session.").await;
    let (_, first_session, _) = boot.status_session(&alice).await;
    let first_session = first_session.expect("alice's first session");
    let reset = boot.runtime().reset(&alice).await;
    assert!(reset.is_ok(), "reset of a live identity: {reset:?}");
    let alice_core = meerkat_meta::member_comms_id::roster_member_id_for_identity(alice.as_str());
    let core_binding = |history: &meerkat_mob::MemberBindingHistory| {
        history
            .last_binding(&alice_core)
            .map(|binding| (binding.session_id().cloned(), binding.generation()))
    };
    let alice_binding = core_binding(&boot.core_history().await);
    assert!(
        alice_binding
            .as_ref()
            .is_some_and(|(session, _)| session.as_ref() != Some(&first_session)),
        "the reset bound alice to a fresh session: {alice_binding:?}"
    );
    boot.stop().await;
    FirstBootManifest::write_new(
        &state,
        vec![FirstBootRow::Resume {
            identity: alice.clone(),
            session: first_session.clone(),
        }],
    )
    .expect("write the manifest");
    let manifest_path = FirstBootManifest::path(&state);
    let written = std::fs::read(&manifest_path).expect("the manifest bytes");

    let boot = Boot::start(&state, &mob_id, &roster).await;
    let outcomes = boot.compile_outcomes(&roster).await;
    assert_first_boot_row_refused(&alice, outcomes.outcomes.get(&alice));
    let send = boot
        .runtime()
        .send(
            &alice,
            &meerkat_core::ContentInput::Text("Are you there?".to_string()),
        )
        .await;
    assert!(
        matches!(
            send,
            Err(IdentityRuntimeError::FirstBootRowRefused {
                kind: FirstBootRowKind::Resume,
                ..
            })
        ),
        "a send to alice is refused typed, nothing submitted: {send:?}"
    );
    assert_eq!(
        core_binding(&boot.core_history().await),
        alice_binding,
        "the refused row never overwrites Core's binding of the identity"
    );
    assert_eq!(
        std::fs::read(&manifest_path).expect("the manifest is kept"),
        written,
        "a refused row keeps the manifest unchanged"
    );
    boot.assert_continuity_and_leases_unread();
    boot.stop().await;
}

/// First boot: a row whose identity the roster does
/// not list stays pending and the manifest is kept; the status lists it as
/// waiting for the roster. The roster compile that lists the identity
/// carries the row out before anything wakes it, so its wake starts fresh.
#[tokio::test(flavor = "multi_thread")]
async fn an_unrostered_first_boot_row_waits_for_the_roster_and_is_carried_out_first() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let state = temp.path().join("state");
    let mob_id = unique_mob_id("first-boot-unrostered");
    let bob = id("personal:bob");
    let roster = vec![spec("personal:alice"), spec("personal:bob")];
    const TOKEN_B: &str = "MARKER-FIRSTBOOT-UNROSTERED-9-OSCAR";

    let boot = Boot::start(&state, &mob_id, &roster).await;
    boot.compile(&roster).await;
    boot.send(&bob, &format!("Remember this token: {TOKEN_B}"))
        .await;
    let (_, bob_old, _) = boot.status_session(&bob).await;
    let bob_old = bob_old.expect("bob's session");
    boot.runtime()
        .retire(&bob)
        .await
        .unwrap_or_else(|error| panic!("retire bob: {error}"));
    boot.stop().await;
    FirstBootManifest::write_new(
        &state,
        vec![FirstBootRow::Fresh {
            identity: bob.clone(),
            retired_session: bob_old.clone(),
        }],
    )
    .expect("write the manifest");

    let alice_roster = vec![spec("personal:alice")];
    let boot = Boot::start(&state, &mob_id, &alice_roster).await;
    boot.compile(&alice_roster).await;
    let pending = boot
        .runtime()
        .identity_bootstrap_status()
        .first_boot_pending;
    assert_eq!(pending.len(), 1, "bob's row waits: {pending:?}");
    assert_eq!(pending[0].identity, bob);
    assert_eq!(pending[0].kind, FirstBootRowKind::Fresh);
    assert_eq!(pending[0].waiting_for, FirstBootWait::Roster);
    assert!(
        FirstBootManifest::path(&state).exists(),
        "the manifest is kept while a row waits"
    );

    boot.compile(&roster).await;
    let (_, bob_session, _) = boot.status_session(&bob).await;
    assert_ne!(
        bob_session,
        Some(bob_old),
        "the compile that lists bob carries his row out before it wakes him"
    );
    boot.send(&bob, "Which token did I give you?").await;
    let request = boot.capture.last().expect("bob's request");
    assert!(!request.contains(TOKEN_B), "bob's wake starts fresh");
    assert!(!FirstBootManifest::path(&state).exists());
    assert!(
        boot.runtime()
            .identity_bootstrap_status()
            .first_boot_pending
            .is_empty()
    );
    boot.assert_continuity_and_leases_unread();
    boot.stop().await;
}

/// A directory `storage-migrate-native` migrated before 0.9.5 can hold a
/// `Fresh` row for a helper or a fork child the host made on the mob and
/// retired (no identity-first spawn made it, and no roster lists it). The
/// next roster compile settles that row, since Core's history shows nothing
/// left to do: the identity's wake resumes only a member the identity
/// runtime spawned. The bootstrap status lists no pending row, the condition
/// a host's mob reset waits for, the manifest is removed, the helper's
/// retired session stays in Core, and the mob reset goes ahead.
#[tokio::test(flavor = "multi_thread")]
async fn a_migrated_fresh_row_for_a_retired_raw_helper_is_settled_by_the_next_roster_compile() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let state = temp.path().join("state");
    let mob_id = unique_mob_id("first-boot-retired-raw-helper");
    let helper = id("helper:digest");
    let roster = vec![spec("personal:alice")];

    let boot = Boot::start(&state, &mob_id, &roster).await;
    boot.compile(&roster).await;
    let helper_session = boot
        .spawn_core_member(
            &helper,
            "MARKER-RETIRED-RAW-HELPER-4-ROMEO",
            BTreeMap::from([("purpose".to_string(), "digest".to_string())]),
        )
        .await;
    boot.retire_core_member(&helper).await;
    boot.stop().await;
    // The row a migration before 0.9.5 wrote for the retired helper.
    FirstBootManifest::write_new(
        &state,
        vec![FirstBootRow::Fresh {
            identity: helper.clone(),
            retired_session: helper_session.clone(),
        }],
    )
    .expect("write the manifest");

    let boot = Boot::start(&state, &mob_id, &roster).await;
    boot.compile(&roster).await;
    let pending = boot
        .runtime()
        .identity_bootstrap_status()
        .first_boot_pending;
    let manifest_kept = FirstBootManifest::path(&state).exists();
    let helper_binding = boot.core_last_binding(&helper).await;
    let reset = boot.unified.mob_handle().reset().await;
    boot.stop().await;

    assert!(
        pending.is_empty(),
        "the helper's row is settled, so nothing is pending: {pending:?}"
    );
    assert!(
        !manifest_kept,
        "the manifest is removed once its last row is settled"
    );
    assert!(
        !helper_binding.live && helper_binding.session == Some(helper_session),
        "the helper stays retired on its session in Core"
    );
    assert!(
        reset.is_ok(),
        "the mob reset goes ahead once no first-boot row is pending: {reset:?}"
    );
}

/// First boot: a `Resume` row whose identity the roster does not
/// list stays pending, waiting for the roster, and the manifest is kept;
/// nothing wakes the identity. Once a roster lists it again, that compile
/// carries the row out: the identity resumes the session the row names,
/// with its transcript.
#[tokio::test(flavor = "multi_thread")]
async fn an_unrostered_resume_row_waits_for_the_roster_then_resumes_its_session() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let state = temp.path().join("state");
    let mob_id = unique_mob_id("first-boot-unrostered-resume");
    let carol = id("personal:carol");
    let roster = vec![spec("personal:alice"), spec("personal:carol")];
    const TOKEN_C: &str = "MARKER-FIRSTBOOT-UNROSTERED-RESUME-5-PAPA";

    let boot = Boot::start(&state, &mob_id, &roster).await;
    boot.compile(&roster).await;
    boot.send(&carol, &format!("Remember this token: {TOKEN_C}"))
        .await;
    let (_, carol_session, _) = boot.status_session(&carol).await;
    let carol_session = carol_session.expect("carol's session");
    boot.stop().await;
    remove_mob_store(&state);
    FirstBootManifest::write_new(
        &state,
        vec![FirstBootRow::Resume {
            identity: carol.clone(),
            session: carol_session.clone(),
        }],
    )
    .expect("write the manifest");
    let manifest_path = FirstBootManifest::path(&state);
    let written = std::fs::read(&manifest_path).expect("the manifest bytes");

    let alice_roster = vec![spec("personal:alice")];
    let boot = Boot::start(&state, &mob_id, &alice_roster).await;
    boot.compile(&alice_roster).await;
    let pending = boot
        .runtime()
        .identity_bootstrap_status()
        .first_boot_pending;
    assert_eq!(pending.len(), 1, "carol's row waits: {pending:?}");
    assert_eq!(pending[0].identity, carol);
    assert_eq!(pending[0].kind, FirstBootRowKind::Resume);
    assert_eq!(pending[0].waiting_for, FirstBootWait::Roster);
    assert_eq!(
        std::fs::read(&manifest_path).expect("the manifest is kept"),
        written,
        "the manifest is kept while a row waits"
    );
    let carol_core = meerkat_meta::member_comms_id::roster_member_id_for_identity(carol.as_str());
    assert!(
        boot.core_history()
            .await
            .last_binding(&carol_core)
            .is_none(),
        "nothing woke carol while her row waits for the roster"
    );

    boot.compile(&roster).await;
    let (state_name, session, _) = boot.status_session(&carol).await;
    assert_eq!(
        state_name, "active",
        "the compile that lists carol carries her row out"
    );
    assert_eq!(
        session,
        Some(carol_session),
        "carol resumes the session her row names"
    );
    boot.send(&carol, "Which token did I give you?").await;
    let request = boot.capture.last().expect("carol's request");
    assert!(
        request.contains(TOKEN_C),
        "the resumed session keeps its transcript"
    );
    assert!(
        !manifest_path.exists(),
        "the manifest is removed once its last row is done"
    );
    boot.assert_continuity_and_leases_unread();
    boot.stop().await;
}

/// First boot: a `Resume` row naming a session Core's
/// session store holds no durable copy of is refused typed, with no Core
/// session (`core_session: None`). Core's session state is read before any
/// resume, so no fresh life starts in the session's place: Core keeps no
/// binding of the identity, and the manifest is kept unchanged.
#[tokio::test(flavor = "multi_thread")]
async fn a_first_boot_resume_row_whose_session_core_does_not_hold_starts_no_fresh_life() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let state = temp.path().join("state");
    std::fs::create_dir_all(&state).expect("create the state directory");
    let mob_id = unique_mob_id("first-boot-absent");
    let alice = id("personal:alice");
    let bob = id("personal:bob");
    let roster = vec![spec("personal:alice"), spec("personal:bob")];
    let never_migrated = SessionId::new();
    FirstBootManifest::write_new(
        &state,
        vec![FirstBootRow::Resume {
            identity: alice.clone(),
            session: never_migrated.clone(),
        }],
    )
    .expect("write the manifest");
    let manifest_path = FirstBootManifest::path(&state);
    let written = std::fs::read(&manifest_path).expect("the manifest bytes");

    let boot = Boot::start(&state, &mob_id, &roster).await;
    let outcomes = boot.compile_outcomes(&roster).await;
    match outcomes.outcomes.get(&alice) {
        Some(RestoreOutcome::Refused(error)) => match error.as_ref() {
            IdentityRuntimeError::FirstBootRowRefused {
                identity,
                kind,
                manifest_session,
                core_session,
                ..
            } => {
                assert_eq!(identity, &alice);
                assert_eq!(*kind, FirstBootRowKind::Resume);
                assert_eq!(manifest_session, &never_migrated);
                assert_eq!(core_session, &None, "Core holds no session for the row");
            }
            other => panic!("alice's row must be refused typed, got {other:?}"),
        },
        other => panic!("alice's row must be refused typed, got {other:?}"),
    }
    assert!(
        !matches!(
            outcomes.outcomes.get(&bob),
            Some(RestoreOutcome::Refused(_))
        ),
        "the rest of the roster boots: {:?}",
        outcomes.outcomes.get(&bob)
    );
    let alice_core = meerkat_meta::member_comms_id::roster_member_id_for_identity(alice.as_str());
    assert!(
        boot.core_history()
            .await
            .last_binding(&alice_core)
            .is_none(),
        "no fresh life started in place of the row's session"
    );
    assert_eq!(boot.core_member_generation(&alice).await, None);
    let send = boot
        .runtime()
        .send(
            &alice,
            &meerkat_core::ContentInput::Text("Are you there?".to_string()),
        )
        .await;
    assert!(
        matches!(
            send,
            Err(IdentityRuntimeError::FirstBootRowRefused {
                core_session: None,
                ..
            })
        ),
        "a send to alice is refused typed and wakes nothing: {send:?}"
    );
    assert!(
        boot.core_history()
            .await
            .last_binding(&alice_core)
            .is_none(),
        "the refused send started no fresh life either"
    );
    assert_eq!(
        std::fs::read(&manifest_path).expect("the manifest is kept"),
        written,
        "a refused row keeps the manifest unchanged"
    );
    boot.assert_continuity_and_leases_unread();
    boot.stop().await;
}

/// First boot: while an identity's row is pending, reset
/// and delete are refused typed, and the refusal names the way out. A row
/// whose identity the roster lists waits for its carry-out: wake the
/// identity. A row whose identity the roster does not list waits for the
/// roster: add the identity to the roster.
#[tokio::test(flavor = "multi_thread")]
async fn a_pending_first_boot_row_refuses_reset_and_delete_with_its_way_out() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let state = temp.path().join("state");
    std::fs::create_dir_all(&state).expect("create the state directory");
    let mob_id = unique_mob_id("first-boot-pending-verbs");
    let alice = id("personal:alice");
    let carol = id("personal:carol");
    let roster = vec![spec("personal:alice")];
    FirstBootManifest::write_new(
        &state,
        vec![
            FirstBootRow::Resume {
                identity: alice.clone(),
                session: SessionId::new(),
            },
            FirstBootRow::Fresh {
                identity: carol.clone(),
                retired_session: SessionId::new(),
            },
        ],
    )
    .expect("write the manifest");

    let boot = Boot::start(&state, &mob_id, &roster).await;
    let outcomes = boot.compile_outcomes(&roster).await;
    assert_first_boot_row_refused(&alice, outcomes.outcomes.get(&alice));

    let pending = |verb: &str,
                   identity: &AgentIdentity,
                   wait: FirstBootWait,
                   remedy: &str,
                   result: Result<(), IdentityRuntimeError>| {
        let Err(error) = result else {
            panic!("{verb} of {identity} must be refused as pending");
        };
        match &error {
            IdentityRuntimeError::FirstBootRowPending {
                identity: refused,
                waiting_for,
                operation,
                ..
            } => {
                assert_eq!(refused, identity, "{verb}");
                assert_eq!(*waiting_for, wait, "{verb}");
                assert_eq!(*operation, verb);
            }
            other => panic!("{verb} of {identity} must be refused as pending: {other:?}"),
        }
        let data = error.structured_data().expect("typed refusal data");
        assert_eq!(data["kind"], "first_boot_row_pending", "{verb}");
        assert_eq!(data["remedy"], remedy, "{verb}: {data}");
    };
    pending(
        "reset",
        &alice,
        FirstBootWait::CarryOut,
        "wake_identity",
        boot.runtime().reset(&alice).await.map(|_| ()),
    );
    pending(
        "delete_identity",
        &alice,
        FirstBootWait::CarryOut,
        "wake_identity",
        boot.runtime().delete_identity(&alice).await.map(|_| ()),
    );
    pending(
        "reset",
        &carol,
        FirstBootWait::Roster,
        "add_to_roster",
        boot.runtime().reset(&carol).await.map(|_| ()),
    );
    let alice_core = meerkat_meta::member_comms_id::roster_member_id_for_identity(alice.as_str());
    let carol_core = meerkat_meta::member_comms_id::roster_member_id_for_identity(carol.as_str());
    let history = boot.core_history().await;
    assert!(
        history.last_binding(&alice_core).is_none() && history.last_binding(&carol_core).is_none(),
        "a refused verb starts no life"
    );
    let pending_rows = boot
        .runtime()
        .identity_bootstrap_status()
        .first_boot_pending;
    assert_eq!(
        pending_rows.len(),
        2,
        "both rows stay pending: {pending_rows:?}"
    );
    boot.assert_continuity_and_leases_unread();
    boot.stop().await;
}

/// First boot: a manifest the reader refuses refuses every
/// identity of the roster compile, typed, and wakes none: which identities
/// it covers is unknown. The compile itself completes, so the runtime stays
/// up and its bootstrap status names the file and the refusal. Once the
/// operator restores a readable manifest, the next compile carries it out.
#[tokio::test(flavor = "multi_thread")]
async fn an_unreadable_first_boot_manifest_refuses_every_identity_and_keeps_the_runtime_up() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let state = temp.path().join("state");
    let mob_id = unique_mob_id("first-boot-unreadable");
    let alice = id("personal:alice");
    let bob = id("personal:bob");
    let roster = vec![spec("personal:alice"), spec("personal:bob")];
    const TOKEN_A: &str = "MARKER-FIRSTBOOT-UNREADABLE-3-PAPA";

    let boot = Boot::start(&state, &mob_id, &roster).await;
    boot.compile(&roster).await;
    boot.send(&alice, &format!("Remember this token: {TOKEN_A}"))
        .await;
    let (_, alice_session, _) = boot.status_session(&alice).await;
    let alice_session = alice_session.expect("alice's session");
    boot.stop().await;
    remove_mob_store(&state);
    let manifest_path = FirstBootManifest::path(&state);
    std::fs::write(
        &manifest_path,
        br#"{"format":"meerkat-meta.first-boot-manifest","version":2,"rows_sha256":"00","rows":[]}"#,
    )
    .expect("write a manifest from a later format version");

    let boot = Boot::start(&state, &mob_id, &roster).await;
    let outcomes = boot.compile_outcomes(&roster).await;
    for identity in [&alice, &bob] {
        match outcomes.outcomes.get(identity) {
            Some(RestoreOutcome::Refused(error)) => assert!(
                matches!(
                    error.as_ref(),
                    IdentityRuntimeError::FirstBootManifestUnreadable { path, .. }
                        if path == &manifest_path
                ),
                "{identity} must be refused with the manifest's refusal, got {error:?}"
            ),
            other => panic!("{identity} must be refused typed, got {other:?}"),
        }
        assert_eq!(
            boot.core_member_generation(identity).await,
            None,
            "{identity} was not woken"
        );
    }
    let status = boot.runtime().identity_bootstrap_status();
    let error = status.error.expect("the pass-level slot names the refusal");
    assert!(
        error.contains(&manifest_path.display().to_string()),
        "the status names the manifest file: {error}"
    );
    let send = boot
        .runtime()
        .send(
            &alice,
            &meerkat_core::ContentInput::Text("Are you there?".to_string()),
        )
        .await;
    assert!(
        matches!(
            send,
            Err(IdentityRuntimeError::FirstBootManifestUnreadable { .. })
        ),
        "a send is refused with the manifest's refusal, and wakes nothing: {send:?}"
    );
    let reset = boot.runtime().reset(&bob).await;
    assert!(
        matches!(
            reset,
            Err(IdentityRuntimeError::FirstBootManifestUnreadable { .. })
        ),
        "reset is refused while the manifest cannot be read: {reset:?}"
    );

    std::fs::remove_file(&manifest_path).expect("set the unreadable manifest aside");
    FirstBootManifest::write_new(
        &state,
        vec![FirstBootRow::Resume {
            identity: alice.clone(),
            session: alice_session.clone(),
        }],
    )
    .expect("restore a readable manifest");
    boot.compile(&roster).await;
    let (state_name, session, _) = boot.status_session(&alice).await;
    assert_eq!(state_name, "active", "the next compile woke alice");
    assert_eq!(session, Some(alice_session), "on the session the row names");
    assert!(
        boot.runtime().identity_bootstrap_status().error.is_none(),
        "the readable manifest's pass reports no pass-level failure"
    );
    assert!(
        !manifest_path.exists(),
        "the carried-out manifest is removed"
    );
    boot.assert_continuity_and_leases_unread();
    boot.stop().await;
}

/// A member Meta 0.9.3 spawned carries no durable-identity marker, and Core
/// restores it live across the upgrade, so no Meta wake resumes it and
/// nothing marks it. The first boot whose roster lists the identity respawns
/// it through Core, non-destructively: the same session and transcript at a
/// higher Core generation, with the marker on the new binding, so the
/// session now resolves as the identity. A member of that era whose identity
/// the roster dropped still carries Meta's identity label; the same boot
/// retires it, as it retires a marked member the roster dropped. A second
/// boot changes neither.
#[tokio::test(flavor = "multi_thread")]
async fn the_first_boot_respawns_an_unmarked_rostered_member_and_retires_a_dropped_one() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let state = temp.path().join("state");
    let mob_id = unique_mob_id("unmarked-respawn");
    let alice = id("personal:alice");
    let bob = id("personal:bob");
    let roster = vec![spec("personal:alice")];
    const TOKEN: &str = "MARKER-UNMARKED-RESPAWN-5-TANGO";

    let boot = Boot::start(&state, &mob_id, &[]).await;
    boot.compile(&[]).await;
    let session = boot.spawn_unmarked(&alice, TOKEN).await;
    let bob_session = boot
        .spawn_unmarked(&bob, "MARKER-UNMARKED-DROPPED-8-WHISKEY")
        .await;
    boot.stop().await;

    let boot = Boot::start(&state, &mob_id, &roster).await;
    let restored = boot.core_last_binding(&alice).await;
    assert!(
        restored.live && !restored.marked,
        "Core restores the 0.9.3 member live and unmarked: {restored:?}"
    );
    assert_eq!(restored.session, Some(session.clone()), "on its session");
    let dropped = boot.core_last_binding(&bob).await;
    assert!(
        dropped.live && !dropped.marked,
        "Core restores the dropped identity's 0.9.3 member live and unmarked: {dropped:?}"
    );
    boot.compile(&roster).await;
    boot.assert_durable_identity_marker(&alice, "the first boot's respawn")
        .await;
    let respawned = boot.core_last_binding(&alice).await;
    assert!(
        respawned.live && respawned.marked,
        "the respawned binding is live and carries alice's marker: {respawned:?}"
    );
    assert!(
        respawned.generation > restored.generation,
        "the respawn runs under a new Core generation ({} -> {})",
        restored.generation,
        respawned.generation
    );
    assert_eq!(
        respawned.session,
        Some(session.clone()),
        "the respawn keeps the session"
    );
    assert_eq!(
        boot.core_member_generation(&alice).await,
        Some(respawned.generation),
        "Core holds alice live at the respawned generation"
    );
    let history = boot.core_history().await;
    let session_binding = history
        .binding_for_session(&session)
        .expect("Core binds the kept session");
    assert_eq!(
        session_binding.continuity_intent(),
        &meerkat_mob::SpawnContinuityIntent::DurableIdentity {
            continuity_key: alice.as_str().to_string(),
        },
        "the binding that names the session carries alice's marker, so the session \
         resolves as alice"
    );
    assert_eq!(
        boot.runtime()
            .core_session_owner(&session)
            .await
            .expect("Core's session owner read"),
        Some(alice.clone())
    );
    let retired = boot.core_last_binding(&bob).await;
    assert!(
        !retired.live,
        "the boot retires the member of the identity its roster dropped: {retired:?}"
    );
    assert_eq!(
        (retired.generation, retired.session.as_ref(), retired.marked),
        (dropped.generation, Some(&bob_session), false),
        "the retirement respawns nothing: the same generation and session, unmarked"
    );
    assert_eq!(
        boot.core_member_generation(&bob).await,
        None,
        "Core holds no member for the dropped identity"
    );
    boot.send(&alice, "Which token did I give you?").await;
    let request = boot
        .capture
        .last()
        .expect("alice's request after the respawn");
    assert!(request.contains(TOKEN), "the respawn keeps the transcript");
    boot.assert_continuity_and_leases_unread();
    boot.stop().await;

    let boot = Boot::start(&state, &mob_id, &roster).await;
    let alice_before = boot.core_last_binding(&alice).await;
    let bob_before = boot.core_last_binding(&bob).await;
    assert!(
        alice_before.live && alice_before.marked,
        "alice stays live and marked across the restart: {alice_before:?}"
    );
    boot.compile(&roster).await;
    assert_eq!(
        boot.core_last_binding(&alice).await,
        alice_before,
        "a second boot respawns nothing"
    );
    assert_eq!(
        boot.core_last_binding(&bob).await,
        bob_before,
        "a second boot leaves the retired member as it is"
    );
    boot.assert_continuity_and_leases_unread();
    boot.stop().await;
}

/// A member the host made directly on the mob under an identity's member id
/// carries no `agent_identity` label naming the identity. A roster that
/// later declares the identity does not adopt it: the compile refuses typed,
/// naming the member and its session, before it registers anything, and
/// leaves the member as the host made it (no respawn, its transcript stays
/// its own). Once the host labels the member for the identity on its
/// session, the next compile adopts it the way it adopts a member Meta 0.9.3
/// spawned: one respawn onto the same session, marked, with the transcript
/// kept.
#[tokio::test(flavor = "multi_thread")]
async fn a_raw_member_under_a_declared_identitys_member_id_is_refused_until_labelled() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let state = temp.path().join("state");
    let mob_id = unique_mob_id("raw-name-refused");
    let alice = id("personal:alice");
    let roster = vec![spec("personal:alice")];
    const TOKEN: &str = "MARKER-RAW-NAME-3-KILO";

    let boot = Boot::start(&state, &mob_id, &[]).await;
    boot.compile(&[]).await;
    let session = boot
        .spawn_core_member(
            &alice,
            TOKEN,
            BTreeMap::from([("team".to_string(), "direct".to_string())]),
        )
        .await;
    boot.stop().await;

    let boot = Boot::start(&state, &mob_id, &roster).await;
    let raw = boot.core_last_binding(&alice).await;
    assert!(
        raw.live && !raw.marked,
        "Core restores the raw member: {raw:?}"
    );
    for compile in ["first", "second"] {
        let refusal = boot.compile_refusal(&roster).await;
        assert!(
            matches!(
                &refusal,
                IdentityRuntimeError::IdentityNameHeldByRawMember {
                    identity,
                    session_id,
                    ..
                } if *identity == alice && session_id.as_ref() == Some(&session)
            ),
            "{compile} compile: a roster entry under a raw member's member id is a typed \
             refusal: {refusal:?}"
        );
        assert_eq!(
            boot.core_last_binding(&alice).await,
            raw,
            "{compile} compile: the refusal leaves the member as the host made it, unrespawned"
        );
    }

    boot.relabel_core_member(
        &alice,
        &session,
        BTreeMap::from([
            ("agent_identity".to_string(), alice.as_str().to_string()),
            ("profile_name".to_string(), "personal".to_string()),
        ]),
    )
    .await;
    boot.compile(&roster).await;
    let adopted = boot.core_last_binding(&alice).await;
    assert!(
        adopted.live && adopted.marked,
        "the labelled member is adopted and marked: {adopted:?}"
    );
    assert_eq!(
        adopted.session,
        Some(session.clone()),
        "the adoption keeps the session"
    );
    boot.send(&alice, "Which token did I give you?").await;
    let request = boot
        .capture
        .last()
        .expect("alice's request after the adoption");
    assert!(request.contains(TOKEN), "the adoption keeps the transcript");
    boot.assert_continuity_and_leases_unread();
    boot.stop().await;
}

/// The other remedy: the host retires its raw member before a roster
/// declares an identity under that member id. Core's last binding of the
/// member id is then that retired raw member, which no identity-first spawn
/// made, so the identity's wake starts fresh on a new session: the raw
/// member's transcript never becomes the identity's history.
#[tokio::test(flavor = "multi_thread")]
async fn a_retired_raw_member_never_becomes_a_declared_identitys_history() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let state = temp.path().join("state");
    let mob_id = unique_mob_id("raw-name-retired");
    let alice = id("personal:alice");
    let roster = vec![spec("personal:alice")];
    const TOKEN: &str = "MARKER-RAW-RETIRED-4-LIMA";

    let boot = Boot::start(&state, &mob_id, &[]).await;
    boot.compile(&[]).await;
    let raw_session = boot
        .spawn_core_member(
            &alice,
            TOKEN,
            BTreeMap::from([("team".to_string(), "direct".to_string())]),
        )
        .await;
    boot.retire_core_member(&alice).await;
    let retired = boot.core_last_binding(&alice).await;
    assert!(!retired.live && !retired.marked, "{retired:?}");
    boot.stop().await;

    let boot = Boot::start(&state, &mob_id, &roster).await;
    boot.compile(&roster).await;
    let woken = boot.core_last_binding(&alice).await;
    assert!(woken.live && woken.marked, "alice wakes marked: {woken:?}");
    assert_ne!(
        woken.session.as_ref(),
        Some(&raw_session),
        "alice starts on a session of her own"
    );
    boot.send(&alice, "Which token did I give you?").await;
    let request = boot.capture.last().expect("alice's first request");
    assert!(
        !request.contains(TOKEN),
        "the raw member's transcript is not alice's history"
    );
    boot.assert_continuity_and_leases_unread();
    boot.stop().await;
}

/// Delete reads the identity's previous life as the wake does. After Meta
/// retired alice, the host made and retired a raw member under her member
/// id, so Core's last binding of the member id is that raw member's. The
/// delete finds no life of alice's to end: it answers as for an identity
/// Core never spawned and leaves Core's last binding as the raw member left
/// it, rather than distilling that member's session and starting a fresh
/// life over it.
#[tokio::test(flavor = "multi_thread")]
async fn a_delete_never_takes_a_retired_raw_members_session_as_the_identitys_life() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let state = temp.path().join("state");
    let mob_id = unique_mob_id("raw-name-delete");
    let alice = id("personal:alice");
    let roster = vec![spec("personal:alice")];
    const TOKEN: &str = "MARKER-RAW-DELETE-8-MIKE";

    let boot = Boot::start(&state, &mob_id, &roster).await;
    boot.compile(&roster).await;
    let retired = boot.runtime().retire(&alice).await;
    assert!(retired.is_ok(), "retire of a live identity: {retired:?}");
    let raw_session = boot
        .spawn_core_member(
            &alice,
            TOKEN,
            BTreeMap::from([("team".to_string(), "direct".to_string())]),
        )
        .await;
    boot.retire_core_member(&alice).await;
    let before = boot.core_last_binding(&alice).await;
    assert!(
        !before.live && !before.marked && before.session.as_ref() == Some(&raw_session),
        "Core's last binding of alice's member id is the retired raw member: {before:?}"
    );

    let deleted = boot.runtime().delete_identity(&alice).await;
    assert!(
        matches!(deleted, Ok(None)),
        "a delete whose last binding is a raw member's acts as for an identity Core never \
         spawned: {deleted:?}"
    );
    assert_eq!(
        boot.core_last_binding(&alice).await,
        before,
        "the delete starts no fresh life over the raw member's binding"
    );
    boot.assert_continuity_and_leases_unread();
    boot.stop().await;
}

/// A host-adopted identity is Core's to drive. The roster compile never
/// respawns it, even when its member carries no durable-identity marker and
/// the roster lists it: the compile reports the typed host-adopted conflict
/// and leaves Core's binding as it was. Nor does the compile retire an
/// adopted member whose identity the roster does not list, although the
/// member carries Meta's identity label. An unmarked identity beside them is
/// respawned by the same compile.
#[tokio::test(flavor = "multi_thread")]
async fn the_roster_compile_never_respawns_or_retires_a_host_adopted_member() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let state = temp.path().join("state");
    let mob_id = unique_mob_id("unmarked-adopted");
    let alice = id("personal:alice");
    let bob = id("personal:bob");
    let carol = id("personal:carol");
    let roster = vec![spec("personal:alice"), spec("personal:bob")];

    let boot = Boot::start(&state, &mob_id, &roster).await;
    boot.compile(&[]).await;
    let alice_session = boot
        .spawn_unmarked(&alice, "MARKER-UNMARKED-ADOPTED-6-UNIFORM")
        .await;
    let bob_session = boot
        .spawn_unmarked(&bob, "MARKER-UNMARKED-CONTROL-7-VICTOR")
        .await;
    let carol_session = boot
        .spawn_unmarked(&carol, "MARKER-UNMARKED-ADOPTED-9-XRAY")
        .await;
    boot.adopt(&alice, &alice_session).await;
    boot.adopt_as(&carol, &carol_session, "core-authority-carol")
        .await;
    let adopted = boot.core_last_binding(&alice).await;
    let adopted_unlisted = boot.core_last_binding(&carol).await;
    assert!(
        adopted.live && adopted_unlisted.live,
        "Core holds both adopted members live: {adopted:?} {adopted_unlisted:?}"
    );
    let unmarked = boot.core_last_binding(&bob).await;

    let compiled = boot.compile_outcomes(&roster).await;
    match compiled.outcomes.get(&alice) {
        Some(RestoreOutcome::Refused(error)) => assert!(
            matches!(
                error.as_ref(),
                IdentityRuntimeError::IdentityHostAdopted { identity } if *identity == alice
            ),
            "{error:?}"
        ),
        other => {
            panic!("a roster entry naming the adopted identity is a typed conflict: {other:?}")
        }
    }
    assert_eq!(
        boot.core_last_binding(&alice).await,
        adopted,
        "the compile leaves the listed adopted member as Core holds it"
    );
    assert_eq!(
        boot.core_last_binding(&carol).await,
        adopted_unlisted,
        "the compile never retires an adopted member the roster does not list"
    );
    let respawned = boot.core_last_binding(&bob).await;
    assert!(
        respawned.live && respawned.marked && respawned.generation > unmarked.generation,
        "the unmarked identity beside them is respawned by the same compile: {unmarked:?} -> \
         {respawned:?}"
    );
    assert_eq!(respawned.session, Some(bob_session), "onto its session");
    boot.assert_continuity_and_leases_unread();
    boot.stop().await;
}
