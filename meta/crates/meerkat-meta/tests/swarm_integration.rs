#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::uninlined_format_args,
    clippy::collapsible_if,
    clippy::redundant_clone,
    clippy::needless_raw_string_hashes,
    clippy::single_match,
    clippy::redundant_closure_for_method_calls,
    clippy::redundant_pattern_matching,
    clippy::ignored_unit_patterns,
    clippy::clone_on_copy,
    clippy::manual_assert,
    clippy::unwrap_in_result,
    clippy::useless_vec,
    clippy::unnecessary_semicolon
)]
use std::net::SocketAddr;
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use meerkat::{AgentFactory, Config, build_ephemeral_service};
use meerkat_client::TestClient;
use meerkat_core::{
    AppendSystemContextRequest, AppendSystemContextResult, CommsRuntime, CreateSessionRequest,
    EventStream, RunResult, SessionControlError, SessionError, SessionHistoryPage,
    SessionHistoryQuery, SessionId, SessionQuery, SessionService, SessionServiceCommsExt,
    SessionServiceControlExt, SessionServiceHistoryExt, SessionSummary, SessionView,
    StartTurnRequest, StreamError,
};
use meerkat_meta::{
    AuthPolicy, BigQueryNaming, ConfigResolutionError, ConsolePolicy, DiscoverySpec, EventEnvelope,
    LifecycleStage, MetaConfig, MetaRuntimeError, MobBootstrapOptions, MobBootstrapSpec,
    ModuleHealthState, ModuleRouteError, ModuleRouteRequest, ModuleRouteResponse,
    NormalizationError, RestartPolicy, RpcRouteError, RuntimeDecisionInputs, RuntimeOpsPolicy,
    RuntimeOptions, TrustedOidcRuntimeConfig, UnifiedEvent, UnifiedRuntime,
    UnifiedRuntimeBootstrapError, build_runtime_decision_state, normalize_event_line,
    route_module_call, route_module_call_rpc_json, route_module_call_rpc_subprocess,
    start_meta_runtime, start_meta_runtime_with_options,
};
use meerkat_mob::{
    AgentIdentity, MobDefinition, MobId, MobSessionService, MobState, MobStorage, ProfileName,
    SpawnMemberSpec,
};
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::oneshot;

fn shell_module(
    id: &str,
    script: &str,
    restart_policy: RestartPolicy,
) -> meerkat_meta::ModuleConfig {
    meerkat_meta::ModuleConfig {
        id: id.to_string(),
        command: "sh".to_string(),
        args: vec!["-c".to_string(), script.to_string()],
        restart_policy,
    }
}

struct UnifiedRuntimeFixture {
    _temp_dir: tempfile::TempDir,
    runtime: UnifiedRuntime,
}

#[derive(Clone)]
struct CheckpointerCancelProbeSessionService {
    inner: Arc<dyn MobSessionService>,
    cancel_calls: Arc<AtomicUsize>,
    /// Member-status observations. meerkat 0.8.45 reads member status
    /// through `observe_member_status_view`, not `read`: counted here, and
    /// held while `observations_blocked` is true.
    observe_calls: Arc<AtomicUsize>,
    observations_blocked: tokio::sync::watch::Sender<bool>,
}

impl CheckpointerCancelProbeSessionService {
    fn new(inner: Arc<dyn MobSessionService>) -> Self {
        Self {
            inner,
            cancel_calls: Arc::new(AtomicUsize::new(0)),
            observe_calls: Arc::new(AtomicUsize::new(0)),
            observations_blocked: tokio::sync::watch::channel(false).0,
        }
    }

    fn observe_calls(&self) -> usize {
        self.observe_calls.load(Ordering::SeqCst)
    }

    /// Hold every member-status observation until [`Self::release_observations`].
    fn block_observations(&self) {
        self.observations_blocked.send_replace(true);
    }

    fn release_observations(&self) {
        self.observations_blocked.send_replace(false);
    }

