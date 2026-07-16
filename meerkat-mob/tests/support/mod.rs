//! Shared multi-host mob test fixtures (multi-host mobs phase 2, Lane W4;
//! design §W4.2 in /tmp/mhm-phase2/design.md).
//!
//! Consumed via `mod support;` from each `meerkat-mob/tests/*.rs` root, and
//! via `#[path = "../../../meerkat-mob/tests/support/mod.rs"]` from
//! `tests/integration/tests/multi_host_bind.rs` (the e2e-fast walk). Each
//! consuming test root uses a subset of the items here, hence the module-wide
//! dead_code allowance.
//!
//! Contents:
//!   * `ProductionExternalTcpTarget` — the production-drain external member
//!     endpoint, MOVED verbatim out of `smoke_mob_flow_runtime.rs` so
//!     deterministic lanes can compose it too (harness map §1 / §5 delta 4).
//!   * `PeerCommsEndpoint` — a standalone real-comms peer (loopback TCP or
//!     inproc) with machine-gated classification, generated projection trust,
//!     and peer request/response authority. Used as the raw "supervisor" /
//!     "attacker" / member-inbox role in the ceremony and demux tests.
//!   * `HostDaemonFixture` — "host B in miniature": everything the
//!     `rkat mob host` daemon composes (W2), minus the CLI. TDD-first: it is
//!     written against the Lane W1/W2 shapes and compiles once those land.
//!   * `ScriptedHostPeer` — a scripted `BindHost` responder with the
//!     `LiveExternalPeerHarness` fault injectors (`fail_next_bind`,
//!     `garble_next_bind_reply`, `override_next_bind_peer_id`) for the W4.3
//!     controlling-side ceremony failure matrix.
//!   * Controlling-mob helpers (real mob + TestClient; the ceremony spawns no
//!     members and needs no LLM turns).

#![allow(dead_code)]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::manual_assert
)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

// Trait imports for method-call resolution on `Arc<meerkat_comms::CommsRuntime>`
// (`send`, `drain_peer_input_candidates`) and on `Arc<MeerkatMachine>`
// (`list_active_inputs`) — the meerkat-mob/src/runtime/tests.rs and
// smoke_mob_flow_runtime.rs import discipline.
use meerkat_core::agent::CommsRuntime as _;
use meerkat_core::comms::TrustedPeerDescriptor;
use meerkat_core::interaction::InteractionContent;
use meerkat_core::{CommsCommand, PeerCorrelationId};
use meerkat_runtime::SessionServiceRuntimeExt as _;
use meerkat_runtime::meerkat_machine::MeerkatMachine;
use meerkat_runtime::meerkat_machine::dsl as machine_dsl;

use meerkat_comms::{
    HostAcceptorBounds, HostAcceptorConfig, HostAcceptorHandle, HostAcceptorIdentityRegistry,
    spawn_host_acceptor,
};
use meerkat_mob::runtime::HostBindRequest;
// Phase-3 note: the V4 materialize/release/status/operator wire family and the
// portable-spec family are consumed through `bridge_protocol` re-exports only —
// the integration-tests consumer of this file has no meerkat-contracts
// dependency (A1 lane owns the re-export block additions).
use meerkat_mob::runtime::bridge_protocol::{
    BridgeAck, BridgeCapabilities, BridgeCommand, BridgeHostBindResponse, BridgeHostMemberRecord,
    BridgeHostRevokedResponse, BridgeHostRuntimeIncarnation, BridgeHostStatusResponse,
    BridgeMaterializePayload, BridgeMaterializedResponse, BridgeMemberIncarnation,
    BridgeMemberReleasedResponse, BridgePeerTrustPayload, BridgeRejectionCause,
    BridgeReleasePayload, BridgeReply, MaterializeLaunchMode, MaterializeLaunchOutcome,
    MemberSessionDisposal as WireMemberSessionDisposal, PortableDefinitionExtract,
    PortableMemberSpec, PortableProfile, PortableSpawnOverlay, PortableSystemPrompt,
    PortableToolConfig, WireHostBindingDescriptor, WireMobRuntimeMode, WireSpawnContinuityIntent,
    portable_member_spec_digest,
};
use meerkat_mob::runtime::host_actor::{
    HostBindingDeletionAuthority, HostBindingPersistenceAuthority, HostCapabilityFacts,
    HostDescriptorSink, HostMemberSubstrate, MemberRowPersistenceAuthority, MobHostActorConfig,
    MobHostActorError, MobHostActorHandle, MobHostBindingPersistence, MobHostBindingRecord,
    MobHostRevocationReceipt, ProviderPresenceProbe, ProviderPresenceProbeError,
    RuntimeStoreHostBindingPersistence, TurnOutcomeAckPersistenceAuthority,
    TurnOutcomePendingPersistenceAuthority, TurnOutcomePersistenceAuthority,
    build_host_comms_runtime, spawn_mob_host_actor,
};
use meerkat_mob::runtime::host_materialize::{
    MaterializeLlmPreflightOutcome, MaterializePreflightProbe,
};
use tokio::sync::watch;

/// Serialize tests that bind real loopback listeners or install inproc comms
/// registrations — the `tests/*.rs` analogue of the in-crate
/// `REAL_COMMS_TEST_LOCK` discipline (meerkat-mob/src/runtime/tests.rs).
pub static REAL_COMMS_TEST_LOCK: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

/// Reserve-then-drop a loopback port (race-prone but serialized under
/// `REAL_COMMS_TEST_LOCK`; moved from `smoke_mob_flow_runtime.rs`).
pub fn unused_loopback_port() -> u16 {
    std::net::TcpListener::bind(std::net::SocketAddr::from(([127, 0, 0, 1], 0)))
        .expect("reserve loopback port")
        .local_addr()
        .expect("reserved listener local addr")
        .port()
}

// ===========================================================================
// PeerCommsEndpoint + raw bridge probe — split into probe.rs (dependency-light
// half consumed by meerkat-cli/tests/system_mob_host_daemon.rs via #[path])
// ===========================================================================

mod probe;
pub use probe::*;
// ===========================================================================
// ProductionExternalTcpTarget — moved verbatim from smoke_mob_flow_runtime.rs
// ===========================================================================

pub struct ProductionExternalTcpTarget {
    pub binding: meerkat_mob::RuntimeBinding,
    pub adapter: Arc<MeerkatMachine>,
    pub session_id: meerkat_core::SessionId,
    pub runtime: Arc<meerkat_comms::CommsRuntime>,
}

impl ProductionExternalTcpTarget {
    pub async fn active_input_count(&self) -> usize {
        self.adapter
            .list_active_inputs(&self.session_id)
            .await
            .expect("target runtime should list active inputs")
            .len()
    }

    pub async fn shutdown(&self) {
        // Teardown: tolerate already-stopped/unknown drains (typed result).
        let _aborted: Result<(), _> = self.adapter.abort_comms_drain(&self.session_id).await;
    }
}

pub async fn spawn_production_external_tcp_target(peer_name: &str) -> ProductionExternalTcpTarget {
    let mut runtime =
        meerkat_comms::CommsRuntime::inproc_only(peer_name).expect("create target comms runtime");
    runtime
        .set_listen_tcp_for_unstarted_runtime(std::net::SocketAddr::from(([127, 0, 0, 1], 0)))
        .expect("configure target TCP listener");
    runtime
        .start_listeners()
        .await
        .expect("start target TCP listener");
    let address = runtime.advertised_address();
    let peer_id = runtime.public_key().to_peer_id().to_string();
    let pubkey = *runtime.public_key().as_bytes();
    let bootstrap_token = runtime.bridge_bootstrap_token().to_string();
    let runtime = Arc::new(runtime);
    let session_id = meerkat_core::SessionId::new();

    // The target runs the PRODUCTION comms_drain on `adapter`, which mints the
    // supervisor trust-publish obligations during BindMember. The runtime's
    // peer-comms classification handle AND its generated trust owner must come
    // from that SAME adapter session — otherwise the inbound BindMember is
    // dropped (ClassificationRejected → PeerOffline) for lack of a classifier,
    // and the supervisor trust publish is rejected ("minted by a different
    // generated owner"). A real session-backed external member gets this wiring
    // from its SessionRuntimeBindings; this simulation installs it explicitly.
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    adapter
        .register_session(session_id.clone())
        .await
        .expect("register session");
    adapter
        .test_install_session_peer_comms_handle_on_runtime(&session_id, runtime.as_ref())
        .await
        .expect("install adapter session peer-comms handle on target runtime");

    // Peer request/response authority records inbound bridge requests on the
    // runtime so the drain can reply (request_received → response).
    let dsl = Arc::new(meerkat_runtime::HandleDslAuthority::ephemeral());
    dsl.apply_signal(
        machine_dsl::MeerkatMachineSignal::Initialize,
        "external_tcp_smoke_target::initialize",
    )
    .expect("initialize target peer interaction authority");
    dsl.apply_input(
        machine_dsl::MeerkatMachineInput::RegisterSession {
            session_id: machine_dsl::SessionId::from(session_id.to_string()),
        },
        "external_tcp_smoke_target::register",
    )
    .expect("register target peer interaction authority");
    runtime.install_peer_request_response_authority(
        meerkat_comms::PeerRequestResponseAuthority::new(
            Arc::new(meerkat_runtime::handles::RuntimePeerInteractionHandle::new(
                Arc::clone(&dsl),
            )),
            Arc::new(meerkat_runtime::handles::RuntimeInteractionStreamHandle::new(dsl)),
        ),
    );

    let spawned = adapter
        .maybe_spawn_comms_drain(
            &session_id,
            true,
            Some(runtime.clone() as Arc<dyn meerkat_core::agent::CommsRuntime>),
        )
        .await
        .expect("update_peer_ingress_context must succeed");
    assert!(spawned, "target must run production comms_drain");

    ProductionExternalTcpTarget {
        binding: meerkat_mob::RuntimeBinding::External {
            peer_id,
            address,
            bootstrap_token: Some(bootstrap_token.into()),
            pubkey,
        },
        adapter,
        session_id,
        runtime,
    }
}

// ===========================================================================
// HostDaemonFixture — "host B in miniature" (design §W4.2, TDD against W1/W2)
// ===========================================================================

/// Fixed presence-probe result injected into the fixture actor (the Tier-1
/// probe is facade-owned in production and injected as a trait object —
/// DEC-P2-5 / W2.5; tests substitute a static set).
pub struct StaticProviderProbe(pub Vec<meerkat_core::Provider>);

#[async_trait::async_trait]
impl ProviderPresenceProbe for StaticProviderProbe {
    async fn resolvable_providers(
        &self,
    ) -> Result<Vec<meerkat_core::Provider>, ProviderPresenceProbeError> {
        Ok(self.0.clone())
    }
}

/// Descriptor sink for fixtures: the daemon's 0600 file writer is CLI-owned;
/// tests read the descriptor from the pairing watch slot instead.
pub struct NoopDescriptorSink;

impl HostDescriptorSink for NoopDescriptorSink {
    fn publish(&self, _descriptor_json: &str) -> Result<(), String> {
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HostPersistenceAckLossFault {
    #[default]
    None,
    Rebind,
    Revoke,
    Materialize,
    Release,
    TurnOutcomePending,
    TurnOutcomeRecord,
    TurnOutcomeAck,
}

pub struct HostFixtureOptions {
    /// Host participant name (also the fixture's comms display name).
    pub name: String,
    /// Advertised ws/wss live base URL; `None` = live-incapable host (DL5).
    ///
    /// Phase 6b: this is the bind-ceremony FACT only. A real serving live
    /// plane (listener + `ServiceMemberLiveHost` install) is composed by the
    /// live-plane fixture half (`support/live_plane.rs`, declared only by
    /// the live test roots so the `#[path]` consumers of this module gain no
    /// meerkat-live dependency) — bind the listener first, put its advertise
    /// URL here, then install the plane on the spawned fixture.
    pub live_endpoint: Option<String>,
    /// Injected Tier-1 presence-probe result (W2.5).
    pub resolvable_providers: Vec<meerkat_core::Provider>,
    pub durable_sessions: bool,
    pub memory_store: bool,
    pub mcp: bool,
    /// R8 store. `None` ⇒ a fresh temp-file `SqliteRuntimeStore`. Pass the
    /// same store again to "restart" the daemon over its durable rows.
    pub runtime_store: Option<Arc<dyn meerkat_runtime::RuntimeStore>>,
    /// Replace the typed R8 persistence with an always-failing impl (boot
    /// recovery sees an empty store; every write fails typed). Pins the W2.3
    /// step (b) fail-closed rule: in-memory authority never advances past
    /// durable truth.
    pub failing_persistence: bool,
    /// Commit one host-binding terminal durably, lose the write completion,
    /// and make the actor's bounded reconciliation reads fail. The running
    /// actor must fail-stop; a fresh actor over the same store recovers the
    /// committed terminal exactly.
    pub persistence_ack_loss: HostPersistenceAckLossFault,
    /// Stable host identity across fixture "restarts" (§20.3 identity_dir
    /// analogue). `None` ⇒ fresh keypair.
    pub identity_secret: Option<[u8; 32]>,
    /// Phase 3: member-build substrate composition (design-H §7 /
    /// design-F §3.1). `None` (default) keeps bind-only fixtures unchanged;
    /// `MaterializeMember` then typed-rejects `Unavailable` (DEC-P3H-2).
    pub member_substrate: MemberSubstrateOption,
    /// Phase 3: reuse an existing member realm across fixture "restarts"
    /// (T-F3 / R T-12 revival rows). `None` ⇒ fresh temp realm when a
    /// substrate is requested.
    pub member_realm: Option<Arc<MemberRealm>>,
    /// Phase 3: carry the previous fixture's R8 sqlite dir across a restart
    /// so `runtime_store: Some(..)` keeps its backing file alive.
    pub carry_state_dir: Option<tempfile::TempDir>,
    /// Phase 3: wrap the member session service so the FIRST `create_session`
    /// fails typed (H T12 build-failure rollback row).
    pub failing_first_create_session: bool,
    /// Fail one selected one-based `create_session` invocation. This extends
    /// the first-call shorthand with a deterministic superseding-build fault.
    pub failing_create_session_call: Option<u64>,
    /// Corrupt the first successful `create_session` result so it reports a
    /// different SessionId from the live session the service actually built.
    /// Exercises the materializer's two-identity rollback fence.
    pub corrupt_first_create_session_result_identity: bool,
    /// Fail this many initial mob-authorized archive calls before delegating.
    /// Used with a corrupted create result to exercise retryable and sticky
    /// unrecorded-session cleanup failures.
    pub failing_archive_with_mob_lifecycle_authority_calls: u64,
    /// Fail the first generation-cutover projection drain after the old live
    /// runtime has quiesced. Pins the pre-commit recovery arm: the old durable
    /// member row remains authoritative and replay-revivable.
    pub failing_first_projection_drain: bool,
    /// Stop the first materialized executor immediately after its machine
    /// ensure returns, before the materializer publishes attach success.
    /// Deterministically exercises executor-sidecar startup cleanup.
    pub stop_first_executor_after_ensure: bool,
    /// Phase 3: LLM client for MEMBER sessions built on this host (`None` ⇒
    /// `TestClient::default()`). Inject [`OneShotToolCallClient`] to drive a
    /// member turn through one deterministic tool call (upcall/budget rows).
    pub member_llm_client: Option<Arc<dyn meerkat_client::LlmClient>>,
    /// Phase 3: R8 persistence that serves reads + `put_if_absent` (bind
    /// works) but FAILS every `compare_and_put` — the materialized-row CAS
    /// path (H T13: persist failure drops the prepared authority and
    /// disposes the built session).
    pub failing_member_region_persistence: bool,
    /// Phase 3: fixed acceptor bind address, carried across `restart()` so
    /// member/host advertised addresses stay reachable (a real daemon binds
    /// a configured address; `None` ⇒ ephemeral port).
    pub listen_tcp: Option<std::net::SocketAddr>,
    /// Override the ephemeral member-event ring capacity. Production exposes
    /// the same composition seam; tests use a small ring when the behavior
    /// under test is cursor overrun rather than the default 1024-row bound.
    pub event_ring_capacity: Option<std::num::NonZeroUsize>,
}

impl HostFixtureOptions {
    pub fn named(name: &str) -> Self {
        Self {
            name: name.to_string(),
            live_endpoint: None,
            resolvable_providers: vec![meerkat_core::Provider::Anthropic],
            durable_sessions: true,
            memory_store: true,
            mcp: true,
            runtime_store: None,
            failing_persistence: false,
            persistence_ack_loss: HostPersistenceAckLossFault::None,
            identity_secret: None,
            member_substrate: MemberSubstrateOption::None,
            member_realm: None,
            carry_state_dir: None,
            failing_first_create_session: false,
            failing_create_session_call: None,
            corrupt_first_create_session_result_identity: false,
            failing_archive_with_mob_lifecycle_authority_calls: 0,
            failing_first_projection_drain: false,
            stop_first_executor_after_ensure: false,
            member_llm_client: None,
            failing_member_region_persistence: false,
            listen_tcp: None,
            event_ring_capacity: None,
        }
    }

    /// Compose the member-build substrate (session service, runtime adapter,
    /// preflight probe, member-comms identity root) into the fixture's
    /// MobHostActor — everything the phase-3 `MobHostActorConfig.member_host`
    /// extension carries (DEC-P3H-2). Uses a persistent temp-realm service
    /// with an injected TestClient so member turns are deterministic.
    pub fn with_member_build(mut self) -> Self {
        self.member_substrate = MemberSubstrateOption::PersistentTemp;
        self
    }

    pub fn stopping_first_executor_after_ensure(mut self) -> Self {
        self.stop_first_executor_after_ensure = true;
        self
    }

    /// `durable_sessions=false` composition: ephemeral member session
    /// service; pins the `ReleaseMember`
    /// `RuntimeReleasedOnly{NoDurableSessions}` degradation rows (H T16).
    pub fn ephemeral(mut self) -> Self {
        self.member_substrate = MemberSubstrateOption::Ephemeral;
        self.durable_sessions = false;
        self
    }

    pub fn with_event_ring_capacity(mut self, capacity: std::num::NonZeroUsize) -> Self {
        self.event_ring_capacity = Some(capacity);
        self
    }
}

/// Member-build substrate flavor for the host fixture (design-H §7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MemberSubstrateOption {
    /// Bind-only fixture (phase-2 behavior, unchanged).
    #[default]
    None,
    /// `EphemeralSessionService` behind the meerkat-mob blanket impl;
    /// `realm_backend_persistent = false`.
    Ephemeral,
    /// `PersistentSessionService` over a temp realm (JSONL sessions + sqlite
    /// runtime store), reusable across fixture "restarts" via the returned
    /// [`MemberRealm`] handle.
    PersistentTemp,
}

/// Host B's member realm: durable session roots + runtime store that survive
/// a fixture restart (the revival rows rebuild the service over the SAME
/// realm — §15.7 / A20).
pub struct MemberRealm {
    pub temp: tempfile::TempDir,
    pub paths: ControllingMobPaths,
    pub runtime_store: Arc<dyn meerkat_runtime::RuntimeStore>,
}

pub fn member_realm() -> Arc<MemberRealm> {
    let temp = tempfile::tempdir().expect("member realm temp dir");
    let paths = ControllingMobPaths::new(temp.path());
    let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> = Arc::new(
        meerkat_runtime::store::SqliteRuntimeStore::new(temp.path().join("member-runtime.sqlite3"))
            .expect("open member realm sqlite runtime store"),
    );
    Arc::new(MemberRealm {
        temp,
        paths,
        runtime_store,
    })
}

/// Scripted Tier-1 + Tier-2 preflight probe for the member substrate
/// (DEC-P3H-8): providers are the Tier-1 presence answer; exact model and
/// binding outcomes are independently scriptable for Tier 2.
pub struct ScriptedPreflightProbe {
    providers: std::sync::Mutex<Vec<meerkat_core::Provider>>,
    model_resolvable: AtomicBool,
    binding_resolvable: AtomicBool,
}

impl ScriptedPreflightProbe {
    pub fn new(providers: Vec<meerkat_core::Provider>) -> Self {
        Self {
            providers: std::sync::Mutex::new(providers),
            model_resolvable: AtomicBool::new(true),
            binding_resolvable: AtomicBool::new(true),
        }
    }

    pub fn set_model_resolvable(&self, resolvable: bool) {
        self.model_resolvable.store(resolvable, Ordering::SeqCst);
    }

    pub fn set_binding_resolvable(&self, resolvable: bool) {
        self.binding_resolvable.store(resolvable, Ordering::SeqCst);
    }

    pub fn set_providers(&self, providers: Vec<meerkat_core::Provider>) {
        *self
            .providers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = providers;
    }
}

#[async_trait::async_trait]
impl ProviderPresenceProbe for ScriptedPreflightProbe {
    async fn resolvable_providers(
        &self,
    ) -> Result<Vec<meerkat_core::Provider>, ProviderPresenceProbeError> {
        Ok(self
            .providers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone())
    }
}

#[async_trait::async_trait]
impl MaterializePreflightProbe for ScriptedPreflightProbe {
    async fn preflight_llm_identity(
        &self,
        _identity: &meerkat_core::SessionLlmIdentity,
        _custom_models: &std::collections::BTreeMap<
            String,
            meerkat_core::config::CustomModelConfig,
        >,
        _preferred_realm: Option<&meerkat_core::RealmId>,
        _auth_lease_handle: &meerkat_core::handles::GeneratedAuthLeaseHandle,
    ) -> Result<MaterializeLlmPreflightOutcome, ProviderPresenceProbeError> {
        if !self.model_resolvable.load(Ordering::SeqCst) {
            return Ok(MaterializeLlmPreflightOutcome::ModelUnresolvable);
        }
        if !self.binding_resolvable.load(Ordering::SeqCst) {
            return Ok(MaterializeLlmPreflightOutcome::BindingUnresolvable);
        }
        Ok(MaterializeLlmPreflightOutcome::Resolved)
    }
}

/// Thin `MobSessionService` decorator failing one selected session-create
/// call typed. Everything the materialize/release arc touches delegates; only
/// the logical create operation is intercepted, regardless of which exact
/// actor/boundary-aware trait entry point carries it.
pub struct FailingOnceSessionService {
    inner: Arc<dyn meerkat_mob::MobSessionService>,
    fail_create_call: Option<u64>,
    create_calls: AtomicU64,
    failed_once: AtomicBool,
    corrupt_create_result_call: Option<u64>,
    corrupted_create_result_once: AtomicBool,
    actual_session_id: std::sync::Mutex<Option<meerkat_core::SessionId>>,
    reported_session_id: std::sync::Mutex<Option<meerkat_core::SessionId>>,
    archive_failures_remaining: AtomicU64,
    archive_failures_fired: AtomicU64,
    fail_projection_drain: bool,
    projection_drain_failed_once: AtomicBool,
}

impl FailingOnceSessionService {
    pub fn new(
        inner: Arc<dyn meerkat_mob::MobSessionService>,
        fail_create_call: Option<u64>,
        corrupt_create_result_call: Option<u64>,
        archive_failure_calls: u64,
        fail_projection_drain: bool,
    ) -> Arc<Self> {
        Arc::new(Self {
            inner,
            fail_create_call,
            create_calls: AtomicU64::new(0),
            failed_once: AtomicBool::new(false),
            corrupt_create_result_call,
            corrupted_create_result_once: AtomicBool::new(false),
            actual_session_id: std::sync::Mutex::new(None),
            reported_session_id: std::sync::Mutex::new(None),
            archive_failures_remaining: AtomicU64::new(archive_failure_calls),
            archive_failures_fired: AtomicU64::new(0),
            fail_projection_drain,
            projection_drain_failed_once: AtomicBool::new(false),
        })
    }

    /// True once the injected failure has fired.
    pub fn fired(&self) -> bool {
        self.failed_once.load(Ordering::SeqCst)
    }

    /// The real live identity returned by the delegated service before the
    /// injected result corruption changed the outward `RunResult`.
    pub fn actual_session_id(&self) -> Option<meerkat_core::SessionId> {
        self.actual_session_id
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub fn reported_session_id(&self) -> Option<meerkat_core::SessionId> {
        self.reported_session_id
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub fn archive_failures_fired(&self) -> u64 {
        self.archive_failures_fired.load(Ordering::SeqCst)
    }

    fn begin_create_call(&self) -> Result<u64, meerkat_core::service::SessionError> {
        let call = self.create_calls.fetch_add(1, Ordering::SeqCst) + 1;
        if self.fail_create_call == Some(call) && !self.failed_once.swap(true, Ordering::SeqCst) {
            return Err(meerkat_core::service::SessionError::FailedWithData {
                message: format!("injected test create_session call {call} failure"),
                data: serde_json::Value::Null,
            });
        }
        Ok(call)
    }

    fn finish_create_call(
        &self,
        call: u64,
        mut result: meerkat_core::RunResult,
    ) -> meerkat_core::RunResult {
        if self.corrupt_create_result_call == Some(call)
            && !self
                .corrupted_create_result_once
                .swap(true, Ordering::SeqCst)
        {
            *self
                .actual_session_id
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) =
                Some(result.session_id.clone());
            let reported = meerkat_core::SessionId::new();
            *self
                .reported_session_id
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(reported.clone());
            result.session_id = reported;
        }
        result
    }
}

#[async_trait::async_trait]
impl meerkat_core::service::SessionService for FailingOnceSessionService {
    async fn create_session(
        &self,
        req: meerkat_core::service::CreateSessionRequest,
    ) -> Result<meerkat_core::RunResult, meerkat_core::service::SessionError> {
        let call = self.begin_create_call()?;
        let result = self.inner.create_session(req).await?;
        Ok(self.finish_create_call(call, result))
    }

    async fn start_turn(
        &self,
        id: &meerkat_core::SessionId,
        req: meerkat_core::service::StartTurnRequest,
    ) -> Result<meerkat_core::RunResult, meerkat_core::service::SessionError> {
        self.inner.start_turn(id, req).await
    }

    async fn interrupt(
        &self,
        id: &meerkat_core::SessionId,
    ) -> Result<(), meerkat_core::service::SessionError> {
        self.inner.interrupt(id).await
    }

    async fn read(
        &self,
        id: &meerkat_core::SessionId,
    ) -> Result<meerkat_core::service::SessionView, meerkat_core::service::SessionError> {
        self.inner.read(id).await
    }

    async fn list(
        &self,
        query: meerkat_core::service::SessionQuery,
    ) -> Result<Vec<meerkat_core::service::SessionSummary>, meerkat_core::service::SessionError>
    {
        self.inner.list(query).await
    }

    async fn archive(
        &self,
        id: &meerkat_core::SessionId,
    ) -> Result<(), meerkat_core::service::SessionError> {
        self.inner.archive(id).await
    }

    async fn has_live_session(
        &self,
        id: &meerkat_core::SessionId,
    ) -> Result<bool, meerkat_core::service::SessionError> {
        self.inner.has_live_session(id).await
    }
}

// Ext supertraits: the T12 row exercises only the materialize create path
// (which the wrapper intercepts) and reads — required ext items delegate to
// the wrapped service, defaults cover the rest.
#[async_trait::async_trait]
impl meerkat_core::service::SessionServiceCommsExt for FailingOnceSessionService {}
#[async_trait::async_trait]
impl meerkat_core::service::SessionServiceControlExt for FailingOnceSessionService {
    async fn append_system_context(
        &self,
        id: &meerkat_core::SessionId,
        req: meerkat_core::service::AppendSystemContextRequest,
    ) -> Result<
        meerkat_core::service::AppendSystemContextResult,
        meerkat_core::service::SessionControlError,
    > {
        self.inner.append_system_context(id, req).await
    }
}
#[async_trait::async_trait]
impl meerkat_core::service::SessionServiceHistoryExt for FailingOnceSessionService {
    async fn read_history(
        &self,
        id: &meerkat_core::SessionId,
        query: meerkat_core::service::SessionHistoryQuery,
    ) -> Result<meerkat_core::service::SessionHistoryPage, meerkat_core::service::SessionError>
    {
        self.inner.read_history(id, query).await
    }
}

#[async_trait::async_trait]
impl meerkat_mob::MobSessionService for FailingOnceSessionService {
    async fn create_session_under_runtime_turn_boundary(
        &self,
        req: meerkat_core::service::CreateSessionRequest,
    ) -> Result<meerkat_core::RunResult, meerkat_core::service::SessionError> {
        let call = self.begin_create_call()?;
        let result = self
            .inner
            .create_session_under_runtime_turn_boundary(req)
            .await?;
        Ok(self.finish_create_call(call, result))
    }

    async fn create_session_with_actor_witness_under_runtime_turn_boundary(
        &self,
        req: meerkat_core::service::CreateSessionRequest,
        actor_witness_slot: &meerkat_session::LiveSessionActorWitnessSlot,
    ) -> Result<meerkat_core::RunResult, meerkat_core::service::SessionError> {
        let call = self.begin_create_call()?;
        let result = self
            .inner
            .create_session_with_actor_witness_under_runtime_turn_boundary(req, actor_witness_slot)
            .await?;
        Ok(self.finish_create_call(call, result))
    }

    async fn create_session_with_machine_archived_resume_authority(
        &self,
        req: meerkat_core::service::CreateSessionRequest,
        authorization: meerkat_runtime::ArchivedSessionActorMaterializationAuthorization,
    ) -> Result<meerkat_core::RunResult, meerkat_core::service::SessionError> {
        let call = self.begin_create_call()?;
        let result = self
            .inner
            .create_session_with_machine_archived_resume_authority(req, authorization)
            .await?;
        Ok(self.finish_create_call(call, result))
    }

    async fn create_session_with_machine_archived_resume_authority_under_runtime_turn_boundary(
        &self,
        req: meerkat_core::service::CreateSessionRequest,
        authorization: meerkat_runtime::ArchivedSessionActorMaterializationAuthorization,
    ) -> Result<meerkat_core::RunResult, meerkat_core::service::SessionError> {
        let call = self.begin_create_call()?;
        let result = self
            .inner
            .create_session_with_machine_archived_resume_authority_under_runtime_turn_boundary(
                req,
                authorization,
            )
            .await?;
        Ok(self.finish_create_call(call, result))
    }

    async fn create_session_with_machine_archived_resume_authority_and_actor_witness_under_runtime_turn_boundary(
        &self,
        req: meerkat_core::service::CreateSessionRequest,
        authorization: meerkat_runtime::ArchivedSessionActorMaterializationAuthorization,
        actor_witness_slot: &meerkat_session::LiveSessionActorWitnessSlot,
    ) -> Result<meerkat_core::RunResult, meerkat_core::service::SessionError> {
        let call = self.begin_create_call()?;
        let result = self
            .inner
            .create_session_with_machine_archived_resume_authority_and_actor_witness_under_runtime_turn_boundary(
                req,
                authorization,
                actor_witness_slot,
            )
            .await?;
        Ok(self.finish_create_call(call, result))
    }

    async fn promote_revivable_retired_session(
        &self,
        session_id: &meerkat_core::SessionId,
        authority: meerkat_runtime::PreparedArchivedResumeCommitLease,
    ) -> Result<
        meerkat_runtime::PromotedArchivedResumeCommitLease,
        meerkat_core::service::SessionError,
    > {
        self.inner
            .promote_revivable_retired_session(session_id, authority)
            .await
    }

    async fn archive_with_mob_lifecycle_authority_under_runtime_turn_boundary(
        &self,
        session_id: &meerkat_core::SessionId,
    ) -> Result<(), meerkat_core::service::SessionError> {
        self.inner
            .archive_with_mob_lifecycle_authority_under_runtime_turn_boundary(session_id)
            .await
    }

    async fn discard_live_session_under_runtime_turn_boundary(
        &self,
        session_id: &meerkat_core::SessionId,
    ) -> Result<(), meerkat_core::service::SessionError> {
        self.inner
            .discard_live_session_under_runtime_turn_boundary(session_id)
            .await
    }

    fn supports_persistent_sessions(&self) -> bool {
        self.inner.supports_persistent_sessions()
    }

    fn runtime_adapter(&self) -> Option<Arc<MeerkatMachine>> {
        self.inner.runtime_adapter()
    }

    async fn live_session_actor_registered(
        &self,
        session_id: &meerkat_core::SessionId,
    ) -> Result<bool, meerkat_core::service::SessionError> {
        self.inner.live_session_actor_registered(session_id).await
    }

    async fn load_persisted_session(
        &self,
        session_id: &meerkat_core::SessionId,
    ) -> Result<Option<meerkat_core::Session>, meerkat_core::service::SessionError> {
        self.inner.load_persisted_session(session_id).await
    }

    async fn load_revivable_retired_session(
        &self,
        session_id: &meerkat_core::SessionId,
    ) -> Result<Option<meerkat_core::Session>, meerkat_core::service::SessionError> {
        self.inner.load_revivable_retired_session(session_id).await
    }

    async fn session_known_to_archive_authority(
        &self,
        session_id: &meerkat_core::SessionId,
    ) -> Result<bool, meerkat_core::service::SessionError> {
        self.inner
            .session_known_to_archive_authority(session_id)
            .await
    }

    async fn archive_with_mob_lifecycle_authority(
        &self,
        session_id: &meerkat_core::SessionId,
    ) -> Result<(), meerkat_core::service::SessionError> {
        let targets_actual_corrupted_session = self
            .actual_session_id
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .is_none_or(|actual| actual == session_id);
        if targets_actual_corrupted_session
            && self
                .archive_failures_remaining
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
                    if remaining == 0 {
                        None
                    } else {
                        Some(remaining - 1)
                    }
                })
                .is_ok()
        {
            let call = self.archive_failures_fired.fetch_add(1, Ordering::SeqCst) + 1;
            return Err(meerkat_core::service::SessionError::FailedWithData {
                message: format!(
                    "injected mob lifecycle archive call {call} failure for {session_id}"
                ),
                data: serde_json::Value::Null,
            });
        }
        self.inner
            .archive_with_mob_lifecycle_authority(session_id)
            .await
    }

    async fn discard_live_session(
        &self,
        session_id: &meerkat_core::SessionId,
    ) -> Result<(), meerkat_core::service::SessionError> {
        self.inner.discard_live_session(session_id).await
    }

    async fn discard_live_session_actor_under_runtime_turn_boundary(
        &self,
        witness: &meerkat_session::LiveSessionActorWitness,
    ) -> Result<bool, meerkat_core::service::SessionError> {
        self.inner
            .discard_live_session_actor_under_runtime_turn_boundary(witness)
            .await
    }

    async fn await_event_projection_drain(
        &self,
        session_id: &meerkat_core::SessionId,
    ) -> Result<bool, meerkat_core::service::SessionError> {
        if self.fail_projection_drain
            && !self
                .projection_drain_failed_once
                .swap(true, Ordering::SeqCst)
        {
            return Err(meerkat_core::service::SessionError::FailedWithData {
                message: "injected generation-cutover projection drain failure".to_string(),
                data: serde_json::Value::Null,
            });
        }
        self.inner.await_event_projection_drain(session_id).await
    }
}

/// Always-failing typed R8 persistence (the actor's `persistence` seam):
/// boot recovery reads an empty store; every write is a typed error. The
/// actor must drop the prepared authority and reply failure — never advance
/// in-memory state past durable truth.
pub struct FailingHostBindingPersistence;

fn injected_persist_failure() -> MobHostActorError {
    MobHostActorError::Internal {
        detail: "injected test persist failure".to_string(),
    }
}

#[async_trait::async_trait]
impl MobHostBindingPersistence for FailingHostBindingPersistence {
    async fn list_records(&self) -> Result<Vec<(String, MobHostBindingRecord)>, MobHostActorError> {
        Ok(Vec::new())
    }

    async fn load(&self, _mob_id: &str) -> Result<Option<MobHostBindingRecord>, MobHostActorError> {
        Ok(None)
    }

    async fn list_revocations(
        &self,
    ) -> Result<Vec<(String, MobHostRevocationReceipt)>, MobHostActorError> {
        Ok(Vec::new())
    }

    async fn load_revocation(
        &self,
        _mob_id: &str,
    ) -> Result<Option<MobHostRevocationReceipt>, MobHostActorError> {
        Ok(None)
    }

    async fn put_if_absent(
        &self,
        _mob_id: &str,
        _record: &MobHostBindingRecord,
        _authority: &HostBindingPersistenceAuthority,
    ) -> Result<bool, MobHostActorError> {
        Err(injected_persist_failure())
    }

    async fn compare_and_put(
        &self,
        _mob_id: &str,
        _expected: &MobHostBindingRecord,
        _next: &MobHostBindingRecord,
        _authority: &HostBindingPersistenceAuthority,
    ) -> Result<bool, MobHostActorError> {
        Err(injected_persist_failure())
    }

    async fn compare_and_put_member_rows(
        &self,
        _mob_id: &str,
        _expected: &MobHostBindingRecord,
        _next: &MobHostBindingRecord,
        _authority: &MemberRowPersistenceAuthority,
    ) -> Result<bool, MobHostActorError> {
        Err(injected_persist_failure())
    }

    async fn revoke(
        &self,
        _mob_id: &str,
        _expected: &MobHostBindingRecord,
        _receipt: &MobHostRevocationReceipt,
        _authority: &HostBindingDeletionAuthority,
    ) -> Result<bool, MobHostActorError> {
        Err(injected_persist_failure())
    }
}

/// R8 persistence that serves reads and `put_if_absent` honestly (binds
/// work) but fails every CAS write typed — the materialized-row CAS write
/// path is `compare_and_put_member_rows` (H T13).
pub struct FailingCasHostBindingPersistence {
    inner: RuntimeStoreHostBindingPersistence,
}

#[async_trait::async_trait]
impl MobHostBindingPersistence for FailingCasHostBindingPersistence {
    async fn list_records(&self) -> Result<Vec<(String, MobHostBindingRecord)>, MobHostActorError> {
        self.inner.list_records().await
    }

    async fn load(&self, mob_id: &str) -> Result<Option<MobHostBindingRecord>, MobHostActorError> {
        self.inner.load(mob_id).await
    }

    async fn list_revocations(
        &self,
    ) -> Result<Vec<(String, MobHostRevocationReceipt)>, MobHostActorError> {
        self.inner.list_revocations().await
    }

    async fn load_revocation(
        &self,
        mob_id: &str,
    ) -> Result<Option<MobHostRevocationReceipt>, MobHostActorError> {
        self.inner.load_revocation(mob_id).await
    }

    async fn put_if_absent(
        &self,
        mob_id: &str,
        record: &MobHostBindingRecord,
        authority: &HostBindingPersistenceAuthority,
    ) -> Result<bool, MobHostActorError> {
        self.inner.put_if_absent(mob_id, record, authority).await
    }

    async fn compare_and_put(
        &self,
        _mob_id: &str,
        _expected: &MobHostBindingRecord,
        _next: &MobHostBindingRecord,
        _authority: &HostBindingPersistenceAuthority,
    ) -> Result<bool, MobHostActorError> {
        Err(injected_persist_failure())
    }

    async fn compare_and_put_member_rows(
        &self,
        _mob_id: &str,
        _expected: &MobHostBindingRecord,
        _next: &MobHostBindingRecord,
        _authority: &MemberRowPersistenceAuthority,
    ) -> Result<bool, MobHostActorError> {
        Err(injected_persist_failure())
    }

    async fn revoke(
        &self,
        mob_id: &str,
        expected: &MobHostBindingRecord,
        receipt: &MobHostRevocationReceipt,
        authority: &HostBindingDeletionAuthority,
    ) -> Result<bool, MobHostActorError> {
        self.inner
            .revoke(mob_id, expected, receipt, authority)
            .await
    }
}

/// Commits one selected binding terminal, then simulates both write-ACK loss
/// and a short-lived read outage. The actor therefore cannot prove which
/// durable terminal won in-process and must enter its sticky fail-stop. A
/// restarted fixture uses the ordinary persistence facade and recovers the
/// exact committed row or receipt from the same runtime store.
pub struct PostCommitUncertainHostBindingPersistence {
    inner: RuntimeStoreHostBindingPersistence,
    fault: HostPersistenceAckLossFault,
    fail_next_loads: AtomicU64,
}

impl PostCommitUncertainHostBindingPersistence {
    fn new(inner: RuntimeStoreHostBindingPersistence, fault: HostPersistenceAckLossFault) -> Self {
        Self {
            inner,
            fault,
            fail_next_loads: AtomicU64::new(0),
        }
    }

    fn arm_uncertain_rereads(&self) {
        // The serving core performs three bounded exact-row observations.
        // Fail all three so this fixture exercises the fail-stop boundary,
        // not the immediate convergence branch covered by comms-free tests.
        self.fail_next_loads.store(3, Ordering::SeqCst);
    }

    fn should_fail_load(&self) -> bool {
        self.fail_next_loads
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
                remaining.checked_sub(1)
            })
            .is_ok()
    }
}

#[async_trait::async_trait]
impl MobHostBindingPersistence for PostCommitUncertainHostBindingPersistence {
    async fn list_records(&self) -> Result<Vec<(String, MobHostBindingRecord)>, MobHostActorError> {
        self.inner.list_records().await
    }

    async fn load(&self, mob_id: &str) -> Result<Option<MobHostBindingRecord>, MobHostActorError> {
        if self.should_fail_load() {
            return Err(MobHostActorError::Internal {
                detail: "injected host binding reread outage after durable commit".to_string(),
            });
        }
        self.inner.load(mob_id).await
    }

    async fn list_revocations(
        &self,
    ) -> Result<Vec<(String, MobHostRevocationReceipt)>, MobHostActorError> {
        self.inner.list_revocations().await
    }

    async fn load_revocation(
        &self,
        mob_id: &str,
    ) -> Result<Option<MobHostRevocationReceipt>, MobHostActorError> {
        self.inner.load_revocation(mob_id).await
    }

    async fn put_if_absent(
        &self,
        mob_id: &str,
        record: &MobHostBindingRecord,
        authority: &HostBindingPersistenceAuthority,
    ) -> Result<bool, MobHostActorError> {
        self.inner.put_if_absent(mob_id, record, authority).await
    }

    async fn compare_and_put(
        &self,
        mob_id: &str,
        expected: &MobHostBindingRecord,
        next: &MobHostBindingRecord,
        authority: &HostBindingPersistenceAuthority,
    ) -> Result<bool, MobHostActorError> {
        let swapped = self
            .inner
            .compare_and_put(mob_id, expected, next, authority)
            .await?;
        if swapped && self.fault == HostPersistenceAckLossFault::Rebind {
            self.arm_uncertain_rereads();
            return Err(MobHostActorError::Internal {
                detail: "injected rebind write completion loss after durable commit".to_string(),
            });
        }
        Ok(swapped)
    }

    async fn compare_and_put_member_rows(
        &self,
        mob_id: &str,
        expected: &MobHostBindingRecord,
        next: &MobHostBindingRecord,
        authority: &MemberRowPersistenceAuthority,
    ) -> Result<bool, MobHostActorError> {
        let swapped = self
            .inner
            .compare_and_put_member_rows(mob_id, expected, next, authority)
            .await?;
        let selected = matches!(
            (self.fault, authority),
            (
                HostPersistenceAckLossFault::Materialize,
                MemberRowPersistenceAuthority::Materialized { .. }
            ) | (
                HostPersistenceAckLossFault::Release,
                MemberRowPersistenceAuthority::Released { .. }
            )
        );
        if swapped && selected {
            self.arm_uncertain_rereads();
            return Err(MobHostActorError::Internal {
                detail: format!(
                    "injected {:?} write completion loss after durable commit",
                    self.fault
                ),
            });
        }
        Ok(swapped)
    }

    async fn compare_and_put_turn_outcome_pending(
        &self,
        mob_id: &str,
        expected: &MobHostBindingRecord,
        next: &MobHostBindingRecord,
        authority: &TurnOutcomePendingPersistenceAuthority,
    ) -> Result<bool, MobHostActorError> {
        let swapped = self
            .inner
            .compare_and_put_turn_outcome_pending(mob_id, expected, next, authority)
            .await?;
        if swapped && self.fault == HostPersistenceAckLossFault::TurnOutcomePending {
            self.arm_uncertain_rereads();
            return Err(MobHostActorError::Internal {
                detail: "injected turn-outcome Pending write completion loss after durable commit"
                    .to_string(),
            });
        }
        Ok(swapped)
    }