    fn cancel_calls(&self) -> usize {
        self.cancel_calls.load(Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl SessionService for CheckpointerCancelProbeSessionService {
    async fn create_session(&self, req: CreateSessionRequest) -> Result<RunResult, SessionError> {
        self.inner.create_session(req).await
    }

    async fn start_turn(
        &self,
        id: &SessionId,
        req: StartTurnRequest,
    ) -> Result<RunResult, SessionError> {
        self.inner.start_turn(id, req).await
    }

    async fn interrupt(&self, id: &SessionId) -> Result<(), SessionError> {
        self.inner.interrupt(id).await
    }

    // meerkat 0.8.22 adds the exact-run hard cancel to `SessionService` with an
    // `Err(Unsupported)` default. The inner ephemeral service implements it for
    // real, so inheriting the default here would let this probe answer
    // "unsupported" on its own authority and hide the inner service's exact-run
    // interrupt (same forwarding class as the lib's
    // `delegate_mob_session_service!`).
    async fn interrupt_run_if_current(
        &self,
        id: &SessionId,
        expected_run_id: &meerkat_core::lifecycle::RunId,
    ) -> Result<bool, SessionError> {
        self.inner
            .interrupt_run_if_current(id, expected_run_id)
            .await
    }

    async fn has_live_session(&self, id: &SessionId) -> Result<bool, SessionError> {
        self.inner.has_live_session(id).await
    }

    // Both cooperative boundary cancels default to `Err(Unsupported)` on the
    // trait. The provisioner's non-runtime interrupt path calls the plain form
    // on the service it holds (this wrapper) and absorbs only `NotRunning`, so
    // inheriting the default would refuse every cooperative interrupt on this
    // probe's own authority while the inner ephemeral service implements both.
    async fn cancel_after_boundary(&self, id: &SessionId) -> Result<(), SessionError> {
        self.inner.cancel_after_boundary(id).await
    }

    async fn cancel_after_boundary_for_run(
        &self,
        id: &SessionId,
        expected_run_id: &meerkat_core::lifecycle::RunId,
    ) -> Result<(), SessionError> {
        self.inner
            .cancel_after_boundary_for_run(id, expected_run_id)
            .await
    }

    async fn read(&self, id: &SessionId) -> Result<SessionView, SessionError> {
        self.inner.read(id).await
    }

    async fn list(&self, query: SessionQuery) -> Result<Vec<SessionSummary>, SessionError> {
        self.inner.list(query).await
    }

    async fn archive(&self, id: &SessionId) -> Result<(), SessionError> {
        self.inner.archive(id).await
    }

    async fn subscribe_session_events(&self, id: &SessionId) -> Result<EventStream, StreamError> {
        SessionService::subscribe_session_events(self.inner.as_ref(), id).await
    }
}

#[async_trait::async_trait]
impl SessionServiceCommsExt for CheckpointerCancelProbeSessionService {
    async fn comms_runtime(&self, session_id: &SessionId) -> Option<Arc<dyn CommsRuntime>> {
        self.inner.comms_runtime(session_id).await
    }
}

#[async_trait::async_trait]
impl SessionServiceControlExt for CheckpointerCancelProbeSessionService {
    async fn append_system_context(
        &self,
        id: &SessionId,
        req: AppendSystemContextRequest,
    ) -> Result<AppendSystemContextResult, SessionControlError> {
        self.inner.append_system_context(id, req).await
    }
}

#[async_trait::async_trait]
impl SessionServiceHistoryExt for CheckpointerCancelProbeSessionService {
    async fn read_history(
        &self,
        id: &SessionId,
        query: SessionHistoryQuery,
    ) -> Result<SessionHistoryPage, SessionError> {
        self.inner.read_history(id, query).await
    }
}

#[async_trait::async_trait]
impl MobSessionService for CheckpointerCancelProbeSessionService {
    async fn subscribe_session_activity(
        &self,
        session_id: &meerkat_core::SessionId,
    ) -> Result<meerkat_mob::MemberSessionActivity, meerkat_core::SessionError> {
        self.inner.subscribe_session_activity(session_id).await
    }

    // Forwarded to the wrapped service, like the production decorators
    // (required since meerkat 0.8.47).
    async fn observe_live_durable_source(
        &self,
        session_id: &meerkat_core::SessionId,
    ) -> Result<meerkat_mob::LiveDurableSourceObservation, meerkat_core::SessionError> {
        self.inner.observe_live_durable_source(session_id).await
    }

    async fn observe_member_status_view(
        &self,
        session_id: &meerkat_core::SessionId,
    ) -> Result<meerkat_mob::MemberStatusSessionView, meerkat_core::SessionError> {
        self.observe_calls.fetch_add(1, Ordering::SeqCst);
        let mut blocked = self.observations_blocked.subscribe();
        let _ = blocked.wait_for(|blocked| !*blocked).await;
        self.inner.observe_member_status_view(session_id).await
    }

    async fn commit_live_delegation_final_transcript(
        &self,
        machine: &meerkat_runtime::MeerkatMachine,
        session_id: &meerkat_core::SessionId,
        provisional: meerkat_core::ProvisionalLiveHandoff,
        final_event: meerkat_core::RealtimeTranscriptEvent,
    ) -> Result<meerkat_core::FinalLiveUserTranscriptCommitEvidence, SessionError> {
        self.inner
            .commit_live_delegation_final_transcript(machine, session_id, provisional, final_event)
            .await
    }

    // meerkat 0.8.41 made `fork_persisted_session_at_turn_boundary` REQUIRED so
    // every wrapper forwards the boundary-then-fork as ONE contract to the owner.
    // This decorator owns no session authority, so it forwards to `inner`.
    async fn commit_live_delegation_final_transcript_at_turn_boundary(
        &self,
        machine: &meerkat_runtime::MeerkatMachine,
        session_id: &meerkat_core::types::SessionId,
        provisional: meerkat_core::ProvisionalLiveHandoff,
        final_event: meerkat_core::RealtimeTranscriptEvent,
        bound: std::time::Duration,
    ) -> Result<meerkat_core::LiveFinalTranscriptCommitAtTurnBoundary, SessionError> {
        self.inner
            .commit_live_delegation_final_transcript_at_turn_boundary(
                machine,
                session_id,
                provisional,
                final_event,
                bound,
            )
            .await
    }

    async fn fork_persisted_session_at_turn_boundary(
        &self,
        source_session_id: &meerkat_core::types::SessionId,
        message_count: Option<usize>,
        tool_access_policy: Option<meerkat_core::ops::ToolAccessPolicy>,
        target: meerkat_core::DurableSessionForkTarget,
        bound: std::time::Duration,
    ) -> Result<meerkat_core::DurableForkAtTurnBoundary, SessionError> {
        self.inner
            .fork_persisted_session_at_turn_boundary(
                source_session_id,
                message_count,
                tool_access_policy,
                target,
                bound,
            )
            .await
    }

    // meerkat 0.8.30 made this REQUIRED, replacing a default that
    // answered "projected nothing" for any wrapper that did not
    // override it. This wrapper owns no session authority - it
    // decorates `inner` - so it forwards. Answering locally would
    // report nothing owing while a persistent inner had real work,
    // which is precisely the silent hole the required-method change
    // exists to remove.
    async fn enqueue_committed_parent_session_boundary_after_runtime_turn(
        &self,
        session_id: &meerkat_core::types::SessionId,
        runtime_adapter: &meerkat_runtime::MeerkatMachine,
    ) -> Result<usize, SessionError> {
        self.inner
            .enqueue_committed_parent_session_boundary_after_runtime_turn(
                session_id,
                runtime_adapter,
            )
            .await
    }

    // The verdict below is issued by `inner`, so the authority bundle it
    // carries must come from the SAME owner: the mob provisioner calls
    // `revalidate_session_resume_authority` on the service it holds (this
    // wrapper), and that trait default compares `self.observe_...` against the
    // bundle the verdict carried. Hardcoding an empty bundle here while
    // forwarding the verdict would reject every persistent-inner resume as
    // `AuthorityChangedDuringMaterialization`. Zero delta for the ephemeral
    // inner used today, whose own observation is exactly the empty bundle.
    async fn observe_session_resume_authority(
        &self,
        session_id: &meerkat::SessionId,
    ) -> Result<meerkat_mob::SessionResumeAuthority, meerkat::SessionError> {
        self.inner
            .observe_session_resume_authority(session_id)
            .await
    }

    // meerkat 0.8.22 DELETED the required `prepare_session_for_resume` hook
    // this probe used to pass through. Its durable-tail convergence now lives
    // only inside `PersistentSessionService`'s override of
    // `materialize_session_resume_verdict`; the TRAIT DEFAULT of that method is
    // the non-persistent composition (bracketed read-only observations plus a
    // `NonPersistent` receipt, converging nothing). Dropping the dead
    // passthrough and stopping there would compile while silently resuming a
    // persistent inner from stale committed authority, so the delegation moves
    // to the method that now owns convergence. This wrapper only counts
    // `cancel_all_checkpointers`; it has no projection of its own to re-apply
    // to the authorized body, so the verdict is forwarded verbatim.
    //
    // The receipt this verdict now carries is consumed by the two
    // `..._actor_witness_...` creates, which this probe has never forwarded
    // (pre-existing at 0.8.21, when they took no receipt). Left inheriting
    // their defaults deliberately: both refuse with `Err(Unsupported)`, and
    // `execute_session_actor_materialization_under_runtime_turn_boundary` is
    // the sole lowering owner with no non-witness fallback, so an unconsumed
    // receipt fails loudly instead of being silently dropped. This probe's
    // bootstrap-failure assertion never reaches actor materialization anyway.
    async fn materialize_session_resume_verdict(
        &self,
        session_id: &SessionId,
    ) -> Result<meerkat_mob::SessionResumeVerdict, SessionError> {
        self.inner
            .materialize_session_resume_verdict(session_id)
            .await
    }

    async fn create_session_under_runtime_turn_boundary(
        &self,
        req: CreateSessionRequest,
    ) -> Result<RunResult, SessionError> {
        self.inner
            .create_session_under_runtime_turn_boundary(req)
            .await
    }

    async fn create_session_with_actor_witness_under_runtime_turn_boundary(
        &self,
        req: CreateSessionRequest,
        resume_preparation: Option<meerkat_mob::SessionResumePreparationReceipt>,
        actor_witness_slot: &meerkat_session::LiveSessionActorWitnessSlot,
    ) -> Result<RunResult, SessionError> {
        self.inner
            .create_session_with_actor_witness_under_runtime_turn_boundary(
                req,
                resume_preparation,
                actor_witness_slot,
            )
            .await
    }

    async fn acknowledge_committed_runtime_session_boundary_under_turn_finalization_boundary(
        &self,
        session_id: &SessionId,
        authority: &meerkat_core::CommittedSessionBoundaryAuthority,
    ) -> Result<(), SessionError> {
        self.inner
            .acknowledge_committed_runtime_session_boundary_under_turn_finalization_boundary(
                session_id, authority,
            )
            .await
    }

    async fn load_session_for_resume(
        &self,
        session_id: &SessionId,
    ) -> Result<meerkat_mob::ResumeSessionLoad, SessionError> {
        self.inner.load_session_for_resume(session_id).await
    }

    // meerkat 0.8.22: durable transcript fork moved onto the session service
    // with an `Err(Unsupported)` default. Not forwarding would make this probe
    // claim a persistent inner has no fork authority at all.
    async fn fork_persisted_session(
        &self,
        source_session_id: &SessionId,
        message_count: Option<usize>,
        tool_access_policy: Option<meerkat_core::ops::ToolAccessPolicy>,
        target: meerkat_core::DurableSessionForkTarget,
    ) -> Result<meerkat_core::SessionForkResult, SessionError> {
        self.inner
            .fork_persisted_session(source_session_id, message_count, tool_access_policy, target)
            .await
    }

    // meerkat 0.8.22: exact-run machine-authorized hard cancel, trait default
    // `Err(Unsupported)`. The mob provisioner's interrupt handle calls this on
    // the service it was handed (this wrapper) and absorbs only `NotRunning`,
    // so an unforwarded default turns every exact-run cancel into a control
    // failure even though the ephemeral inner implements it.
    async fn interrupt_run_with_machine_authority(
        &self,
        session_id: &SessionId,
        expected_run_id: &meerkat_core::lifecycle::RunId,
        authority: meerkat_runtime::MachineSessionControlAuthority,
    ) -> Result<bool, SessionError> {
        self.inner
            .interrupt_run_with_machine_authority(session_id, expected_run_id, authority)
            .await
    }

    // The three machine-authority siblings of the forward above. All default
    // to `Err(Unsupported)` on the trait, and MeerkatMachine dispatches them to
    // the service it was handed at bootstrap - this wrapper - after it has
    // ALREADY admitted the cancel. The one that bit: `MobHandle::stop` on a
    // member whose autonomous kickoff turn is still running issues a boundary
    // cancel for that live run; the machine admits it and calls
    // `cancel_after_boundary_with_machine_authority` here. With the default,
    // the probe answered "unsupported" on its own authority and `stop()` failed
    // with `CancelAfterBoundary: failed to apply live boundary cancel: Control
    // failed: unsupported: ...` (#401). Whether the kickoff is still running
    // when the test reaches `stop()` depends on runner load, which is why it
    // passed 1/1 in isolation and failed roughly one CI run in two. Same
    // forwarding class as the lib's `delegate_mob_session_service!`.
    async fn interrupt_with_machine_authority(
        &self,
        session_id: &SessionId,
        authority: meerkat_runtime::MachineSessionControlAuthority,
    ) -> Result<(), SessionError> {
        self.inner
            .interrupt_with_machine_authority(session_id, authority)
            .await
    }

    // Mirrors the lib's `cancel_after_boundary_with_machine_authority_if_live`:
    // once the machine has admitted the cancel, a run that finished (or a
    // session that went away) between admission and dispatch proves there is
    // nothing left to interrupt. Propagating `NotRunning`/`NotFound` back as a
    // failed effect would wedge the machine in its pre-cancel phase and turn the
    // kickoff-completes-mid-stop ordering into a second flake of the same test.
    async fn cancel_after_boundary_with_machine_authority(
        &self,
        session_id: &SessionId,
        expected_run_id: &meerkat_core::lifecycle::RunId,
        authority: meerkat_runtime::MachineSessionControlAuthority,
    ) -> Result<(), SessionError> {
        self.inner
            .cancel_after_boundary_with_machine_authority(session_id, expected_run_id, authority)
            .await
            .or_else(|error| match error {
                SessionError::NotFound { .. } | SessionError::NotRunning { .. } => Ok(()),
                error => Err(error),
            })
    }

    async fn cancel_current_after_boundary_with_machine_authority(
        &self,
        session_id: &SessionId,
        authority: meerkat_runtime::MachineSessionControlAuthority,
    ) -> Result<(), SessionError> {
        self.inner
            .cancel_current_after_boundary_with_machine_authority(session_id, authority)
            .await
    }

    async fn archive_with_mob_lifecycle_authority_under_runtime_turn_boundary(
        &self,
        session_id: &SessionId,
    ) -> Result<(), SessionError> {
        self.inner
            .archive_with_mob_lifecycle_authority_under_runtime_turn_boundary(session_id)
            .await
    }

    // meerkat 0.8.22: member-session disposal moved from the boundary archive
    // above to this deadline-aware sibling, whose trait default is
    // `Err(Unsupported)`. The ephemeral inner implements it explicitly, so an
    // unforwarded default would make every member retirement archive through
    // this probe fail and wedge the roster anchor in `retiring`.
    async fn archive_with_mob_lifecycle_authority_under_runtime_turn_boundary_before(
        &self,
        session_id: &SessionId,
        deadline: meerkat_core::time_compat::Instant,
    ) -> Result<(), SessionError> {
        self.inner
            .archive_with_mob_lifecycle_authority_under_runtime_turn_boundary_before(
                session_id, deadline,
            )
            .await
    }

    async fn discard_live_session_under_runtime_turn_boundary(
        &self,
        session_id: &SessionId,
    ) -> Result<(), SessionError> {
        self.inner
            .discard_live_session_under_runtime_turn_boundary(session_id)
            .await
    }

    fn supports_persistent_sessions(&self) -> bool {
        self.inner.supports_persistent_sessions()
    }

    fn acquire_runtime_adapter(
        &self,
        explicit: Option<Arc<meerkat_runtime::MeerkatMachine>>,
    ) -> Result<Option<Arc<meerkat_runtime::MeerkatMachine>>, meerkat_runtime::RuntimeDriverError>
    {
        self.inner.acquire_runtime_adapter(explicit)
    }

    async fn session_belongs_to_mob(&self, session_id: &SessionId, mob_id: &MobId) -> bool {
        self.inner.session_belongs_to_mob(session_id, mob_id).await
    }

    async fn cancel_all_checkpointers(&self) {
        self.cancel_calls.fetch_add(1, Ordering::SeqCst);
        self.inner.cancel_all_checkpointers().await;
    }

    async fn rearm_all_checkpointers(&self) {
        self.inner.rearm_all_checkpointers().await;
    }
}

fn build_phase1_session_service(temp_dir: &tempfile::TempDir) -> Arc<dyn MobSessionService> {
    build_phase1_session_service_with_capacity(temp_dir, 16)
}

fn build_phase1_session_service_with_capacity(
    temp_dir: &tempfile::TempDir,
    max_sessions: usize,
) -> Arc<dyn MobSessionService> {
    let session_path = temp_dir.path().join("sessions");
    std::fs::create_dir_all(&session_path).expect("session path");

    let factory = AgentFactory::new(&session_path).comms(true);
    Arc::new(build_ephemeral_service(
        factory,
        Config::default(),
        max_sessions,
    ))
}

fn build_phase1_mob_spec_with_session_service(
    session_service: Arc<dyn MobSessionService>,
) -> MobBootstrapSpec {
    let definition = MobDefinition::from_toml(
        r#"
[mob]
id = "phase1-unified-mob"

[profiles.worker]
model = "gpt-5.5"
external_addressable = true

[profiles.worker.tools]
comms = true
"#,
    )
    .expect("parse mob definition");

    MobBootstrapSpec::new(definition, MobStorage::in_memory(), session_service).with_options(
        MobBootstrapOptions {
            allow_ephemeral_sessions: true,
            notify_orchestrator_on_resume: true,
            default_llm_client: Some(Arc::new(TestClient::default())),
        },
    )
}

fn build_phase1_mob_spec(temp_dir: &tempfile::TempDir) -> MobBootstrapSpec {
    build_phase1_mob_spec_with_session_service(build_phase1_session_service(temp_dir))
}

fn reference_runtime_decision_state() -> meerkat_meta::RuntimeDecisionState {
    build_runtime_decision_state(RuntimeDecisionInputs {
        bigquery: BigQueryNaming {
            dataset: "phase1_reference_dataset".to_string(),
            table: "phase1_reference_table".to_string(),
        },
        trusted_meta_toml: trusted_modules_toml(),
        auth: AuthPolicy::default(),
        trusted_oidc: trusted_oidc(),
        console: ConsolePolicy {
            require_app_auth: false,
            ..ConsolePolicy::default()
        },
        ops: RuntimeOpsPolicy::default(),
        release_metadata_json: include_str!("../assets/release-targets.json").to_string(),
    })
    .expect("build decision state")
}

fn trusted_modules_toml() -> String {
    r#"
[[modules]]
id = "router"
command = "router-bin"
args = ["--mode", "fast"]
restart_policy = "always"
"#
    .to_string()
}

fn trusted_oidc() -> TrustedOidcRuntimeConfig {
    TrustedOidcRuntimeConfig {
        discovery_json:
            r#"{"issuer":"https://trusted.meta.local","jwks_uri":"https://trusted.meta.local/.well-known/jwks.json"}"#
                .to_string(),
        jwks_json: r#"{"keys":[{"kid":"kid-current","kty":"oct","alg":"HS256","k":"cGhhc2U3LXRydXN0ZWQtY3VycmVudC1zZWNyZXQ"}]}"#
            .to_string(),
        audience: "meerkat-console".to_string(),
        require_verified_email: false,
    }
}

async fn http_get_response(address: SocketAddr, path: &str) -> String {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        match TcpStream::connect(address).await {
            Ok(mut stream) => {
                let request =
                    format!("GET {path} HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n\r\n");
                stream
                    .write_all(request.as_bytes())
                    .await
                    .expect("write request");
                stream.flush().await.expect("flush request");

                let mut bytes = Vec::new();
                tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut bytes))
                    .await
                    .expect("read response within timeout")
                    .expect("read response bytes");
                return String::from_utf8(bytes).expect("utf8 response");
            }
            Err(error) => {
                assert!(
                    tokio::time::Instant::now() < deadline,
                    "failed to connect to reference app at {address}: {error}"
                );
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        }
    }
}

async fn build_unified_runtime_fixture(module_config: MetaConfig) -> UnifiedRuntimeFixture {
    Box::pin(build_unified_runtime_fixture_with_agent_events(
        module_config,
        Vec::new(),
    ))
    .await
}

async fn build_unified_runtime_fixture_with_agent_events(
    module_config: MetaConfig,
    module_agent_events: Vec<EventEnvelope<UnifiedEvent>>,
) -> UnifiedRuntimeFixture {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let runtime = Box::pin(
        UnifiedRuntime::builder()
            .mob_spec(build_phase1_mob_spec(&temp_dir))
            .module_config(module_config)
            .module_agent_events(module_agent_events)
            .timeout(Duration::from_secs(2))
            .build(),
    )
    .await
    .expect("bootstrap unified runtime");

    UnifiedRuntimeFixture {
        _temp_dir: temp_dir,
        runtime,
    }
}

/// meerkat 0.8.45's member-status lane, end to end through
/// `meta/member_status` (it replaced the one-permit fail-fast lane whose
/// refusals and 30 s timeouts the previous version of this test pinned):
///
/// - concurrent reads of ONE member join one observation. A session view
///   that does not answer ends at the 1 s observation deadline with the typed
///   `preview_unavailable: "observation_deadline"` marker, as a snapshot
///   rather than an error, and the inner service is read exactly once;
/// - reads of different members run concurrently under a mob-wide capacity
///   of 16. A read the session never answers keeps its unit until it
///   finishes or is orphaned at the drain ceiling, so with all 16 held a
///   17th member's read waits the bounded 2 s for capacity and is refused
///   typed with `observation_lane_saturated`, without ever reading its
///   session.
#[tokio::test]
async fn member_status_reads_join_one_bounded_observation_and_wait_boundedly_for_capacity() {
    const CAPACITY: usize = 16;
    const JOINED_REQUESTS: usize = 9;

    let temp_dir = tempfile::tempdir().expect("temp dir");
    // One more member than the member-status capacity, so the session cap
    // must not be what refuses.
    let probe = Arc::new(CheckpointerCancelProbeSessionService::new(
        build_phase1_session_service_with_capacity(&temp_dir, CAPACITY + 4),
    ));
    let session_service: Arc<dyn MobSessionService> = probe.clone();
    let runtime = Arc::new(
        UnifiedRuntime::bootstrap(
            build_phase1_mob_spec_with_session_service(session_service),
            MetaConfig {
                modules: vec![],
                discovery: DiscoverySpec {
                    namespace: "bounded-member-status".to_string(),
                    modules: vec![],
                },
                pre_spawn: vec![],
            },
            Duration::from_secs(1),
        )
        .await
        .expect("bootstrap unified runtime"),
    );
    let members: Vec<AgentIdentity> = (0..=CAPACITY)
        .map(|index| AgentIdentity::from(format!("status-worker-{index}")))
        .collect();
    for member in &members {
        runtime
            .mob_handle()
            .spawn_spec(SpawnMemberSpec::new(
                ProfileName::from("worker"),
                member.clone(),
            ))
            .await
            .expect("spawn status worker");
    }

    tokio::time::pause();
    let status_request = |member: &AgentIdentity, id: usize| {
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "meta/member_status",
            "params": {"member_id": member.as_str()},
        })
        .to_string()
    };
    let spawn_request = |runtime: Arc<UnifiedRuntime>, request: String| {
        tokio::spawn(async move {
            let response = meerkat_meta::rpc::handle_unified_rpc_json(
                runtime.as_ref(),
                &request,
                Duration::from_secs(10),
                None,
                None,
            )
            .await;
            serde_json::from_str::<serde_json::Value>(&response).expect("json-rpc response")
        })
    };

    // Joined reads: every request for one member shares one observation.
    probe.block_observations();
    let joined: Vec<_> = (0..JOINED_REQUESTS)
        .map(|id| spawn_request(Arc::clone(&runtime), status_request(&members[0], id)))
        .collect();
    for task in joined {
        let response = task.await.expect("joined request task");
        assert!(response.get("error").is_none(), "{response:#?}");
        assert_eq!(
            response["result"]["preview_unavailable"],
            json!("observation_deadline"),
            "{response:#?}"
        );
    }
    assert_eq!(
        probe.observe_calls(),
        1,
        "concurrent reads of one member join one observation"
    );

    // Bounded capacity: the first member's unanswered read still holds its
    // unit; 15 more members' unanswered reads take the rest.
    let holders: Vec<_> = members[1..CAPACITY]
        .iter()
        .enumerate()
        .map(|(id, member)| spawn_request(Arc::clone(&runtime), status_request(member, 100 + id)))
        .collect();
    for task in holders {
        let response = task.await.expect("capacity holder task");
        assert_eq!(
            response["result"]["preview_unavailable"],
            json!("observation_deadline"),
            "{response:#?}"
        );
    }
    assert_eq!(probe.observe_calls(), CAPACITY);
    let started = tokio::time::Instant::now();
    let refused = spawn_request(
        Arc::clone(&runtime),
        status_request(&members[CAPACITY], 999),
    )
    .await
    .expect("over-capacity request task");
    let waited = started.elapsed();
    assert_eq!(refused["error"]["code"], json!(-32000), "{refused:#?}");
    assert!(
        refused["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("observation_lane_saturated")),
        "{refused:#?}"
    );
    assert!(
        waited >= Duration::from_secs(2),
        "the refusal comes only after the bounded capacity wait: {waited:?}"
    );
    assert_eq!(
        probe.observe_calls(),
        CAPACITY,
        "a read refused for capacity never reads its session"
    );

    // Released, the held reads drain, and a fresh read is answered from the
    // session.
    probe.release_observations();
    let answered = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let response = spawn_request(
                Arc::clone(&runtime),
                status_request(&members[CAPACITY], 1_000),
            )
            .await
            .expect("post-release request task");
            if response.get("error").is_none() {
                return response;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("capacity frees once the held reads drain");
    assert!(
        answered["result"]["preview_unavailable"].is_null(),
        "{answered:#?}"
    );
}

#[tokio::test]
#[ignore]
async fn unified_bootstrap_failure_rolls_back_started_mob_runtime() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let probe = Arc::new(CheckpointerCancelProbeSessionService::new(
        build_phase1_session_service(&temp_dir),
    ));
    let session_service: Arc<dyn MobSessionService> = probe.clone();

    let config = MetaConfig {
        modules: vec![],
        discovery: DiscoverySpec {
            namespace: "phase1-bootstrap-failure".to_string(),
            modules: vec!["missing-module".to_string()],
        },
        pre_spawn: vec![],
    };

    let error = match UnifiedRuntime::bootstrap(
        build_phase1_mob_spec_with_session_service(session_service),
        config,
        Duration::from_millis(250),
    )
    .await
    {
        Ok(_) => panic!("bootstrap should fail when discovered module is not configured"),
        Err(error) => error,
    };

    assert!(matches!(
        error,
        UnifiedRuntimeBootstrapError::Module(MetaRuntimeError::Config(
            ConfigResolutionError::ModuleNotConfigured(module_id)
        )) if module_id == "missing-module"
    ));
    assert_eq!(
        probe.cancel_calls(),
        1,
        "module bootstrap failure must rollback started mob runtime"
    );
}