    async fn compare_and_put_turn_outcomes(
        &self,
        mob_id: &str,
        expected: &MobHostBindingRecord,
        next: &MobHostBindingRecord,
        authority: &TurnOutcomePersistenceAuthority,
    ) -> Result<bool, MobHostActorError> {
        let swapped = self
            .inner
            .compare_and_put_turn_outcomes(mob_id, expected, next, authority)
            .await?;
        if swapped && self.fault == HostPersistenceAckLossFault::TurnOutcomeRecord {
            self.arm_uncertain_rereads();
            return Err(MobHostActorError::Internal {
                detail: "injected turn-outcome record completion loss after durable commit"
                    .to_string(),
            });
        }
        Ok(swapped)
    }

    async fn compare_and_put_turn_outcome_ack(
        &self,
        mob_id: &str,
        expected: &MobHostBindingRecord,
        next: &MobHostBindingRecord,
        authority: &TurnOutcomeAckPersistenceAuthority,
    ) -> Result<bool, MobHostActorError> {
        let swapped = self
            .inner
            .compare_and_put_turn_outcome_ack(mob_id, expected, next, authority)
            .await?;
        if swapped && self.fault == HostPersistenceAckLossFault::TurnOutcomeAck {
            self.arm_uncertain_rereads();
            return Err(MobHostActorError::Internal {
                detail:
                    "injected turn-outcome acknowledgement completion loss after durable commit"
                        .to_string(),
            });
        }
        Ok(swapped)
    }

    async fn revoke(
        &self,
        mob_id: &str,
        expected: &MobHostBindingRecord,
        receipt: &MobHostRevocationReceipt,
        authority: &HostBindingDeletionAuthority,
    ) -> Result<bool, MobHostActorError> {
        let revoked = self
            .inner
            .revoke(mob_id, expected, receipt, authority)
            .await?;
        if revoked && self.fault == HostPersistenceAckLossFault::Revoke {
            self.arm_uncertain_rereads();
            return Err(MobHostActorError::Internal {
                detail: "injected revoke write completion loss after durable commit".to_string(),
            });
        }
        Ok(revoked)
    }
}

/// Everything the W2 daemon composes, minus the CLI: real loopback acceptor
/// (port 0), host identity comms runtime with NO listener of its own (ingress
/// comes from the acceptor), the mob host actor with recover-or-create over
/// the R8 store, injected capabilities/probe, and the live descriptor slot.
pub struct HostDaemonFixture {
    pub actor: MobHostActorHandle,
    pub acceptor: HostAcceptorHandle,
    pub registry: Arc<HostAcceptorIdentityRegistry>,
    /// Host identity comms runtime (from `build_host_comms_runtime`).
    pub runtime: Arc<meerkat_comms::CommsRuntime>,
    pub dsl: Arc<meerkat_runtime::HandleDslAuthority>,
    /// The daemon's descriptor slot (the pairing watch; the actor republishes
    /// it on every token re-mint).
    descriptor_rx: watch::Receiver<String>,
    pub store: Arc<dyn meerkat_runtime::RuntimeStore>,
    /// Keep the R8 sqlite file alive for the fixture's lifetime.
    pub state_dir: Option<tempfile::TempDir>,
    /// Host identity secret — reuse to "restart" the daemon with the same
    /// identity (`HostFixtureOptions::identity_secret`).
    pub identity_secret: [u8; 32],
    // --- phase-3 member-build substrate (design-H §7 / design-F §3.1) ---
    /// Fixture display name (kept for [`Self::restart`]).
    pub name: String,
    pub member_substrate: MemberSubstrateOption,
    /// Host B's realm-local member session service; session-count /
    /// session-metadata assertions land here.
    pub member_service: Option<Arc<dyn meerkat_mob::MobSessionService>>,
    /// The CONCRETE persistent service behind `member_service`
    /// (PersistentTemp substrate only) — the live-plane fixture half builds
    /// `ServiceLiveProjection` over it (phase 6b, C6).
    pub member_concrete_service:
        Option<Arc<meerkat_session::PersistentSessionService<meerkat::FactoryAgentBuilder>>>,
    pub member_runtime_adapter: Option<Arc<MeerkatMachine>>,
    /// Host-lifetime observation tasks owned by the fixture composition.
    member_observation_recovery:
        Option<meerkat_mob::runtime::host_observation::HostMemberObservationTaskOwner>,
    /// Durable member realm — survives [`Self::restart`].
    pub member_realm: Option<Arc<MemberRealm>>,
    /// Scriptable preflight probe injected into the substrate.
    pub preflight: Option<Arc<ScriptedPreflightProbe>>,
    /// Present when a create-session or projection-drain fault was requested.
    pub failing_create: Option<Arc<FailingOnceSessionService>>,
}

impl HostDaemonFixture {
    /// The CURRENT host binding descriptor, exactly as the daemon shell would
    /// write it to `--descriptor-out` (0600). Re-read after every bind: the
    /// one-time token is consumed on `HostBindAccepted` and re-minted
    /// (DEC-P2-4), rewriting this slot.
    pub fn current_descriptor(&self) -> WireHostBindingDescriptor {
        serde_json::from_str(self.descriptor_rx.borrow().as_str())
            .expect("descriptor watch slot holds a valid host binding descriptor")
    }

    /// The acceptor's advertised address (`tcp://127.0.0.1:<port>`).
    pub fn advertised_address(&self) -> String {
        self.current_descriptor().address
    }

    /// Trust-descriptor form of the host identity for raw sends.
    pub fn host_peer_descriptor(&self) -> TrustedPeerDescriptor {
        let descriptor = self.current_descriptor();
        let resolved = descriptor
            .identity
            .resolve()
            .expect("host descriptor identity resolves");
        TrustedPeerDescriptor::unsigned_with_pubkey(
            self.runtime.participant_name(),
            resolved.peer_id.to_string(),
            resolved.pubkey,
            descriptor.address,
        )
        .expect("host trusted-peer descriptor")
    }

    pub async fn shutdown(mut self) {
        self.acceptor.shutdown().await;
        if let Some(owner) = self.member_observation_recovery.take() {
            owner.shutdown().await;
        }
        self.actor.shutdown().await;
    }

    /// Host B's realm-local member session service (requires a member-build
    /// substrate composition).
    pub fn member_session_service(&self) -> Arc<dyn meerkat_mob::MobSessionService> {
        Arc::clone(
            self.member_service
                .as_ref()
                .expect("fixture composed without a member-build substrate"),
        )
    }

    /// Decoded R8 rows: `(mob_id, MobHostBindingRecord)` including the
    /// phase-3 materialized/release regions (design-H §6). This is the
    /// durable PUBLIC carrier of the host authority's committed facts.
    pub async fn host_binding_records(&self) -> Vec<(String, MobHostBindingRecord)> {
        self.store
            .list_mob_host_bindings()
            .await
            .expect("list host binding rows")
            .into_iter()
            .map(|(mob_id, bytes)| {
                let record: MobHostBindingRecord = serde_json::from_slice(&bytes)
                    .expect("host binding row decodes as MobHostBindingRecord");
                (mob_id, record)
            })
            .collect()
    }

    /// The bound record for one mob (panics if unbound — fixture use only).
    pub async fn host_binding_record(&self, mob_id: &str) -> MobHostBindingRecord {
        self.host_binding_records()
            .await
            .into_iter()
            .find_map(|(id, record)| (id == mob_id).then_some(record))
            .unwrap_or_else(|| panic!("no host binding record for mob {mob_id}"))
    }

    /// Live-session census for one member session id on Host B's service.
    pub async fn member_session_exists(&self, session_id: &str) -> bool {
        let id = meerkat_core::SessionId::parse(session_id)
            .unwrap_or_else(|_| panic!("ack session id parses: {session_id}"));
        self.member_session_service()
            .load_persisted_session(&id)
            .await
            .map(|loaded| loaded.is_some())
            .unwrap_or(false)
    }

    /// Host B's live comms runtime for one materialized member session
    /// (phase 4: the delivery-differential send/receive handle).
    pub async fn member_comms_runtime(
        &self,
        session_id: &str,
    ) -> Arc<dyn meerkat_core::agent::CommsRuntime> {
        let id = meerkat_core::SessionId::parse(session_id)
            .unwrap_or_else(|_| panic!("member session id parses: {session_id}"));
        self.member_session_service()
            .comms_runtime(&id)
            .await
            .expect("materialized member session exposes a live comms runtime")
    }

    /// Whether the materialized member session's MeerkatMachine-owned direct
    /// peer set holds a row for `peer_id` — the EXACT machine fact
    /// `InstallPeerTrust`/`RemovePeerTrust` realize through the member's
    /// machine-gated trust seam (DEC-P4H-3). Absent session / absent row
    /// both read as `false` so negative rows can deadline-poll it.
    pub async fn member_trusts_peer(&self, session_id: &str, peer_id: &str) -> bool {
        let Ok(id) = meerkat_core::SessionId::parse(session_id) else {
            return false;
        };
        let adapter = self
            .member_runtime_adapter
            .as_ref()
            .expect("fixture composed without a member-build substrate");
        adapter
            .direct_peer_endpoints(&id)
            .await
            .map(|endpoints| endpoints.iter().any(|row| row.peer_id.0 == peer_id))
            .unwrap_or(false)
    }

    /// S3b (phase 6b): total live bridge commands served by this host's
    /// member-live drain arms — the zero-bridge-traffic pin for the
    /// controlling-side gates (T-C6/T-C7/T-C8c) and the T-C9 open-count row.
    ///
    /// MERGE-SENSITIVE: the counter is recorded by the live-pipeline lane's
    /// comms_drain serving arms and exposed on the machine adapter
    /// (`MeerkatMachine::live_commands_served`, meerkat-runtime
    /// test-support). This fixture only SURFACES it (ADJ-P6B seam S3b).
    /// A fixture without a member substrate served nothing by construction.
    pub fn live_commands_served(&self) -> u64 {
        self.member_runtime_adapter
            .as_ref()
            .map(|adapter| adapter.live_commands_served())
            .unwrap_or(0)
    }

    /// Simulate a member-host PARTITION: fully quiesce the daemon (both the
    /// acceptor and the actor) while RETAINING its durable truth — R8 store,
    /// identity secret, member realm, and the acceptor's bound address — so
    /// [`PartitionedHostDaemon::restore`] brings the SAME host identity back
    /// at the SAME address. This is [`Self::restart_after`] split into two
    /// halves so a test can act (and assert) WHILE the host is down (the §9
    /// obligations-retained-across-partition row).
    pub async fn partition(self) -> PartitionedHostDaemon {
        let HostDaemonFixture {
            actor,
            acceptor,
            store,
            state_dir,
            identity_secret,
            name,
            member_substrate,
            member_realm,
            member_observation_recovery,
            ..
        } = self;
        let listen_tcp = acceptor.local_addr();
        acceptor.shutdown().await;
        if let Some(owner) = member_observation_recovery {
            owner.shutdown().await;
        }
        actor.shutdown().await;
        PartitionedHostDaemon {
            store,
            state_dir,
            identity_secret,
            name,
            member_substrate,
            member_realm,
            listen_tcp,
        }
    }

    /// "Restart" the daemon over its durable truth: drop the actor + acceptor,
    /// rebuild from the SAME R8 store + SAME identity secret + SAME member
    /// realm (fresh service over the same realm dirs) — the R8 recovery-walk
    /// driver for the revival rows (R T-12, H T18/T19, F T-F3).
    pub async fn restart(self) -> HostDaemonFixture {
        self.restart_after(|| {}).await
    }

    /// `restart` with a fault injector that runs AFTER the old daemon fully
    /// quiesced and BEFORE the new one boots — durable-state tampering (e.g.
    /// wiping a session file) must not race the old daemon's shutdown
    /// persistence, which would silently re-write the tampered state.
    pub async fn restart_after(self, between: impl FnOnce()) -> HostDaemonFixture {
        let partitioned = self.partition().await;
        between();
        partitioned.restore().await
    }
}

/// A partitioned member host (see [`HostDaemonFixture::partition`]): the
/// daemon is down, its durable truth and identity are retained, and the
/// acceptor address is reserved for the restore — descriptor / member
/// advertised addresses recorded pre-partition stay reachable afterwards
/// (a real daemon binds a configured address).
pub struct PartitionedHostDaemon {
    store: Arc<dyn meerkat_runtime::RuntimeStore>,
    state_dir: Option<tempfile::TempDir>,
    identity_secret: [u8; 32],
    name: String,
    member_substrate: MemberSubstrateOption,
    member_realm: Option<Arc<MemberRealm>>,
    listen_tcp: std::net::SocketAddr,
}

impl PartitionedHostDaemon {
    /// Test-only durable-row mutation between daemon incarnations. This is
    /// intentionally raw/CAS-based so recovery tests can model a separately
    /// written but self-consistent row without minting live actor authority.
    pub async fn rewrite_host_binding_record(
        self,
        mob_id: &str,
        mutate: impl FnOnce(&mut MobHostBindingRecord),
    ) -> Self {
        let expected = self
            .store
            .load_mob_host_binding(mob_id)
            .await
            .expect("load host binding row for restart mutation")
            .unwrap_or_else(|| panic!("missing host binding row for {mob_id}"));
        let mut record: MobHostBindingRecord = serde_json::from_slice(&expected)
            .expect("decode host binding row for restart mutation");
        mutate(&mut record);
        let next =
            serde_json::to_vec(&record).expect("encode host binding row after restart mutation");
        assert!(
            self.store
                .compare_and_put_mob_host_binding(mob_id, &expected, &next)
                .await
                .expect("CAS host binding restart mutation"),
            "host binding row changed during partitioned restart mutation"
        );
        self
    }

    /// Boot the daemon back over its retained durable truth at the SAME
    /// identity and acceptor address — the partition-heals half of the §9
    /// row (boot revival recomposes materialized members from stored specs;
    /// revived members hold NO peer trust rows until the controlling side
    /// re-drives its outstanding obligations, DEC-P4H-5).
    pub async fn restore(self) -> HostDaemonFixture {
        self.restore_result()
            .await
            .expect("restore partitioned host daemon over durable truth")
    }

    /// Fallible restore seam for corruption/recovery tests that must assert
    /// boot rejects invalid durable authority instead of panicking inside the
    /// fixture helper.
    pub async fn restore_result(self) -> Result<HostDaemonFixture, MobHostActorError> {
        self.restore_result_with_faults(false, false).await
    }

    /// Restore over the retained durable rows while making member-region CAS
    /// writes fail before commit. The next partition/ordinary restore removes
    /// the injector again, which lets recovery tests prove the same snapshot
    /// remains retryable after a definite no-write.
    pub async fn restore_with_failing_member_region_persistence(self) -> HostDaemonFixture {
        self.restore_result_with_faults(true, false)
            .await
            .expect("restore partitioned host with failing member-region persistence")
    }

    /// Restore with a one-shot attach-window failure during boot revival.
    /// The same durable row can then be retried through MaterializeReplay.
    pub async fn restore_with_stopping_first_executor_after_ensure(self) -> HostDaemonFixture {
        self.restore_result_with_faults(false, true)
            .await
            .expect("restore partitioned host with stopped first revived executor")
    }

    async fn restore_result_with_faults(
        self,
        failing_member_region_persistence: bool,
        stop_first_executor_after_ensure: bool,
    ) -> Result<HostDaemonFixture, MobHostActorError> {
        spawn_host_daemon_fixture(HostFixtureOptions {
            runtime_store: Some(self.store),
            identity_secret: Some(self.identity_secret),
            member_substrate: self.member_substrate,
            member_realm: self.member_realm,
            carry_state_dir: self.state_dir,
            durable_sessions: !matches!(self.member_substrate, MemberSubstrateOption::Ephemeral),
            failing_member_region_persistence,
            stop_first_executor_after_ensure,
            listen_tcp: Some(self.listen_tcp),
            ..HostFixtureOptions::named(&self.name)
        })
        .await
    }
}

/// Map an operator-held descriptor into the controlling-side bind request
/// (W3.1: `HostBindRequest` is built from `WireHostBindingDescriptor` by
/// callers; the peer id derives from the descriptor's Ed25519 identity,
/// never from a display name).
pub fn descriptor_to_bind_request(descriptor: &WireHostBindingDescriptor) -> HostBindRequest {
    HostBindRequest::from_descriptor(descriptor)
        .expect("descriptor identity resolves to a canonical bind request")
}

pub async fn spawn_host_daemon_fixture(
    opts: HostFixtureOptions,
) -> Result<HostDaemonFixture, MobHostActorError> {
    // Env-gated diagnostics (RUST_LOG): first fixture wins, later inits
    // no-op.
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_test_writer()
        .try_init();
    // Host identity (stable across restarts when injected).
    let keypair = match opts.identity_secret {
        Some(secret) => meerkat_comms::Keypair::from_secret(secret),
        None => meerkat_comms::Keypair::generate(),
    };
    let identity_secret = keypair.secret_bytes();

    // R8 store: temp-file sqlite by default (rows must be inspectable).
    let (store, state_dir) = match opts.runtime_store {
        Some(store) => (store, opts.carry_state_dir),
        None => {
            let dir = tempfile::tempdir().expect("host fixture state dir");
            let store: Arc<dyn meerkat_runtime::RuntimeStore> = Arc::new(
                meerkat_runtime::store::SqliteRuntimeStore::new(dir.path().join("runtime.sqlite3"))
                    .expect("open host fixture sqlite runtime store"),
            );
            (store, Some(dir))
        }
    };

    // Phase-3 member-build substrate (DEC-P3H-2): session service + runtime
    // adapter + preflight probe + member-comms identity root, injected into
    // the actor as `MobHostActorConfig.member_host`. TestClient rides the
    // service builder so member turns are deterministic.
    let preflight = Arc::new(ScriptedPreflightProbe::new(
        opts.resolvable_providers.clone(),
    ));
    let mut member_realm_handle = None;
    let mut member_service: Option<Arc<dyn meerkat_mob::MobSessionService>> = None;
    let mut member_concrete_service: Option<
        Arc<meerkat_session::PersistentSessionService<meerkat::FactoryAgentBuilder>>,
    > = None;
    let mut member_runtime_adapter = None;
    let mut member_durable_log: Option<
        Arc<dyn meerkat_runtime::member_observation::DurableEventLogRead>,
    > = None;
    let mut failing_create = None;
    let member_host = match opts.member_substrate {
        MemberSubstrateOption::None => None,
        MemberSubstrateOption::Ephemeral => {
            let realm = opts.member_realm.clone().unwrap_or_else(member_realm);
            realm.paths.materialize_project_context();
            let factory =
                meerkat::AgentFactory::new(realm.paths.runtime_root.join("factory-store"))
                    .user_config_root(realm.paths.user_config_root.clone())
                    .runtime_root(realm.paths.runtime_root.clone())
                    .project_root(realm.paths.project_root.clone())
                    .context_root(realm.paths.context_root.clone())
                    .builtins(true)
                    .comms(true);
            let mut builder =
                meerkat::FactoryAgentBuilder::new(factory, meerkat::Config::default());
            builder.default_llm_client = Some(
                opts.member_llm_client
                    .clone()
                    .unwrap_or_else(|| Arc::new(meerkat_client::TestClient::default())),
            );
            let service: Arc<dyn meerkat_mob::MobSessionService> =
                Arc::new(meerkat_session::EphemeralSessionService::new(builder, 32));
            let adapter = service
                .runtime_adapter()
                .expect("ephemeral member service exposes a runtime adapter");
            if opts.stop_first_executor_after_ensure {
                adapter.test_stop_next_executor_after_ensure();
            }
            member_service = Some(Arc::clone(&service));
            member_runtime_adapter = Some(Arc::clone(&adapter));
            let identity_root = realm.temp.path().join("comms-identity");
            member_realm_handle = Some(realm);
            Some(HostMemberSubstrate {
                session_service: service,
                runtime_adapter: adapter,
                durable_event_log: None,
                realm_backend_persistent: false,
                member_identity_root: identity_root,
                preflight_probe: Arc::clone(&preflight) as Arc<dyn MaterializePreflightProbe>,
            })
        }
        MemberSubstrateOption::PersistentTemp => {
            let realm = opts.member_realm.clone().unwrap_or_else(member_realm);
            let service = persistent_service_with_client(
                &realm.paths,
                Arc::clone(&realm.runtime_store),
                opts.member_llm_client
                    .clone()
                    .unwrap_or_else(|| Arc::new(meerkat_client::TestClient::default())),
            );
            // Phase 6b: the live-plane fixture half composes
            // `ServiceLiveProjection` over the CONCRETE service (the
            // mob_host.rs:553-590 shape), so the pre-erasure handle is
            // retained beside the erased `MobSessionService`.
            member_concrete_service = Some(Arc::clone(&service));
            // Durable-log read adapter over the CONCRETE service (the
            // mob_host.rs 11b wiring shape): the phase-6 observation
            // substrate serves PollMemberEvents / terminal_seq binding from
            // the durable event projection.
            member_durable_log = Some(Arc::new(FixtureDurableEventLogRead {
                service: Arc::clone(&service),
            }));
            let mob_service: Arc<dyn meerkat_mob::MobSessionService> = service;
            let fail_create_call = opts
                .failing_create_session_call
                .or(opts.failing_first_create_session.then_some(1));
            let corrupt_create_result_call = opts
                .corrupt_first_create_session_result_identity
                .then_some(1);
            let mob_service: Arc<dyn meerkat_mob::MobSessionService> = if fail_create_call.is_some()
                || corrupt_create_result_call.is_some()
                || opts.failing_archive_with_mob_lifecycle_authority_calls > 0
                || opts.failing_first_projection_drain
            {
                let failing = FailingOnceSessionService::new(
                    mob_service,
                    fail_create_call,
                    corrupt_create_result_call,
                    opts.failing_archive_with_mob_lifecycle_authority_calls,
                    opts.failing_first_projection_drain,
                );
                failing_create = Some(Arc::clone(&failing));
                failing as Arc<dyn meerkat_mob::MobSessionService>
            } else {
                mob_service
            };
            let adapter = mob_service
                .runtime_adapter()
                .expect("persistent member service exposes a runtime adapter");
            if opts.stop_first_executor_after_ensure {
                adapter.test_stop_next_executor_after_ensure();
            }
            member_service = Some(Arc::clone(&mob_service));
            member_runtime_adapter = Some(Arc::clone(&adapter));
            let identity_root = realm.temp.path().join("comms-identity");
            member_realm_handle = Some(realm);
            Some(HostMemberSubstrate {
                session_service: mob_service,
                runtime_adapter: adapter,
                durable_event_log: member_durable_log.clone().map(|log| {
                    log as Arc<dyn meerkat_runtime::member_observation::DurableEventLogRead>
                }),
                realm_backend_persistent: true,
                member_identity_root: identity_root,
                preflight_probe: Arc::clone(&preflight) as Arc<dyn MaterializePreflightProbe>,
            })
        }
    };

    // Host identity runtime: inproc-only with the host keypair, NO listener
    // of its own — ingress arrives via the host acceptor (W2.1 step 5). The
    // production composition helper wires classification, the peer
    // request/response authority, and exposes the inbox sender for demux
    // registration.
    let host = build_host_comms_runtime(&opts.name, keypair)?;

    // Acceptor: real loopback TCP, port 0, mandatory peer auth by
    // construction (no config field exists to disable it — D1). The actor
    // installs the registry owner (the generated authority's owner token)
    // and registers the host identity itself.
    let registry = Arc::new(HostAcceptorIdentityRegistry::new());
    let acceptor = spawn_host_acceptor(HostAcceptorConfig {
        listen_tcp: opts
            .listen_tcp
            .unwrap_or_else(|| std::net::SocketAddr::from(([127, 0, 0, 1], 0))),
        advertise_address: None,
        registry: Arc::clone(&registry),
        pairing: None,
        bounds: HostAcceptorBounds::default(),
    })
    .await?;
    let advertised_address = format!("tcp://{}", acceptor.local_addr());

    // The typed R8 persistence layer (the actor's `persistence` seam): the
    // sole production impl wraps the raw RuntimeStore accessors; the failing
    // variant pins the fail-closed persist path.
    let persistence: Arc<dyn MobHostBindingPersistence> = if opts.failing_persistence {
        Arc::new(FailingHostBindingPersistence)
    } else if opts.persistence_ack_loss != HostPersistenceAckLossFault::None {
        Arc::new(PostCommitUncertainHostBindingPersistence::new(
            RuntimeStoreHostBindingPersistence::new(Arc::clone(&store)),
            opts.persistence_ack_loss,
        ))
    } else if opts.failing_member_region_persistence {
        Arc::new(FailingCasHostBindingPersistence {
            inner: RuntimeStoreHostBindingPersistence::new(Arc::clone(&store)),
        })
    } else {
        Arc::new(RuntimeStoreHostBindingPersistence::new(Arc::clone(&store)))
    };

    let (descriptor_tx, descriptor_rx) = watch::channel(String::new());
    let actor = spawn_mob_host_actor(MobHostActorConfig {
        host_runtime: Arc::clone(&host.runtime),
        host_dsl: Arc::clone(&host.dsl),
        host_inbox_sender: host.inbox_sender.clone(),
        host_keypair: Arc::new(meerkat_comms::Keypair::from_secret(identity_secret)),
        registry: Arc::clone(&registry),
        persistence,
        // The scripted probe serves BOTH the Tier-1 bind-time presence answer
        // and the Tier-2 materialize preflight (same Arc, trait widened —
        // the daemon wiring shape, DEC-P3H-8).
        probe: Arc::clone(&preflight) as Arc<dyn ProviderPresenceProbe>,
        capability_facts: HostCapabilityFacts {
            durable_sessions: opts.durable_sessions,
            memory_store: opts.memory_store,
            mcp: opts.mcp,
        },
        advertised_address,
        live_endpoint: opts.live_endpoint,
        descriptor_watch_tx: descriptor_tx,
        descriptor_sink: Arc::new(NoopDescriptorSink),
        member_host,
    })
    .await?;

    // Member observation substrate (§7.4 phase 6, DEC-P6E-2 — the
    // mob_host.rs 11b wiring): machine-wide injected host serving
    // ReadMemberHistory / PollMemberEvents / directed-turn admission.
    // Ephemeral substrates install it WITHOUT a durable log (the bounded
    // ring substitutes; directive-bearing turns are machine-rejected for
    // such hosts anyway).
    let member_observation_recovery = if let (Some(service), Some(adapter)) =
        (&member_service, &member_runtime_adapter)
    {
        let mut observation = meerkat_mob::runtime::host_observation::HostMemberObservation::new(
            actor.runtime_incarnation(),
            Arc::clone(service),
            member_durable_log.clone(),
            actor.observation_watch(),
            actor.observation_pending_sender(),
            actor.observation_ack_sender(),
        );
        if let Some(capacity) = opts.event_ring_capacity {
            observation = observation.with_event_ring_capacity(capacity);
        }
        let observation = Arc::new(observation);
        adapter.set_member_observation_host(observation.clone());
        observation.recover_pending_turns().await
    } else {
        None
    };

    Ok(HostDaemonFixture {
        actor,
        acceptor,
        registry,
        runtime: host.runtime,
        dsl: host.dsl,
        descriptor_rx,
        store,
        state_dir,
        identity_secret,
        name: opts.name,
        member_substrate: opts.member_substrate,
        member_service,
        member_concrete_service,
        member_runtime_adapter,
        member_observation_recovery,
        member_realm: member_realm_handle,
        preflight: Some(preflight),
        failing_create,
    })
}

// ===========================================================================
// ScriptedHostPeer — fault-injectable BindHost responder (design §W4.2/§W4.3)
// ===========================================================================

/// A scripted host-side responder speaking the `BindHost` wire protocol with
/// the `LiveExternalPeerHarness` fault injectors. Used to pin the
/// CONTROLLING side's failure matrix (rollback, no-commit, retry idempotence)
/// against a misbehaving host — the real `HostDaemonFixture` cannot be asked
/// to garble its own replies.
pub struct ScriptedHostPeer {
    pub endpoint: Arc<PeerCommsEndpoint>,
    /// Honest descriptor for this scripted host (fixed token, real address).
    pub descriptor: WireHostBindingDescriptor,
    fail_next_bind: Arc<AtomicBool>,
    garble_next_bind_reply: Arc<AtomicBool>,
    override_next_bind_peer_id: Arc<std::sync::Mutex<Option<String>>>,
    bind_count: Arc<AtomicU64>,
    host_status_count: Arc<AtomicU64>,
    drop_next_host_status_replies: Arc<AtomicU64>,
    /// Phase-3 materialize/release/status scripting (design-F §3.1 /
    /// design-S §1 `MaterializeServeMode::Scripted`).
    materialize: Arc<std::sync::Mutex<ScriptedMaterializeState>>,
    release: Arc<std::sync::Mutex<ScriptedReleaseState>>,
    /// Phase-4 `InstallPeerTrust`/`RemovePeerTrust` scripting
    /// (DEC-P4H-10 accept/reject/drop failure matrix).
    trust: Arc<std::sync::Mutex<ScriptedPeerTrustState>>,
    /// Probe-as-member endpoints (ADJ-P4-10): identities bound here have
    /// accepted trust installs/removals APPLIED to the real test-held
    /// endpoint, so retry rows assert REAL delivery differentials instead
    /// of counters.
    member_endpoints:
        Arc<std::sync::Mutex<std::collections::BTreeMap<String, Arc<PeerCommsEndpoint>>>>,
    task: tokio::task::JoinHandle<()>,
}

/// Scripted peer-trust responder state (the `ScriptedMaterializeState`
/// injector template applied to the phase-4 trust pair).
#[derive(Default)]
pub struct ScriptedPeerTrustState {
    reject_next_install: Option<(BridgeRejectionCause, String)>,
    drop_next_install_replies: u32,
    install_received: Vec<BridgePeerTrustPayload>,
    reject_next_remove: Option<(BridgeRejectionCause, String)>,
    reject_remove_for_identity: Option<(String, BridgeRejectionCause, String)>,
    drop_next_remove_replies: u32,
    remove_received: Vec<BridgePeerTrustPayload>,
}

/// Scripted materialize-responder state (fault injectors in the
/// `fail_next_bind` template).
#[derive(Default)]
pub struct ScriptedMaterializeState {
    reject_next: Option<(BridgeRejectionCause, String)>,
    wrong_digest_next: bool,
    engine_version_next: Option<String>,
    drop_next_reply: u32,
    count: u64,
    received: Vec<BridgeMaterializePayload>,
    /// Dedup memory keyed on the idempotency tuple — a replayed tuple returns
    /// the recorded ack byte-identical (the host authority Replay arm shape).
    recorded: std::collections::BTreeMap<(String, u64, u64), BridgeMaterializedResponse>,
    /// Scripted per-identity member identity material, so a test-held
    /// endpoint can BE the member (upcall-lane rows).
    member_identities: std::collections::BTreeMap<String, ScriptedMemberIdentity>,
}

/// Ack identity material for one scripted member.
#[derive(Clone)]
pub struct ScriptedMemberIdentity {
    pub member_pubkey: String,
    pub member_peer_id: String,
    pub advertised_address: String,
}

/// Ack identity material derived from a live probe endpoint — the endpoint
/// then IS the member for reply routing and upcall sends.
pub fn member_identity_of(endpoint: &PeerCommsEndpoint) -> ScriptedMemberIdentity {
    ScriptedMemberIdentity {
        member_pubkey: format!(
            "ed25519:{}",
            base64_encode(endpoint.runtime.public_key().as_bytes())
        ),
        member_peer_id: endpoint.runtime.public_key().to_peer_id().to_string(),
        advertised_address: endpoint.runtime.advertised_address(),
    }
}

#[derive(Default)]
pub struct ScriptedReleaseState {
    fail_next: Option<(BridgeRejectionCause, String)>,
    drop_next_reply: u32,
    count: u64,
    received: Vec<BridgeReleasePayload>,
    recorded: std::collections::BTreeMap<(String, u64, u64), WireMemberSessionDisposal>,
}

impl ScriptedHostPeer {
    pub fn fail_next_bind(&self) {
        self.fail_next_bind.store(true, Ordering::SeqCst);
    }

    pub fn garble_next_bind_reply(&self) {
        self.garble_next_bind_reply.store(true, Ordering::SeqCst);
    }

    pub fn override_next_bind_peer_id(&self, peer_id: &str) {
        *self
            .override_next_bind_peer_id
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(peer_id.to_string());
    }

    pub fn bind_count(&self) -> u64 {
        self.bind_count.load(Ordering::SeqCst)
    }

    pub fn host_status_count(&self) -> u64 {
        self.host_status_count.load(Ordering::SeqCst)
    }

    pub fn drop_next_host_status_replies(&self, count: u64) {
        self.drop_next_host_status_replies
            .store(count, Ordering::SeqCst);
    }

    fn materialize_state(&self) -> std::sync::MutexGuard<'_, ScriptedMaterializeState> {
        self.materialize
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn release_state(&self) -> std::sync::MutexGuard<'_, ScriptedReleaseState> {
        self.release
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Next `MaterializeMember` replies `Rejected { cause, reason }`.
    pub fn reject_next_materialize(&self, cause: BridgeRejectionCause, reason: &str) {
        self.materialize_state().reject_next = Some((cause, reason.to_string()));
    }

    /// Next ack echoes a doctored `spec_digest` (commit-refusal row I4/T-L5).
    pub fn echo_wrong_digest_next_materialize(&self) {
        self.materialize_state().wrong_digest_next = true;
    }

    /// Next ack echoes `v` as `engine_version` (bound-host mismatch twin).
    pub fn echo_engine_version_next_materialize(&self, v: &str) {
        self.materialize_state().engine_version_next = Some(v.to_string());
    }

    /// Drop the next `n` materialize replies (controlling-side timeout /
    /// resend rows). The command is still served + recorded, so a resend at
    /// the same tuple observes the dedup replay (I6) and an exhausted send
    /// leaves the documented orphan seed (I7).
    pub fn drop_next_materialize_replies(&self, n: u32) {
        self.materialize_state().drop_next_reply = n;
    }

    pub fn materialize_count(&self) -> u64 {
        self.materialize_state().count
    }

    /// Every `BridgeMaterializePayload` received, in order — the captured-
    /// payload pin surface (digest, tuple, launch, spec bytes decode).
    pub fn received_materialize_payloads(&self) -> Vec<BridgeMaterializePayload> {
        self.materialize_state().received.clone()
    }

    /// Script the member identity material acks for `identity` will carry.
    pub fn script_member_identity(&self, identity: &str, member: ScriptedMemberIdentity) {
        self.materialize_state()
            .member_identities
            .insert(identity.to_string(), member);
    }

    /// Next `ReleaseMember` replies `Rejected { cause, reason }` (release
    /// failure/retry rows R T-7/T-8).
    pub fn fail_next_release(&self, cause: BridgeRejectionCause, reason: &str) {
        self.release_state().fail_next = Some((cause, reason.to_string()));
    }

    /// Drop the next `n` release replies (reply-lost retry convergence row).
    pub fn drop_next_release_replies(&self, n: u32) {
        self.release_state().drop_next_reply = n;
    }

    pub fn release_count(&self) -> u64 {
        self.release_state().count
    }

    pub fn received_release_payloads(&self) -> Vec<BridgeReleasePayload> {
        self.release_state().received.clone()
    }

    /// Materialized (identity, generation, fence) tuples this scripted host
    /// currently holds — mirrors the HostStatus reply it serves.
    pub fn scripted_member_rows(&self) -> Vec<(String, u64, u64)> {
        self.materialize_state().recorded.keys().cloned().collect()
    }

    /// Seed one host-only materialized row that the controlling MobMachine
    /// never committed. This is the direct scripted fixture for orphan-sweep
    /// tests now that the normal failed-spawn path synchronously releases or
    /// certifies absence before returning.
    pub fn seed_orphan_member_row(&self, identity: &str, generation: u64, fence_token: u64) {
        let keypair = meerkat_comms::Keypair::generate();
        let row = BridgeMaterializedResponse {
            member_pubkey: format!("ed25519:{}", base64_encode(keypair.public_key().as_bytes())),
            member_peer_id: keypair.public_key().to_peer_id().to_string(),
            advertised_address: "tcp://127.0.0.1:1".to_string(),
            session_id: meerkat_core::SessionId::new().to_string(),
            spec_digest: format!("scripted-orphan-{identity}-{generation}-{fence_token}"),
            engine_version: "scripted-host-test".to_string(),
            launch_outcome: MaterializeLaunchOutcome::Fresh,
            resolved_auth_binding: None,
        };
        let replaced = self
            .materialize_state()
            .recorded
            .insert((identity.to_string(), generation, fence_token), row);
        assert!(replaced.is_none(), "orphan fixture tuple must be unique");
    }

    fn trust_state(&self) -> std::sync::MutexGuard<'_, ScriptedPeerTrustState> {
        self.trust
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// ADJ-P4-10: bind a REAL test-held endpoint as `identity`'s member
    /// runtime — accepted `InstallPeerTrust`/`RemovePeerTrust` commands are
    /// APPLIED to it (`PeerCommsEndpoint::trust`/`untrust`), so trust rows
    /// produce real delivery differentials. Pair with
    /// [`Self::script_member_identity`]`(identity, member_identity_of(&endpoint))`
    /// so materialize acks carry the same identity material.
    pub fn bind_member_endpoint(&self, identity: &str, endpoint: Arc<PeerCommsEndpoint>) {
        self.member_endpoints
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(identity.to_string(), endpoint);
    }

    /// Next `InstallPeerTrust` replies `Rejected { cause, reason }` and the
    /// descriptor is NOT applied to a bound endpoint (partial-install rows).
    pub fn reject_next_install_peer_trust(&self, cause: BridgeRejectionCause, reason: &str) {
        self.trust_state().reject_next_install = Some((cause, reason.to_string()));
    }

    /// Drop the next `n` `InstallPeerTrust` replies. The command is still
    /// served + applied (the materialize drop-injector semantics), so the
    /// controlling side observes a timeout while the trust row exists —
    /// the reply-lost retry-convergence flavor.
    pub fn drop_next_install_peer_trust_replies(&self, n: u32) {
        self.trust_state().drop_next_install_replies = n;
    }

    pub fn install_peer_trust_count(&self) -> u64 {
        self.trust_state().install_received.len() as u64
    }

    /// Every `BridgePeerTrustPayload` received by the install arm, in order —
    /// the captured-payload pin surface (identity, peer material).
    pub fn received_install_peer_trust_payloads(&self) -> Vec<BridgePeerTrustPayload> {
        self.trust_state().install_received.clone()
    }

    /// Next `RemovePeerTrust` replies `Rejected { cause, reason }` and the
    /// removal is NOT applied to a bound endpoint.
    pub fn reject_next_remove_peer_trust(&self, cause: BridgeRejectionCause, reason: &str) {
        self.trust_state().reject_next_remove = Some((cause, reason.to_string()));
    }

    /// Reject the next Remove directed at one exact member identity while
    /// allowing survivor-targeted removals to proceed.
    pub fn reject_remove_peer_trust_for(
        &self,
        identity: &str,
        cause: BridgeRejectionCause,
        reason: &str,
    ) {
        self.trust_state().reject_remove_for_identity =
            Some((identity.to_string(), cause, reason.to_string()));
    }

    /// Drop the next `n` `RemovePeerTrust` replies (served + applied, reply
    /// lost).
    pub fn drop_next_remove_peer_trust_replies(&self, n: u32) {
        self.trust_state().drop_next_remove_replies = n;
    }

    pub fn remove_peer_trust_count(&self) -> u64 {
        self.trust_state().remove_received.len() as u64
    }

    pub fn received_remove_peer_trust_payloads(&self) -> Vec<BridgePeerTrustPayload> {
        self.trust_state().remove_received.clone()
    }

    pub fn shutdown(&self) {
        self.task.abort();
    }
}

/// Serve one scripted `MaterializeMember` (state-machine mirror of the host
/// authority's Fresh/Replay dedup, plus the fault injectors). Returns `None`
/// when the reply is scripted to drop.
fn scripted_materialize_reply(
    state: &mut ScriptedMaterializeState,
    payload: &BridgeMaterializePayload,
    engine_version_default: &str,
) -> Option<BridgeReply> {
    state.received.push(payload.clone());
    state.count += 1;
    if let Some((cause, reason)) = state.reject_next.take() {
        return Some(BridgeReply::Rejected { cause, reason });
    }
    let agent_identity = &payload.spec.agent_identity;
    let tuple = (
        agent_identity.clone(),
        payload.generation,
        payload.fence_token,
    );
    let mut response = if let Some(recorded) = state.recorded.get(&tuple) {
        recorded.clone()
    } else {
        let identity = state
            .member_identities
            .get(agent_identity)
            .cloned()
            .unwrap_or_else(|| {
                // Stable synthetic member identity per agent identity.
                let keypair = meerkat_comms::Keypair::generate();
                ScriptedMemberIdentity {
                    member_pubkey: format!(
                        "ed25519:{}",
                        base64_encode(keypair.public_key().as_bytes())
                    ),
                    member_peer_id: keypair.public_key().to_peer_id().to_string(),
                    advertised_address: "tcp://127.0.0.1:1".to_string(),
                }
            });
        state
            .member_identities
            .insert(agent_identity.clone(), identity.clone());
        let (session_id, launch_outcome) = match &payload.launch {
            MaterializeLaunchMode::Fresh {} => (
                meerkat_core::SessionId::new().to_string(),
                MaterializeLaunchOutcome::Fresh,
            ),
            MaterializeLaunchMode::Resume { session_id } => (
                session_id.clone(),
                MaterializeLaunchOutcome::ResumedFromSnapshot,
            ),
        };
        let fresh = BridgeMaterializedResponse {
            member_pubkey: identity.member_pubkey,
            member_peer_id: identity.member_peer_id,
            advertised_address: identity.advertised_address,
            session_id,
            spec_digest: payload.spec_digest.clone(),
            engine_version: engine_version_default.to_string(),
            launch_outcome,
            resolved_auth_binding: None,
        };
        state.recorded.insert(tuple, fresh.clone());
        fresh
    };
    if state.wrong_digest_next {
        state.wrong_digest_next = false;
        response.spec_digest = format!("doctored-{}", response.spec_digest);
    }
    if let Some(version) = state.engine_version_next.take() {
        response.engine_version = version;
    }
    if state.drop_next_reply > 0 {
        state.drop_next_reply -= 1;
        return None;
    }
    Some(BridgeReply::MemberMaterialized(response))
}

/// Serve one scripted trust command (install or remove). Returns the reply
/// (`None` on a scripted drop) and whether the accept path should APPLY the
/// mutation to a bound probe endpoint: a scripted rejection never applies; a
/// dropped reply still applies (served + recorded, reply lost — the
/// materialize drop-injector semantics).
fn scripted_peer_trust_reply(
    reject_next: &mut Option<(BridgeRejectionCause, String)>,
    drop_next_replies: &mut u32,
) -> (Option<BridgeReply>, bool) {
    if let Some((cause, reason)) = reject_next.take() {
        return (Some(BridgeReply::Rejected { cause, reason }), false);
    }
    if *drop_next_replies > 0 {
        *drop_next_replies -= 1;
        return (None, true);
    }
    (Some(BridgeReply::Ack(BridgeAck { ok: true })), true)
}

/// Serve one scripted `ReleaseMember`. Returns `None` on a scripted drop.
fn scripted_release_reply(
    state: &mut ScriptedReleaseState,
    payload: &BridgeReleasePayload,
) -> Option<BridgeReply> {
    state.received.push(payload.clone());
    state.count += 1;
    if let Some((cause, reason)) = state.fail_next.take() {
        return Some(BridgeReply::Rejected { cause, reason });
    }
    let tuple = (
        payload.agent_identity.clone(),
        payload.generation,
        payload.fence_token,
    );
    let disposal = *state
        .recorded
        .entry(tuple)
        .or_insert(WireMemberSessionDisposal::Archived);
    if state.drop_next_reply > 0 {
        state.drop_next_reply -= 1;
        return None;
    }
    Some(BridgeReply::MemberReleased(BridgeMemberReleasedResponse {
        disposal,
    }))
}