#[test]
#[ignore]
fn req_001_startup_ordering_and_graceful_shutdown_kills_tracked_children() {
    let config = MetaConfig {
        modules: vec![shell_module(
            "mod-a",
            r#"printf '%s\n' "{\"event_id\":\"mod-a-evt\",\"source\":\"module\",\"timestamp_ms\":20,\"event\":{\"kind\":\"module\",\"module\":\"mod-a\",\"event_type\":\"ready\",\"payload\":{\"ok\":true,\"pid\":$$}}}"; exec sleep 30"#,
            RestartPolicy::Never,
        )],
        discovery: meerkat_meta::DiscoverySpec {
            namespace: "default".to_string(),
            modules: vec!["mod-a".to_string()],
        },
        pre_spawn: vec![],
    };

    let mut runtime =
        start_meta_runtime(config, vec![], Duration::from_secs(1)).expect("runtime starts");

    assert_eq!(
        runtime
            .lifecycle_events()
            .iter()
            .map(|event| event.stage.clone())
            .collect::<Vec<_>>(),
        vec![
            LifecycleStage::MobStarted,
            LifecycleStage::ModulesStarted,
            LifecycleStage::MergedStreamStarted,
        ]
    );

    let pid = runtime
        .merged_events()
        .iter()
        .find_map(|event| match &event.event {
            UnifiedEvent::Module(module) if module.module == "mod-a" => {
                module.payload.get("pid").and_then(|value| value.as_i64())
            }
            _ => None,
        })
        .expect("module pid should be present in payload");

    let shutdown = runtime.shutdown();
    assert_eq!(shutdown.orphan_processes, 0);
    assert_eq!(shutdown.terminated_modules, vec!["mod-a".to_string()]);
    assert!(!runtime.is_running());

    // OS-level proof that the module process is not alive after shutdown.
    let kill_status = Command::new("sh")
        .args(["-c", &format!("kill -0 {pid}")])
        .status()
        .expect("run kill -0");
    assert!(
        !kill_status.success(),
        "module process {pid} is still alive after shutdown"
    );

    assert_eq!(
        runtime
            .lifecycle_events()
            .iter()
            .map(|event| event.stage.clone())
            .collect::<Vec<_>>(),
        vec![
            LifecycleStage::MobStarted,
            LifecycleStage::ModulesStarted,
            LifecycleStage::MergedStreamStarted,
            LifecycleStage::ShutdownRequested,
            LifecycleStage::ShutdownComplete,
        ]
    );
}

#[test]
#[ignore]
fn req_002_supervisor_transitions_and_restart_policy_enforced_with_budgets() {
    let temp = tempfile::tempdir().expect("temp dir");
    let on_failure_state = temp.path().join("on-failure-state");

    let on_failure_script = format!(
        "if [ ! -f '{}' ]; then echo first > '{}'; exit 1; fi; if ! grep -q second '{}'; then echo second > '{}'; exit 1; fi; printf '%s\\n' '{{\"event_id\":\"on-failure-healthy\",\"source\":\"module\",\"timestamp_ms\":30,\"event\":{{\"kind\":\"module\",\"module\":\"on-failure\",\"event_type\":\"ready\",\"payload\":{{\"attempt\":3}}}}}}'",
        on_failure_state.display(),
        on_failure_state.display(),
        on_failure_state.display(),
        on_failure_state.display()
    );

    let always_script = "printf '%s\\n' '{\"event_id\":\"always-healthy\",\"source\":\"module\",\"timestamp_ms\":31,\"event\":{\"kind\":\"module\",\"module\":\"always\",\"event_type\":\"ready\",\"payload\":{\"attempt\":1}}}'".to_string();

    let config = MetaConfig {
        modules: vec![
            shell_module("never", "exit 1", RestartPolicy::Never),
            shell_module("on-failure", &on_failure_script, RestartPolicy::OnFailure),
            shell_module("always", &always_script, RestartPolicy::Always),
        ],
        discovery: meerkat_meta::DiscoverySpec {
            namespace: "default".to_string(),
            modules: vec![
                "never".to_string(),
                "on-failure".to_string(),
                "always".to_string(),
            ],
        },
        pre_spawn: vec![],
    };

    let runtime = start_meta_runtime_with_options(
        config,
        vec![],
        Duration::from_secs(1),
        RuntimeOptions {
            on_failure_retry_budget: 2,
            always_restart_budget: 2,
            ..RuntimeOptions::default()
        },
    )
    .expect("runtime starts with supervisor transitions");

    let never = runtime
        .supervisor_report()
        .transitions
        .iter()
        .filter(|transition| transition.module_id == "never")
        .map(|transition| transition.to.clone())
        .collect::<Vec<_>>();
    assert_eq!(
        never,
        vec![
            ModuleHealthState::Starting,
            ModuleHealthState::Failed,
            ModuleHealthState::Stopped,
        ]
    );

    let on_failure = runtime
        .supervisor_report()
        .transitions
        .iter()
        .filter(|transition| transition.module_id == "on-failure")
        .map(|transition| transition.to.clone())
        .collect::<Vec<_>>();
    assert_eq!(
        on_failure,
        vec![
            ModuleHealthState::Starting,
            ModuleHealthState::Failed,
            ModuleHealthState::Restarting,
            ModuleHealthState::Failed,
            ModuleHealthState::Restarting,
            ModuleHealthState::Healthy,
        ]
    );

    let always = runtime
        .supervisor_report()
        .transitions
        .iter()
        .filter(|transition| transition.module_id == "always")
        .map(|transition| transition.to.clone())
        .collect::<Vec<_>>();
    assert_eq!(
        always,
        vec![
            ModuleHealthState::Starting,
            ModuleHealthState::Healthy,
            ModuleHealthState::Restarting,
            ModuleHealthState::Healthy,
            ModuleHealthState::Restarting,
            ModuleHealthState::Healthy,
        ]
    );
}