pub async fn spawn_scripted_host_peer(name: &str) -> ScriptedHostPeer {
    let endpoint = Arc::new(spawn_peer_comms_endpoint(name, true, None).await);
    let token = format!("scripted-host-token-{}", uuid::Uuid::new_v4().simple());
    // Built through the wire form so this module never names the identity
    // enum (`WireTrustedPeerIdentity` is not re-exported through
    // bridge_protocol, and the integration-tests consumer has no
    // meerkat-contracts dependency).
    let descriptor: WireHostBindingDescriptor = serde_json::from_value(serde_json::json!({
        "kind": "host",
        "address": endpoint.runtime.advertised_address(),
        "identity": {
            "kind": "ed25519_public_key",
            "public_key": format!(
                "ed25519:{}",
                base64_encode(endpoint.runtime.public_key().as_bytes())
            ),
        },
        "bootstrap_token": token.as_str(),
    }))
    .expect("scripted host descriptor decodes");

    let fail_next_bind = Arc::new(AtomicBool::new(false));
    let garble_next_bind_reply = Arc::new(AtomicBool::new(false));
    let override_next_bind_peer_id = Arc::new(std::sync::Mutex::new(None::<String>));
    let bind_count = Arc::new(AtomicU64::new(0));
    let host_status_count = Arc::new(AtomicU64::new(0));
    let drop_next_host_status_replies = Arc::new(AtomicU64::new(0));

    let materialize = Arc::new(std::sync::Mutex::new(ScriptedMaterializeState::default()));
    let release = Arc::new(std::sync::Mutex::new(ScriptedReleaseState::default()));
    let trust = Arc::new(std::sync::Mutex::new(ScriptedPeerTrustState::default()));
    let member_endpoints = Arc::new(std::sync::Mutex::new(std::collections::BTreeMap::<
        String,
        Arc<PeerCommsEndpoint>,
    >::new()));

    let responder_endpoint = Arc::clone(&endpoint);
    let responder_fail = Arc::clone(&fail_next_bind);
    let responder_garble = Arc::clone(&garble_next_bind_reply);
    let responder_override = Arc::clone(&override_next_bind_peer_id);
    let responder_bind_count = Arc::clone(&bind_count);
    let responder_host_status_count = Arc::clone(&host_status_count);
    let responder_drop_next_host_status_replies = Arc::clone(&drop_next_host_status_replies);
    let responder_materialize = Arc::clone(&materialize);
    let responder_release = Arc::clone(&release);
    let responder_trust = Arc::clone(&trust);
    let responder_member_endpoints = Arc::clone(&member_endpoints);
    let responder_address = endpoint.runtime.advertised_address();
    let responder_token = token;
    let responder_runtime_incarnation = BridgeHostRuntimeIncarnation::new();

    let task = tokio::spawn(async move {
        let runtime = Arc::clone(&responder_endpoint.runtime);
        let notify = runtime.inbox_notify();
        loop {
            let notified = notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let candidates = runtime.drain_peer_input_candidates().await;
            if candidates.is_empty() {
                (&mut notified).await;
                continue;
            }
            for candidate in candidates {
                let InteractionContent::Request { params, .. } = &candidate.interaction.content
                else {
                    continue;
                };
                runtime
                    .peer_interaction_handle()
                    .expect("scripted host peer interaction authority")
                    .request_received(
                        PeerCorrelationId::from_uuid(candidate.interaction.id.0),
                        candidate.interaction.handling_mode,
                    )
                    .expect("record inbound peer request before response");

                let command: Result<BridgeCommand, _> = serde_json::from_value(params.clone());
                let reply_value: Option<serde_json::Value> = match command {
                    // --- phase-3 scripted V4 serving arms ---
                    Ok(BridgeCommand::MaterializeMember(payload)) => {
                        let supervisor_spec =
                            TrustedPeerDescriptor::try_from(payload.supervisor.clone())
                                .expect("valid supervisor spec");
                        responder_endpoint.trust(supervisor_spec.clone()).await;
                        let bound = responder_member_endpoints
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .get(&payload.spec.agent_identity)
                            .cloned();
                        if let Some(bound) = bound {
                            // Real host materialization installs the owning
                            // supervisor trust before exposing the member.
                            // Mirror it so a freshly spawned event pump cannot
                            // race its first poll ahead of turn delivery auth.
                            bound.trust(supervisor_spec).await;
                        }
                        scripted_materialize_reply(
                            &mut responder_materialize
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner),
                            &payload,
                            "scripted-host-test",
                        )
                        .map(|reply| {
                            serde_json::to_value(reply).expect("scripted materialize reply")
                        })
                    }
                    Ok(BridgeCommand::ReleaseMember(payload)) => {
                        let supervisor_spec =
                            TrustedPeerDescriptor::try_from(payload.supervisor.clone())
                                .expect("valid supervisor spec");
                        responder_endpoint.trust(supervisor_spec).await;
                        let reply = scripted_release_reply(
                            &mut responder_release
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner),
                            &payload,
                        );
                        if matches!(reply, Some(BridgeReply::MemberReleased(_))) {
                            // Mirror the host authority: the released row
                            // leaves the materialized inventory (HostStatus
                            // stops reporting it — sweep no-op convergence).
                            responder_materialize
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner)
                                .recorded
                                .remove(&(
                                    payload.agent_identity.clone(),
                                    payload.generation,
                                    payload.fence_token,
                                ));
                        }
                        reply.map(|reply| {
                            serde_json::to_value(reply).expect("scripted release reply")
                        })
                    }
                    Ok(BridgeCommand::RevokeHost(payload)) => {
                        let supervisor_spec =
                            TrustedPeerDescriptor::try_from(payload.supervisor.clone())
                                .expect("valid supervisor spec");
                        responder_endpoint.trust(supervisor_spec).await;
                        let released_members = {
                            let mut state = responder_materialize
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner);
                            let released = state
                                .recorded
                                .keys()
                                .map(|(identity, _, _)| identity.clone())
                                .collect::<std::collections::BTreeSet<_>>()
                                .into_iter()
                                .collect();
                            state.recorded.clear();
                            released
                        };
                        Some(
                            serde_json::to_value(BridgeReply::HostRevoked(
                                BridgeHostRevokedResponse {
                                    host_peer_id: runtime.public_key().to_peer_id().to_string(),
                                    mob_id: payload.mob_id,
                                    epoch: payload.epoch,
                                    binding_generation: payload.binding_generation,
                                    released_members,
                                },
                            ))
                            .expect("scripted host revoke reply"),
                        )
                    }
                    // --- phase-4 scripted V4 trust arms (DEC-P4H-10:
                    // accept/reject/drop matrix; default = ack + probe-as-
                    // member apply, repeat = ack — mirroring the real arm's
                    // machine-level idempotency) ---
                    Ok(BridgeCommand::InstallPeerTrust(payload)) => {
                        let supervisor_spec =
                            TrustedPeerDescriptor::try_from(payload.supervisor.clone())
                                .expect("valid supervisor spec");
                        responder_endpoint.trust(supervisor_spec).await;
                        let (reply, apply) = {
                            let mut guard = responder_trust
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner);
                            let state = &mut *guard;
                            state.install_received.push(payload.clone());
                            scripted_peer_trust_reply(
                                &mut state.reject_next_install,
                                &mut state.drop_next_install_replies,
                            )
                        };
                        if apply {
                            let bound = responder_member_endpoints
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner)
                                .get(&payload.agent_identity)
                                .cloned();
                            if let Some(bound) = bound {
                                let peer = TrustedPeerDescriptor::try_from(payload.peer.clone())
                                    .expect("valid peer spec");
                                bound.trust(peer).await;
                            }
                        }
                        reply.map(|reply| {
                            serde_json::to_value(reply).expect("scripted install trust reply")
                        })
                    }
                    Ok(BridgeCommand::RemovePeerTrust(payload)) => {
                        let supervisor_spec =
                            TrustedPeerDescriptor::try_from(payload.supervisor.clone())
                                .expect("valid supervisor spec");
                        responder_endpoint.trust(supervisor_spec).await;
                        let (reply, apply) =
                            {
                                let mut guard = responder_trust
                                    .lock()
                                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                                let state = &mut *guard;
                                state.remove_received.push(payload.clone());
                                if state.reject_remove_for_identity.as_ref().is_some_and(
                                    |(identity, _, _)| identity == &payload.agent_identity,
                                ) {
                                    let (_, cause, reason) = state
                                        .reject_remove_for_identity
                                        .take()
                                        .expect("targeted remove rejection present");
                                    (Some(BridgeReply::Rejected { cause, reason }), false)
                                } else {
                                    scripted_peer_trust_reply(
                                        &mut state.reject_next_remove,
                                        &mut state.drop_next_remove_replies,
                                    )
                                }
                            };
                        if apply {
                            let bound = responder_member_endpoints
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner)
                                .get(&payload.agent_identity)
                                .cloned();
                            if let Some(bound) = bound {
                                bound.untrust(&payload.peer.peer_id).await;
                            }
                        }
                        reply.map(|reply| {
                            serde_json::to_value(reply).expect("scripted remove trust reply")
                        })
                    }
                    Ok(BridgeCommand::HostStatus(_)) => {
                        responder_host_status_count.fetch_add(1, Ordering::SeqCst);
                        if responder_drop_next_host_status_replies
                            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
                                remaining.checked_sub(1)
                            })
                            .is_ok()
                        {
                            continue;
                        }
                        // Inventory mirrors the recorded materialize dedup map
                        // (the orphan-sweep read surface).
                        let members = {
                            let state = responder_materialize
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner);
                            state
                                .recorded
                                .iter()
                                .map(|((identity, generation, fence), ack)| {
                                    BridgeHostMemberRecord {
                                        agent_identity: identity.clone(),
                                        generation: *generation,
                                        fence_token: *fence,
                                        session_id: ack.session_id.clone(),
                                        spec_digest: ack.spec_digest.clone(),
                                        healthy: true,
                                    }
                                })
                                .collect()
                        };
                        Some(
                            serde_json::to_value(BridgeReply::HostStatus(
                                BridgeHostStatusResponse {
                                    runtime_incarnation: responder_runtime_incarnation,
                                    members,
                                    capabilities: BridgeCapabilities {
                                        durable_sessions: true,
                                        autonomous_members: true,
                                        tracked_input_cancel: true,
                                        engine_version: "scripted-host-test".to_string(),
                                        ..BridgeCapabilities::default()
                                    },
                                },
                            ))
                            .expect("scripted host status reply"),
                        )
                    }
                    Ok(BridgeCommand::BindHost(payload)) => Some({
                        // Reply route: trust the embedded supervisor spec (the
                        // member-harness reply discipline) so the failure
                        // matrix observes replies instead of send timeouts.
                        let supervisor_spec =
                            TrustedPeerDescriptor::try_from(payload.supervisor.clone())
                                .expect("valid supervisor spec");
                        responder_endpoint.trust(supervisor_spec).await;
                        if responder_fail.swap(false, Ordering::SeqCst) {
                            serde_json::to_value(BridgeReply::Rejected {
                                // This fault models a certified pre-write
                                // rejection. `Internal` is deliberately not
                                // suitable: it may follow a durable host-side
                                // commit and therefore requires cold replay.
                                cause: BridgeRejectionCause::InvalidBootstrapToken,
                                reason: "bind host failed: injected pre-write rejection"
                                    .to_string(),
                            })
                            .expect("bind rejection")
                        } else if payload.bootstrap_proof.as_str()
                            != meerkat_mob::runtime::bridge_protocol::seal_host_bind_bootstrap_proof(
                                payload.clone(),
                                &meerkat_mob::runtime::bridge_protocol::BridgeBootstrapToken::new(
                                    responder_token.clone(),
                                ),
                            )
                            .bootstrap_proof
                            .as_str()
                        {
                            serde_json::to_value(BridgeReply::Rejected {
                                cause: BridgeRejectionCause::InvalidBootstrapToken,
                                reason: "scripted host token mismatch".to_string(),
                            })
                            .expect("bind rejection")
                        } else {
                            responder_bind_count.fetch_add(1, Ordering::SeqCst);
                            let host_peer_id = responder_override
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner)
                                .take()
                                .unwrap_or_else(|| runtime.public_key().to_peer_id().to_string());
                            let mut value = serde_json::to_value(BridgeReply::BindHost(
                                BridgeHostBindResponse {
                                    host_peer_id,
                                    address: responder_address.clone(),
                                    binding_generation: payload.binding_generation,
                                    capabilities: BridgeCapabilities {
                                        durable_sessions: true,
                                        autonomous_members: true,
                                        tracked_input_cancel: true,
                                        engine_version: "scripted-host-test".to_string(),
                                        ..BridgeCapabilities::default()
                                    },
                                    live_endpoint: None,
                                },
                            ))
                            .expect("bind host response");
                            if responder_garble.swap(false, Ordering::SeqCst) {
                                // Structurally invalid reply: decode must fail
                                // typed on the controlling side (templates
                                // runtime/tests.rs:8654).
                                value = serde_json::json!({
                                    "result": "bind_host",
                                    "garbled": true
                                });
                            }
                            value
                        }
                    }),
                    // This fixture advertises no live endpoint and composes
                    // no MemberLiveHost. Lifecycle reconciliation may still
                    // probe the exact placed incarnation to prove that no
                    // channel exists, so mirror the real member drain's
                    // structural absent-slot reply rather than the generic
                    // catch-all Unsupported response.
                    Ok(
                        BridgeCommand::OpenMemberLiveChannel(_)
                        | BridgeCommand::CloseMemberLiveChannel(_)
                        | BridgeCommand::MemberLiveChannelStatus(_)
                        | BridgeCommand::ControlMemberLiveChannel(_),
                    ) => Some(
                        serde_json::to_value(BridgeReply::Rejected {
                            cause: BridgeRejectionCause::LiveTransportUnavailable,
                            reason: "scripted host serves no live substrate".to_string(),
                        })
                        .expect("scripted absent live-substrate reply"),
                    ),
                    Ok(_) | Err(_) => Some(
                        serde_json::to_value(BridgeReply::Rejected {
                            cause: BridgeRejectionCause::Unsupported,
                            reason: "scripted host serves BindHost/MaterializeMember/\
                                     ReleaseMember/RevokeHost/InstallPeerTrust/\
                                     RemovePeerTrust/HostStatus only"
                                .to_string(),
                        })
                        .expect("unsupported rejection"),
                    ),
                };

                // Scripted reply drop: the command was served + recorded but
                // the controlling side observes a timeout.
                let Some(reply_value) = reply_value else {
                    continue;
                };

                let reply_route = candidate.ingress.route.clone().or_else(|| {
                    candidate
                        .interaction
                        .from_route
                        .map(meerkat_core::PeerRoute::new)
                });
                let Some(route) = reply_route else {
                    continue;
                };
                let _ = runtime
                    .send(CommsCommand::PeerResponse {
                        to: route,
                        in_reply_to: candidate.interaction.id,
                        status: meerkat_core::ResponseStatus::Completed,
                        result: reply_value,
                        blocks: None,
                        content_taint: None,
                        handling_mode: None,
                        objective_id: candidate.interaction.objective_id,
                    })
                    .await;
            }
        }
    });

    ScriptedHostPeer {
        endpoint,
        descriptor,
        fail_next_bind,
        garble_next_bind_reply,
        override_next_bind_peer_id,
        bind_count,
        host_status_count,
        drop_next_host_status_replies,
        materialize,
        release,
        trust,
        member_endpoints,
        task,
    }
}