#[test]
#[ignore]
fn req_003_event_bus_merges_agent_and_module_events_with_deterministic_order() {
    let config = MetaConfig {
        modules: vec![
            shell_module(
                "mod-a",
                r#"printf '%s\n' '{"event_id":"evt-module-a","source":"module","timestamp_ms":20,"event":{"kind":"module","module":"mod-a","event_type":"ready","payload":{"m":"a"}}}'"#,
                RestartPolicy::Never,
            ),
            shell_module(
                "mod-b",
                r#"printf '%s\n' '{"event_id":"evt-module-b","source":"module","timestamp_ms":20,"event":{"kind":"module","module":"mod-b","event_type":"ready","payload":{"m":"b"}}}'"#,
                RestartPolicy::Never,
            ),
        ],
        discovery: meerkat_meta::DiscoverySpec {
            namespace: "default".to_string(),
            modules: vec!["mod-a".to_string(), "mod-b".to_string()],
        },
        pre_spawn: vec![],
    };

    let agent_events = vec![
        EventEnvelope {
            event_id: "evt-agent-early".to_string(),
            source: "agent".to_string(),
            timestamp_ms: 10,
            event: UnifiedEvent::Agent {
                agent_id: "a-1".to_string(),
                event_type: "heartbeat".to_string(),
                payload: None,
            },
        },
        EventEnvelope {
            event_id: "evt-agent-mid".to_string(),
            source: "agent".to_string(),
            timestamp_ms: 20,
            event: UnifiedEvent::Agent {
                agent_id: "a-2".to_string(),
                event_type: "heartbeat".to_string(),
                payload: None,
            },
        },
    ];

    let runtime =
        start_meta_runtime(config, agent_events, Duration::from_secs(1)).expect("runtime starts");

    assert_eq!(
        runtime
            .merged_events()
            .iter()
            .map(|event| event.event_id.clone())
            .collect::<Vec<_>>(),
        vec![
            "evt-agent-early".to_string(),
            "evt-agent-mid".to_string(),
            "evt-module-a".to_string(),
            "evt-module-b".to_string(),
        ]
    );
    assert!(matches!(
        runtime.merged_events()[0].event,
        UnifiedEvent::Agent { .. }
    ));
    assert!(matches!(
        runtime.merged_events()[2].event,
        UnifiedEvent::Module(_)
    ));
}