fn base64_encode(bytes: &[u8]) -> String {
    // Minimal std-only base64 (standard alphabet, padded) to avoid a dev-dep.
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(ALPHABET[(n >> 18) as usize & 63] as char);
        out.push(ALPHABET[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

// ===========================================================================
// Controlling mob (real mob, TestClient, no members, no LLM turns)
// ===========================================================================

pub struct ControllingMobPaths {
    pub user_config_root: std::path::PathBuf,
    pub runtime_root: std::path::PathBuf,
    pub project_root: std::path::PathBuf,
    pub context_root: std::path::PathBuf,
    pub sessions_root: std::path::PathBuf,
    pub mob_db_path: std::path::PathBuf,
}

impl ControllingMobPaths {
    pub fn new(root: &std::path::Path) -> Self {
        Self {
            user_config_root: root.join("user-config"),
            runtime_root: root.join("runtime-root"),
            project_root: root.join("project-root"),
            context_root: root.join("context-root"),
            sessions_root: root.join("sessions-jsonl"),
            mob_db_path: root.join("mob.db"),
        }
    }

    fn materialize_project_context(&self) {
        for root in [&self.project_root, &self.context_root] {
            std::fs::create_dir_all(root).expect("create project/context root");
            std::fs::write(
                root.join("AGENTS.md"),
                "# Multi-host bind ceremony\n\nDeterministic TestClient harness.\n",
            )
            .expect("write AGENTS.md");
        }
    }
}

/// Durable event-log read adapter over the fixture's concrete member
/// service (the mob_host.rs `ServiceDurableEventLogRead` wiring shape,
/// DEC-P6E-2).
struct FixtureDurableEventLogRead {
    service: Arc<meerkat_session::PersistentSessionService<meerkat::FactoryAgentBuilder>>,
}

#[async_trait::async_trait]
impl meerkat_runtime::member_observation::DurableEventLogRead for FixtureDurableEventLogRead {
    async fn read_from(
        &self,
        session: &meerkat_core::types::SessionId,
        from_seq: u64,
        max_rows: usize,
    ) -> Result<
        Option<
            Vec<(
                u64,
                meerkat_core::event::EventEnvelope<meerkat_core::event::AgentEvent>,
            )>,
        >,
        meerkat_runtime::member_observation::MemberObservationError,
    > {
        match self
            .service
            .event_log_read_from_bounded(session, from_seq, max_rows)
            .await
        {
            Ok(Some(rows)) => Ok(Some(
                rows.into_iter()
                    .map(|row| (row.seq, row.to_envelope()))
                    .collect(),
            )),
            Ok(None) => Ok(None),
            Err(error) => Err(
                meerkat_runtime::member_observation::MemberObservationError::Unavailable {
                    reason: error.to_string(),
                },
            ),
        }
    }

    async fn latest_seq(
        &self,
        session: &meerkat_core::types::SessionId,
    ) -> Result<Option<u64>, meerkat_runtime::member_observation::MemberObservationError> {
        self.service
            .event_log_latest_seq(session)
            .await
            .map_err(|error| {
                meerkat_runtime::member_observation::MemberObservationError::Unavailable {
                    reason: error.to_string(),
                }
            })
    }
}

pub fn persistent_service(
    paths: &ControllingMobPaths,
    runtime_store: Arc<dyn meerkat_runtime::RuntimeStore>,
) -> Arc<meerkat_session::PersistentSessionService<meerkat::FactoryAgentBuilder>> {
    persistent_service_with_client(
        paths,
        runtime_store,
        Arc::new(meerkat_client::TestClient::default()),
    )
}

/// `persistent_service` with an injected LLM client (phase-3: the member
/// substrate scripts deterministic member TOOL turns through
/// [`OneShotToolCallClient`]).
pub fn persistent_service_with_client(
    paths: &ControllingMobPaths,
    runtime_store: Arc<dyn meerkat_runtime::RuntimeStore>,
    client: Arc<dyn meerkat_client::LlmClient>,
) -> Arc<meerkat_session::PersistentSessionService<meerkat::FactoryAgentBuilder>> {
    paths.materialize_project_context();
    let factory = meerkat::AgentFactory::new(paths.runtime_root.join("factory-store"))
        .user_config_root(paths.user_config_root.clone())
        .runtime_root(paths.runtime_root.clone())
        .project_root(paths.project_root.clone())
        .context_root(paths.context_root.clone())
        .builtins(true)
        .comms(true);
    let mut builder = meerkat::FactoryAgentBuilder::new(factory, meerkat::Config::default());
    builder.default_llm_client = Some(client);
    // Workgraph-profiled members build fail-closed without installed
    // WorkGraph tools (the persistence-bundle default in production).
    meerkat::surface::set_default_workgraph_tools(
        &builder,
        Some(Arc::new(meerkat::WorkGraphToolSurface::new(
            meerkat::WorkGraphService::with_scope(
                Arc::new(meerkat::MemoryWorkGraphStore::new()),
                "test-realm",
                meerkat::WorkNamespace::default(),
            ),
        ))),
    );
    let store = Arc::new(meerkat_store::JsonlStore::new(paths.sessions_root.clone()));
    builder.default_session_store = Some(Arc::new(meerkat_store::StoreAdapter::new(store.clone())));

    let store_dyn: Arc<dyn meerkat::SessionStore> = store;
    let blob_store: Arc<dyn meerkat_core::BlobStore> =
        Arc::new(meerkat_store::MemoryBlobStore::default());
    // Durable event projection (the production Jsonl composition shape,
    // meerkat/src/persistence.rs): phase-6 member observation serves
    // PollMemberEvents and terminal_seq binding from this log.
    Arc::new(
        meerkat_session::PersistentSessionService::new(
            builder,
            32,
            store_dyn,
            runtime_store,
            blob_store,
        )
        .with_event_projection(
            Arc::new(meerkat_session::event_store::FileEventStore::new(
                paths.project_root.join(".rkat").join("events"),
            )),
            Arc::new(meerkat_session::projector::SessionProjector::new(
                paths.project_root.join(".rkat"),
            )),
        ),
    )
}

/// External-backend controlling-mob definition: one lead profile (never
/// spawned by the ceremony tests) and a supervisor bridge on a reserved
/// loopback port so `BindHost` replies can route back.
pub fn controlling_mob_definition(mob_id: meerkat_mob::MobId) -> meerkat_mob::MobDefinition {
    let supervisor_port = unused_loopback_port();
    let mut profiles = std::collections::BTreeMap::new();
    profiles.insert(
        meerkat_mob::ProfileName::from("lead"),
        meerkat_mob::ProfileBinding::Inline(Box::new(meerkat_mob::Profile {
            model: "claude-haiku-4-5-20251001".to_string(),
            provider: None,
            self_hosted_server_id: None,
            image_generation_provider: None,
            auto_compact_threshold: None,
            resume_overrides: Vec::new(),
            skills: vec![],
            tools: meerkat_mob::ToolConfig {
                comms: true,
                mob: true,
                ..meerkat_mob::ToolConfig::default()
            },
            peer_description: "Multi-host ceremony controlling lead".to_string(),
            external_addressable: true,
            backend: Some(meerkat_mob::MobBackendKind::External),
            runtime_mode: meerkat_mob::MobRuntimeMode::TurnDriven,
            max_inline_peer_notifications: None,
            output_schema: None,
            provider_params: None,
        })),
    );

    // Phase-3 profiles (design-F §3.1): a PORTABLE worker (no rust bundles,
    // no host-surface MCP allowlist, mob tools on — the placed-spawn +
    // upcall profile), a mob-tool-less twin (forwarding dispatcher absence
    // pin), a builtins-only portable profile (task-workflow preload pin) and
    // a deliberately NON-portable sibling (explicit workgraph assertion) for
    // the placement reject matrix.
    let base_worker = meerkat_mob::Profile {
        model: "claude-haiku-4-5-20251001".to_string(),
        provider: None,
        self_hosted_server_id: None,
        image_generation_provider: None,
        auto_compact_threshold: None,
        resume_overrides: Vec::new(),
        skills: vec![],
        tools: meerkat_mob::ToolConfig {
            comms: true,
            mob: true,
            ..meerkat_mob::ToolConfig::default()
        },
        peer_description: "Portable multi-host worker".to_string(),
        external_addressable: true,
        // Session at the PROFILE level: placement-less spawns of these
        // profiles walk the local arms (`resolve_binding` refuses a
        // binding-less External default by contract), while placed spawns
        // stay valid — the DEC-P3-3 mutual-exclusion check covers only
        // SPEC-level backend/binding.
        backend: Some(meerkat_mob::MobBackendKind::Session),
        runtime_mode: meerkat_mob::MobRuntimeMode::TurnDriven,
        max_inline_peer_notifications: None,
        output_schema: None,
        provider_params: None,
    };
    profiles.insert(
        meerkat_mob::ProfileName::from("worker"),
        meerkat_mob::ProfileBinding::Inline(Box::new(base_worker.clone())),
    );
    profiles.insert(
        meerkat_mob::ProfileName::from("quiet-worker"),
        meerkat_mob::ProfileBinding::Inline(Box::new(meerkat_mob::Profile {
            tools: meerkat_mob::ToolConfig {
                comms: true,
                mob: false,
                ..meerkat_mob::ToolConfig::default()
            },
            peer_description: "Portable worker without mob tools".to_string(),
            ..base_worker.clone()
        })),
    );
    profiles.insert(
        meerkat_mob::ProfileName::from("builtins-worker"),
        meerkat_mob::ProfileBinding::Inline(Box::new(meerkat_mob::Profile {
            tools: meerkat_mob::ToolConfig {
                comms: true,
                builtins: true,
                workgraph: false,
                ..meerkat_mob::ToolConfig::default()
            },
            peer_description: "Portable builtins worker (task-workflow preload)".to_string(),
            ..base_worker.clone()
        })),
    );
    profiles.insert(
        meerkat_mob::ProfileName::from("workgrapher"),
        meerkat_mob::ProfileBinding::Inline(Box::new(meerkat_mob::Profile {
            tools: meerkat_mob::ToolConfig {
                comms: true,
                builtins: true,
                workgraph: true,
                ..meerkat_mob::ToolConfig::default()
            },
            peer_description: "Non-portable workgraph worker".to_string(),
            ..base_worker
        })),
    );

    let mut definition = meerkat_mob::MobDefinition::explicit(mob_id);
    definition.orchestrator = Some(meerkat_mob::definition::OrchestratorConfig {
        profile: meerkat_mob::ProfileName::from("lead"),
    });
    definition.profiles = profiles;
    definition.backend = meerkat_mob::definition::BackendConfig {
        default: meerkat_mob::MobBackendKind::External,
        external: Some(meerkat_mob::definition::ExternalBackendConfig {
            address_base: "tcp://127.0.0.1".to_string(),
            supervisor_bridge: Some(meerkat_mob::definition::SupervisorBridgeEndpointConfig {
                bind_address: Some(format!("0.0.0.0:{supervisor_port}")),
                advertised_address: Some(format!("tcp://127.0.0.1:{supervisor_port}")),
            }),
        }),
    };
    definition
}

pub struct ControllingMob {
    pub handle: meerkat_mob::MobHandle,
    pub service: Arc<meerkat_session::PersistentSessionService<meerkat::FactoryAgentBuilder>>,
    pub storage_metadata: Arc<dyn meerkat_mob::store::MobRuntimeMetadataStore>,
    /// Flow-run authority retained across fixture restarts. This includes the
    /// replay-complete private remote-turn intents; recreating it while
    /// retaining only public events fabricates a recovery corruption that a
    /// real persistent MobStorage cannot produce.
    pub storage_runs: Arc<dyn meerkat_mob::store::MobRunStore>,
    pub storage_specs: Arc<dyn meerkat_mob::store::MobSpecStore>,
    pub mob_id: meerkat_mob::MobId,
    pub temp: tempfile::TempDir,
    /// Event-store handle retained so the mob can be rebuilt over the SAME
    /// durable truth (`MobBuilder::for_resume`) — the controlling-restart
    /// rows (R T-15/T-16, U T-21).
    pub storage_events: Arc<dyn meerkat_mob::store::MobEventStore>,
    /// Concrete test-support handle for deterministic append fault injection.
    pub storage_events_in_memory: Arc<meerkat_mob::store::InMemoryMobEventStore>,
    pub member_live_host: Option<Arc<dyn meerkat_runtime::member_live::MemberLiveHost>>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ControllingMobRestartMode {
    Graceful,
    CrashCommand,
    ActorAlreadyFailStopped,
}

/// Build a controlling mob whose `MobRuntimeMetadataStore` handle the test
/// retains, so FLAG-3 `MobHostAuthorityRecord` rows are directly inspectable.
/// The controlling host's base system prompt injected into every fixture
/// mob (ADJ-2: skill-less remote spawns resolve their `Set(base)` prompt
/// from this source; absence is a typed compile failure).
pub const CONTROLLING_BASE_PROMPT: &str =
    "You are a mob member on a test controlling host. Answer tersely.";

pub async fn create_controlling_mob(label: &str) -> ControllingMob {
    create_controlling_mob_with_customizer(label, None).await
}

/// [`create_controlling_mob`] with an optional pre-build
/// [`SpawnMemberCustomizer`] (the respawn placement-immutability rows drive
/// the customizer seam against placed members).
pub async fn create_controlling_mob_with_customizer(
    label: &str,
    customizer: Option<Arc<dyn meerkat_mob::runtime::SpawnMemberCustomizer>>,
) -> ControllingMob {
    create_controlling_mob_composed(label, customizer, None, |_| {}, |builder| builder).await
}

/// Phase 6: a controlling mob whose definition carries FLOWS (the two-host
/// flow rows target the fixture `worker`/`quiet-worker` roles).
pub async fn create_controlling_mob_with_flows(
    label: &str,
    flows: Vec<(meerkat_mob::FlowId, meerkat_mob::definition::FlowSpec)>,
) -> ControllingMob {
    create_controlling_mob_composed(
        label,
        None,
        None,
        move |definition| {
            for (flow_id, flow) in flows {
                definition.flows.insert(flow_id, flow);
            }
        },
        |builder| builder,
    )
    .await
}

/// Phase 6b: a controlling mob with a mutated DEFINITION (the live rows add
/// a realtime-modeled profile via [`add_realtime_worker_profile`]).
pub async fn create_controlling_mob_with_definition(
    label: &str,
    mutate_definition: impl FnOnce(&mut meerkat_mob::MobDefinition),
) -> ControllingMob {
    create_controlling_mob_composed(label, None, None, mutate_definition, |builder| builder).await
}

/// Phase 6b (T-C14): a controlling mob whose LOCAL live branch routes
/// through an injected `meerkat_runtime::member_live::MemberLiveHost` — the
/// ADJ-P6B-1 builder knob (`MobBuilder::with_member_live_host`).
pub async fn create_controlling_mob_with_member_live_host(
    label: &str,
    live_host: Arc<dyn meerkat_runtime::member_live::MemberLiveHost>,
) -> ControllingMob {
    create_controlling_mob_composed(
        label,
        None,
        Some(Arc::clone(&live_host)),
        |_| {},
        |builder| builder,
    )
    .await
}

/// Insert the `rt-worker` profile: the portable worker shape on the ONE
/// realtime-capable catalog model (`gpt-realtime-2`), provider-pinned to
/// OpenAI so B19 admits and B18 becomes a pure scripted-factory fact.
pub fn add_realtime_worker_profile(definition: &mut meerkat_mob::MobDefinition) {
    let worker = match definition
        .profiles
        .get(&meerkat_mob::ProfileName::from("worker"))
    {
        Some(meerkat_mob::ProfileBinding::Inline(profile)) => (**profile).clone(),
        other => panic!("fixture definition carries an inline worker profile, got {other:?}"),
    };
    definition.profiles.insert(
        meerkat_mob::ProfileName::from("rt-worker"),
        meerkat_mob::ProfileBinding::Inline(Box::new(meerkat_mob::Profile {
            model: "gpt-realtime-2".to_string(),
            provider: Some(meerkat_core::Provider::OpenAI),
            peer_description: "Portable realtime worker (phase 6b live rows)".to_string(),
            ..worker
        })),
    );
}

async fn create_controlling_mob_composed(
    label: &str,
    customizer: Option<Arc<dyn meerkat_mob::runtime::SpawnMemberCustomizer>>,
    member_live_host: Option<Arc<dyn meerkat_runtime::member_live::MemberLiveHost>>,
    mutate_definition: impl FnOnce(&mut meerkat_mob::MobDefinition),
    mutate_builder: impl FnOnce(meerkat_mob::MobBuilder) -> meerkat_mob::MobBuilder,
) -> ControllingMob {
    let temp = tempfile::tempdir().expect("controlling mob temp dir");
    let paths = ControllingMobPaths::new(temp.path());
    let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> =
        Arc::new(meerkat_runtime::InMemoryRuntimeStore::new());
    let service = persistent_service(&paths, runtime_store);

    let mob_id = meerkat_mob::MobId::from(format!("{label}-{}", uuid::Uuid::new_v4().simple()));
    let metadata: Arc<dyn meerkat_mob::store::MobRuntimeMetadataStore> =
        Arc::new(meerkat_mob::store::InMemoryMobRuntimeMetadataStore::new());
    let events_in_memory = Arc::new(meerkat_mob::store::InMemoryMobEventStore::new());
    let events: Arc<dyn meerkat_mob::store::MobEventStore> = events_in_memory.clone();
    let runs: Arc<dyn meerkat_mob::store::MobRunStore> =
        Arc::new(meerkat_mob::store::InMemoryMobRunStore::new());
    let specs: Arc<dyn meerkat_mob::store::MobSpecStore> =
        Arc::new(meerkat_mob::store::InMemoryMobSpecStore::new());
    let storage = meerkat_mob::MobStorage::custom_with_runtime_metadata(
        Arc::clone(&events),
        Arc::clone(&runs),
        Arc::clone(&specs),
        Arc::clone(&metadata),
    );

    let mob_service: Arc<dyn meerkat_mob::MobSessionService> = service.clone();
    let runtime_adapter = mob_service
        .runtime_adapter()
        .expect("persistent service exposes a runtime adapter");
    let controlling_acceptor = meerkat_mob::ControllingAcceptorConfig::for_session_service(
        "127.0.0.1:0".parse().expect("loopback acceptor address"),
        None,
        Arc::clone(&mob_service),
    );
    let mut definition = controlling_mob_definition(mob_id.clone());
    mutate_definition(&mut definition);
    // NO actor-level `with_default_llm_client`: an installed actor override
    // typed-rejects every placed spawn (plan §18.9 —
    // `NonPortableResource::DefaultLlmClientOverride`). Local turns stay
    // deterministic through `persistent_service`'s factory-level TestClient.
    let builder = meerkat_mob::MobBuilder::new(definition, storage)
        .with_session_service(mob_service)
        .with_runtime_adapter(runtime_adapter)
        .with_controlling_acceptor(controlling_acceptor)
        // §19.L5 invariant: `member_placement ≠ ∅ ⇒ owner_bridge_session_id ≠
        // None` — a mob without owner-bridge authority typed-rejects every
        // placed spawn, so the fixture binds one at create.
        .with_owner_bridge_session_create_authority(meerkat_core::SessionId::new(), false, false)
        // ADJ-2: remote spawns of skill-less profiles need the controlling
        // host's base prompt; absent a source they fail typed.
        .with_spawn_base_prompt_source(Arc::new(meerkat_mob::StaticSpawnBasePromptSource(
            CONTROLLING_BASE_PROMPT.to_string(),
        )));
    let builder = match customizer {
        Some(customizer) => builder.with_spawn_member_customizer(customizer),
        None => builder,
    };
    let builder = match member_live_host.as_ref() {
        Some(live_host) => builder.with_member_live_host(Arc::clone(live_host)),
        None => builder,
    };
    let builder = mutate_builder(builder);
    let handle = builder.create().await.expect("create controlling mob");

    ControllingMob {
        handle,
        service,
        storage_metadata: metadata,
        storage_runs: runs,
        storage_specs: specs,
        mob_id,
        temp,
        storage_events: events,
        storage_events_in_memory: events_in_memory,
        member_live_host,
    }
}

impl ControllingMob {
    /// `bind_host` against a `HostDaemonFixture` descriptor (wraps
    /// `descriptor_to_bind_request` + `MobHandle::bind_host`).
    pub async fn bind_fixture(
        &self,
        host: &HostDaemonFixture,
    ) -> meerkat_mob::runtime::HostBindReport {
        self.handle
            .bind_host(descriptor_to_bind_request(&host.current_descriptor()))
            .await
            .expect("bind host fixture")
    }

    /// Bind a scripted host peer (controlling-side failure-matrix rows).
    pub async fn bind_scripted(
        &self,
        scripted: &ScriptedHostPeer,
    ) -> meerkat_mob::runtime::HostBindReport {
        self.handle
            .bind_host(descriptor_to_bind_request(&scripted.descriptor))
            .await
            .expect("bind scripted host")
    }

    /// Spawn a member with placement (`SpawnMemberSpec.placement`, ADJ-7:
    /// `Option<mob_dsl::HostId>` — the value is the bound host's canonical
    /// comms peer id).
    pub async fn spawn_placed(
        &self,
        profile: &str,
        member: &str,
        host_id: &str,
    ) -> Result<meerkat_mob::SpawnResult, meerkat_mob::MobError> {
        self.handle
            .spawn_spec(placed_spawn_spec(profile, member, host_id))
            .await
    }

    /// The controlling machine's CURRENT bridge-session binding for a LOCAL
    /// member, read through the public member-status projection
    /// (`MobMemberSnapshot.current_session_id` — the diagnostic session-id
    /// observable; session-bound runtimes are machine-projected).
    pub async fn member_session_id(
        &self,
        identity: &meerkat_mob::AgentIdentity,
    ) -> meerkat_core::SessionId {
        let name = identity.as_str();
        self.handle
            .member_status(identity)
            .await
            .unwrap_or_else(|error| panic!("member status for {name}: {error}"))
            .current_session_id
            .unwrap_or_else(|| panic!("local member {name} carries a bound session id"))
    }

    /// The in-process comms runtime backing a local member's session
    /// (phase 4: the delivery-differential send handle for the local arm).
    pub async fn member_comms_runtime(
        &self,
        session_id: &meerkat_core::SessionId,
    ) -> Arc<dyn meerkat_core::agent::CommsRuntime> {
        self.service
            .comms_runtime(session_id)
            .await
            .expect("local member session exposes a comms runtime")
    }

    /// Whether the local member's comms runtime holds a PUBLIC trust row for
    /// `peer_id` — the controlling-side trust observable (local wiring trust
    /// is MobMachine-minted onto the member's own runtime). Absent runtime /
    /// absent row both read as `false` so negative rows can deadline-poll it.
    pub async fn local_member_trusts_peer(
        &self,
        session_id: &meerkat_core::SessionId,
        peer_id: &str,
    ) -> bool {
        let Some(runtime) = self.service.comms_runtime(session_id).await else {
            return false;
        };
        runtime
            .public_trusted_peer_projection_snapshot()
            .await
            .map(|rows| rows.iter().any(|row| row.peer_id.to_string() == peer_id))
            .unwrap_or(false)
    }

    /// Drop the running mob and rebuild it over the SAME storage handles —
    /// the controlling cold-restart idiom (`MobBuilder::for_resume`).
    pub async fn restart(self) -> ControllingMob {
        self.restart_inner(ControllingMobRestartMode::Graceful)
            .await
    }

    /// Rebuild over the same exact stores after stopping only volatile actor
    /// tasks. Unlike graceful [`Self::restart`], this preserves in-flight run
    /// and private remote-turn custody as a process crash would.
    #[cfg(feature = "test-support")]
    pub async fn crash_restart(self) -> ControllingMob {
        self.restart_inner(ControllingMobRestartMode::CrashCommand)
            .await
    }

    /// Rebuild after the production post-append uncertainty latch has already
    /// crash-quiesced and terminated the actor. No second stop command is
    /// possible or desirable; the same stores are replayed directly.
    pub async fn restart_after_actor_fail_stop(self) -> ControllingMob {
        self.restart_inner(ControllingMobRestartMode::ActorAlreadyFailStopped)
            .await
    }

    async fn restart_inner(self, mode: ControllingMobRestartMode) -> ControllingMob {
        let ControllingMob {
            handle,
            service,
            storage_metadata,
            storage_runs,
            storage_specs,
            mob_id,
            temp,
            storage_events,
            storage_events_in_memory,
            member_live_host,
        } = self;
        match mode {
            ControllingMobRestartMode::Graceful => {
                handle.shutdown().await.expect("shutdown controlling mob");
            }
            ControllingMobRestartMode::CrashCommand => {
                #[cfg(feature = "test-support")]
                handle
                    .crash_stop_preserving_durable_work_for_test()
                    .await
                    .expect("crash-stop controlling mob without durable terminalization");
                #[cfg(not(feature = "test-support"))]
                unreachable!("durable crash restart requires the test-support feature");
            }
            ControllingMobRestartMode::ActorAlreadyFailStopped => {}
        }
        drop(handle);

        // A production cold restart creates a fresh session/runtime process
        // over the same durable stores. Reusing `service` here retained the
        // previous process's generated-owner tokens and made this fixture a
        // same-runtime hot rebuild instead. The fail-stopped lane deliberately
        // drops that volatile process and constructs a new one while retaining
        // the JSONL session/event roots below.
        let service = if mode == ControllingMobRestartMode::ActorAlreadyFailStopped {
            drop(service);
            let paths = ControllingMobPaths::new(temp.path());
            let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> =
                Arc::new(meerkat_runtime::InMemoryRuntimeStore::new());
            persistent_service(&paths, runtime_store)
        } else {
            service
        };

        let storage = meerkat_mob::MobStorage::custom_with_runtime_metadata(
            Arc::clone(&storage_events),
            Arc::clone(&storage_runs),
            Arc::clone(&storage_specs),
            Arc::clone(&storage_metadata),
        );
        let mob_service: Arc<dyn meerkat_mob::MobSessionService> = service.clone();
        let runtime_adapter = mob_service
            .runtime_adapter()
            .expect("persistent service exposes a runtime adapter");
        let controlling_acceptor = meerkat_mob::ControllingAcceptorConfig::for_session_service(
            "127.0.0.1:0".parse().expect("loopback acceptor address"),
            None,
            Arc::clone(&mob_service),
        );
        // Same rule as `create_controlling_mob`: no actor-level LLM client
        // override, or placed spawns typed-reject (plan §18.9).
        let builder = meerkat_mob::MobBuilder::for_resume(storage)
            .with_session_service(mob_service)
            .with_runtime_adapter(runtime_adapter)
            .with_controlling_acceptor(controlling_acceptor)
            .with_spawn_base_prompt_source(Arc::new(meerkat_mob::StaticSpawnBasePromptSource(
                CONTROLLING_BASE_PROMPT.to_string(),
            )))
            .notify_orchestrator_on_resume(false);
        let builder = match member_live_host.as_ref() {
            Some(live_host) => builder.with_member_live_host(Arc::clone(live_host)),
            None => builder,
        };
        let handle = builder
            .resume()
            .await
            .expect("resume controlling mob over durable truth");

        ControllingMob {
            handle,
            service,
            storage_metadata,
            storage_runs,
            storage_specs,
            mob_id,
            temp,
            storage_events,
            storage_events_in_memory,
            member_live_host,
        }
    }
}

/// A placement-carrying spawn spec (the A4 embedder bypass path: the ladder,
/// not the tool seam, is the gate).
pub fn placed_spawn_spec(
    profile: &str,
    member: &str,
    host_id: &str,
) -> meerkat_mob::SpawnMemberSpec {
    let mut spec = meerkat_mob::SpawnMemberSpec::new(profile, member);
    spec.placement = Some(meerkat_mob::machines::mob_machine::HostId(
        host_id.to_string(),
    ));
    spec
}

// ===========================================================================
// Portable-spec + raw-materialize helpers (phase 3; design-H §2 header)
// ===========================================================================

/// Minimal PORTABLE member spec that builds deterministically on the fixture
/// substrate: pinned provider (preflight `model_resolvable` holds), no MCP
/// decls, no env keys, never-Inherit prompt/policy by construction.
pub fn sample_portable_member_spec(
    mob_id: &str,
    identity: &str,
    profile_name: &str,
) -> PortableMemberSpec {
    PortableMemberSpec {
        mob_id: mob_id.to_string(),
        profile_name: profile_name.to_string(),
        agent_identity: identity.to_string(),
        profile: PortableProfile {
            model: "claude-haiku-4-5-20251001".to_string(),
            provider: meerkat_core::Provider::Anthropic,
            self_hosted_server_id: None,
            image_generation_provider: None,
            auto_compact_threshold: None,
            resume_overrides: Vec::new(),
            skills: Vec::new(),
            tools: PortableToolConfig {
                builtins: false,
                shell: false,
                comms: true,
                memory: false,
                workgraph: false,
                mob: false,
                schedule: false,
                image_generation: false,
                mcp_servers: std::collections::BTreeMap::new(),
                non_portable_disabled: Vec::new(),
            },
            peer_description: "portable fixture worker".to_string(),
            external_addressable: true,
            runtime_mode: WireMobRuntimeMode::TurnDriven,
            max_inline_peer_notifications: None,
            output_schema: None,
            provider_params: None,
        },
        definition_extract: PortableDefinitionExtract {
            models: std::collections::BTreeMap::new(),
            image_generation_provider: None,
            skills: std::collections::BTreeMap::new(),
            profile_names: vec![profile_name.to_string()],
        },
        overlay: PortableSpawnOverlay {
            context: None,
            labels: None,
            additional_instructions: None,
            // R3: `Inherit` is decode-unrepresentable on this wire; the
            // compiler always resolves the base.
            system_prompt: PortableSystemPrompt::Set {
                text: format!("You are {identity}, a deterministic fixture member."),
            },
            tool_access_policy: None,
            mob_tool_authority_context: None,
            auth_binding: None,
            budget_limits: None,
            runtime_mode: WireMobRuntimeMode::TurnDriven,
            continuity_intent: WireSpawnContinuityIntent::Ephemeral,
        },
        required_env_keys: Vec::new(),
    }
}

/// Honest `MaterializeMember` payload over a portable spec (digest computed,
/// launch Fresh; ADJ-1: there is NO payload-level budget sibling — the
/// digest-covered `overlay.budget_limits` is the one carrier).
pub fn sample_materialize_payload(
    sender: &PeerCommsEndpoint,
    epoch: u64,
    spec: PortableMemberSpec,
    generation: u64,
    fence_token: u64,
    launch: MaterializeLaunchMode,
) -> Box<BridgeMaterializePayload> {
    let spec_digest =
        portable_member_spec_digest(&spec).expect("portable member spec digest computes");
    Box::new(BridgeMaterializePayload {
        supervisor: supervisor_spec_for_endpoint(sender, &spec.mob_id),
        epoch,
        binding_generation: 1,
        protocol_version: meerkat_mob::runtime::bridge_protocol::BridgeProtocolVersion::V4,
        generation,
        fence_token,
        spec,
        spec_digest,
        launch,
    })
}

/// A stateful deterministic LLM client: the FIRST stream emits exactly one
/// complete tool call (`name`/`args`), every later stream emits plain text +
/// `EndTurn`. Drives one member-side tool dispatch (upcall spawn rows,
/// budget-stop rows) without prompt engineering or replay loops.
pub struct OneShotToolCallClient {
    name: String,
    args: serde_json::Value,
    fired: AtomicBool,
}

impl OneShotToolCallClient {
    pub fn new(name: &str, args: serde_json::Value) -> Arc<Self> {
        Arc::new(Self {
            name: name.to_string(),
            args,
            fired: AtomicBool::new(false),
        })
    }

    pub fn fired(&self) -> bool {
        self.fired.load(Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl meerkat_client::LlmClient for OneShotToolCallClient {
    fn project_replay_messages(
        &self,
        messages: &[meerkat_core::Message],
    ) -> Result<Vec<meerkat_core::Message>, meerkat_client::LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(
        &'a self,
        _request: &'a meerkat_client::LlmRequest,
    ) -> meerkat_client::types::LlmStream<'a> {
        let events = if self.fired.swap(true, Ordering::SeqCst) {
            vec![
                meerkat_client::LlmEvent::TextDelta {
                    delta: "ok".to_string(),
                    meta: None,
                },
                meerkat_client::LlmEvent::Done {
                    outcome: meerkat_client::LlmDoneOutcome::Success {
                        stop_reason: meerkat_core::StopReason::EndTurn,
                    },
                },
            ]
        } else {
            vec![
                meerkat_client::LlmEvent::ToolCallComplete {
                    id: format!("one-shot-{}", uuid::Uuid::new_v4().simple()),
                    name: self.name.clone(),
                    args: self.args.clone(),
                    meta: None,
                },
                meerkat_client::LlmEvent::Done {
                    outcome: meerkat_client::LlmDoneOutcome::Success {
                        stop_reason: meerkat_core::StopReason::ToolUse,
                    },
                },
            ]
        };
        // Both scripts end in an explicit `Done`, so the raw scripted stream
        // is already terminal-complete.
        Box::pin(futures::stream::iter(events.into_iter().map(Ok)))
    }

    fn provider(&self) -> meerkat_core::Provider {
        meerkat_core::Provider::Other
    }

    async fn health_check(&self) -> Result<(), meerkat_client::LlmError> {
        Ok(())
    }
}

/// Decode an `ed25519:<base64>` pubkey string into its 32 raw bytes (the
/// ack's `member_pubkey` form; std-only, mirrors `base64_encode`).
pub fn pubkey_bytes(pubkey: &str) -> [u8; 32] {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let encoded = pubkey
        .strip_prefix("ed25519:")
        .expect("ed25519-prefixed pubkey");
    let value_of = |c: u8| -> u32 {
        ALPHABET
            .iter()
            .position(|&a| a == c)
            .map(|v| v as u32)
            .unwrap_or(0)
    };
    let stripped: Vec<u8> = encoded.bytes().filter(|&c| c != b'=').collect();
    let mut bytes = Vec::new();
    for chunk in stripped.chunks(4) {
        let mut n: u32 = 0;
        for (i, &c) in chunk.iter().enumerate() {
            n |= value_of(c) << (18 - 6 * i as u32);
        }
        for i in 0..chunk.len().saturating_sub(1) {
            bytes.push(((n >> (16 - 8 * i as u32)) & 0xff) as u8);
        }
    }
    bytes.try_into().expect("32-byte ed25519 pubkey")
}

/// Trust-descriptor form of a materialize ack's member identity, addressed
/// at the owning host's acceptor (D1 demux).
pub fn member_descriptor_from_ack(
    name: &str,
    ack: &BridgeMaterializedResponse,
) -> TrustedPeerDescriptor {
    TrustedPeerDescriptor::unsigned_with_pubkey(
        name,
        ack.member_peer_id.clone(),
        pubkey_bytes(&ack.member_pubkey),
        ack.advertised_address.clone(),
    )
    .expect("member descriptor from ack")
}

/// Exact host-resident member address for raw delivery probes. The host id is
/// the descriptor's authenticated peer identity, never its display name or
/// transport address.
pub fn member_incarnation_from_ack(
    fixture: &HostDaemonFixture,
    mob_id: &str,
    identity: &str,
    ack: &BridgeMaterializedResponse,
    binding_generation: u64,
    generation: u64,
    fence_token: u64,
) -> BridgeMemberIncarnation {
    let host_id = fixture
        .current_descriptor()
        .identity
        .resolve()
        .expect("fixture host identity resolves")
        .peer_id
        .to_string();
    BridgeMemberIncarnation {
        mob_id: mob_id.to_string(),
        agent_identity: identity.to_string(),
        host_id,
        binding_generation,
        member_session_id: ack.session_id.clone(),
        generation,
        fence_token,
    }
}

pub const FIXTURE_BRIDGE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Raw bind (probe endpoint AS supervisor, epoch 1), then materialize
/// `identity` at `(generation=1, fence=1)`; returns the typed ack. The probe
/// must already trust the fixture's host descriptor.
pub async fn bind_then_materialize(
    sender: &PeerCommsEndpoint,
    fixture: &HostDaemonFixture,
    mob_id: &str,
    identity: &str,
) -> BridgeMaterializedResponse {
    let descriptor = fixture.current_descriptor();
    let bind = raw_bind_host_command(sender, mob_id, &descriptor, 1);
    let reply = sender
        .send_bridge_command_raw(
            &fixture.host_peer_descriptor(),
            &bind,
            FIXTURE_BRIDGE_TIMEOUT,
        )
        .await
        .expect("raw bind reply");
    assert!(
        matches!(reply, BridgeReply::BindHost(_)),
        "raw bind must succeed before materialize, got {reply:?}"
    );
    let payload = sample_materialize_payload(
        sender,
        1,
        sample_portable_member_spec(mob_id, identity, "worker"),
        1,
        1,
        MaterializeLaunchMode::Fresh {},
    );
    let reply = sender
        .send_bridge_command_raw(
            &fixture.host_peer_descriptor(),
            &BridgeCommand::MaterializeMember(payload),
            FIXTURE_BRIDGE_TIMEOUT,
        )
        .await
        .expect("raw materialize reply");
    match reply {
        BridgeReply::MemberMaterialized(response) => response,
        other => panic!("expected MemberMaterialized, got {other:?}"),
    }
}

/// [`bind_then_materialize`] with an explicit portable spec (phase 6b: the
/// live rows materialize REALTIME-modeled members).
pub async fn bind_then_materialize_with_spec(
    sender: &PeerCommsEndpoint,
    fixture: &HostDaemonFixture,
    mob_id: &str,
    spec: PortableMemberSpec,
) -> BridgeMaterializedResponse {
    let descriptor = fixture.current_descriptor();
    let bind = raw_bind_host_command(sender, mob_id, &descriptor, 1);
    let reply = sender
        .send_bridge_command_raw(
            &fixture.host_peer_descriptor(),
            &bind,
            FIXTURE_BRIDGE_TIMEOUT,
        )
        .await
        .expect("raw bind reply");
    assert!(
        matches!(reply, BridgeReply::BindHost(_)),
        "raw bind must succeed before materialize, got {reply:?}"
    );
    let payload =
        sample_materialize_payload(sender, 1, spec, 1, 1, MaterializeLaunchMode::Fresh {});
    let reply = sender
        .send_bridge_command_raw(
            &fixture.host_peer_descriptor(),
            &BridgeCommand::MaterializeMember(payload),
            FIXTURE_BRIDGE_TIMEOUT,
        )
        .await
        .expect("raw materialize reply");
    match reply {
        BridgeReply::MemberMaterialized(response) => response,
        other => panic!("expected MemberMaterialized, got {other:?}"),
    }
}

/// A portable member spec on the ONE realtime-capable catalog model
/// (`gpt-realtime-2`, B19's `realtime: true` row) — the live open rows'
/// member shape. The member's TURNS still ride the injected deterministic
/// LLM client; the model id only drives the catalog realtime gate.
pub fn realtime_portable_member_spec(
    mob_id: &str,
    identity: &str,
    profile_name: &str,
) -> PortableMemberSpec {
    let mut spec = sample_portable_member_spec(mob_id, identity, profile_name);
    spec.profile.model = "gpt-realtime-2".to_string();
    spec.profile.provider = meerkat_core::Provider::OpenAI;
    spec
}

// ───────────────── phase 6b: raw live-family command builders ─────────────────
//
// Member-addressed (sent TO the member descriptor, the raw_deliver /
// raw_poll shape). Epoch 1 = the bind_then_materialize ceremony epoch; the
// T-C9 reply-loss row discards replies to mint owning-side orphans.

/// Raw `OpenMemberLiveChannel`.
pub fn raw_open_member_live_channel_command(
    sender: &PeerCommsEndpoint,
    mob_id: &str,
    epoch: u64,
    expected_member: BridgeMemberIncarnation,
    turning_mode: Option<meerkat_mob::runtime::bridge_protocol::RealtimeTurningMode>,
    transport: Option<meerkat_mob::runtime::bridge_protocol::LiveOpenTransport>,
) -> BridgeCommand {
    BridgeCommand::OpenMemberLiveChannel(
        meerkat_mob::runtime::bridge_protocol::BridgeLiveOpenPayload {
            supervisor: supervisor_spec_for_endpoint(sender, mob_id),
            epoch,
            protocol_version: meerkat_mob::runtime::bridge_protocol::BridgeProtocolVersion::V4,
            expected_member,
            turning_mode,
            transport,
        },
    )
}

/// Raw `CloseMemberLiveChannel` (required id — close-what-you-name).
pub fn raw_close_member_live_channel_command(
    sender: &PeerCommsEndpoint,
    mob_id: &str,
    epoch: u64,
    expected_member: BridgeMemberIncarnation,
    channel_id: &str,
) -> BridgeCommand {
    BridgeCommand::CloseMemberLiveChannel(
        meerkat_mob::runtime::bridge_protocol::BridgeLiveChannelPayload {
            supervisor: supervisor_spec_for_endpoint(sender, mob_id),
            epoch,
            protocol_version: meerkat_mob::runtime::bridge_protocol::BridgeProtocolVersion::V4,
            expected_member,
            channel_id: channel_id.to_string(),
        },
    )
}

/// Raw `MemberLiveChannelStatus`; `channel_id: None` = "the member's active
/// channel" (ADJ-P6B-2 — the reply-loss discovery shape).
pub fn raw_member_live_status_command(
    sender: &PeerCommsEndpoint,
    mob_id: &str,
    epoch: u64,
    expected_member: BridgeMemberIncarnation,
    channel_id: Option<String>,
) -> BridgeCommand {
    BridgeCommand::MemberLiveChannelStatus(
        meerkat_mob::runtime::bridge_protocol::BridgeLiveStatusPayload {
            supervisor: supervisor_spec_for_endpoint(sender, mob_id),
            epoch,
            protocol_version: meerkat_mob::runtime::bridge_protocol::BridgeProtocolVersion::V4,
            expected_member,
            channel_id,
        },
    )
}

// ───────────────────── phase 5: principal fixtures ─────────────────────
//
// The §11 scope-matrix rows drive verbs through the PRODUCTION
// principal-binding seam (ADJ-P5-10): a handle clone rebound with
// `CommandAuthority::principal(MobControlPrincipal::External(..))` — the
// exact seam the v2 bearer-token resolver lands on. Deliberately NOT a
// cfg(test) backdoor (DEC-P5E-13).

/// Parse a test principal string through the ONE fail-closed boundary type.
pub fn principal_id(principal: &str) -> meerkat_core::auth::PrincipalId {
    meerkat_core::auth::PrincipalId::new(principal).expect("valid test principal id")
}

/// A named (non-owner) console principal for `principal`.
pub fn control_principal(principal: &str) -> meerkat_mob::MobControlPrincipal {
    meerkat_mob::MobControlPrincipal::External(principal_id(principal))
}

impl ControllingMob {
    /// A handle clone rebound to the named principal's console authority.
    pub fn handle_as(&self, principal: &str) -> meerkat_mob::MobHandle {
        self.handle
            .clone()
            .with_command_authority(meerkat_mob::CommandAuthority::principal(control_principal(
                principal,
            )))
    }

    /// Owner-shortcut grant for arrange steps: record `scopes` for
    /// `principal` (full-replace semantics) with an optional raw expiry.
    pub async fn grant(
        &self,
        principal: &str,
        scopes: &[meerkat_mob::machines::mob_machine::ControlScope],
        expires_at_ms: Option<u64>,
    ) -> meerkat_mob::OperatorGrant {
        self.handle
            .grant_scopes(
                meerkat_mob::MobControlPrincipal::Owner,
                principal_id(principal),
                scopes.iter().copied().collect(),
                expires_at_ms,
            )
            .await
            .expect("owner grant records")
    }
}

// ───────────────────── phase 6: flow + event fixtures ─────────────────────
//
// design-flow-spine §5.0: two-host flow drivers, deterministic scripted
// member turns, merged-stream observation, the host journal probe, and the
// scripted member-turn responder (delivery-fault injectors in the
// `drop_next_materialize_replies` template).

/// Deadline-poll `predicate` (25ms tick, 60s cap) — the shared no-sleeps
/// convergence helper for the phase-6 test roots.
pub async fn wait_until<F, Fut>(what: &str, mut predicate: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    tokio::time::timeout(std::time::Duration::from_secs(60), async {
        loop {
            if predicate().await {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {what}"));
}

/// One flat single-step flow targeting `role` (StepId "step-1").
pub fn single_step_flow(role: &str, message: &str) -> meerkat_mob::definition::FlowSpec {
    flow_from_step(role, message, None, None, None)
}

/// [`single_step_flow`] with a `blocked_tools` overlay row.
pub fn single_step_flow_with_blocked_tools(
    role: &str,
    message: &str,
    blocked: &[&str],
) -> meerkat_mob::definition::FlowSpec {
    flow_from_step(
        role,
        message,
        None,
        Some(blocked.iter().map(|name| (*name).to_string()).collect()),
        None,
    )
}

/// [`single_step_flow`] with an explicit per-step timeout.
pub fn single_step_flow_with_timeout(
    role: &str,
    message: &str,
    timeout_ms: u64,
) -> meerkat_mob::definition::FlowSpec {
    flow_from_step(role, message, None, None, Some(timeout_ms))
}

fn flow_from_step(
    role: &str,
    message: &str,
    allowed_tools: Option<Vec<String>>,
    blocked_tools: Option<Vec<String>>,
    timeout_ms: Option<u64>,
) -> meerkat_mob::definition::FlowSpec {
    // Borrow the empty steps map from a default FlowSpec so this module
    // never names the IndexMap type (the integration-tests consumer of this
    // file carries no indexmap dependency).
    let mut steps = meerkat_mob::definition::FlowSpec::default().steps;
    steps.insert(
        meerkat_mob::StepId::from("step-1"),
        meerkat_mob::definition::FlowStepSpec {
            role: meerkat_mob::ProfileName::from(role),
            message: message.into(),
            depends_on: Vec::new(),
            dispatch_mode: Default::default(),
            collection_policy: Default::default(),
            condition: None,
            timeout_ms,
            expected_schema_ref: None,
            branch: None,
            depends_on_mode: Default::default(),
            allowed_tools,
            blocked_tools,
            output_format: None,
        },
    );
    meerkat_mob::definition::FlowSpec::new(None, steps, None)
}

/// Deadline-poll a flow run to its terminal status.
pub async fn wait_for_flow_run_terminal(
    handle: &meerkat_mob::MobHandle,
    run_id: &meerkat_mob::RunId,
    timeout: std::time::Duration,
) -> meerkat_mob::MobRun {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let run = handle
            .flow_status(run_id.clone())
            .await
            .expect("flow status reads")
            .expect("run exists");
        if matches!(
            run.status,
            meerkat_mob::MobRunStatus::Completed
                | meerkat_mob::MobRunStatus::Failed
                | meerkat_mob::MobRunStatus::Canceled
        ) {
            return run;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for run {run_id} terminal; last status {:?}",
            run.status
        );
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
}

/// Collect merged-stream items until `until` matches (the matching item is
/// included), or panic at the deadline with everything collected so far.
pub async fn collect_mob_stream_until(
    router: &mut meerkat_mob::runtime::MobEventRouterHandle,
    mut until: impl FnMut(&meerkat_mob::AttributedEvent) -> bool,
    timeout: std::time::Duration,
) -> Vec<meerkat_mob::AttributedEvent> {
    let deadline = tokio::time::Instant::now() + timeout;
    let mut collected: Vec<meerkat_mob::AttributedEvent> = Vec::new();
    loop {
        let remaining = deadline
            .checked_duration_since(tokio::time::Instant::now())
            .unwrap_or_default();
        if remaining.is_zero() {
            panic!(
                "timed out collecting the merged mob stream; {} item(s) so far: {:?}",
                collected.len(),
                collected
                    .iter()
                    .map(|event| {
                        format!(
                            "{}#{}:{}",
                            event.source.identity.as_str(),
                            event.envelope.seq,
                            support_event_kind(&event.envelope.payload)
                        )
                    })
                    .collect::<Vec<_>>()
            );
        }
        match tokio::time::timeout(remaining, router.event_rx.recv()).await {
            Ok(Some(event)) => {
                let done = until(&event);
                collected.push(event);
                if done {
                    return collected;
                }
            }
            Ok(None) => panic!("merged mob stream closed before the awaited item"),
            Err(_) => {} // loop re-checks the deadline and panics with context
        }
    }
}

/// Debug label for a collected event (timeout diagnostics only).
pub fn support_event_kind(event: &meerkat_core::AgentEvent) -> &'static str {
    match event {
        meerkat_core::AgentEvent::RunStarted { .. } => "run_started",
        meerkat_core::AgentEvent::RunCompleted { .. } => "run_completed",
        meerkat_core::AgentEvent::RunFailed { .. } => "run_failed",
        meerkat_core::AgentEvent::InteractionComplete { .. } => "interaction_complete",
        meerkat_core::AgentEvent::InteractionFailed { .. } => "interaction_failed",
        _ => "other",
    }
}

// ───────────────────── scripted member LLM clients ─────────────────────

/// Every stream completes immediately with `text` (EndTurn).
struct ScriptedCompletingClient {
    text: String,
}

#[async_trait::async_trait]
impl meerkat_client::LlmClient for ScriptedCompletingClient {
    fn project_replay_messages(
        &self,
        messages: &[meerkat_core::Message],
    ) -> Result<Vec<meerkat_core::Message>, meerkat_client::LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(
        &'a self,
        _request: &'a meerkat_client::LlmRequest,
    ) -> meerkat_client::types::LlmStream<'a> {
        let events = vec![
            meerkat_client::LlmEvent::TextDelta {
                delta: self.text.clone(),
                meta: None,
            },
            meerkat_client::LlmEvent::Done {
                outcome: meerkat_client::LlmDoneOutcome::Success {
                    stop_reason: meerkat_core::StopReason::EndTurn,
                },
            },
        ];
        Box::pin(futures::stream::iter(events.into_iter().map(Ok)))
    }

    fn provider(&self) -> meerkat_core::Provider {
        meerkat_core::Provider::Other
    }

    async fn health_check(&self) -> Result<(), meerkat_client::LlmError> {
        Ok(())
    }
}

/// A member client whose every turn completes with `text`.
pub fn scripted_member_client_completing(text: &str) -> Arc<dyn meerkat_client::LlmClient> {
    Arc::new(ScriptedCompletingClient {
        text: text.to_string(),
    })
}

/// FIRST stream: exactly one tool call; every later stream: `text` + EndTurn
/// (the overlay-denial row — the denied result comes back, the member then
/// completes).
struct ToolThenTextClient {
    tool: String,
    args: serde_json::Value,
    text: String,
    fired: AtomicBool,
}

#[async_trait::async_trait]
impl meerkat_client::LlmClient for ToolThenTextClient {
    fn project_replay_messages(
        &self,
        messages: &[meerkat_core::Message],
    ) -> Result<Vec<meerkat_core::Message>, meerkat_client::LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(
        &'a self,
        _request: &'a meerkat_client::LlmRequest,
    ) -> meerkat_client::types::LlmStream<'a> {
        let events = if self.fired.swap(true, Ordering::SeqCst) {
            vec![
                meerkat_client::LlmEvent::TextDelta {
                    delta: self.text.clone(),
                    meta: None,
                },
                meerkat_client::LlmEvent::Done {
                    outcome: meerkat_client::LlmDoneOutcome::Success {
                        stop_reason: meerkat_core::StopReason::EndTurn,
                    },
                },
            ]
        } else {
            vec![
                meerkat_client::LlmEvent::ToolCallComplete {
                    id: format!("phase6-{}", uuid::Uuid::new_v4().simple()),
                    name: self.tool.clone(),
                    args: self.args.clone(),
                    meta: None,
                },
                meerkat_client::LlmEvent::Done {
                    outcome: meerkat_client::LlmDoneOutcome::Success {
                        stop_reason: meerkat_core::StopReason::ToolUse,
                    },
                },
            ]
        };
        Box::pin(futures::stream::iter(events.into_iter().map(Ok)))
    }

    fn provider(&self) -> meerkat_core::Provider {
        meerkat_core::Provider::Other
    }

    async fn health_check(&self) -> Result<(), meerkat_client::LlmError> {
        Ok(())
    }
}

/// A member client that calls `tool(args)` once, then completes with `text`.
pub fn scripted_member_client_calling_tool_then_completing(
    tool: &str,
    args: serde_json::Value,
    text: &str,
) -> Arc<dyn meerkat_client::LlmClient> {
    Arc::new(ToolThenTextClient {
        tool: tool.to_string(),
        args,
        text: text.to_string(),
        fired: AtomicBool::new(false),
    })
}

/// Test-held release gate for [`scripted_member_client_stalling`].
#[derive(Clone)]
pub struct StallGate {
    released: Arc<AtomicBool>,
    notify: Arc<tokio::sync::Notify>,
}

impl StallGate {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self {
            released: Arc::new(AtomicBool::new(false)),
            notify: Arc::new(tokio::sync::Notify::new()),
        }
    }

    /// Release every parked (and future) stalled stream.
    pub fn release(&self) {
        self.released.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
    }

    async fn wait(&self) {
        while !self.released.load(Ordering::SeqCst) {
            let notified = self.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.released.load(Ordering::SeqCst) {
                return;
            }
            (&mut notified).await;
        }
    }
}

/// A member client whose streams PARK until the test releases the gate,
/// then complete (the timeout / hard-cancel / generation-bump rows). A
/// parked stream aborts cleanly when the member runtime is torn down.
struct StallingClient {
    gate: StallGate,
}

#[async_trait::async_trait]
impl meerkat_client::LlmClient for StallingClient {
    fn project_replay_messages(
        &self,
        messages: &[meerkat_core::Message],
    ) -> Result<Vec<meerkat_core::Message>, meerkat_client::LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(
        &'a self,
        _request: &'a meerkat_client::LlmRequest,
    ) -> meerkat_client::types::LlmStream<'a> {
        let gate = self.gate.clone();
        Box::pin(futures::stream::unfold(
            (gate, 0u8),
            |(gate, emitted)| async move {
                match emitted {
                    0 => {
                        gate.wait().await;
                        Some((
                            Ok(meerkat_client::LlmEvent::TextDelta {
                                delta: "released".to_string(),
                                meta: None,
                            }),
                            (gate, 1),
                        ))
                    }
                    1 => Some((
                        Ok(meerkat_client::LlmEvent::Done {
                            outcome: meerkat_client::LlmDoneOutcome::Success {
                                stop_reason: meerkat_core::StopReason::EndTurn,
                            },
                        }),
                        (gate, 2),
                    )),
                    _ => None,
                }
            },
        ))
    }

    fn provider(&self) -> meerkat_core::Provider {
        meerkat_core::Provider::Other
    }

    async fn health_check(&self) -> Result<(), meerkat_client::LlmError> {
        Ok(())
    }
}

/// A member client that parks every turn until `gate.release()`.
pub fn scripted_member_client_stalling(gate: StallGate) -> Arc<dyn meerkat_client::LlmClient> {
    Arc::new(StallingClient { gate })
}

// ───────────────────── host journal probe ─────────────────────

/// One decoded turn-outcome journal row from the host's durable
/// `MobHostBindingAuthority` record (phase-6 O2).
#[derive(Debug, Clone)]
pub struct TurnOutcomeRow {
    pub agent_identity: String,
    pub generation: u64,
    pub input_id: String,
    pub terminal_seq: u64,
    pub outcome_kind: String,
}

impl HostDaemonFixture {
    /// Decoded turn-outcome journal rows for `mob_id`, read from the durable
    /// R8 record (the `host_binding_records` read-only projection). Decoded
    /// structurally through JSON so the probe tracks the events-lane region
    /// shape: rows must carry `generation`, `input_id`, `terminal_seq`,
    /// `outcome`, keyed by (or carrying) the member identity.
    pub async fn turn_outcome_rows(&self, mob_id: &str) -> Vec<TurnOutcomeRow> {
        let Some(record) = self
            .host_binding_records()
            .await
            .into_iter()
            .find_map(|(id, record)| (id == mob_id).then_some(record))
        else {
            return Vec::new();
        };
        let value = serde_json::to_value(&record).expect("host binding record serializes");
        let Some(region) = value.get("turn_outcomes") else {
            return Vec::new();
        };

        fn outcome_kind(value: &serde_json::Value) -> String {
            match value {
                // Unit wire variants serialize as bare strings.
                serde_json::Value::String(kind) => kind.clone(),
                // Payload-bearing wire variants serialize externally tagged:
                // one object with the kind as its single key.
                serde_json::Value::Object(map) => map
                    .get("kind")
                    .and_then(|kind| kind.as_str())
                    .map(str::to_string)
                    .or_else(|| {
                        (map.len() == 1)
                            .then(|| map.keys().next().cloned())
                            .flatten()
                    })
                    .unwrap_or_else(|| value.to_string()),
                other => other.to_string(),
            }
        }

        fn row_from(identity: &str, row: &serde_json::Value) -> Option<TurnOutcomeRow> {
            Some(TurnOutcomeRow {
                agent_identity: identity.to_string(),
                generation: row.get("generation")?.as_u64()?,
                input_id: row.get("input_id")?.as_str()?.to_string(),
                terminal_seq: row.get("terminal_seq")?.as_u64()?,
                outcome_kind: outcome_kind(row.get("outcome")?),
            })
        }

        let mut rows = Vec::new();
        match region {
            serde_json::Value::Object(by_identity) => {
                for (identity, entries) in by_identity {
                    match entries {
                        serde_json::Value::Array(entries) => {
                            rows.extend(entries.iter().filter_map(|row| row_from(identity, row)));
                        }
                        single @ serde_json::Value::Object(_) => {
                            rows.extend(row_from(identity, single));
                        }
                        _ => {}
                    }
                }
            }
            serde_json::Value::Array(entries) => {
                for row in entries {
                    let identity = row
                        .get("agent_identity")
                        .and_then(|identity| identity.as_str())
                        .unwrap_or_default()
                        .to_string();
                    rows.extend(row_from(&identity, row));
                }
            }
            _ => {}
        }
        rows
    }

    /// The full transcript page of one member session on this host.
    pub async fn member_session_history(
        &self,
        session_id: &str,
    ) -> Result<meerkat_core::service::SessionHistoryPage, String> {
        let id = meerkat_core::SessionId::parse(session_id)
            .map_err(|error| format!("member session id parses: {error}"))?;
        self.member_session_service()
            .read_history(&id, meerkat_core::service::SessionHistoryQuery::default())
            .await
            .map_err(|error| format!("member history read: {error}"))
    }
}

// ───────────────────── scripted member-turn responder ─────────────────────

/// Injectable delivery-fault state for [`ScriptedMemberTurnResponder`]
/// (the `drop_next_materialize_replies` template applied to the phase-6
/// directive-delivery family).
#[derive(Default)]
struct ScriptedMemberTurnState {
    drop_next_deliver_replies: u32,
    silence_next_deliveries_before_admission: u32,
    reject_next_deliver_unavailable: u32,
    reject_next_deliver_decode: bool,
    received: Vec<meerkat_mob::runtime::bridge_protocol::BridgeDeliveryPayload>,
    admitted: u64,
    /// Dedup memory keyed on `input_id`: a replayed delivery returns
    /// `Deduplicated` (the member-drain idempotency shape).
    accepted_input_ids: std::collections::BTreeSet<String>,
    emit_interaction_terminal: bool,
    terminal_rows: Vec<meerkat_mob::runtime::bridge_protocol::WireEventRow>,
    terminal_outcomes: Vec<meerkat_mob::runtime::bridge_protocol::BridgeTurnOutcomeRecord>,
    /// Exact materialized incarnation served before the first delivery.
    /// The event pump may poll immediately after placement, so it cannot
    /// derive this solely from a later delivery payload.
    expected_incarnation: Option<(u64, u64)>,
}

/// A scripted DeliverMemberInput/AuthorizeSupervisor responder over a
/// test-held [`PeerCommsEndpoint`] that IS the member (ADJ-P4-10
/// probe-as-member). Pairs with `ScriptedHostPeer::{script_member_identity,
/// bind_member_endpoint}` so materialize acks carry this endpoint's
/// identity. Pins the CONTROLLING side's directive-send policy (ADJ-4
/// single resend, decode-reject shape, unreachable-host exhaustion).
pub struct ScriptedMemberTurnResponder {
    state: Arc<std::sync::Mutex<ScriptedMemberTurnState>>,
    task: tokio::task::JoinHandle<()>,
}

impl ScriptedMemberTurnResponder {
    fn state(&self) -> std::sync::MutexGuard<'_, ScriptedMemberTurnState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Drop the next `n` delivery replies (served + recorded; the reply is
    /// lost — the ADJ-4 transient class the single resend covers).
    pub fn drop_next_deliver_replies(&self, n: u32) {
        self.state().drop_next_deliver_replies = n;
    }

    /// Consume the next `n` delivery requests without admitting member input
    /// and without replying. The controller sees an honestly ambiguous
    /// timeout while the fixture proves that no turn ran; this is the crash
    /// setup for recovered same-ID rejection tests.
    pub fn silence_next_deliveries_before_admission(&self, n: u32) {
        self.state().silence_next_deliveries_before_admission = n;
    }

    /// Reject the next `n` deliveries with the command-level wire
    /// `Unavailable` shape. Each rejection certifies that no member input was
    /// admitted; the controller must still apply ADJ-4's one exact resend
    /// while retaining custody afterward because this command-level shape is
    /// not durable no-effect proof.
    pub fn reject_next_deliver_unavailable(&self, n: u32) {
        self.state().reject_next_deliver_unavailable = n;
    }

    /// The next delivery replies with the pre-V4 decode-reject shape (a
    /// `deny_unknown_fields` receiver refusing a directive-bearing payload).
    pub fn reject_next_deliver_decode(&self) {
        self.state().reject_next_deliver_decode = true;
    }

    /// Every `BridgeDeliveryPayload` received, in order (attempt counting +
    /// byte-identical resend pins).
    pub fn received_deliveries(
        &self,
    ) -> Vec<meerkat_mob::runtime::bridge_protocol::BridgeDeliveryPayload> {
        self.state().received.clone()
    }

    /// Deliveries ADMITTED (accepted or deduplicated) — a decode-rejected
    /// delivery never counts (no turn ran).
    pub fn admitted_delivery_count(&self) -> u64 {
        self.state().admitted
    }

    /// Make each newly accepted plain delivery publish one durable
    /// InteractionComplete row keyed by its payload input UUID. This drives
    /// the production completion waiter/pump path while retaining delivery
    /// reply-loss injection.
    pub fn complete_plain_deliveries_via_event_pump(&self) {
        self.state().emit_interaction_terminal = true;
    }

    pub fn set_expected_incarnation(&self, generation: u64, fence_token: u64) {
        self.state().expected_incarnation = Some((generation, fence_token));
    }

    pub fn shutdown(&self) {
        self.task.abort();
    }

    /// Abort and join the responder before handing its endpoint back to a
    /// caller that will drain replies itself.
    pub async fn shutdown_and_join(self) {
        self.task.abort();
        let _ = self.task.await;
    }
}

/// Spawn the scripted member-turn responder loop over `endpoint`.
pub fn spawn_scripted_member_turn_responder(
    endpoint: Arc<PeerCommsEndpoint>,
) -> ScriptedMemberTurnResponder {
    use meerkat_mob::runtime::bridge_protocol::{
        BridgeAck, BridgeDeliveryOutcome, BridgeDeliveryResponse,
    };

    let state = Arc::new(std::sync::Mutex::new(ScriptedMemberTurnState::default()));
    let responder_state = Arc::clone(&state);
    let runtime_incarnation =
        meerkat_mob::runtime::bridge_protocol::BridgeHostRuntimeIncarnation::new();
    let task = tokio::spawn(async move {
        let runtime = Arc::clone(&endpoint.runtime);
        let notify = runtime.inbox_notify();
        loop {
            let notified = notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let candidates = runtime.drain_peer_input_candidates().await;
            if candidates.is_empty() {
                (&mut notified).await;
                continue;
            }
            for candidate in candidates {
                let InteractionContent::Request { params, .. } = &candidate.interaction.content
                else {
                    continue;
                };
                runtime
                    .peer_interaction_handle()
                    .expect("scripted member peer interaction authority")
                    .request_received(
                        PeerCorrelationId::from_uuid(candidate.interaction.id.0),
                        candidate.interaction.handling_mode,
                    )
                    .expect("record inbound peer request before response");

                let command: Result<BridgeCommand, _> = serde_json::from_value(params.clone());
                let reply_value: Option<serde_json::Value> = match command {
                    Ok(BridgeCommand::AuthorizeSupervisor(payload)) => {
                        let supervisor_spec =
                            TrustedPeerDescriptor::try_from(payload.supervisor.clone())
                                .expect("valid supervisor spec");
                        endpoint.trust(supervisor_spec).await;
                        Some(
                            serde_json::to_value(BridgeReply::Ack(BridgeAck { ok: true }))
                                .expect("authorize ack"),
                        )
                    }
                    Ok(BridgeCommand::DeliverMemberInput(payload)) => {
                        let supervisor_spec =
                            TrustedPeerDescriptor::try_from(payload.supervisor.clone())
                                .expect("valid supervisor spec");
                        endpoint.trust(supervisor_spec).await;
                        let mut guard = responder_state
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        guard.received.push(payload.clone());
                        if guard.silence_next_deliveries_before_admission > 0 {
                            guard.silence_next_deliveries_before_admission -= 1;
                            None
                        } else if guard.reject_next_deliver_unavailable > 0 {
                            guard.reject_next_deliver_unavailable -= 1;
                            Some(
                                serde_json::to_value(BridgeReply::Rejected {
                                    cause: BridgeRejectionCause::Unavailable,
                                    reason:
                                        "scripted receiver temporarily unavailable before admission"
                                            .to_string(),
                                })
                                .expect("unavailable rejection"),
                            )
                        } else if guard.reject_next_deliver_decode {
                            guard.reject_next_deliver_decode = false;
                            // The pre-V4 receiver shape: the directive-bearing
                            // payload fails deny_unknown_fields decode; the
                            // reply is a typed command-level rejection, never
                            // an undirected turn.
                            Some(
                                serde_json::to_value(BridgeReply::Rejected {
                                    cause: BridgeRejectionCause::Unsupported,
                                    reason: "unknown field `turn` at line 1 (pre-V4 receiver)"
                                        .to_string(),
                                })
                                .expect("decode rejection"),
                            )
                        } else {
                            guard.admitted += 1;
                            let newly_accepted =
                                guard.accepted_input_ids.insert(payload.input_id.clone());
                            let outcome = if newly_accepted {
                                BridgeDeliveryOutcome::Accepted
                            } else {
                                BridgeDeliveryOutcome::Deduplicated {
                                    existing_input_id: payload.input_id.clone(),
                                }
                            };
                            if newly_accepted && guard.emit_interaction_terminal {
                                let interaction_id = uuid::Uuid::parse_str(&payload.input_id)
                                    .expect("plain delivery test payload is a UUID");
                                let durable_seq = guard.terminal_rows.len() as u64 + 1;
                                let mob_id = payload
                                    .expected_member
                                    .as_ref()
                                    .map(|member| member.mob_id.clone());
                                guard.terminal_rows.push(
                                    meerkat_mob::runtime::bridge_protocol::WireEventRow {
                                        durable_seq,
                                        envelope: meerkat_core::EventEnvelope::new(
                                            "scripted-member",
                                            durable_seq,
                                            mob_id,
                                            meerkat_core::AgentEvent::InteractionComplete {
                                                interaction_id:
                                                    meerkat_core::interaction::InteractionId(
                                                        interaction_id,
                                                    ),
                                                result: "scripted-complete".to_string(),
                                                structured_output: None,
                                            },
                                        ),
                                    },
                                );
                                let expected_member = payload
                                    .expected_member
                                    .as_ref()
                                    .expect("tracked plain delivery carries an exact incarnation");
                                guard.terminal_outcomes.push(
                                    meerkat_mob::runtime::bridge_protocol::BridgeTurnOutcomeRecord {
                                        input_id: payload.input_id.clone(),
                                        generation: expected_member.generation,
                                        fence_token: expected_member.fence_token,
                                        terminal_seq: durable_seq,
                                        outcome: meerkat_mob::runtime::bridge_protocol::WireFlowTurnOutcome::InteractionComplete,
                                    },
                                );
                            }
                            if guard.drop_next_deliver_replies > 0 {
                                guard.drop_next_deliver_replies -= 1;
                                None
                            } else {
                                Some(
                                    serde_json::to_value(BridgeReply::Delivery(
                                        BridgeDeliveryResponse {
                                            input_id: payload.input_id.clone(),
                                            canonical_input_id: Some(payload.input_id.clone()),
                                            outcome,
                                        },
                                    ))
                                    .expect("delivery reply"),
                                )
                            }
                        }
                    }
                    Ok(BridgeCommand::PollMemberEvents(payload)) => {
                        let mut guard = responder_state
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        for ack in &payload.outcome_acks {
                            guard.terminal_outcomes.retain(|record| {
                                record.input_id != ack.input_id
                                    || record.generation != ack.generation
                                    || record.fence_token != ack.fence_token
                            });
                        }
                        let (generation, fence_token) = guard
                            .received
                            .last()
                            .and_then(|delivery| delivery.expected_member.as_ref())
                            .map(|member| (member.generation, member.fence_token))
                            .or(guard.expected_incarnation)
                            .unwrap_or((1, 1));
                        let start_seq = match payload.cursor {
                            meerkat_mob::runtime::bridge_protocol::BridgeEventCursor::Tail => guard
                                .terminal_rows
                                .last()
                                .map_or(1, |row| row.durable_seq.saturating_add(1)),
                            meerkat_mob::runtime::bridge_protocol::BridgeEventCursor::At {
                                generation: requested_generation,
                                seq,
                            } if requested_generation == generation => seq,
                            meerkat_mob::runtime::bridge_protocol::BridgeEventCursor::At {
                                ..
                            } => 1,
                        };
                        let max = payload.max.unwrap_or(256) as usize;
                        let events = guard
                            .terminal_rows
                            .iter()
                            .filter(|row| row.durable_seq >= start_seq)
                            .take(max)
                            .cloned()
                            .collect::<Vec<_>>();
                        let next_seq = events
                            .last()
                            .map_or(start_seq, |row| row.durable_seq.saturating_add(1));
                        let watermark = guard.terminal_rows.last().map_or(0, |row| row.durable_seq);
                        let max_outcomes = payload.max_outcomes.unwrap_or(256) as usize;
                        let turn_outcomes = guard
                            .terminal_outcomes
                            .iter()
                            .take(max_outcomes)
                            .cloned()
                            .collect::<Vec<_>>();
                        let outcomes_complete =
                            turn_outcomes.len() == guard.terminal_outcomes.len();
                        Some(
                            serde_json::to_value(BridgeReply::MemberEventsPage(
                                meerkat_mob::runtime::bridge_protocol::BridgeMemberEventsPage {
                                    runtime_incarnation,
                                    generation,
                                    fence_token,
                                    events,
                                    from_seq: start_seq,
                                    next_seq,
                                    watermark,
                                    turn_outcomes,
                                    outcomes_complete,
                                },
                            ))
                            .expect("scripted member events page"),
                        )
                    }
                    Ok(
                        BridgeCommand::OpenMemberLiveChannel(_)
                        | BridgeCommand::CloseMemberLiveChannel(_)
                        | BridgeCommand::MemberLiveChannelStatus(_)
                        | BridgeCommand::ControlMemberLiveChannel(_),
                    ) => Some(
                        serde_json::to_value(BridgeReply::Rejected {
                            cause: BridgeRejectionCause::LiveTransportUnavailable,
                            reason: "scripted member serves no live substrate".to_string(),
                        })
                        .expect("scripted absent live-substrate reply"),
                    ),
                    // Everything else is honestly Unsupported (a flow step
                    // resolves through the timeout ladder).
                    Ok(_) | Err(_) => Some(
                        serde_json::to_value(BridgeReply::Rejected {
                            cause: BridgeRejectionCause::Unsupported,
                            reason: "scripted member does not serve this bridge command"
                                .to_string(),
                        })
                        .expect("unsupported rejection"),
                    ),
                };

                let Some(reply_value) = reply_value else {
                    continue;
                };
                let reply_route = candidate.ingress.route.clone().or_else(|| {
                    candidate
                        .interaction
                        .from_route
                        .map(meerkat_core::PeerRoute::new)
                });
                let Some(route) = reply_route else {
                    continue;
                };
                let _ = runtime
                    .send(CommsCommand::PeerResponse {
                        to: route,
                        in_reply_to: candidate.interaction.id,
                        status: meerkat_core::ResponseStatus::Completed,
                        result: reply_value,
                        blocks: None,
                        content_taint: None,
                        handling_mode: None,
                        objective_id: candidate.interaction.objective_id,
                    })
                    .await;
            }
        }
    });

    ScriptedMemberTurnResponder { state, task }
}