#[test]
fn req_003_attribution_integrity_rejects_source_event_mismatch() {
    let mismatched = json!({
        "event_id": "evt-bad",
        "source": "agent",
        "timestamp_ms": 7,
        "event": {
            "kind": "module",
            "module": "mod-x",
            "event_type": "ready",
            "payload": {"ok": true}
        }
    })
    .to_string();

    let err = normalize_event_line(&mismatched).expect_err("mismatch should fail");
    assert_eq!(
        err,
        NormalizationError::SourceMismatch {
            expected: "module",
            got: "agent".to_string(),
        }
    );
}

#[test]
#[ignore]
fn req_004_and_req_005_router_parity_library_and_rpc_with_typed_unloaded_error() {
    let config = MetaConfig {
        modules: vec![shell_module(
            "router-mod",
            r#"printf '%s\n' '{"event_id":"evt-router","source":"module","timestamp_ms":55,"event":{"kind":"module","module":"router-mod","event_type":"response","payload":{"ok":true,"via":"module"}}}'"#,
            RestartPolicy::Never,
        )],
        discovery: meerkat_meta::DiscoverySpec {
            namespace: "default".to_string(),
            modules: vec!["router-mod".to_string()],
        },
        pre_spawn: vec![],
    };

    let runtime =
        start_meta_runtime(config, vec![], Duration::from_secs(1)).expect("runtime starts");

    let request = ModuleRouteRequest {
        module_id: "router-mod".to_string(),
        method: "echo".to_string(),
        params: json!({"msg":"hello"}),
    };

    let library_response = route_module_call(&runtime, &request, Duration::from_secs(1))
        .expect("library router call succeeds");
    assert_eq!(library_response.module_id, "router-mod");
    assert_eq!(library_response.method, "echo");
    assert_eq!(
        library_response.payload,
        json!({"ok": true, "via": "module"})
    );

    let rpc_response_json = route_module_call_rpc_json(
        &runtime,
        &serde_json::to_string(&request).expect("serialize request"),
        Duration::from_secs(1),
    )
    .expect("rpc wrapper succeeds");
    let rpc_response: ModuleRouteResponse =
        serde_json::from_str(&rpc_response_json).expect("deserialize rpc response");
    assert_eq!(rpc_response.module_id, "router-mod");
    assert_eq!(rpc_response.method, "echo");
    assert_eq!(rpc_response.payload, json!({"ok": true, "via": "module"}));

    let rpc_subprocess_response_json = route_module_call_rpc_subprocess(
        &runtime,
        "sh",
        &[
            "-c".to_string(),
            format!(
                "printf '%s\\n' '{}'",
                serde_json::to_string(&request).expect("serialize request")
            ),
        ],
        &[],
        Duration::from_secs(1),
    )
    .expect("rpc subprocess boundary succeeds");
    let rpc_subprocess_response: ModuleRouteResponse =
        serde_json::from_str(&rpc_subprocess_response_json)
            .expect("deserialize subprocess rpc response");
    assert_eq!(rpc_subprocess_response.module_id, "router-mod");
    assert_eq!(rpc_subprocess_response.method, "echo");
    assert_eq!(
        rpc_subprocess_response.payload,
        json!({"ok": true, "via": "module"})
    );

    let missing = ModuleRouteRequest {
        module_id: "missing".to_string(),
        method: "echo".to_string(),
        params: json!({}),
    };

    let library_error = route_module_call(&runtime, &missing, Duration::from_secs(1))
        .expect_err("missing module should fail");
    assert_eq!(
        library_error,
        ModuleRouteError::UnloadedModule("missing".to_string())
    );

    let rpc_error = route_module_call_rpc_json(
        &runtime,
        &serde_json::to_string(&missing).expect("serialize request"),
        Duration::from_secs(1),
    )
    .expect_err("rpc missing module should fail");
    assert_eq!(
        rpc_error,
        RpcRouteError::Route(ModuleRouteError::UnloadedModule("missing".to_string()))
    );

    let rpc_subprocess_error = route_module_call_rpc_subprocess(
        &runtime,
        "sh",
        &[
            "-c".to_string(),
            format!(
                "printf '%s\\n' '{}'",
                serde_json::to_string(&missing).expect("serialize request")
            ),
        ],
        &[],
        Duration::from_secs(1),
    )
    .expect_err("rpc subprocess missing module should fail");
    assert_eq!(
        rpc_subprocess_error,
        RpcRouteError::Route(ModuleRouteError::UnloadedModule("missing".to_string()))
    );
}

#[test]
fn req_001_config_error_when_discovery_references_unknown_module() {
    let config = MetaConfig {
        modules: vec![],
        discovery: meerkat_meta::DiscoverySpec {
            namespace: "default".to_string(),
            modules: vec!["ghost".to_string()],
        },
        pre_spawn: vec![],
    };

    let error = start_meta_runtime(config, vec![], Duration::from_secs(1))
        .expect_err("unknown module should fail startup");
    assert_eq!(
        error,
        meerkat_meta::MetaRuntimeError::Config(ConfigResolutionError::ModuleNotConfigured(
            "ghost".to_string()
        ))
    );
}

#[tokio::test]
#[ignore]
async fn req_001_unified_owner_starts_and_shuts_down_from_single_object() {
    let config = MetaConfig {
        modules: vec![shell_module(
            "mod-a",
            r#"printf '%s\n' "{\"event_id\":\"mod-a-unified\",\"source\":\"module\",\"timestamp_ms\":20,\"event\":{\"kind\":\"module\",\"module\":\"mod-a\",\"event_type\":\"ready\",\"payload\":{\"ok\":true}}}"; exec sleep 30"#,
            RestartPolicy::Never,
        )],
        discovery: DiscoverySpec {
            namespace: "phase1-unified".to_string(),
            modules: vec!["mod-a".to_string()],
        },
        pre_spawn: vec![],
    };

    let fixture = Box::pin(build_unified_runtime_fixture(config)).await;
    assert_eq!(
        fixture.runtime.mob_handle().status().await.unwrap(),
        MobState::Running
    );
    assert!(fixture.runtime.module_is_running().await);
    assert_eq!(
        fixture.runtime.loaded_modules().await,
        vec!["mod-a".to_string()]
    );

    let shutdown = fixture.runtime.shutdown().await;
    assert_eq!(shutdown.module_shutdown.orphan_processes, 0);
    assert_eq!(
        shutdown.module_shutdown.terminated_modules,
        vec!["mod-a".to_string()]
    );
    assert!(
        shutdown.mob_stop.is_ok(),
        "mob stop failed at teardown: {:?}",
        shutdown.mob_stop
    );
    assert!(!fixture.runtime.module_is_running().await);
    assert_eq!(
        fixture.runtime.mob_handle().status().await.unwrap(),
        MobState::Stopped
    );
}

#[tokio::test]
#[ignore]
async fn choke_001_unified_subscribe_merges_module_and_agent_events() {
    let config = MetaConfig {
        modules: vec![shell_module(
            "mod-interaction",
            r#"printf '%s\n' '{"event_id":"evt-module-interaction","source":"module","timestamp_ms":10,"event":{"kind":"module","module":"mod-interaction","event_type":"ready","payload":{"ok":true}}}'"#,
            RestartPolicy::Never,
        )],
        discovery: DiscoverySpec {
            namespace: "phase1-unified".to_string(),
            modules: vec!["mod-interaction".to_string()],
        },
        pre_spawn: vec![],
    };

    let fixture = Box::pin(build_unified_runtime_fixture_with_agent_events(
        config,
        vec![EventEnvelope {
            event_id: "evt-agent-worker-1".to_string(),
            source: "agent".to_string(),
            timestamp_ms: 11,
            event: UnifiedEvent::Agent {
                agent_id: "worker-1".to_string(),
                event_type: "interaction_complete".to_string(),
                payload: Some(json!({"text":"seeded worker event"})),
            },
        }],
    ))
    .await;

    let ready_deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let response = fixture
            .runtime
            .subscribe_events(meerkat_meta::runtime::SubscribeRequest {
                scope: meerkat_meta::runtime::SubscribeScope::Mob,
                last_event_id: None,
                agent_id: None,
            })
            .await
            .expect("subscribe should succeed");
        let has_agent = response.events.iter().any(|event| {
            matches!(&event.event, UnifiedEvent::Agent { agent_id, .. } if agent_id == "worker-1")
        });
        if has_agent {
            break;
        }

        assert!(
            tokio::time::Instant::now() < ready_deadline,
            "expected unified subscribe to include worker-1 agent events"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let subscribed = loop {
        let response = fixture
            .runtime
            .subscribe_events(meerkat_meta::runtime::SubscribeRequest {
                scope: meerkat_meta::runtime::SubscribeScope::Mob,
                last_event_id: None,
                agent_id: None,
            })
            .await
            .expect("subscribe should succeed");
        let has_module = response
            .events
            .iter()
            .any(|event| matches!(&event.event, UnifiedEvent::Module(_)));
        let has_agent = response.events.iter().any(|event| {
            matches!(&event.event, UnifiedEvent::Agent { agent_id, .. } if agent_id == "worker-1")
        });
        if has_module && has_agent {
            break response;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "expected unified subscribe to include both module + agent events"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    };

    assert!(
        subscribed
            .events
            .iter()
            .any(|event| { matches!(&event.event, UnifiedEvent::Module(_)) })
    );
    assert!(subscribed.events.iter().any(|event| {
        matches!(&event.event, UnifiedEvent::Agent { agent_id, .. } if agent_id == "worker-1")
    }));

    let shutdown = fixture.runtime.shutdown().await;
    assert!(
        shutdown.mob_stop.is_ok(),
        "mob stop failed at teardown: {:?}",
        shutdown.mob_stop
    );
}

#[tokio::test]
#[ignore]
async fn req_001_reference_entrypoint_real_listener_graceful_shutdown_stops_runtime_cleanly() {
    let config = MetaConfig {
        modules: vec![shell_module(
            "listener-mod",
            r#"printf '%s\n' "{\"event_id\":\"listener-mod-ready\",\"source\":\"module\",\"timestamp_ms\":20,\"event\":{\"kind\":\"module\",\"module\":\"listener-mod\",\"event_type\":\"ready\",\"payload\":{\"ok\":true,\"pid\":$$}}}"; exec sleep 30"#,
            RestartPolicy::Never,
        )],
        discovery: DiscoverySpec {
            namespace: "phase1-reference-entrypoint".to_string(),
            modules: vec!["listener-mod".to_string()],
        },
        pre_spawn: vec![],
    };

    let fixture = Box::pin(build_unified_runtime_fixture(config)).await;
    let module_pid = fixture
        .runtime
        .module_events()
        .await
        .iter()
        .find_map(|event| match &event.event {
            UnifiedEvent::Module(module) if module.module == "listener-mod" => {
                module.payload.get("pid").and_then(|value| value.as_i64())
            }
            _ => None,
        })
        .expect("listener module pid should be present in payload");

    let app = fixture
        .runtime
        .build_reference_app_router(reference_runtime_decision_state());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind reference app listener");
    let address = listener
        .local_addr()
        .expect("resolve reference app listener address");
    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
            })
            .await
    });

    let healthz_response = http_get_response(address, "/healthz").await;
    let mut response_parts = healthz_response.split("\r\n\r\n");
    let response_head = response_parts.next().unwrap_or_default();
    let response_body = response_parts.collect::<Vec<_>>().join("\r\n\r\n");
    assert!(
        response_head.starts_with("HTTP/1.1 200"),
        "expected HTTP 200 for /healthz, got: {response_head}"
    );
    assert!(
        response_body.contains("ok"),
        "expected /healthz response body to contain ok, got: {response_body}"
    );

    shutdown_tx
        .send(())
        .expect("signal graceful shutdown for reference listener");
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .expect("reference listener should stop after graceful shutdown")
        .expect("reference listener task should join")
        .expect("reference listener should stop cleanly");

    let shutdown = fixture.runtime.shutdown().await;
    assert_eq!(shutdown.module_shutdown.orphan_processes, 0);
    assert_eq!(
        shutdown.module_shutdown.terminated_modules,
        vec!["listener-mod".to_string()]
    );
    assert!(
        shutdown.mob_stop.is_ok(),
        "mob stop failed at teardown: {:?}",
        shutdown.mob_stop
    );
    assert!(!fixture.runtime.module_is_running().await);
    assert_eq!(
        fixture.runtime.mob_handle().status().await.unwrap(),
        MobState::Stopped
    );

    let kill_status = Command::new("sh")
        .args(["-c", &format!("kill -0 {module_pid}")])
        .status()
        .expect("run kill -0");
    assert!(
        !kill_status.success(),
        "module process {module_pid} is still alive after shutdown"
    );
}
