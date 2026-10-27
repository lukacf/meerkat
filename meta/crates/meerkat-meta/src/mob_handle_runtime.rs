//! Mob member lifecycle management — bootstrap, spawn, reconcile, and roster queries.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use futures::StreamExt;
use meerkat::{AgentFactory, Config, FactoryAgentBuilder, SessionStore};
use meerkat_client::types::LlmStream;
use meerkat_client::{LlmClient, LlmRequest};
use meerkat_core::agent::CommsRuntime;
use meerkat_core::service::{
    CreateSessionRequest, SessionError, SessionHistoryPage, SessionHistoryQuery,
    SessionServiceHistoryExt,
};
use meerkat_core::{AgentSessionStore, AssistantBlock, Message, Provider};
use meerkat_mob::{
    MobBuilder, MobDefinition, MobError, MobHandle, MobSessionService, MobStorage, Profile,
    ProfileName, SpawnMemberSpec,
};
use meerkat_store::StoreAdapter;
use serde::Serialize;
use serde_json::Value;

#[path = "mob_runtime_delivery.rs"]
mod runtime_delivery;
use runtime_delivery::RunningMobRuntimeDelivery;
pub use runtime_delivery::{
    ContinuationSourceAnnouncer, MobRuntimeDelivery, MobRuntimeDeliveryError,
};

use crate::blob_store::{
    Base64BlobStoreAdapter, BinaryBlobStore, BlobStoreInjection, ObjectStoreBlobStore,
};
use crate::console_spawn::{
    ConsoleSpawnSink, SharedConsoleSpawnSinkSlot, new_console_spawn_sink_slot,
};
use crate::storage_health::{
    BlobDurability, BlobStoreResolutionError, ResolvedStorageSummary, RuntimeStoreResolutionError,
    StorageResolutionError, StorageSlotSummary, blob_slot_summary, probe_session_store_incremental,
    scratch_ring_buffer_slots,
};

/// The single definition of Meta's normalized-provider-accounting contract
/// for LLM test doubles, shared with the integration tests that include the
/// same file through `#[path = "support/llm_usage.rs"]`.
///
/// meerkat 0.8.22 fails a turn closed when its stream carried no
/// `LlmEvent::UsageUpdate`, and `LlmEvent::Done` is still constructible on its
/// own - so a double that omits usage compiles and then fails every turn. The
/// helpers in that file cannot hand out a `Done` without the `UsageUpdate`
/// that must precede it. In-crate `#[cfg(test)]` doubles live in the lib and
/// cannot reach `tests/`, so they reach the ONE definition from here rather
/// than growing a second copy that can drift.
#[cfg(test)]
#[path = "../tests/support/llm_usage.rs"]
pub(crate) mod test_llm_usage;

#[cfg(test)]
#[path = "mob_handle_runtime_prepared_attempt_tests.rs"]
mod prepared_attempt_tests;

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
#[path = "mob_handle_runtime_child_mob_tests.rs"]
mod child_mob_tests;

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
#[path = "mob_handle_runtime_reserved_identity_tests.rs"]
mod reserved_identity_tests;

#[cfg(test)]
pub(crate) fn acquisition_observation_probe(
    reject: bool,
    reads: Arc<std::sync::atomic::AtomicUsize>,
) -> Arc<dyn MobSessionService> {
    tests::acquisition_observation_probe(reject, reads)
}

/// A session service whose execution snapshot fails as `NotFound`, whose
/// live-actor read answers `live` (`None`: the read fails), and whose durable
/// store holds the session when `persisted`.
#[cfg(test)]
pub(crate) fn failed_snapshot_probe(
    live: Option<bool>,
    persisted: bool,
) -> Arc<dyn MobSessionService> {
    tests::failed_snapshot_probe(live, persisted)
}

/// A session service whose archive authority answers `answer`: whether it
/// knows the session, or `None` when the read fails.
#[cfg(test)]
pub(crate) fn archive_authority_probe(answer: Option<bool>) -> Arc<dyn MobSessionService> {
    tests::archive_authority_probe(answer)
}

/// Whether durable input rows include one short of a terminal phase
/// (consumed, superseded, coalesced or abandoned). `None` when a row does not
/// decode. A phase this build does not know counts as not terminal.
pub(crate) fn durable_inputs_owe_work(
    rows: &[meerkat_runtime::store::InputStateRow],
) -> Option<bool> {
    use meerkat_runtime::input_state::InputLifecycleState;
    let mut owes = false;
    for row in rows {
        match row {
            meerkat_runtime::store::InputStateRow::Decoded(state) => {
                owes |= !matches!(
                    state.seed.phase,
                    InputLifecycleState::Consumed
                        | InputLifecycleState::Superseded
                        | InputLifecycleState::Coalesced
                        | InputLifecycleState::Abandoned
                );
            }
            meerkat_runtime::store::InputStateRow::Corrupt { .. } => return None,
        }
    }
    Some(owes)
}

pub(crate) const DELEGATE_IDLE_RETIRE_SECS_LABEL: &str = "implicit_delegate_idle_retire_secs";
pub(crate) const DELEGATE_IDLE_RETIRE_DISABLED_LABEL: &str = "disabled";

pub(crate) fn is_previous_member_cleanup_ambiguous_error(error: &str) -> bool {
    error.contains("previous member cleanup ambiguous for member ")
}

pub(crate) fn is_recoverable_lifecycle_cleanup_error(error: &str) -> bool {
    is_previous_member_cleanup_ambiguous_error(error)
        || (error.contains("disposal completed but ArchiveSession failed")
            && (
                // Cancel/retire race: the runtime was still running when the
                // archive step tried to cancel it.
                (error.contains("cancel-before-retire failed")
                    && error.contains("Runtime not ready: running"))
                // meerkat 0.7.1: the session machine of an idle member sits in
                // `Stopped`, whose DSL authority rejects the archive step's
                // final `Retire` input. Disposal already completed — the
                // member left the roster (retire) or is anchored for cleanup
                // retry (respawn) — so the failed bookkeeping transition must
                // not fail the lifecycle operation.
                || (error.contains("guard rejected transition from Stopped")
                    && error.contains("input::Retire"))
                // meerkat 0.7.1 retire performs a final fenced continuity
                // save for the old session. Identity-first reset/delete flows
                // advance or remove the meta-owned continuity record before
                // retiring the old generation, so that save is intentionally
                // stale; disposal itself completed.
                || (error.contains("continuity save")
                    && (error.contains("continuity record not found")
                        || error.contains("stale fencing token")))
            ))
}

/// True when a session archive failed only at its final runtime-retire
/// realization because the session machine sits in `Stopped`.
///
/// meerkat 0.7.1 splits the archive across two realizations, committed
/// document-first: the durable session-document lifecycle commit lands,
/// then `MachineSessionArchiveProtocol::retire_session` drives the machine
/// `Retire` transition. The session machine accepts `Retire` from
/// Idle/Attached/Running/Retired but NOT from `Stopped` — and an idle mob
/// member's runtime is stopped (and persisted as `Stopped`) between turns,
/// so member retire/respawn disposal deterministically fails here.
/// meerkat-mob's own archive helper (`retire_runtime_session_for_archive`)
/// explicitly treats `Stopped` as already-retired; the meerkat-session
/// protocol misses that tolerance.
pub(crate) fn is_stopped_session_archive_retire_rejection(error: &str) -> bool {
    error.contains("machine archive retire failed")
        && error.contains("guard rejected transition from Stopped")
        && error.contains("input::Retire")
}

pub(crate) fn topology_restore_failed_peer_ids(
    error: &meerkat_mob::MobRespawnError,
) -> Option<Vec<String>> {
    match error {
        meerkat_mob::MobRespawnError::TopologyRestoreFailed {
            receipt: _,
            failed_peer_ids,
        } => Some(failed_peer_ids.iter().map(ToString::to_string).collect()),
        _ => None,
    }
}

pub(crate) fn topology_restore_warning_json(failed_peer_ids: &[String]) -> Value {
    serde_json::json!({
        "kind": "topology_restore_degraded",
        "failed_peer_ids": failed_peer_ids,
    })
}

#[cfg(test)]
use std::sync::{Mutex, OnceLock};

/// Member state constant for active members.
pub const MEMBER_STATE_ACTIVE: &str = "active";
/// Member state constant for members transitioning to retired.
pub const MEMBER_STATE_RETIRING: &str = "retiring";

/// Project a machine-owned member status into the console's member-state
/// string vocabulary.
///
/// Meerkat 0.7 replaced the roster-owned two-state `MemberState` with the
/// machine-projected [`meerkat_mob::MobMemberStatus`]; the legacy
/// active/retiring pair keeps the existing console constants and the newer
/// machine statuses (broken/completed/unknown) project as their canonical
/// snake_case names.
pub(crate) fn member_status_state_string(status: meerkat_mob::MobMemberStatus) -> String {
    match status {
        meerkat_mob::MobMemberStatus::Active => MEMBER_STATE_ACTIVE.to_string(),
        meerkat_mob::MobMemberStatus::Retiring => MEMBER_STATE_RETIRING.to_string(),
        other => format!("{other:?}").to_ascii_lowercase(),
    }
}

/// Core's members it holds Broken, each with Core's reason: the members it
/// could not restore (a restored member whose comms identity is missing on
/// this host, say). Read from Core's member list, which never waits on the
/// mob actor.
pub(crate) async fn broken_members_of(
    handle: &MobHandle,
) -> Vec<(meerkat_mob::AgentIdentity, String)> {
    handle
        .list_members()
        .await
        .into_iter()
        .filter(|entry| entry.status == meerkat_mob::MobMemberStatus::Broken)
        .map(|entry| {
            let reason = entry
                .error
                .unwrap_or_else(|| "Core holds the member broken".to_string());
            (entry.agent_identity, reason)
        })
        .collect()
}

/// Whether a mob runs as its handles observe it without asking its actor:
/// the last phase its actor published is live (creating or running) and the
/// actor has not stopped. An actor that fail-stopped keeps the live phase it
/// last published, so its phase alone reads it as running.
pub(crate) fn mob_observed_running(observation: &meerkat_mob::MobLifecycleObservation) -> bool {
    observation.failure.is_none()
        && matches!(
            observation.phase,
            meerkat_mob::MobState::Creating | meerkat_mob::MobState::Running
        )
}

/// Options for bootstrapping a mob runtime.
#[derive(Clone, Default)]
pub struct MobBootstrapOptions {
    pub allow_ephemeral_sessions: bool,
    pub notify_orchestrator_on_resume: bool,
    pub default_llm_client: Option<Arc<dyn LlmClient>>,
}

/// Wraps an LLM client and strips provider-emitted evidence blocks that are
/// useful for UI/citation projection but unsafe to replay into the next
/// stateless provider request.
pub struct ReplaySanitizingLlmClient {
    inner: Arc<dyn LlmClient>,
}

type SharedDefaultLlmClientSlot = Arc<std::sync::RwLock<Option<Arc<dyn LlmClient>>>>;

/// The image-generation machine a factory carries, held weakly. The factory
/// is cloned into the machine's own LLM reconfigure host, so a strong handle
/// is a cycle that keeps every Meta-built machine (and a host's native grant
/// owner) alive after shutdown. A call after the machine is gone is refused
/// as `Destroyed`.
pub fn weak_image_generation_machine(
    machine: &Arc<meerkat_runtime::MeerkatMachine>,
) -> Arc<dyn meerkat_tools::builtin::image_generation::ImageGenerationMachine> {
    Arc::new(WeakImageGenerationMachine(Arc::downgrade(machine)))
}

struct WeakImageGenerationMachine(std::sync::Weak<meerkat_runtime::MeerkatMachine>);

impl WeakImageGenerationMachine {
    fn machine(
        &self,
    ) -> Result<Arc<meerkat_runtime::MeerkatMachine>, meerkat_runtime::RuntimeDriverError> {
        self.0
            .upgrade()
            .ok_or(meerkat_runtime::RuntimeDriverError::Destroyed)
    }
}

#[async_trait]
impl meerkat_tools::builtin::image_generation::ImageGenerationMachine
    for WeakImageGenerationMachine
{
    async fn session_model_routing_status(
        &self,
        session_id: &meerkat_core::types::SessionId,
    ) -> Result<
        meerkat_core::image_generation::SessionModelRoutingStatus,
        meerkat_runtime::RuntimeDriverError,
    > {
        meerkat_tools::builtin::image_generation::ImageGenerationMachine::session_model_routing_status(
            self.machine()?.as_ref(),
            session_id,
        )
        .await
    }

    async fn begin_image_operation(
        &self,
        session_id: &meerkat_core::types::SessionId,
        request: meerkat_runtime::ImageOperationRoutingRequest,
    ) -> Result<meerkat_runtime::ImageOperationRoutingResult, meerkat_runtime::RuntimeDriverError>
    {
        meerkat_tools::builtin::image_generation::ImageGenerationMachine::begin_image_operation(
            self.machine()?.as_ref(),
            session_id,
            request,
        )
        .await
    }

    async fn deny_image_operation_plan(
        &self,
        session_id: &meerkat_core::types::SessionId,
        operation_id: meerkat_core::image_generation::ImageOperationId,
        reason: meerkat_core::image_generation::ImageOperationDenialReason,
    ) -> Result<
        meerkat_core::image_generation::ImageOperationPhase,
        meerkat_runtime::RuntimeDriverError,
    > {
        meerkat_tools::builtin::image_generation::ImageGenerationMachine::deny_image_operation_plan(
            self.machine()?.as_ref(),
            session_id,
            operation_id,
            reason,
        )
        .await
    }

    async fn activate_image_operation_override(
        &self,
        session_id: &meerkat_core::types::SessionId,
        operation_id: meerkat_core::image_generation::ImageOperationId,
    ) -> Result<
        meerkat_core::image_generation::ImageOperationPhase,
        meerkat_runtime::RuntimeDriverError,
    > {
        meerkat_tools::builtin::image_generation::ImageGenerationMachine::activate_image_operation_override(
            self.machine()?.as_ref(),
            session_id,
            operation_id,
        )
        .await
    }

    async fn classify_image_operation_terminal(
        &self,
        session_id: &meerkat_core::types::SessionId,
        operation_id: meerkat_core::image_generation::ImageOperationId,
        observation: meerkat_core::image_generation::ImageProviderTerminalObservation,
        provider_text: meerkat_core::image_generation::ProviderTextDisposition,
    ) -> Result<
        meerkat_core::image_generation::ImageOperationTerminalClass,
        meerkat_runtime::RuntimeDriverError,
    > {
        meerkat_tools::builtin::image_generation::ImageGenerationMachine::classify_image_operation_terminal(
            self.machine()?.as_ref(),
            session_id,
            operation_id,
            observation,
            provider_text,
        )
        .await
    }

    async fn complete_image_operation(
        &self,
        session_id: &meerkat_core::types::SessionId,
        operation_id: meerkat_core::image_generation::ImageOperationId,
        terminal: meerkat_core::image_generation::ImageOperationTerminalClass,
    ) -> Result<
        meerkat_core::image_generation::ImageOperationPhase,
        meerkat_runtime::RuntimeDriverError,
    > {
        meerkat_tools::builtin::image_generation::ImageGenerationMachine::complete_image_operation(
            self.machine()?.as_ref(),
            session_id,
            operation_id,
            terminal,
        )
        .await
    }

    async fn restore_image_operation_override(
        &self,
        session_id: &meerkat_core::types::SessionId,
        operation_id: meerkat_core::image_generation::ImageOperationId,
    ) -> Result<
        meerkat_core::image_generation::ImageOperationPhase,
        meerkat_runtime::RuntimeDriverError,
    > {
        meerkat_tools::builtin::image_generation::ImageGenerationMachine::restore_image_operation_override(
            self.machine()?.as_ref(),
            session_id,
            operation_id,
        )
        .await
    }
}

fn session_llm_reconfigure_blueprint(
    builder: &FactoryAgentBuilder,
    store_path: &Path,
) -> (
    meerkat::session_runtime::llm_reconfigure::SessionRuntimeLlmReconfigureHostBlueprint,
    SharedDefaultLlmClientSlot,
) {
    let default_client_slot = Arc::new(std::sync::RwLock::new(None::<Arc<dyn LlmClient>>));
    let blueprint =
        meerkat::session_runtime::llm_reconfigure::SessionRuntimeLlmReconfigureHostBlueprint::new(
            builder,
            store_path.join("session-llm-reconfigure-config-state.json"),
            Arc::clone(&default_client_slot),
        );
    (blueprint, default_client_slot)
}

impl ReplaySanitizingLlmClient {
    pub fn new(inner: Arc<dyn LlmClient>) -> Self {
        Self { inner }
    }

    pub fn wrap(inner: Arc<dyn LlmClient>) -> Arc<dyn LlmClient> {
        Arc::new(Self::new(inner))
    }
}

/// Provider-agnostic claim for the mob-wide default LLM client.
///
/// The default client (test stubs, `demo_llm` gateways) is installed as the
/// raw `llm_client_override` on EVERY member build, across whatever providers
/// the definition's profiles resolve to — including a provider a member MOVES
/// to when a definition edit changes its model and the resume-override pair
/// applies (model, provider) atomically from the catalog. A concrete
/// [`LlmClient::provider`] claim turns that composition into a typed factory
/// rejection ("raw LLM client override claims provider 'openai' but canonical
/// model ... belongs to 'anthropic'") the moment any member's canonical
/// provider differs from the claim — the production pair-coherence fix surfaced
/// exactly this on the resume path. `Provider::Other` is meerkat's typed
/// "serves any provider" claim; the member's canonical (model, provider)
/// identity keeps coming from the build config and catalog, never from this
/// client.
///
/// Deliberately NOT applied to per-session `llm_client_override`s entering
/// through `CreateSessionRequest`
/// ([`sanitize_create_session_request_llm_override`]): those are scoped to
/// one session, and a concrete claim contradicting that session's canonical
/// provider is a real composition error the factory guard exists to catch.
struct ProviderAgnosticLlmClient {
    inner: Arc<dyn LlmClient>,
}

impl ProviderAgnosticLlmClient {
    fn wrap(inner: Arc<dyn LlmClient>) -> Arc<dyn LlmClient> {
        Arc::new(Self { inner })
    }
}

#[async_trait]
impl LlmClient for ProviderAgnosticLlmClient {
    // The wrapped client's actual controller: its selection, request-free
    // route and existing-credential maintenance. Only `provider()` below is
    // this wrapper's own claim; a selection still names the wrapped client's
    // actual provider, which the adapter checks against the member's
    // canonical identity.
    async fn prepare_controller_credential(&self) -> Result<(), meerkat_core::auth::AuthError> {
        self.inner.prepare_controller_credential().await
    }

    fn controller_model_selection(&self) -> Option<meerkat_core::ControllerModelSelection> {
        self.inner.controller_model_selection()
    }

    fn plain_model_route(
        &self,
        logical_model: &str,
    ) -> Result<meerkat_llm_core::PlainModelRoute, meerkat_core::ControllerFactsUnavailable> {
        self.inner.plain_model_route(logical_model)
    }

    // The wrapped client's own projection, with any route witness it
    // captured for its prepared dispatch.
    fn project_replay_request(
        &self,
        messages: &[Message],
    ) -> Result<meerkat_llm_core::LlmReplayProjection, meerkat_client::LlmError> {
        self.inner.project_replay_request(messages)
    }

    fn project_replay_messages(
        &self,
        messages: &[Message],
    ) -> Result<Vec<Message>, meerkat_client::LlmError> {
        self.inner.project_replay_messages(messages)
    }

    fn stream<'a>(&'a self, request: &'a LlmRequest) -> LlmStream<'a> {
        self.inner.stream(request)
    }

    // Unchanged, as `stream` is: this wrapper lowers no request and claims no
    // cache breakpoints, so the prepared request reaches the wrapped client
    // with its admitted work, target and route witness.
    fn stream_prepared<'a>(
        &'a self,
        request: &'a meerkat_llm_core::PreparedLlmRequest,
    ) -> LlmStream<'a> {
        self.inner.stream_prepared(request)
    }

    fn provider(&self) -> meerkat_core::Provider {
        meerkat_core::Provider::Other
    }

    async fn health_check(&self) -> Result<(), meerkat_client::LlmError> {
        self.inner.health_check().await
    }

    fn compile_schema(
        &self,
        output_schema: &meerkat_core::OutputSchema,
    ) -> Result<meerkat_core::schema::CompiledSchema, meerkat_core::schema::SchemaError> {
        self.inner.compile_schema(output_schema)
    }
}

/// Agent-layer companion to [`ReplaySanitizingLlmClient`].
///
/// Meerkat session services can also receive already-adapted
/// `AgentLlmClient`s through live replacement and hot-swap APIs. Sanitize at
/// that boundary too so provider-emitted server tool telemetry is never
/// replayed into the next stateless model request just because the client
/// entered below the raw `LlmClient` adapter seam.
pub struct ReplaySanitizingAgentLlmClient {
    inner: Arc<dyn meerkat_core::AgentLlmClient>,
}

impl ReplaySanitizingAgentLlmClient {
    pub fn new(inner: Arc<dyn meerkat_core::AgentLlmClient>) -> Self {
        Self { inner }
    }

    pub fn wrap(
        inner: Arc<dyn meerkat_core::AgentLlmClient>,
    ) -> Arc<dyn meerkat_core::AgentLlmClient> {
        Arc::new(Self::new(inner))
    }
}

#[async_trait]
impl meerkat_core::AgentLlmClient for ReplaySanitizingAgentLlmClient {
    fn prepare_request_attempt(
        self: Arc<Self>,
        messages: Arc<Vec<Message>>,
        tools: Arc<[Arc<meerkat_core::ToolDef>]>,
        max_tokens: u32,
        temperature: Option<f32>,
        provider_params: Option<meerkat_core::ProviderParamsOverride>,
    ) -> Result<Arc<dyn meerkat_core::AgentLlmRequestAttempt>, meerkat_core::AgentError> {
        let sanitized = messages
            .iter()
            .cloned()
            .map(sanitize_message_for_stateless_replay)
            .collect();
        self.inner.clone().prepare_request_attempt(
            Arc::new(sanitized),
            tools,
            max_tokens,
            temperature,
            provider_params,
        )
    }

    // Forward the inner client's request-attempt authority. `AgentLlmClient`
    // gives this method a DEFAULT returning `LegacySplit`, so a decorator that
    // omits it compiles cleanly and silently downgrades every wrapped client.
    // meerkat 0.8.31 rejects resume for a client reporting LegacySplit when the
    // inner adapter is Unified: production measured 72 identities marked Broken at boot
    // on the candidate and 0 across three 0.8.30 runs. A decorator must report
    // what it wraps, never what it is.
    fn request_attempt_authority(&self) -> meerkat_core::RequestAttemptAuthority {
        self.inner.request_attempt_authority()
    }

    // The controller surface. Its trait defaults are "none" or a refusal, so
    // a wrapper that omits these strips the member of its governed
    // controller: the actor's controller pin is `None` and governed
    // admission refuses the member as malformed.
    fn controller_model_selection(&self) -> Option<meerkat_core::ControllerModelSelection> {
        self.inner.controller_model_selection()
    }

    fn controller_model_facts(
        &self,
    ) -> Result<meerkat_core::ControllerModelFacts, meerkat_core::ControllerFactsUnavailable> {
        self.inner.controller_model_facts()
    }

    async fn prepare_controller_credential(&self) -> Result<(), meerkat_core::auth::AuthError> {
        self.inner.prepare_controller_credential().await
    }

    // Governed controller requests run through the pinned client, not this
    // one, so the pin keeps the sanitization: the wrapped client's pinned
    // child (its immutable selection), sanitized again.
    fn pin_controller(self: Arc<Self>) -> Option<meerkat_core::ControllerModelClient> {
        let pinned = Arc::clone(&self.inner).pin_controller()?;
        Some(meerkat_core::ControllerModelClient::new(
            pinned.selection().clone(),
            Self::wrap(Arc::clone(pinned.client())),
        ))
    }

    // The default refuses an admitted work authorization rather than drop
    // it; the wrapped client decides, over the sanitized messages.
    fn prepare_request_attempt_authorized(
        self: Arc<Self>,
        messages: Arc<Vec<Message>>,
        tools: Arc<[Arc<meerkat_core::ToolDef>]>,
        max_tokens: u32,
        temperature: Option<f32>,
        provider_params: Option<meerkat_core::ProviderParamsOverride>,
        authorization: Option<meerkat_core::LlmRequestAuthorization>,
    ) -> Result<Arc<dyn meerkat_core::AgentLlmRequestAttempt>, meerkat_core::AgentError> {
        let sanitized = messages
            .iter()
            .cloned()
            .map(sanitize_message_for_stateless_replay)
            .collect();
        self.inner.clone().prepare_request_attempt_authorized(
            Arc::new(sanitized),
            tools,
            max_tokens,
            temperature,
            provider_params,
            authorization,
        )
    }

    async fn stream_response_authorized(
        &self,
        messages: &[Message],
        tools: &[Arc<meerkat_core::ToolDef>],
        max_tokens: u32,
        temperature: Option<f32>,
        provider_params: Option<&meerkat_core::lifecycle::run_primitive::ProviderParamsOverride>,
        authorization: Option<meerkat_core::LlmRequestAuthorization>,
    ) -> Result<meerkat_core::agent::LlmStreamResult, meerkat_core::AgentError> {
        let sanitized: Vec<Message> = messages
            .iter()
            .cloned()
            .map(sanitize_message_for_stateless_replay)
            .collect();
        self.inner
            .stream_response_authorized(
                &sanitized,
                tools,
                max_tokens,
                temperature,
                provider_params,
                authorization,
            )
            .await
    }

    // A live-bridge fork is sanitized like the client it forks.
    fn fork_noncommitting_live_bridge(
        &self,
    ) -> Result<Arc<dyn meerkat_core::AgentLlmClient>, meerkat_core::AgentError> {
        Ok(Self::wrap(self.inner.fork_noncommitting_live_bridge()?))
    }

    async fn stream_response(
        &self,
        messages: &[Message],
        tools: &[Arc<meerkat_core::ToolDef>],
        max_tokens: u32,
        temperature: Option<f32>,
        provider_params: Option<&meerkat_core::lifecycle::run_primitive::ProviderParamsOverride>,
    ) -> Result<meerkat_core::agent::LlmStreamResult, meerkat_core::AgentError> {
        let sanitized: Vec<Message> = messages
            .iter()
            .cloned()
            .map(sanitize_message_for_stateless_replay)
            .collect();
        self.inner
            .stream_response(&sanitized, tools, max_tokens, temperature, provider_params)
            .await
    }

    fn provider(&self) -> meerkat_core::Provider {
        self.inner.provider()
    }

    fn model(&self) -> &str {
        self.inner.model()
    }

    fn request_pressure(
        &self,
        messages: &[Message],
        tools: &[Arc<meerkat_core::ToolDef>],
        max_tokens: u32,
        temperature: Option<f32>,
        provider_params: Option<&meerkat_core::lifecycle::run_primitive::ProviderParamsOverride>,
    ) -> Result<Option<meerkat_core::ProviderRequestPressure>, meerkat_core::AgentError> {
        let sanitized: Vec<Message> = messages
            .iter()
            .cloned()
            .map(sanitize_message_for_stateless_replay)
            .collect();
        self.inner
            .request_pressure(&sanitized, tools, max_tokens, temperature, provider_params)
    }

    fn prepare_model_fallback(
        &self,
        failure: &meerkat_core::AgentError,
        request: &meerkat_core::model_fallback::ModelFallbackRequest<'_>,
    ) -> Result<
        meerkat_core::AgentLlmFallbackSwitch,
        Vec<meerkat_core::AgentLlmFallbackSkippedTarget>,
    > {
        let sanitized: Vec<Message> = request
            .messages
            .iter()
            .cloned()
            .map(sanitize_message_for_stateless_replay)
            .collect();
        self.inner.prepare_model_fallback(
            failure,
            &meerkat_core::model_fallback::ModelFallbackRequest {
                messages: &sanitized,
                ..*request
            },
        )
    }

    fn commit_model_fallback(
        &self,
        previous_identity: &meerkat_core::SessionLlmIdentity,
        target_identity: &meerkat_core::SessionLlmIdentity,
    ) -> Result<(), meerkat_core::AgentError> {
        self.inner
            .commit_model_fallback(previous_identity, target_identity)
    }

    fn active_model_fallback_identity(&self) -> Option<meerkat_core::SessionLlmIdentity> {
        self.inner.active_model_fallback_identity()
    }

    fn compile_model_fallback_schema(
        &self,
        target_identity: &meerkat_core::SessionLlmIdentity,
        output_schema: &meerkat_core::OutputSchema,
    ) -> Result<meerkat_core::schema::CompiledSchema, meerkat_core::AgentError> {
        self.inner
            .compile_model_fallback_schema(target_identity, output_schema)
    }

    fn begin_stream_output_observation(&self) {
        self.inner.begin_stream_output_observation();
    }

    fn stream_output_observed(&self) -> bool {
        self.inner.stream_output_observed()
    }

    fn stream_activity_count(&self) -> Option<u64> {
        self.inner.stream_activity_count()
    }

    fn compile_schema(
        &self,
        output_schema: &meerkat_core::OutputSchema,
    ) -> Result<meerkat_core::schema::CompiledSchema, meerkat_core::schema::SchemaError> {
        self.inner.compile_schema(output_schema)
    }

    // meerkat 0.8.22 adds `target_cache_lowering_capabilities` with an
    // `Ok(Vec::new())` default. NOT forwarded, deliberately: its contract is
    // "capabilities only from the lowering this client would use for THIS
    // exact request", and `stream_response` above rewrites every message
    // through `sanitize_message_for_stateless_replay` before the inner client
    // sees it. Forwarding the caller's unsanitized messages would author a
    // cache proof over bytes this wrapper never puts on the wire. The empty
    // default remains truthful "no target lowering available"; pressure and
    // fallback admission above instead delegate with the sanitized messages.
}

#[async_trait]
impl LlmClient for ReplaySanitizingLlmClient {
    // The wrapped client's actual controller: its selection, request-free
    // route and existing-credential maintenance. The trait defaults (none, a
    // refusal) left a member built on a raw client override without a pinned
    // controller.
    async fn prepare_controller_credential(&self) -> Result<(), meerkat_core::auth::AuthError> {
        self.inner.prepare_controller_credential().await
    }

    fn controller_model_selection(&self) -> Option<meerkat_core::ControllerModelSelection> {
        self.inner.controller_model_selection()
    }

    fn plain_model_route(
        &self,
        logical_model: &str,
    ) -> Result<meerkat_llm_core::PlainModelRoute, meerkat_core::ControllerFactsUnavailable> {
        self.inner.plain_model_route(logical_model)
    }

    // The wrapped client's own projection of the sanitized messages, so the
    // route witness it captures reaches its prepared dispatch below.
    fn project_replay_request(
        &self,
        messages: &[Message],
    ) -> Result<meerkat_llm_core::LlmReplayProjection, meerkat_client::LlmError> {
        let sanitized: Vec<Message> = messages
            .iter()
            .cloned()
            .map(sanitize_message_for_stateless_replay)
            .collect();
        self.inner.project_replay_request(&sanitized)
    }

    fn project_replay_messages(
        &self,
        messages: &[Message],
    ) -> Result<Vec<Message>, meerkat_client::LlmError> {
        let sanitized: Vec<Message> = messages
            .iter()
            .cloned()
            .map(sanitize_message_for_stateless_replay)
            .collect();
        self.inner.project_replay_messages(&sanitized)
    }

    fn stream<'a>(&'a self, request: &'a LlmRequest) -> LlmStream<'a> {
        let inner = Arc::clone(&self.inner);
        let sanitized = sanitize_llm_request_for_stateless_replay(request);
        Box::pin(async_stream::stream! {
            let mut stream = inner.stream(&sanitized);
            while let Some(event) = stream.next().await {
                if runtime_turn_diagnostics_enabled() && event.is_err() {
                    tracing::error!("meta llm client stream error");
                }
                yield event;
            }
        })
    }

    // Prepared (governed) dispatch runs the same sanitization as `stream`,
    // lowered onto the prepared request: its admitted work, authorization
    // target and route witness reach the wrapped client unchanged, and the
    // sanitized payload is the only one it is sent.
    fn stream_prepared<'a>(
        &'a self,
        request: &'a meerkat_llm_core::PreparedLlmRequest,
    ) -> LlmStream<'a> {
        let inner = Arc::clone(&self.inner);
        let sanitized = request
            .with_lowered_request(sanitize_llm_request_for_stateless_replay(request.request()));
        Box::pin(async_stream::stream! {
            let mut stream = inner.stream_prepared(&sanitized);
            while let Some(event) = stream.next().await {
                if runtime_turn_diagnostics_enabled() && event.is_err() {
                    tracing::error!("meta llm client stream error");
                }
                yield event;
            }
        })
    }

    fn provider(&self) -> meerkat_core::Provider {
        self.inner.provider()
    }

    async fn health_check(&self) -> Result<(), meerkat_client::LlmError> {
        self.inner.health_check().await
    }

    fn compile_schema(
        &self,
        output_schema: &meerkat_core::OutputSchema,
    ) -> Result<meerkat_core::schema::CompiledSchema, meerkat_core::schema::SchemaError> {
        self.inner.compile_schema(output_schema)
    }

    // meerkat 0.8.22 adds `authored_cache_breakpoints` with an `Ok(Vec::new())`
    // default. NOT forwarded, for the same reason as
    // `target_cache_lowering_capabilities` on the `AgentLlmClient` wrapper
    // above: `LlmClientAdapter` builds the request from the CANONICAL messages
    // and asks this client to claim breakpoints over it, while `stream` and
    // `stream_prepared` above rewrite that request through
    // `sanitize_llm_request_for_stateless_replay` before it leaves. A
    // forwarded claim would describe a lowering that never reached the
    // provider. Empty is the truthful claim set.
}

/// Async hook called before each session is created. Receives the mutable
/// `CreateSessionRequest` so the app can inject external tools, augment the
/// system prompt, set labels, override the model, load session resume data
/// from external stores, etc.
///
/// The hook runs **before** `create_session` captures labels and LLM identity,
/// so all mutations are reflected in session metadata, not just the agent build.
///
/// ```rust,ignore
/// let (storage, provenance) = persistent_mob_storage(store_path.join("event_log.sqlite3"))?;
/// let spec = MobBootstrapSpec::persistent_with_hook(
///     definition, storage, provenance, store_path, 64,
///     |req: &mut CreateSessionRequest| {
///         Box::pin(async move {
///             // Async: load session from external store
///             let session = my_store.load_by_owner(&owner_id).await;
///             if let Some(s) = session {
///                 let build = req.build.get_or_insert_with(SessionBuildOptions::default);
///                 build.resume_session = Some(s);
///             }
///             // Sync: inject tools, augment prompt
///             let build = req.build.get_or_insert_with(SessionBuildOptions::default);
///             build.external_tools = Some(my_tools());
///             Ok(())
///         })
///     },
/// )
/// .await?;
/// ```
pub(crate) type PreBuildHook = Arc<
    dyn Fn(
            &mut CreateSessionRequest,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<(), SessionError>> + Send + '_>,
        > + Send
        + Sync,
>;

/// Optional post-creation hook invoked after `create_session` succeeds.
pub type AfterCreateHook = Arc<
    dyn Fn(
            meerkat_core::types::SessionId,
            SessionCreatedContext,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
        + Send
        + Sync,
>;

/// Direct service creates do not register a machine driver. Preserve the
/// constructor's best-effort registration before the caller's post-create hook,
/// using the owner already acquired from that exact service.
fn register_runtime_after_create(
    runtime_adapter: Arc<meerkat_runtime::MeerkatMachine>,
    user_hook: Option<AfterCreateHook>,
) -> AfterCreateHook {
    Arc::new(move |session_id, context| {
        let runtime_adapter = runtime_adapter.clone();
        let user_hook = user_hook.clone();
        Box::pin(async move {
            if let Err(error) = runtime_adapter.register_session(session_id.clone()).await {
                tracing::error!(
                    session_id = %session_id,
                    error = %error,
                    "post-create session runtime registration failed"
                );
            }
            if let Some(user_hook) = user_hook {
                user_hook(session_id, context).await;
            }
        })
    })
}

/// Wraps a `MobSessionService`, applying a `PreBuildHook` to the
/// `CreateSessionRequest` in `create_session()` before delegating.
///
/// The hook runs before labels and LLM identity are captured by the inner
/// session service, so mutations to `req.labels`, `req.model`, `req.build`,
/// and `req.system_prompt` are fully reflected in session metadata.
struct PreBuildMobSessionService {
    inner: Arc<dyn MobSessionService>,
    hook: PreBuildHook,
    /// §10.1 dispatch-time taint join (`crate::memory::dispatch_taint`):
    /// present ONLY on the wrapper `MobBootstrapSpec::new` installs - the
    /// one layer every spec has exactly once - so `with_*` re-wraps never
    /// double-decorate. The slot is late-bound: compositions fill it when
    /// the memory stack attaches.
    dispatch_taint: Option<crate::memory::dispatch_taint::DispatchTaintSlot>,
    after_create_hook: Option<AfterCreateHook>,
    runtime_adapter_override: Option<Arc<meerkat_runtime::MeerkatMachine>>,
    /// RuntimeStore whose store-owned lifecycle facts overlay the resume-seam
    /// read (persistent runtime-backed path only).
    ///
    /// At meerkat 0.8.11 the archive protocol never rewrites session BODIES
    /// to carry archive authority — the absorbing terminal is a
    /// RuntimeStore-owned fact (the catalog entry committed with the physical
    /// authority, or the Retired/Destroyed machine lifecycle row) — while the
    /// mob resume seam still classifies from the body terminal. Without this
    /// overlay a runtime-archived session reads as `Revivable` WITHOUT its
    /// archived terminal, which is exactly the "archived collapses into
    /// active/absent" host confusion the typed seam exists to prevent (hosts
    /// rotated identities off intact preserved transcripts, the 0.8.6 field
    /// failure).
    archived_terminal_authority: Option<Arc<dyn meerkat_runtime::RuntimeStore>>,
    /// Set only by the composition whose member sessions live in in-memory
    /// stores (the runtime-backed launch without `persistent_state` or a
    /// storage provider). The persistent session service it wraps keeps the
    /// persistent-session contract, but its member sessions do not outlive
    /// the process, so this layer answers `keeps_durable_member_sessions`
    /// false and Meerkat refuses a durable profile's spawn
    /// (`MobError::NonDurableService`). Every other layer forwards.
    member_sessions_in_memory: bool,
}

impl PreBuildMobSessionService {
    /// Project the RuntimeStore-owned archived terminal onto a revivable
    /// resume read (see `archived_terminal_authority`). Read-side only: the
    /// returned document copy carries the store-owned fact; nothing is
    /// written. The probe mirrors meerkat-session's
    /// `session_archived_by_runtime_store_authority`: the catalog entry's
    /// committed terminal, else the Retired/Destroyed machine lifecycle row.
    async fn overlay_runtime_archived_terminal(
        &self,
        load: meerkat_mob::ResumeSessionLoad,
    ) -> Result<meerkat_mob::ResumeSessionLoad, SessionError> {
        let meerkat_mob::ResumeSessionLoad::Revivable(mut session) = load else {
            return Ok(load);
        };
        self.overlay_runtime_archived_terminal_in_place(&mut session)
            .await?;
        Ok(meerkat_mob::ResumeSessionLoad::Revivable(session))
    }

    /// Body-level form of the overlay above.
    ///
    /// meerkat 0.8.22 moved operational resume off this wrapper's
    /// `load_session_for_resume`: `materialize_session_resume_verdict` now
    /// hands back an already-authorized body produced by the inner service's
    /// durable-tail convergence. That override therefore applies the overlay
    /// to the verdict's body directly, which needs the projection separated
    /// from the `ResumeSessionLoad` carrier.
    async fn overlay_runtime_archived_terminal_in_place(
        &self,
        session: &mut meerkat_core::Session,
    ) -> Result<(), SessionError> {
        if session.lifecycle_terminal().is_some() {
            return Ok(());
        }
        let Some(store) = self.archived_terminal_authority.as_ref() else {
            return Ok(());
        };
        let runtime_id = meerkat_runtime::LogicalRuntimeId::for_session(session.id());
        let store_error = |detail: String| {
            SessionError::Agent(meerkat_core::error::AgentError::InternalError(detail))
        };
        let archived_by_catalog = store
            .load_runtime_session_catalog_entry(&runtime_id)
            .await
            .map_err(|e| {
                store_error(format!(
                    "archived-terminal overlay: runtime catalog read for session {}: {e}",
                    session.id()
                ))
            })?
            .is_some_and(|entry| {
                entry.lifecycle_terminal() == Some(meerkat_core::SessionLifecycleTerminal::Archived)
            });
        let archived = archived_by_catalog
            || matches!(
                meerkat_runtime::store::load_runtime_state(store.as_ref(), &runtime_id)
                    .await
                    .map_err(|e| {
                        store_error(format!(
                            "archived-terminal overlay: runtime lifecycle read for session {}: {e}",
                            session.id()
                        ))
                    })?,
                Some(
                    meerkat_runtime::RuntimeState::Retired
                        | meerkat_runtime::RuntimeState::Destroyed
                )
            );
        if archived {
            session
                .set_lifecycle_terminal(meerkat_core::SessionLifecycleTerminal::Archived)
                .map_err(|e| {
                    store_error(format!(
                        "archived-terminal overlay: terminal projection for session {}: {e}",
                        session.id()
                    ))
                })?;
        }
        Ok(())
    }

    async fn prepare_create_request(
        &self,
        mut req: CreateSessionRequest,
    ) -> Result<
        (
            CreateSessionRequest,
            SessionCreatedContext,
            Option<crate::capability_invariant::MemberCapabilityInvariantContext>,
        ),
        SessionError,
    > {
        (self.hook)(&mut req).await?;
        // Only a typed Resume continues a persisted session; a Mint carrier
        // (a spawn's pre-assigned id, meerkat #1225) has nothing to load.
        let resume_id = req
            .build
            .as_ref()
            .filter(|build| build.initial_tool_filter.is_none())
            .filter(|build| {
                build.session_build_intent() == meerkat_core::service::SessionBuildIntent::Resume
            })
            .and_then(|build| build.resume_session.as_ref())
            .map(|session| session.id().clone());
        // The heal reads only the session's build state, never its transcript,
        // and makes no tail decision: the create's own recovery decides the
        // open run. So it takes the committed snapshot the tolerant read
        // serves, also while a resumed run's provisional tail stands, where the
        // strict read would refuse the resume. Archived still reads as absent.
        let persisted_resume = match resume_id {
            Some(session_id) => {
                self.inner
                    .load_persisted_session_tolerating_tail(&session_id)
                    .await?
            }
            None => None,
        };
        heal_legacy_shell_tool_visibility(&mut req, persisted_resume.as_ref());
        sanitize_create_session_request_llm_override(&mut req);
        // After the user hook (composes over any decorator it set) and after
        // sanitize (which only touches the raw llm_client_override).
        if let Some(slot) = self.dispatch_taint.as_ref() {
            crate::memory::dispatch_taint::attach_member_taint_decorator(&mut req, slot);
        }

        // Capture the complete resolved profile declaration while the typed
        // build options and actual supplied dispatchers still exist. The
        // factory consumes both during materialization. Inline and realm
        // profiles have already converged onto this same request boundary.
        let capability_invariant = req.build.as_ref().and_then(
            crate::capability_invariant::MemberCapabilityInvariantContext::from_build_options,
        );

        let context = SessionCreatedContext {
            model: req.model.clone(),
            labels: req.labels.clone().unwrap_or_default(),
            system_prompt: req.system_prompt.as_set_prompt().map(ToString::to_string),
        };
        Ok((req, context, capability_invariant))
    }

    async fn complete_create(
        &self,
        result: meerkat_core::types::RunResult,
        context: SessionCreatedContext,
        capability_context: Option<crate::capability_invariant::MemberCapabilityInvariantContext>,
    ) -> meerkat_core::types::RunResult {
        if let Some(capability_context) = capability_context {
            use crate::capability_invariant::{
                CapabilityCatalogExactness, CapabilityInvariantObservation,
                CapabilityInvariantUnverifiable,
            };

            let observation = match capability_context.catalog_exactness {
                CapabilityCatalogExactness::Exact => {
                    match self.inner.tool_scope_snapshot(&result.session_id).await {
                        Ok(Some(scope)) => CapabilityInvariantObservation::ExactCatalog {
                            declared: capability_context.declared.clone(),
                            names: scope
                                .known_base_names
                                .into_iter()
                                .map(meerkat_core::types::ToolName::into_string)
                                .collect(),
                        },
                        Ok(None) => CapabilityInvariantObservation::Unverifiable {
                            declared: capability_context.declared.clone(),
                            cause: CapabilityInvariantUnverifiable::CatalogUnavailable,
                        },
                        Err(error) => CapabilityInvariantObservation::Unverifiable {
                            declared: capability_context.declared.clone(),
                            cause: CapabilityInvariantUnverifiable::CatalogReadFailed(
                                error.to_string(),
                            ),
                        },
                    }
                }
                CapabilityCatalogExactness::NonExactActualDispatcher => {
                    CapabilityInvariantObservation::Unverifiable {
                        declared: capability_context.declared.clone(),
                        cause: CapabilityInvariantUnverifiable::NonExactCatalog,
                    }
                }
            };
            let decision = crate::capability_invariant::decide(observation);
            let transition = crate::capability_invariant::transition_for(
                crate::capability_invariant::CapabilityInvariantPolicy::WarnOnly,
                decision,
            );
            crate::capability_invariant::emit_transition(
                &capability_context.mob_id,
                &capability_context.role,
                &capability_context.member,
                &result.session_id.to_string(),
                &transition,
            );
        }
        if let Some(ref after_hook) = self.after_create_hook {
            after_hook(result.session_id.clone(), context).await;
        }
        result
    }
}

fn no_op_pre_build_hook() -> PreBuildHook {
    Arc::new(|_req: &mut CreateSessionRequest| Box::pin(async { Ok(()) }))
}

/// `hook`, then the refusal of a marked document MCP descriptor it left
/// unresolved ([`crate::document_mcp_marker`]). A host document binding
/// resolves its descriptors in the hook, so the refusal runs after it, on
/// the innermost wrapper: every wrapper outside it has a no-op hook.
fn refusing_unresolved_document_markers(hook: Option<PreBuildHook>) -> PreBuildHook {
    let hook = hook.unwrap_or_else(no_op_pre_build_hook);
    Arc::new(move |request| {
        let hook = Arc::clone(&hook);
        Box::pin(async move {
            hook(request).await?;
            crate::document_mcp_marker::refuse_unresolved_marked_descriptors(request)
        })
    })
}

/// A spawned member's idle-retirement policy, chosen by the spawning call
/// (`delegate`, `mob_spawn_member` or `fork_off` with `idle_retire_secs`, or
/// an RPC/console fork).
///
/// Persisted through [`crate::PersistentMetadataStore`] so the opt-in
/// survives a gateway restart; the serialized form is stable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DelegateIdleRetireOverride {
    /// Never idle-retire the member.
    ///
    /// No agent tool produces this any more (`idle_retire_secs: null` means
    /// "not set"); it stays for host-side callers and for opt-in rows an
    /// earlier Meta persisted with it, which must keep loading.
    Disabled,
    /// Idle-retire the member after this many idle seconds.
    Seconds(u64),
    /// Opt the member into idle retirement on the runtime default timeout.
    ///
    /// Members seated in the primary mob are only swept when something opts
    /// them in. `fork_off` children live in the caller's mob, so without this
    /// they would stay live until the session ceiling refuses new work.
    RuntimeDefault,
}

/// One member's idle-retirement opt-in, bound to the member instance it was
/// set for.
///
/// `session_id` is the member's bridge session when the opt-in was recorded
/// (or carried to by a respawn). A member id can be reused once its member is
/// retired, so the opt-in holds only while the member seated under that id
/// still runs that session. `None` only in standalone wiring without a seat
/// resolver (unit tests): such an opt-in is honoured for whatever member holds
/// the id and is never persisted.
///
/// `recorded_at` orders the opt-in against mob events: a member reset, mob
/// reset or mob destroy releases only opt-ins recorded before it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BoundIdleRetireOverride {
    pub(crate) policy: DelegateIdleRetireOverride,
    pub(crate) session_id: Option<meerkat_core::types::SessionId>,
    pub(crate) recorded_at: chrono::DateTime<chrono::Utc>,
}

impl BoundIdleRetireOverride {
    fn new(
        policy: DelegateIdleRetireOverride,
        session_id: Option<meerkat_core::types::SessionId>,
    ) -> Self {
        Self {
            policy,
            session_id,
            recorded_at: chrono::Utc::now(),
        }
    }

    /// The durable form; `None` for an unbound (standalone) opt-in.
    fn record(
        &self,
        mob_id: &str,
        member_id: &str,
    ) -> Option<crate::MemberIdleRetireOverrideRecord> {
        Some(crate::MemberIdleRetireOverrideRecord {
            mob_id: mob_id.to_string(),
            member_id: member_id.to_string(),
            session_id: self.session_id.clone()?,
            policy: self.policy,
            recorded_at: self.recorded_at,
        })
    }
}

/// Respawn `member` through `respawn` and carry its idle-retire opt-in to the
/// session it comes back on.
///
/// A respawn keeps the member id but mints a new bridge session; an opt-in
/// bound to the old session would read as belonging to a gone member and be
/// dropped. Every Meta surface that respawns a member (operator RPC,
/// console, the agent `mob_respawn` tool) goes through here: it reads the
/// member's incarnation (roster generation and session) before the respawn
/// and, once the respawn returns, rebinds exactly the opt-in bound to that
/// session to the session the member came back on. The old session's
/// retirement usually releases the opt-in first; the rebind then takes it
/// from the bounded released record.
///
/// The rebind runs only when `respawned` says the respawn succeeded AND the
/// member now under the id is the immediate successor of the incarnation read
/// before (see [`MemberIncarnation::is_followed_by`]). If another surface
/// retired the member and seated a new one under the id meanwhile, or reset
/// or respawned it, the member under the id is further on and keeps no
/// opt-in.
///
/// Limit: every doubt (a failed or degraded respawn, an interleaved change of
/// the member, a crash, or this future being dropped between the respawn and
/// the rebind) loses the respawned member's opt-in. That errs toward a leak
/// (the member is not idle-retired), never toward retiring the wrong member.
pub(crate) async fn respawn_carrying_idle_retire_opt_in<T>(
    overrides: Option<&ImplicitDelegateRetirementOverrides>,
    handle: &MobHandle,
    member: &meerkat_mob::AgentIdentity,
    respawn: impl std::future::Future<Output = T>,
    respawned: impl FnOnce(&T) -> bool,
) -> T {
    let Some(overrides) = overrides else {
        return respawn.await;
    };
    let mob_id = handle.mob_id();
    let _in_flight = overrides.respawn_in_flight(mob_id.as_str(), member.as_str());
    let before = MemberIncarnation::read(handle, member).await;
    let outcome = respawn.await;
    if respawned(&outcome)
        && let Some(before) = before
        && let Some(after) = MemberIncarnation::read(handle, member).await
        && before.is_followed_by(&after)
    {
        overrides
            .rebind(
                mob_id.as_str(),
                member.as_str(),
                &before.session_id,
                after.session_id,
            )
            .await;
    }
    outcome
}

/// One incarnation of a mob member: its roster generation and the bridge
/// session it runs.
struct MemberIncarnation {
    generation: meerkat_mob::Generation,
    session_id: meerkat_core::types::SessionId,
}

impl MemberIncarnation {
    /// The incarnation now under `member`, if it has a session. The session
    /// is read between two roster reads that agree on the runtime id, so it
    /// belongs to that incarnation. Any failed lookup reads as `None`.
    async fn read(handle: &MobHandle, member: &meerkat_mob::AgentIdentity) -> Option<Self> {
        let entry = handle.get_member(member).await.ok().flatten()?;
        let session_id = handle.resolve_bridge_session_id(member).await?;
        let confirmed = handle.get_member(member).await.ok().flatten()?;
        (confirmed.agent_runtime_id == entry.agent_runtime_id).then_some(Self {
            generation: entry.generation,
            session_id,
        })
    }

    /// Whether `next` is the incarnation that immediately follows this one.
    ///
    /// meerkat mints a member id's generations strictly in order and keeps
    /// the count through a retire (only destroying the mob clears it): a new
    /// member seated under a retired id, a reset, and a respawn each take the
    /// next generation. So when anything else changed the member between the
    /// two reads, the member now under the id sits further on than one step.
    fn is_followed_by(&self, next: &Self) -> bool {
        self.generation.next() == Some(next.generation) && self.session_id != next.session_id
    }
}

/// A respawn of one member in flight, for as long as this guard lives
/// (dropping it, however the respawning future ends, closes it).
pub(crate) struct RespawnInFlight {
    respawning: Arc<std::sync::Mutex<BTreeMap<(String, String), usize>>>,
    key: (String, String),
}

impl Drop for RespawnInFlight {
    fn drop(&mut self) {
        let mut respawning = self
            .respawning
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(count) = respawning.get_mut(&self.key) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                respawning.remove(&self.key);
            }
        }
    }
}

/// Carries an identity member's idle-retire opt-in across a continuity
/// rebind to a new session (respawn or delivery repair).
pub(crate) struct IdleRetireRotationObserver {
    overrides: ImplicitDelegateRetirementOverrides,
    mob_id: String,
}

impl IdleRetireRotationObserver {
    pub(crate) fn new(overrides: ImplicitDelegateRetirementOverrides, mob_id: String) -> Self {
        Self { overrides, mob_id }
    }
}

#[async_trait::async_trait]
impl crate::identity_first::runtime::MemberSessionRotationObserver for IdleRetireRotationObserver {
    async fn member_session_rotated(
        &self,
        member: &meerkat_mob::ids::AgentIdentity,
        previous: &meerkat_core::types::SessionId,
        current: &meerkat_core::types::SessionId,
    ) {
        self.overrides
            .rebind(&self.mob_id, member.as_str(), previous, current.clone())
            .await;
    }
}

/// Resolves the member instance an opt-in is recorded for: the bridge
/// session of a member seated in a mob the idle sweep manages (the primary
/// mob or an implicit delegation mob).
#[derive(Clone)]
pub(crate) struct IdleRetireSeatResolver {
    // Weak: the tools factory holding these overrides is reachable from the
    // state's session service, so a strong reference would be a cycle.
    state: std::sync::Weak<meerkat_mob_mcp::MobMcpState>,
    primary_mob_id: String,
}

impl IdleRetireSeatResolver {
    pub(crate) fn new(state: &Arc<meerkat_mob_mcp::MobMcpState>, primary_mob_id: String) -> Self {
        Self {
            state: Arc::downgrade(state),
            primary_mob_id,
        }
    }

    /// The seated member's bridge session, or `None` when the member is not
    /// seated or its mob is not swept.
    async fn seated_session(
        &self,
        mob_id: &str,
        member_id: &str,
    ) -> Option<meerkat_core::types::SessionId> {
        let state = self.state.upgrade()?;
        let mob = meerkat_mob::MobId::from(mob_id);
        if mob_id != self.primary_mob_id && !state.is_implicit_mob(&mob).await {
            return None;
        }
        let handle = state.handle_for(&mob).await.ok()?;
        handle
            .resolve_bridge_session_id(&meerkat_mob::AgentIdentity::from(member_id))
            .await
    }

    async fn mob_handle(&self, mob_id: &str) -> Option<MobHandle> {
        let state = self.state.upgrade()?;
        state
            .handle_for(&meerkat_mob::MobId::from(mob_id))
            .await
            .ok()
    }
}

/// Per-member idle-retirement opt-ins, keyed by `(mob_id, member_id)` and
/// bound to the member instance (see [`BoundIdleRetireOverride`]).
///
/// The idle sweep reads this map on every pass. Once a durable store is
/// attached ([`Self::attach_durable_store`]), every bound opt-in writes
/// through to it and the attach restores the opt-ins a previous process
/// recorded, so a restored `fork_off` child is swept again after a restart
/// instead of holding its session until retired by hand.
#[derive(Clone, Default)]
pub(crate) struct ImplicitDelegateRetirementOverrides {
    inner: Arc<tokio::sync::RwLock<BTreeMap<(String, String), BoundIdleRetireOverride>>>,
    durable: Arc<std::sync::RwLock<Option<Arc<dyn crate::PersistentMetadataStore>>>>,
    seats: Arc<std::sync::RwLock<Option<IdleRetireSeatResolver>>>,
    released: Arc<std::sync::Mutex<std::collections::VecDeque<ReleasedIdleRetireOptIn>>>,
    respawning: Arc<std::sync::Mutex<BTreeMap<(String, String), usize>>>,
}

/// An opt-in cleared because the session it was bound to was retired or
/// replaced. Kept so that a respawn or a rotation of that exact session can
/// carry it on, and so that the member resuming onto that exact session gets
/// it back.
#[derive(Debug, Clone)]
struct ReleasedIdleRetireOptIn {
    mob_id: String,
    member_id: String,
    session_id: meerkat_core::types::SessionId,
    policy: DelegateIdleRetireOverride,
}

/// Released opt-ins kept in memory. Only the most recent ones matter: a
/// rotation or resume follows its retirement closely.
const RELEASED_IDLE_RETIRE_OPT_INS_CAP: usize = 1024;

impl ImplicitDelegateRetirementOverrides {
    /// Overrides whose opt-ins bind to the member instance `seats` resolves.
    pub(crate) fn with_seat_resolver(seats: IdleRetireSeatResolver) -> Self {
        let overrides = Self::default();
        *overrides
            .seats
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(seats);
        overrides
    }

    fn seat_resolver(&self) -> Option<IdleRetireSeatResolver> {
        self.seats
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// The handle of `mob_id`, when a seat resolver is installed.
    pub(crate) async fn mob_handle(&self, mob_id: &str) -> Option<MobHandle> {
        self.seat_resolver()?.mob_handle(mob_id).await
    }

    /// Record a member's opt-in, bound to its current session.
    ///
    /// With a seat resolver installed, a member that is not seated (a
    /// delegate helper is retired before its call returns) or not in a swept
    /// mob is not recorded at all: the sweep would never act on it.
    pub(crate) async fn set(
        &self,
        mob_id: impl Into<String>,
        member_id: impl Into<String>,
        override_policy: DelegateIdleRetireOverride,
    ) {
        let (mob_id, member_id) = (mob_id.into(), member_id.into());
        let session_id = match self.seat_resolver() {
            Some(seats) => match seats.seated_session(&mob_id, &member_id).await {
                Some(session_id) => Some(session_id),
                None => {
                    tracing::debug!(
                        mob_id,
                        member_id,
                        "idle-retire opt-in not recorded: the member is not seated in a swept mob"
                    );
                    return;
                }
            },
            None => None,
        };
        let bound = BoundIdleRetireOverride::new(override_policy, session_id);
        self.inner
            .write()
            .await
            .insert((mob_id.clone(), member_id.clone()), bound.clone());
        self.persist(&mob_id, &member_id, &bound, "idle-retire opt-in")
            .await;
    }

    /// Write a bound opt-in through to the durable store, if attached.
    async fn persist(
        &self,
        mob_id: &str,
        member_id: &str,
        bound: &BoundIdleRetireOverride,
        what: &str,
    ) {
        let (Some(record), Some(store)) = (bound.record(mob_id, member_id), self.durable_store())
        else {
            return;
        };
        if let Err(error) = store.set_member_idle_retire_override(&record).await {
            tracing::warn!(
                mob_id,
                member_id,
                error = %error,
                "{what} was not persisted; it lasts until the next restart"
            );
        }
    }

    /// Forget exactly this opt-in, in memory and durably. An opt-in recorded
    /// for the member id since (a newer member instance) is kept. A bound
    /// opt-in is remembered as released (see [`ReleasedIdleRetireOptIn`]).
    pub(crate) async fn clear(
        &self,
        mob_id: &str,
        member_id: &str,
        bound: &BoundIdleRetireOverride,
    ) {
        let key = (mob_id.to_string(), member_id.to_string());
        let removed = {
            let mut inner = self.inner.write().await;
            if inner.get(&key) == Some(bound) {
                inner.remove(&key);
                true
            } else {
                false
            }
        };
        if !removed {
            return;
        }
        if let Some(session_id) = bound.session_id.clone() {
            let mut released = self
                .released
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            released.push_back(ReleasedIdleRetireOptIn {
                mob_id: mob_id.to_string(),
                member_id: member_id.to_string(),
                session_id,
                policy: bound.policy,
            });
            while released.len() > RELEASED_IDLE_RETIRE_OPT_INS_CAP {
                released.pop_front();
            }
        }
        if let (Some(record), Some(store)) = (bound.record(mob_id, member_id), self.durable_store())
            && let Err(error) = store.clear_member_idle_retire_override(&record).await
        {
            tracing::warn!(
                mob_id,
                member_id,
                error = %error,
                "a stale idle-retire opt-in could not be cleared durably"
            );
        }
    }

    /// The member instance bound to `session_id` is being retired: forget its
    /// opt-in (a respawn of it takes the opt-in back from the released
    /// record). An opt-in bound to any other session is untouched.
    pub(crate) async fn release_session(
        &self,
        mob_id: &str,
        member_id: &str,
        session_id: &meerkat_core::types::SessionId,
    ) {
        if let Some(bound) = self.get_bound(mob_id, member_id).await
            && bound.session_id.as_ref() == Some(session_id)
        {
            self.clear(mob_id, member_id, &bound).await;
        }
    }

    /// A reset or destroy at `at`: forget the opt-ins it applies to, which
    /// are exactly those recorded before it (`member_id` narrows it to one
    /// member). An opt-in recorded after the event, which a lagging event
    /// stream can deliver late, belongs to a member seated after it and is
    /// kept.
    pub(crate) async fn release_recorded_before(
        &self,
        mob_id: &str,
        member_id: Option<&str>,
        at: chrono::DateTime<chrono::Utc>,
    ) {
        let held: Vec<(String, BoundIdleRetireOverride)> = self
            .inner
            .read()
            .await
            .iter()
            .filter(|((mob, member), bound)| {
                mob == mob_id
                    && member_id.is_none_or(|only| only == member)
                    && bound.recorded_at < at
            })
            .map(|((_, member), bound)| (member.clone(), bound.clone()))
            .collect();
        for (member, bound) in held {
            self.clear(mob_id, &member, &bound).await;
        }
    }

    /// Mark a respawn of `member_id` in flight until the returned guard drops.
    pub(crate) fn respawn_in_flight(&self, mob_id: &str, member_id: &str) -> RespawnInFlight {
        let key = (mob_id.to_string(), member_id.to_string());
        *self
            .respawning
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entry(key.clone())
            .or_insert(0) += 1;
        RespawnInFlight {
            respawning: Arc::clone(&self.respawning),
            key,
        }
    }

    /// Whether a Meta respawn of `member_id` is in flight.
    pub(crate) fn is_respawning(&self, mob_id: &str, member_id: &str) -> bool {
        self.respawning
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains_key(&(mob_id.to_string(), member_id.to_string()))
    }

    /// Carry the opt-in bound to `from` over to `to`: the same member was
    /// respawned, or its session rotated (an identity member's continuity
    /// rebind). Applies to a live opt-in bound to `from` or to one released
    /// with `from` since. Returns whether an opt-in was carried.
    pub(crate) async fn rebind(
        &self,
        mob_id: &str,
        member_id: &str,
        from: &meerkat_core::types::SessionId,
        to: meerkat_core::types::SessionId,
    ) -> bool {
        let key = (mob_id.to_string(), member_id.to_string());
        let carried = {
            let mut inner = self.inner.write().await;
            let live = inner
                .get(&key)
                .filter(|bound| bound.session_id.as_ref() == Some(from))
                .map(|bound| bound.policy);
            let Some(policy) = live.or_else(|| self.take_released(mob_id, member_id, from)) else {
                return false;
            };
            let carried = BoundIdleRetireOverride::new(policy, Some(to));
            inner.insert(key, carried.clone());
            carried
        };
        self.persist(
            mob_id,
            member_id,
            &carried,
            "idle-retire opt-in carried to a respawned or rotated member's new session",
        )
        .await;
        true
    }

    fn take_released(
        &self,
        mob_id: &str,
        member_id: &str,
        session_id: &meerkat_core::types::SessionId,
    ) -> Option<DelegateIdleRetireOverride> {
        let mut released = self
            .released
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let position = released.iter().rposition(|opt_in| {
            opt_in.mob_id == mob_id
                && opt_in.member_id == member_id
                && &opt_in.session_id == session_id
        })?;
        released.remove(position).map(|opt_in| opt_in.policy)
    }

    fn has_released(&self, mob_id: &str, member_id: &str) -> bool {
        self.released
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .any(|opt_in| opt_in.mob_id == mob_id && opt_in.member_id == member_id)
    }

    /// The member runs `current`, with no recorded opt-in: if it resumed onto
    /// the exact session its opt-in was just released with (retired, then
    /// re-attached or repaired onto the same session), that opt-in is
    /// restored. Returns the restored policy.
    async fn restore_resumed(
        &self,
        mob_id: &str,
        member_id: &str,
        current: &meerkat_core::types::SessionId,
    ) -> Option<DelegateIdleRetireOverride> {
        let key = (mob_id.to_string(), member_id.to_string());
        let restored = {
            let mut inner = self.inner.write().await;
            if inner.contains_key(&key) {
                return None;
            }
            let policy = self.take_released(mob_id, member_id, current)?;
            let restored = BoundIdleRetireOverride::new(policy, Some(current.clone()));
            inner.insert(key, restored.clone());
            restored
        };
        self.persist(
            mob_id,
            member_id,
            &restored,
            "resumed member's idle-retire opt-in",
        )
        .await;
        Some(restored.policy)
    }

    /// Decide the opt-in that applies to the member seated under `member_id`
    /// running `current` (as the sweep sees it), settling the bookkeeping on
    /// the way: an opt-in bound to another session belongs to a gone member
    /// and is dropped; a just-released opt-in is restored when the member
    /// resumed onto its exact session. `None` when nothing applies or the
    /// member's session is not resolvable yet.
    pub(crate) async fn reconcile_seated(
        &self,
        mob_id: &str,
        member_id: &str,
        current: Option<&meerkat_core::types::SessionId>,
    ) -> Option<DelegateIdleRetireOverride> {
        let Some(bound) = self.get_bound(mob_id, member_id).await else {
            let current = current?;
            if !self.has_released(mob_id, member_id) {
                return None;
            }
            return self.restore_resumed(mob_id, member_id, current).await;
        };
        let Some(bound_session) = bound.session_id.clone() else {
            return Some(bound.policy);
        };
        let current = current?;
        if current == &bound_session {
            return Some(bound.policy);
        }
        self.clear(mob_id, member_id, &bound).await;
        None
    }

    /// Opt-ins held for members of mobs other than `primary_mob_id`.
    pub(crate) async fn bindings_outside(
        &self,
        primary_mob_id: &str,
    ) -> Vec<((String, String), BoundIdleRetireOverride)> {
        self.inner
            .read()
            .await
            .iter()
            .filter(|((mob, _), _)| mob != primary_mob_id)
            .map(|(key, bound)| (key.clone(), bound.clone()))
            .collect()
    }

    /// Apply a mob event to the recorded opt-ins:
    /// - every retirement of a member session, whatever surface caused it,
    ///   releases the opt-in bound to exactly that session;
    /// - a member (re)seated on the exact session an opt-in was released with
    ///   gets it back (a retire, then a resume onto the same session). The
    ///   spawn event's own session is not public in meerkat 0.8.42, so this
    ///   reads the member's session when the event arrives; if it is not
    ///   published yet, the next sweep restores it;
    /// - a member reset, mob reset or mob destroy releases the opt-ins
    ///   recorded before the event.
    pub(crate) async fn observe_mob_event(&self, mob_id: &str, event: &meerkat_mob::MobEvent) {
        match &event.kind {
            meerkat_mob::MobEventKind::MemberRetirementStarted {
                agent_identity,
                session_id: Some(session_id),
                ..
            } => {
                self.release_session(mob_id, agent_identity.as_str(), session_id)
                    .await;
            }
            meerkat_mob::MobEventKind::MemberSpawned(spawned) => {
                let member_id = spawned.agent_identity.as_str();
                if let Some(seats) = self.seat_resolver()
                    && let Some(current) = seats.seated_session(mob_id, member_id).await
                {
                    self.reconcile_seated(mob_id, member_id, Some(&current))
                        .await;
                }
            }
            meerkat_mob::MobEventKind::MemberReset { agent_identity, .. } => {
                self.release_recorded_before(
                    mob_id,
                    Some(agent_identity.as_str()),
                    event.timestamp,
                )
                .await;
            }
            meerkat_mob::MobEventKind::MobDestroying | meerkat_mob::MobEventKind::MobReset => {
                self.release_recorded_before(mob_id, None, event.timestamp)
                    .await;
            }
            _ => {}
        }
    }

    /// Attach the durable store: restore the opt-ins it holds (an opt-in set
    /// earlier in this process wins over a persisted one), then persist every
    /// bound opt-in set before the attach, so none is lost. A store that
    /// cannot be read restores nothing but still receives those opt-ins.
    /// Returns how many opt-ins were restored.
    pub(crate) async fn attach_durable_store(
        &self,
        store: Arc<dyn crate::PersistentMetadataStore>,
    ) -> usize {
        // Install first: a `set` racing the attach then writes through
        // itself, and the flush below rewrites the same value.
        *self
            .durable
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::clone(&store));
        let persisted = match store.load_member_idle_retire_overrides().await {
            Ok(persisted) => persisted,
            Err(error) => {
                tracing::warn!(
                    error = %error,
                    "idle-retire opt-ins could not be restored; members restored from a \
                     previous process are not idle-retired until retired by hand"
                );
                Vec::new()
            }
        };
        let (restored, set_before_attach) = {
            let mut inner = self.inner.write().await;
            let set_before_attach = inner.clone();
            let mut restored = 0;
            for record in persisted {
                if let std::collections::btree_map::Entry::Vacant(slot) =
                    inner.entry((record.mob_id, record.member_id))
                {
                    slot.insert(BoundIdleRetireOverride {
                        policy: record.policy,
                        session_id: Some(record.session_id),
                        recorded_at: record.recorded_at,
                    });
                    restored += 1;
                }
            }
            (restored, set_before_attach)
        };
        for ((mob_id, member_id), bound) in set_before_attach {
            self.persist(&mob_id, &member_id, &bound, "idle-retire opt-in")
                .await;
        }
        restored
    }

    fn durable_store(&self) -> Option<Arc<dyn crate::PersistentMetadataStore>> {
        self.durable
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Test seam: place an opt-in in memory as `set` would, bypassing the
    /// seat resolver.
    #[cfg(test)]
    pub(crate) async fn insert_for_test(
        &self,
        mob_id: &str,
        member_id: &str,
        bound: BoundIdleRetireOverride,
    ) {
        self.inner
            .write()
            .await
            .insert((mob_id.to_string(), member_id.to_string()), bound);
    }

    /// The recorded opt-in for a member id, with its binding.
    pub(crate) async fn get_bound(
        &self,
        mob_id: &str,
        member_id: &str,
    ) -> Option<BoundIdleRetireOverride> {
        self.inner
            .read()
            .await
            .get(&(mob_id.to_string(), member_id.to_string()))
            .cloned()
    }

    #[cfg(test)]
    pub(crate) async fn get(
        &self,
        mob_id: &str,
        member_id: &str,
    ) -> Option<DelegateIdleRetireOverride> {
        self.get_bound(mob_id, member_id)
            .await
            .map(|bound| bound.policy)
    }
}

pub(crate) type SharedIdentityRuntimeSlot =
    Arc<std::sync::RwLock<Option<Arc<crate::identity_first::IdentityRuntime>>>>;

struct AutoWireParentMobToolsFactory {
    inner: Arc<dyn meerkat_core::service::MobToolsFactory>,
    implicit_delegate_retirement_overrides: ImplicitDelegateRetirementOverrides,
    console_spawn_sink: SharedConsoleSpawnSinkSlot,
    identity_runtime: SharedIdentityRuntimeSlot,
    protected_mob_id: String,
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl meerkat_core::service::MobToolsFactory for AutoWireParentMobToolsFactory {
    async fn build_mob_tools(
        &self,
        args: meerkat_core::service::MobToolsBuildArgs,
    ) -> Result<Arc<dyn meerkat_core::AgentToolDispatcher>, Box<dyn std::error::Error + Send + Sync>>
    {
        let spawner_comms_name = args.comms_name.clone();
        let inner = self.inner.build_mob_tools(args).await?;
        Ok(Arc::new(AutoWireParentMobToolDispatcher {
            inner,
            implicit_delegate_retirement_overrides: self
                .implicit_delegate_retirement_overrides
                .clone(),
            console_spawn_sink: Arc::clone(&self.console_spawn_sink),
            identity_runtime: Arc::clone(&self.identity_runtime),
            protected_mob_id: self.protected_mob_id.clone(),
            spawner_comms_name,
        }))
    }
}

struct AutoWireParentMobToolDispatcher {
    inner: Arc<dyn meerkat_core::AgentToolDispatcher>,
    implicit_delegate_retirement_overrides: ImplicitDelegateRetirementOverrides,
    /// Late-bound console sink; empty until a console-bearing runtime
    /// installs one, in which case successful spawns project into it.
    console_spawn_sink: SharedConsoleSpawnSinkSlot,
    /// Identity authority attaches after the raw mob tool factory is built.
    /// Every dispatcher reads this shared slot at call time.
    identity_runtime: SharedIdentityRuntimeSlot,
    protected_mob_id: String,
    /// Comms name of the agent owning this tool surface — identifies the
    /// spawning parent for console lineage.
    spawner_comms_name: Option<String>,
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl meerkat_core::AgentToolDispatcher for AutoWireParentMobToolDispatcher {
    /// The wrapped mob tools admit against the session's mob operator
    /// authority, so it reaches them unchanged.
    fn bind_session_mob_tool_authority(
        &self,
        authority: Option<Arc<std::sync::RwLock<meerkat_core::service::MobToolAuthorityContext>>>,
    ) {
        self.inner.bind_session_mob_tool_authority(authority);
    }

    fn tools(&self) -> Arc<[Arc<meerkat_core::types::ToolDef>]> {
        self.inner
            .tools()
            .iter()
            .map(idle_retire_patched_tool_def)
            .collect::<Vec<_>>()
            .into()
    }

    /// The wrapper patches three tool definitions and adds none, so the inner
    /// exactness holds for the patched catalog as well.
    fn tool_catalog_capabilities(&self) -> meerkat_core::ToolCatalogCapabilities {
        self.inner.tool_catalog_capabilities()
    }

    fn tool_catalog(&self) -> Arc<[meerkat_core::ToolCatalogEntry]> {
        self.inner
            .tool_catalog()
            .iter()
            .map(|entry| meerkat_core::ToolCatalogEntry {
                tool: idle_retire_patched_tool_def(&entry.tool),
                ..entry.clone()
            })
            .collect::<Vec<_>>()
            .into()
    }

    async fn dispatch(
        &self,
        call: meerkat_core::types::ToolCallView<'_>,
    ) -> Result<meerkat_core::ToolDispatchOutcome, meerkat_core::ToolError> {
        self.dispatch_with_context(call, &meerkat_core::ToolDispatchContext::default())
            .await
    }

    async fn dispatch_with_context(
        &self,
        call: meerkat_core::types::ToolCallView<'_>,
        context: &meerkat_core::ToolDispatchContext,
    ) -> Result<meerkat_core::ToolDispatchOutcome, meerkat_core::ToolError> {
        if matches!(
            call.name,
            "delegate"
                | "mob_spawn_member"
                | "mob_retire_member"
                | "mob_wire"
                | "mob_unwire"
                | "mob_destroy"
                | "spawn_member"
                | "spawn_many_members"
                | "retire_member"
                | "force_cancel_member"
                | "member_status"
                | "wire_members"
                | "unwire_members"
                | "fork_off"
        ) {
            let args = serde_json::from_str::<Value>(call.args.get()).map_err(|error| {
                meerkat_core::ToolError::invalid_arguments(call.name, error.to_string())
            })?;
            if let Some((field, alias)) = reserved_raw_member_tool_argument(call.name, &args) {
                return Err(meerkat_core::ToolError::invalid_arguments(
                    call.name,
                    format!(
                        "{field} '{alias}' uses Meta's reserved rt:* / mk-- member namespace; use a public non-reserved alias or the IdentityRuntime authority"
                    ),
                ));
            }
            let identity_runtime = self
                .identity_runtime
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            if let Some(identity_runtime) = identity_runtime {
                for (field, alias) in raw_member_tool_arguments(call.name, &args) {
                    let alias = crate::member_comms_id::runtime_alias_str(&alias).into_owned();
                    if identity_runtime
                        .identity_for_member_mutation(&alias)
                        .await
                        .is_some()
                    {
                        return Err(meerkat_core::ToolError::invalid_arguments(
                            call.name,
                            format!(
                                "{field} '{alias}' is owned by the attached IdentityRuntime; use the identity lifecycle/topology authority"
                            ),
                        ));
                    }
                }
                if call.name == "mob_destroy"
                    && args.get("mob_id").and_then(Value::as_str)
                        == Some(self.protected_mob_id.as_str())
                {
                    return Err(meerkat_core::ToolError::invalid_arguments(
                        call.name,
                        format!(
                            "mob '{}' is owned by the attached IdentityRuntime and cannot be destroyed through raw agent tools",
                            self.protected_mob_id
                        ),
                    ));
                }
            }
        }
        if call.name == "delegate" {
            return self.dispatch_delegate(call, context).await;
        }
        if call.name == "mob_spawn_member" {
            return self.dispatch_mob_spawn_member(call, context).await;
        }
        if call.name == "fork_off" {
            return self.dispatch_fork_off(call, context).await;
        }
        if call.name == "mob_respawn" {
            return self.dispatch_mob_respawn(call, context).await;
        }
        if crate::console_spawn::is_console_spawn_tool(call.name) {
            // Spawn variants this wrapper does not otherwise intercept
            // (e.g. spawn_member/spawn_many_members surfaces) still get
            // their members projected into the console.
            let args = serde_json::from_str::<Value>(call.args.get()).ok();
            let name = call.name.to_string();
            let outcome = self.inner.dispatch_with_context(call, context).await?;
            if let Some(args) = args {
                self.project_spawn_to_console(&name, &args, &outcome, context)
                    .await;
            }
            return Ok(outcome);
        }
        self.inner.dispatch_with_context(call, context).await
    }

    fn capabilities(&self) -> meerkat_core::agent::DispatcherCapabilities {
        self.inner.capabilities()
    }

    fn bind_ops_lifecycle(
        self: Arc<Self>,
        registry: Arc<dyn meerkat_core::ops_lifecycle::OpsLifecycleRegistry>,
        owner_bridge_session_id: meerkat_core::types::SessionId,
    ) -> Result<meerkat_core::agent::BindOutcome, meerkat_core::agent::OpsLifecycleBindError> {
        let owned = Arc::try_unwrap(self)
            .map_err(|_| meerkat_core::agent::OpsLifecycleBindError::SharedOwnership)?;
        let outcome = owned
            .inner
            .bind_ops_lifecycle(registry, owner_bridge_session_id)?;
        let was_bound = outcome.was_bound();
        let dispatcher = Arc::new(Self {
            inner: outcome.into_dispatcher(),
            implicit_delegate_retirement_overrides: owned.implicit_delegate_retirement_overrides,
            console_spawn_sink: owned.console_spawn_sink,
            identity_runtime: owned.identity_runtime,
            protected_mob_id: owned.protected_mob_id,
            spawner_comms_name: owned.spawner_comms_name,
        });
        Ok(if was_bound {
            meerkat_core::agent::BindOutcome::Bound(dispatcher)
        } else {
            meerkat_core::agent::BindOutcome::Skipped(dispatcher)
        })
    }
}

/// Return the first raw mob-tool argument that attempts to create or mutate
/// an identity-runtime alias. Agent tools dispatch directly to meerkat-mob's
/// lower plane, so permitting either the public or encoded form here would
/// bypass durable lifecycle and topology authority.
fn reserved_raw_member_tool_argument(
    tool_name: &str,
    args: &Value,
) -> Option<(&'static str, String)> {
    raw_member_tool_arguments(tool_name, args)
        .into_iter()
        .find(|(_, value)| {
            crate::member_comms_id::is_reserved_generated_alias(value)
                || crate::member_comms_id::uses_reserved_roster_marker(value)
        })
        .map(|(field, value)| {
            (
                field,
                crate::member_comms_id::runtime_alias_str(&value).into_owned(),
            )
        })
}

fn raw_member_tool_arguments(tool_name: &str, args: &Value) -> Vec<(&'static str, String)> {
    let mut candidates = Vec::new();
    match tool_name {
        "delegate"
        | "mob_spawn_member"
        | "mob_retire_member"
        | "spawn_member"
        | "retire_member"
        | "force_cancel_member"
        | "member_status"
        | "fork_off" => {
            candidates.push(("member_id", args.get("member_id").and_then(Value::as_str)));
        }
        "spawn_many_members" => {
            if let Some(specs) = args.get("specs").and_then(Value::as_array) {
                for spec in specs {
                    candidates.push((
                        "specs[].member_id",
                        spec.get("member_id").and_then(Value::as_str),
                    ));
                }
            }
        }
        "mob_wire" | "mob_unwire" => {
            candidates.push(("member_id", args.get("member_id").and_then(Value::as_str)));
            candidates.push((
                "peer.local",
                args.get("peer")
                    .and_then(|peer| peer.get("local"))
                    .and_then(Value::as_str),
            ));
        }
        "wire_members" | "unwire_members" => {
            candidates.push(("member_id", args.get("member_id").and_then(Value::as_str)));
            candidates.push((
                "peer_member_id",
                args.get("peer_member_id").and_then(Value::as_str),
            ));
        }
        _ => return Vec::new(),
    }
    candidates
        .into_iter()
        .filter_map(|(field, value)| value.map(|value| (field, value.to_string())))
        .collect()
}

impl AutoWireParentMobToolDispatcher {
    async fn dispatch_mob_spawn_member(
        &self,
        call: meerkat_core::types::ToolCallView<'_>,
        context: &meerkat_core::ToolDispatchContext,
    ) -> Result<meerkat_core::ToolDispatchOutcome, meerkat_core::ToolError> {
        let mut args = serde_json::from_str::<Value>(call.args.get()).map_err(|error| {
            meerkat_core::ToolError::invalid_arguments(call.name, error.to_string())
        })?;
        let idle_retire_override = delegate_idle_retire_override_from_args(call.name, &mut args)?;
        let idle_retire_targets = idle_retire_targets_from_spawn_args(&args);
        if let Some(object) = args.as_object_mut() {
            object
                .entry("auto_wire_parent".to_string())
                .or_insert(Value::Bool(true));
        }
        let name = call.name.to_string();
        let args_for_console = args.clone();
        let args = serde_json::value::RawValue::from_string(args.to_string()).map_err(|error| {
            meerkat_core::ToolError::invalid_arguments(call.name, error.to_string())
        })?;
        let call = meerkat_core::types::ToolCallView {
            id: call.id,
            name: call.name,
            args: &args,
        };
        let outcome = self.inner.dispatch_with_context(call, context).await?;
        self.register_idle_retire_override_from_outcome(
            &outcome,
            idle_retire_override,
            &idle_retire_targets,
        )
        .await;
        self.project_spawn_to_console(&name, &args_for_console, &outcome, context)
            .await;

        Ok(outcome)
    }

    async fn dispatch_delegate(
        &self,
        call: meerkat_core::types::ToolCallView<'_>,
        context: &meerkat_core::ToolDispatchContext,
    ) -> Result<meerkat_core::ToolDispatchOutcome, meerkat_core::ToolError> {
        let mut args = serde_json::from_str::<Value>(call.args.get()).map_err(|error| {
            meerkat_core::ToolError::invalid_arguments(call.name, error.to_string())
        })?;
        let idle_retire_override = delegate_idle_retire_override_from_args(call.name, &mut args)?;
        let name = call.name.to_string();
        let args_for_console = args.clone();
        let args = serde_json::value::RawValue::from_string(args.to_string()).map_err(|error| {
            meerkat_core::ToolError::invalid_arguments(call.name, error.to_string())
        })?;
        let call = meerkat_core::types::ToolCallView {
            id: call.id,
            name: call.name,
            args: &args,
        };
        let outcome = self.inner.dispatch_with_context(call, context).await?;

        self.register_idle_retire_override_from_outcome(&outcome, idle_retire_override, &[])
            .await;
        self.project_spawn_to_console(&name, &args_for_console, &outcome, context)
            .await;

        Ok(outcome)
    }

    /// `fork_off` seats the child in the caller's own mob, where idle
    /// retirement is opt-in. A fork that nobody retires holds one of the
    /// bounded sessions forever, so the child is opted in on the runtime
    /// default unless the caller passes an integer `idle_retire_secs` (null
    /// is the same as omitting it).
    async fn dispatch_fork_off(
        &self,
        call: meerkat_core::types::ToolCallView<'_>,
        context: &meerkat_core::ToolDispatchContext,
    ) -> Result<meerkat_core::ToolDispatchOutcome, meerkat_core::ToolError> {
        let mut args = serde_json::from_str::<Value>(call.args.get()).map_err(|error| {
            meerkat_core::ToolError::invalid_arguments(call.name, error.to_string())
        })?;
        let idle_retire_override = fork_off_idle_retire_override_from_args(call.name, &mut args)?;
        let name = call.name.to_string();
        let args_for_console = args.clone();
        let args = serde_json::value::RawValue::from_string(args.to_string()).map_err(|error| {
            meerkat_core::ToolError::invalid_arguments(call.name, error.to_string())
        })?;
        let call = meerkat_core::types::ToolCallView {
            id: call.id,
            name: call.name,
            args: &args,
        };
        let outcome = self.inner.dispatch_with_context(call, context).await?;

        self.register_idle_retire_override_from_outcome(&outcome, Some(idle_retire_override), &[])
            .await;
        self.project_spawn_to_console(&name, &args_for_console, &outcome, context)
            .await;

        Ok(outcome)
    }

    /// `mob_respawn` keeps the member (a forker respawning its own child), so
    /// the child keeps its idle-retire opt-in: the respawn goes through the
    /// same carry as every other Meta respawn surface.
    async fn dispatch_mob_respawn(
        &self,
        call: meerkat_core::types::ToolCallView<'_>,
        context: &meerkat_core::ToolDispatchContext,
    ) -> Result<meerkat_core::ToolDispatchOutcome, meerkat_core::ToolError> {
        let target = serde_json::from_str::<Value>(call.args.get())
            .ok()
            .and_then(|args| {
                Some((
                    args.get("mob_id")?.as_str()?.to_string(),
                    args.get("agent_identity")?.as_str()?.to_string(),
                ))
            });
        let handle = match target.as_ref() {
            Some((mob_id, _)) => {
                self.implicit_delegate_retirement_overrides
                    .mob_handle(mob_id)
                    .await
            }
            None => None,
        };
        match (target, handle) {
            (Some((_, member_id)), Some(handle)) => {
                let member = meerkat_mob::AgentIdentity::from(member_id.as_str());
                respawn_carrying_idle_retire_opt_in(
                    Some(&self.implicit_delegate_retirement_overrides),
                    &handle,
                    &member,
                    self.inner.dispatch_with_context(call, context),
                    |outcome| matches!(outcome, Ok(outcome) if !outcome.result.is_error),
                )
                .await
            }
            _ => self.inner.dispatch_with_context(call, context).await,
        }
    }

    /// Project a successful spawn into the console, when a console-bearing
    /// runtime installed a sink. Failure-isolated and additive: the spawn
    /// outcome is never altered, and a runtime without a console store
    /// behaves exactly as before.
    async fn project_spawn_to_console(
        &self,
        tool_name: &str,
        args: &Value,
        outcome: &meerkat_core::ToolDispatchOutcome,
        context: &meerkat_core::ToolDispatchContext,
    ) {
        if outcome.result.is_error {
            return;
        }
        let sink = self
            .console_spawn_sink
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let Some(sink) = sink else {
            return;
        };
        let mut seeds = crate::console_spawn::console_spawn_seeds(
            tool_name,
            args,
            &outcome.result.text_content(),
            self.spawner_comms_name.as_deref(),
        );
        // The dispatching session is the spawning incarnation: captured
        // console lineage binds to it rather than to the parent's name.
        let spawning_session = context.origin_session_id().map(ToString::to_string);
        for seed in &mut seeds {
            if seed.spawned_by.is_some() {
                seed.spawned_by_session = spawning_session.clone();
            }
        }
        for seed in &seeds {
            sink.project_spawned_member(seed).await;
        }
    }

    async fn register_idle_retire_override_from_outcome(
        &self,
        outcome: &meerkat_core::ToolDispatchOutcome,
        idle_retire_override: Option<DelegateIdleRetireOverride>,
        fallback_targets: &[IdleRetireTarget],
    ) {
        if outcome.result.is_error {
            return;
        }
        let Some(override_policy) = idle_retire_override else {
            return;
        };
        for target in
            idle_retire_targets_from_outcome_text(&outcome.result.text_content(), fallback_targets)
        {
            self.implicit_delegate_retirement_overrides
                .set(&target.mob_id, &target.member_id, override_policy)
                .await;
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct IdleRetireTarget {
    mob_id: String,
    member_id: String,
}

fn text_field<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
}

fn member_identity_field(value: &Value) -> Option<&str> {
    text_field(value, "agent_identity")
        .or_else(|| text_field(value, "member_id"))
        .or_else(|| text_field(value, "identity"))
}

fn target_from_value(value: &Value, default_mob_id: Option<&str>) -> Option<IdleRetireTarget> {
    let mob_id = text_field(value, "mob_id").or(default_mob_id)?;
    let member_id = member_identity_field(value)?;
    Some(IdleRetireTarget {
        mob_id: mob_id.to_string(),
        member_id: member_id.to_string(),
    })
}

fn idle_retire_targets_from_spawn_args(args: &Value) -> Vec<IdleRetireTarget> {
    let default_mob_id = text_field(args, "mob_id");
    let mut targets = BTreeSet::new();
    if let Some(target) = target_from_value(args, default_mob_id) {
        targets.insert(target);
    }
    for key in ["specs", "members"] {
        let Some(values) = args.get(key).and_then(Value::as_array) else {
            continue;
        };
        for value in values {
            if let Some(target) = target_from_value(value, default_mob_id) {
                targets.insert(target);
            }
        }
    }
    targets.into_iter().collect()
}

fn target_from_result_value(
    value: &Value,
    fallback_targets: &[IdleRetireTarget],
) -> Option<IdleRetireTarget> {
    if let Some(target) = target_from_value(value, None) {
        return Some(target);
    }
    let member_id = member_identity_field(value)?;
    let mut matches = fallback_targets
        .iter()
        .filter(|target| target.member_id == member_id);
    let target = matches.next()?;
    if matches.next().is_some() {
        return None;
    }
    Some(target.clone())
}

fn collect_idle_retire_result_targets(
    value: &Value,
    fallback_targets: &[IdleRetireTarget],
    targets: &mut BTreeSet<IdleRetireTarget>,
) {
    if let Some(target) = target_from_result_value(value, fallback_targets) {
        targets.insert(target);
    }
    for key in ["members", "specs", "spawned", "results"] {
        let Some(values) = value.get(key).and_then(Value::as_array) else {
            continue;
        };
        for value in values {
            collect_idle_retire_result_targets(value, fallback_targets, targets);
        }
    }
}

fn idle_retire_targets_from_outcome_text(
    text: &str,
    fallback_targets: &[IdleRetireTarget],
) -> Vec<IdleRetireTarget> {
    let Ok(payload) = serde_json::from_str::<Value>(text) else {
        return fallback_targets.to_vec();
    };
    let mut targets = BTreeSet::new();
    collect_idle_retire_result_targets(&payload, fallback_targets, &mut targets);
    if targets.is_empty() {
        targets.extend(fallback_targets.iter().cloned());
    }
    targets.into_iter().collect()
}

struct DefinitionSeededRealmProfileStore {
    inner: Arc<dyn meerkat_mob::RealmProfileStore>,
    profiles: BTreeMap<String, Profile>,
}

impl DefinitionSeededRealmProfileStore {
    fn new(
        definition: &MobDefinition,
        inner: Arc<dyn meerkat_mob::RealmProfileStore>,
    ) -> Option<Self> {
        let profiles = definition
            .profiles
            .iter()
            .filter_map(|(name, binding)| {
                binding
                    .as_inline()
                    .cloned()
                    .map(|profile| (name.to_string(), profile))
            })
            .collect::<BTreeMap<_, _>>();

        (!profiles.is_empty()).then_some(Self { inner, profiles })
    }

    fn stored(&self, name: &str, profile: &Profile) -> meerkat_mob::StoredRealmProfile {
        let now = chrono::Utc::now();
        meerkat_mob::StoredRealmProfile {
            name: name.to_string(),
            profile: profile.clone(),
            revision: 0,
            created_at: now,
            updated_at: now,
        }
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl meerkat_mob::RealmProfileStore for DefinitionSeededRealmProfileStore {
    async fn create(
        &self,
        name: &str,
        profile: &Profile,
    ) -> Result<meerkat_mob::StoredRealmProfile, meerkat_mob::MobStoreError> {
        if self.profiles.contains_key(name) {
            return Err(meerkat_mob::MobStoreError::CasConflict(format!(
                "realm profile already exists: {name}"
            )));
        }
        self.inner.create(name, profile).await
    }

    async fn get(
        &self,
        name: &str,
    ) -> Result<Option<meerkat_mob::StoredRealmProfile>, meerkat_mob::MobStoreError> {
        if let Some(profile) = self.profiles.get(name) {
            return Ok(Some(self.stored(name, profile)));
        }
        self.inner.get(name).await
    }

    async fn list(
        &self,
    ) -> Result<Vec<meerkat_mob::StoredRealmProfile>, meerkat_mob::MobStoreError> {
        let mut merged = self.inner.list().await?;
        merged.retain(|profile| !self.profiles.contains_key(profile.name.as_str()));
        merged.extend(
            self.profiles
                .iter()
                .map(|(name, profile)| self.stored(name, profile)),
        );
        merged.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(merged)
    }

    async fn update(
        &self,
        name: &str,
        profile: &Profile,
        expected_revision: u64,
    ) -> Result<meerkat_mob::StoredRealmProfile, meerkat_mob::MobStoreError> {
        if self.profiles.contains_key(name) {
            return Err(meerkat_mob::MobStoreError::CasConflict(format!(
                "realm profile '{name}' is provided by the mob definition"
            )));
        }
        self.inner.update(name, profile, expected_revision).await
    }

    async fn delete(
        &self,
        name: &str,
        expected_revision: u64,
    ) -> Result<meerkat_mob::StoredRealmProfile, meerkat_mob::MobStoreError> {
        if self.profiles.contains_key(name) {
            return Err(meerkat_mob::MobStoreError::CasConflict(format!(
                "realm profile '{name}' is provided by the mob definition"
            )));
        }
        self.inner.delete(name, expected_revision).await
    }
}

/// Strip and parse the model-facing `idle_retire_secs` argument of
/// `delegate` and `mob_spawn_member` (and, through
/// [`fork_off_idle_retire_override_from_args`], `fork_off`).
///
/// JSON null means "not set", exactly like an omitted field, as for every
/// `Option` argument of meerkat's own tools: models routinely send null for
/// optional fields, and a null must not silently pin a helper or fork child
/// forever. There is no model-facing "disable"; hosts that want no
/// retirement use the runtime default (`implicit_delegate_idle_retire_secs`).
/// [`DelegateIdleRetireOverride::Disabled`] stays for persisted rows and
/// host-side callers.
fn delegate_idle_retire_override_from_args(
    tool_name: &str,
    args: &mut Value,
) -> Result<Option<DelegateIdleRetireOverride>, meerkat_core::ToolError> {
    let Some(object) = args.as_object_mut() else {
        return Ok(None);
    };
    match object.remove("idle_retire_secs") {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_u64()
            .map(DelegateIdleRetireOverride::Seconds)
            .map(Some)
            .ok_or_else(|| {
                meerkat_core::ToolError::invalid_arguments(
                    tool_name,
                    "idle_retire_secs must be a non-negative integer, or null to use the default",
                )
            }),
    }
}

/// `fork_off` idle policy: an integer `idle_retire_secs` behaves exactly as
/// for `delegate`; an omitted or null one opts the fork child into the
/// runtime default instead of leaving it out of retirement.
fn fork_off_idle_retire_override_from_args(
    tool_name: &str,
    args: &mut Value,
) -> Result<DelegateIdleRetireOverride, meerkat_core::ToolError> {
    Ok(delegate_idle_retire_override_from_args(tool_name, args)?
        .unwrap_or(DelegateIdleRetireOverride::RuntimeDefault))
}

/// The one place the wrapper's advertised definitions differ from the inner
/// surface; `tools()` and `tool_catalog()` must apply the same patch.
fn idle_retire_patched_tool_def(
    tool: &Arc<meerkat_core::types::ToolDef>,
) -> Arc<meerkat_core::types::ToolDef> {
    if tool.name == "delegate" || tool.name == "fork_off" {
        Arc::new(delegate_tool_def_with_idle_retire_secs(tool))
    } else if tool.name == "mob_spawn_member" {
        Arc::new(mob_spawn_tool_def_with_idle_retire_secs(tool))
    } else {
        Arc::clone(tool)
    }
}

fn delegate_tool_def_with_idle_retire_secs(
    tool: &meerkat_core::types::ToolDef,
) -> meerkat_core::types::ToolDef {
    let mut patched = tool.clone();
    if !patched.description.contains("IDLE RETIREMENT:") {
        patched.description.push_str(
            "\n\nIDLE RETIREMENT:\n\
             Omit idle_retire_secs to use the runtime default. Pass an integer \
             number of seconds to override idle auto-retirement for this helper. \
             null is the same as omitting it.",
        );
    }
    if let Some(properties) = patched
        .input_schema
        .get_mut("properties")
        .and_then(Value::as_object_mut)
    {
        properties
            .entry("idle_retire_secs".to_string())
            .or_insert_with(|| {
                serde_json::json!({
                    "description": "Override idle auto-retirement for this helper: an integer number of seconds. Omit to use the runtime default; null is the same as omitting it.",
                    "anyOf": [
                        {"type": "integer", "minimum": 0},
                        {"type": "null"}
                    ]
                })
            });
    }
    patched
}

fn mob_spawn_tool_def_with_idle_retire_secs(
    tool: &meerkat_core::types::ToolDef,
) -> meerkat_core::types::ToolDef {
    let mut patched = tool.clone();
    if !patched.description.contains("IDLE RETIREMENT:") {
        patched.description.push_str(
            "\n\nIDLE RETIREMENT:\n\
             Omit idle_retire_secs to leave this spawned member out of auto-retirement. \
             Pass an integer number of seconds to retire the member after it has been \
             idle for that long. null is the same as omitting it.",
        );
    }
    if let Some(properties) = patched
        .input_schema
        .get_mut("properties")
        .and_then(Value::as_object_mut)
    {
        properties
            .entry("idle_retire_secs".to_string())
            .or_insert_with(|| {
                serde_json::json!({
                    "description": "Opt this spawned member into idle auto-retirement: an integer number of seconds to retire after that much idle time. Omit to keep the member indefinitely; null is the same as omitting it.",
                    "anyOf": [
                        {"type": "integer", "minimum": 0},
                        {"type": "null"}
                    ]
                })
            });
    }
    patched
}

type AgentMobToolsSlot =
    Arc<std::sync::RwLock<Option<Arc<dyn meerkat_core::service::MobToolsFactory>>>>;

/// Builder tool slots whose contents point back at the composition that owns
/// the builder: the agent mob-tool factory (session service and machine) and
/// the WorkGraph admission (session service and mob handle). Each is a
/// reference cycle through the session service's builder. The slots are
/// emptied at shutdown, and in any case when the last holder of this release
/// drops: a spec abandoned by a failed build, or the last clone of a runtime
/// dropped without shutdown.
#[derive(Clone, Default)]
pub(crate) struct CompositionRelease(Arc<CompositionReleaseSlots>);

type SlotClear = Box<dyn Fn() + Send + Sync>;

#[derive(Default)]
struct CompositionReleaseSlots(std::sync::Mutex<Vec<SlotClear>>);

impl CompositionRelease {
    fn register_slot<T: ?Sized + Send + Sync + 'static>(
        &self,
        slot: &Arc<std::sync::RwLock<Option<Arc<T>>>>,
    ) {
        let slot = Arc::clone(slot);
        self.0
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(Box::new(move || {
                *slot
                    .write()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
            }));
    }

    pub(crate) fn release(&self) {
        self.0.release();
    }
}

impl CompositionReleaseSlots {
    fn release(&self) {
        for clear in self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
        {
            clear();
        }
    }
}

impl Drop for CompositionReleaseSlots {
    fn drop(&mut self) {
        self.release();
    }
}

/// What [`install_agent_mob_tools`] was given, so a spec can reinstall the
/// agent mob-tool state with its final child policy before bootstrap.
#[derive(Clone)]
pub(crate) struct AgentMobToolsInstall {
    slot: AgentMobToolsSlot,
    session_service: Arc<dyn MobSessionService>,
    runtime_adapter: Option<Arc<meerkat_runtime::MeerkatMachine>>,
    workgraph_service: Option<meerkat::WorkGraphService>,
    default_llm_client_slot: SharedDefaultLlmClientSlot,
    council_store: Option<Arc<dyn meerkat_mob::store::TemporaryCouncilStore>>,
    forked_participant_store: Option<Arc<dyn meerkat_mob::store::ForkedParticipantStore>>,
    delivery: MobRuntimeDelivery,
    /// The runtime root the installed state persists child mobs under;
    /// `None` while child mobs are process-bound.
    child_mob_root: Option<PathBuf>,
    /// A host job source was composed after the state was bound: bootstrap
    /// rebuilds and rebinds it even when nothing else changed.
    host_job_source_changed: bool,
}

/// The host application tool policy for child mobs (agent `mob_create` and
/// delegate's implicit mob), applied to the agent mob-tool state.
#[derive(Clone, Default)]
pub(crate) struct AgentMobChildPolicy {
    registry: Option<Arc<meerkat_core::ToolConsequencePolicyRegistry>>,
    binding: Option<meerkat_core::ApplicationToolPolicyBinding>,
    bundles: meerkat_mob_mcp::ChildToolBundles,
    has_child_bundles: bool,
    /// The host's member registration hook, so child-mob members register
    /// through the same native seam as the root mob's members.
    registration_hook: Option<Arc<dyn meerkat_mob::MemberRuntimeRegistrationHook>>,
}

impl AgentMobChildPolicy {
    /// Whether there is anything to forward: without a registry, a binding,
    /// a bundle available or offered to child mobs, or a registration hook,
    /// the installed state already matches.
    fn is_empty(&self) -> bool {
        self.registry.is_none()
            && self.binding.is_none()
            && !self.has_child_bundles
            && self.registration_hook.is_none()
    }
}

type InstalledAgentMobTools = (
    Arc<meerkat_mob_mcp::MobMcpState>,
    ImplicitDelegateRetirementOverrides,
    SharedDefaultLlmClientSlot,
    SharedConsoleSpawnSinkSlot,
    SharedIdentityRuntimeSlot,
    AgentMobToolsInstall,
);

/// Install the agent mob tools with no child policy yet; the spec applies its
/// final child policy at bootstrap (see `MobBootstrapSpec::apply_agent_mob_child_policy`).
#[allow(
    clippy::too_many_arguments,
    reason = "composition passes each host-owned service and delivery handle explicitly"
)]
fn install_agent_mob_tools(
    definition: &MobDefinition,
    slot: Arc<std::sync::RwLock<Option<Arc<dyn meerkat_core::service::MobToolsFactory>>>>,
    session_service: Arc<dyn MobSessionService>,
    runtime_adapter: Option<Arc<meerkat_runtime::MeerkatMachine>>,
    workgraph_service: Option<meerkat::WorkGraphService>,
    default_llm_client_slot: Option<SharedDefaultLlmClientSlot>,
    council_store: Option<Arc<dyn meerkat_mob::store::TemporaryCouncilStore>>,
    forked_participant_store: Option<Arc<dyn meerkat_mob::store::ForkedParticipantStore>>,
    delivery: MobRuntimeDelivery,
) -> Result<InstalledAgentMobTools, MobRuntimeDeliveryError> {
    // Child mobs start process-bound: a durable child root is applied at the
    // bootstrap boundary, so a caller can still opt out before any path opens.
    let state = new_agent_mob_state(
        Arc::clone(&session_service),
        runtime_adapter.clone(),
        workgraph_service.clone(),
        council_store.clone(),
        forked_participant_store.clone(),
    )?;
    let (state, overrides, llm_slot, console_spawn_sink, identity_runtime) =
        install_agent_mob_tools_with(
            definition,
            Arc::clone(&slot),
            state,
            default_llm_client_slot,
            AgentMobChildPolicy::default(),
            None,
            &delivery,
            None,
        )?;
    let install = AgentMobToolsInstall {
        slot,
        session_service,
        runtime_adapter,
        workgraph_service,
        default_llm_client_slot: Arc::clone(&llm_slot),
        council_store,
        forked_participant_store,
        delivery,
        child_mob_root: None,
        host_job_source_changed: false,
    };
    Ok((
        state,
        overrides,
        llm_slot,
        console_spawn_sink,
        identity_runtime,
        install,
    ))
}

/// The unshared agent mob-tool state with its explicit stores, before any
/// child root, child policy or realm-profile overlay is applied.
fn new_agent_mob_state(
    session_service: Arc<dyn MobSessionService>,
    runtime_adapter: Option<Arc<meerkat_runtime::MeerkatMachine>>,
    workgraph_service: Option<meerkat::WorkGraphService>,
    // Durable temporary-council store, when the host resolved one.
    //
    // `MobMcpState` defaults this to an in-memory implementation, so `None`
    // preserves the pre-0.8.29 behaviour exactly: councils stay process-bound
    // and do not survive a restart. Supplying one is what makes
    // `TemporaryCouncilStore::list_unfinished` able to see anything after a
    // reboot, which is the whole point of the recovery path.
    council_store: Option<Arc<dyn meerkat_mob::store::TemporaryCouncilStore>>,
    forked_participant_store: Option<Arc<dyn meerkat_mob::store::ForkedParticipantStore>>,
) -> Result<meerkat_mob_mcp::MobMcpState, MobRuntimeDeliveryError> {
    // Forward the workgraph service so agent-spawned child mobs
    // (delegate / mob_spawn_member) inherit apply-time attention overlays.
    let mut state = meerkat_mob_mcp::MobMcpState::new_with_runtime_adapter(
        session_service,
        runtime_adapter,
        meerkat_mob::MobControlPrincipal::Owner,
    )?
    .with_workgraph_service(workgraph_service)
    // Child mobs refuse runtime-plane member identities like the root mob.
    // The child application tool policy, which runs after it, never changes
    // a member's identity or labels.
    .with_child_spawn_member_customizer(
        crate::member_comms_id::RuntimePlaneIdentityGuard::customizer(),
    );
    // Explicit stores are installed before any persistent child root: the
    // native root upgrades only stores still at their default, so these stay
    // the host's choice.
    if let Some(council_store) = council_store {
        state = state.with_temporary_council_store(council_store);
    }
    // The root handle is built separately and inserted into this state. Its
    // source-owned capabilities must remain readable when a temporary council
    // seats them in a mob built by this state. Absence stays absent on the
    // root storage; never replace it with the state's default custody store.
    if let Some(store) = forked_participant_store {
        state = state.with_forked_participant_store(store);
    }
    Ok(state)
}

/// Finish and share the agent mob-tool state: child policy, realm-profile
/// overlay, continuation binding and the published tool factory.
#[allow(clippy::too_many_arguments)]
fn install_agent_mob_tools_with(
    definition: &MobDefinition,
    slot: Arc<std::sync::RwLock<Option<Arc<dyn meerkat_core::service::MobToolsFactory>>>>,
    mut state: meerkat_mob_mcp::MobMcpState,
    default_llm_client_slot: Option<SharedDefaultLlmClientSlot>,
    child_policy: AgentMobChildPolicy,
    existing_slots: Option<(SharedConsoleSpawnSinkSlot, SharedIdentityRuntimeSlot)>,
    delivery: &MobRuntimeDelivery,
    replaced: Option<meerkat::ContinuationBindingGeneration>,
) -> Result<
    (
        Arc<meerkat_mob_mcp::MobMcpState>,
        ImplicitDelegateRetirementOverrides,
        SharedDefaultLlmClientSlot,
        SharedConsoleSpawnSinkSlot,
        SharedIdentityRuntimeSlot,
    ),
    MobRuntimeDeliveryError,
> {
    let default_llm_client_slot = default_llm_client_slot
        .unwrap_or_else(|| Arc::new(std::sync::RwLock::new(None::<Arc<dyn LlmClient>>)));
    let default_llm_client_provider_slot = Arc::clone(&default_llm_client_slot);
    // The host's application tool policy for child mobs (agent `mob_create`
    // and delegate's implicit mob): the registry, the binding every child
    // member is built with, and the bundles the host makes available to them.
    if let Some(registry) = child_policy.registry {
        state = state.with_tool_consequence_policy_registry(registry);
    }
    if let Some(binding) = child_policy.binding {
        state = state.with_child_application_tool_policy(binding);
    }
    state = state.with_child_tool_bundles(child_policy.bundles);
    if let Some(hook) = child_policy.registration_hook {
        state = state.with_member_runtime_registration_hook(hook);
    }
    // The definition overlay wraps whatever profile store the state holds:
    // with a durable child root that is the native durable store, so authored
    // rows survive a restart while inline definition profiles keep precedence.
    if let Some(base_store) = state.realm_profile_store().cloned()
        && let Some(store) = DefinitionSeededRealmProfileStore::new(definition, base_store)
    {
        state = state.with_realm_profile_store(Some(Arc::new(store)));
    }
    state = state
        .with_realm_skill_sources(definition.skills.clone())
        .with_default_llm_client_provider(Some(Arc::new(move || {
            default_llm_client_provider_slot
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        })));
    // `into_shared`, not `Arc::new`: the shared state owns its restore
    // sweeps. With a durable council store (supplied above for persistent
    // mobs), restoring a mob through `mob_insert_handle` schedules meerkat's
    // council sweep: recovery of councils a restart interrupted, the retry
    // once the dead coordinator's claim lease lapses, and the re-link that
    // tells a detached council's convener the outcome. A state shared with
    // `Arc::new` cannot own that task, so the sweep never ran for Meta.
    // The host's own committed results join the job source this state binds
    // before its first delivery pass.
    if let Some(host) = delivery.host_job_source() {
        state = state.with_host_job_source(host);
    }
    let state = state.into_shared();
    delivery.bind(&state, replaced)?;
    let implicit_delegate_retirement_overrides =
        ImplicitDelegateRetirementOverrides::with_seat_resolver(IdleRetireSeatResolver::new(
            &state,
            definition.id.to_string(),
        ));
    // A reinstall keeps the slots already handed out.
    let (console_spawn_sink, identity_runtime) = existing_slots.unwrap_or_else(|| {
        (
            new_console_spawn_sink_slot(),
            Arc::new(std::sync::RwLock::new(None)),
        )
    });
    let inner = Arc::new(meerkat_mob_mcp::AgentMobToolSurfaceFactory::new(
        Arc::clone(&state),
    ));
    let factory = Arc::new(AutoWireParentMobToolsFactory {
        inner,
        implicit_delegate_retirement_overrides: implicit_delegate_retirement_overrides.clone(),
        console_spawn_sink: Arc::clone(&console_spawn_sink),
        identity_runtime: Arc::clone(&identity_runtime),
        protected_mob_id: definition.id.to_string(),
    });
    *slot
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(factory);
    Ok((
        state,
        implicit_delegate_retirement_overrides,
        default_llm_client_slot,
        console_spawn_sink,
        identity_runtime,
    ))
}

#[cfg(test)]
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub(crate) struct RuntimeTurnTrace {
    pub(crate) session_id: String,
    pub(crate) boundary: String,
    pub(crate) contributing_input_count: usize,
    pub(crate) outcome: String,
}

fn is_replay_unsafe_server_tool_content(name: &str, content: &Value) -> bool {
    name == "web_search_annotations"
        || content
            .get("type")
            .and_then(Value::as_str)
            .is_some_and(|kind| kind.starts_with("response."))
}

fn sanitize_llm_request_for_stateless_replay(request: &LlmRequest) -> LlmRequest {
    let mut sanitized = request.clone();
    sanitized.messages = request
        .messages
        .iter()
        .cloned()
        .map(sanitize_message_for_stateless_replay)
        .collect();
    sanitized
}

fn sanitize_create_session_request_llm_override(req: &mut CreateSessionRequest) {
    let Some(build) = req.build.as_mut() else {
        return;
    };
    let Some(client) = build
        .llm_client_override
        .as_ref()
        .and_then(meerkat::decode_llm_client_override_from_service)
    else {
        return;
    };
    build.llm_client_override = Some(meerkat::encode_llm_client_override_for_service(
        ReplaySanitizingLlmClient::wrap(client),
    ));
}

#[cfg(test)]
const NATIVE_SHELL_TOOL_NAMES: [&str; 4] = [
    "shell",
    "shell_job_status",
    "shell_jobs",
    "shell_job_cancel",
];
const LEGACY_SHELL_AND_COMMS_TOOL_NAMES: [&str; 8] = [
    "shell",
    "shell_job_status",
    "shell_jobs",
    "shell_job_cancel",
    "peers",
    "send_message",
    "send_request",
    "send_response",
];

fn is_legacy_shell_and_comms_allow_filter(filter: &meerkat_core::ToolFilter) -> bool {
    let meerkat_core::ToolFilter::Allow(names) = filter else {
        return false;
    };
    let actual = names
        .iter()
        .map(meerkat_core::ToolName::as_str)
        .collect::<std::collections::BTreeSet<_>>();
    let expected = LEGACY_SHELL_AND_COMMS_TOOL_NAMES
        .iter()
        .copied()
        .collect::<std::collections::BTreeSet<_>>();
    actual == expected
}

/// Lift the exact legacy synthetic Allow-8 filter on resume.
///
/// Meerkat 0.8.22 composes shell as an independent native category, so Meta
/// must no longer enable the broad builtin category and hide the resulting
/// surface behind a global allow-list. Existing sessions can still carry that
/// old filter durably. Match only the byte-era fingerprint established from
/// the downstream app's production specimen:
///
/// - active filter set-equals the four shell plus four comms tools;
/// - staged filter set-equals the same set, or is `All` because the field was
///   absent in the legacy serialized form;
/// - this build has no explicit caller-supplied initial filter.
///
/// The repair is expressed through `initial_tool_filter = All`. Meerkat's
/// generated visibility authority then applies and persists the transition,
/// including witness cleanup. Meta never rewrites the visibility document,
/// so capability and inherited base filters remain untouched and owned by the
/// upstream state machine.
fn heal_legacy_shell_tool_visibility(
    req: &mut CreateSessionRequest,
    persisted_resume: Option<&meerkat_core::Session>,
) {
    let Some(build) = req.build.as_mut() else {
        return;
    };
    if build.initial_tool_filter.is_some() {
        return;
    }
    let Some(resume_session) = build.resume_session.as_ref() else {
        return;
    };
    let Some(persisted_resume) = persisted_resume else {
        return;
    };
    if resume_session.id() != persisted_resume.id() {
        return;
    }
    let Ok(Some(state)) = persisted_resume.try_tool_visibility_state() else {
        return;
    };
    if !is_legacy_shell_and_comms_allow_filter(&state.active_filter) {
        return;
    }
    let staged_matches = is_legacy_shell_and_comms_allow_filter(&state.staged_filter)
        || matches!(state.staged_filter, meerkat_core::ToolFilter::All);
    if staged_matches {
        build.initial_tool_filter = Some(meerkat_core::ToolFilter::All);
    }
}

fn sanitize_message_for_stateless_replay(message: Message) -> Message {
    match message {
        Message::BlockAssistant(mut assistant) => {
            assistant.blocks = assistant
                .blocks
                .into_iter()
                .filter_map(|block| match block {
                    // Meerkat 0.7 types the server-tool name as `ServerToolKind`;
                    // the replay-unsafe predicate keys on the provider-native name.
                    AssistantBlock::ServerToolContent { kind, content, .. }
                        if is_replay_unsafe_server_tool_content(kind.provider_name(), &content) =>
                    {
                        None
                    }
                    other => Some(other),
                })
                .collect();
            Message::BlockAssistant(assistant)
        }
        other => other,
    }
}

/// How a composite storage provider's session store is reported.
const PROVIDER_SESSION_STORE_KIND: &str = "storage provider session store";

/// Core's session store and runtime store over one `runtime.sqlite`.
type NativeSessionPair = (
    Arc<dyn SessionStore>,
    Arc<dyn meerkat_runtime::RuntimeStore>,
);

/// The session and runtime stores a persistent composition runs on, with
/// the session store's kind, the runtime store's slot summary and where the
/// runtime store lives.
type ResolvedSessionStores = (
    Arc<dyn SessionStore>,
    &'static str,
    Arc<dyn meerkat_runtime::RuntimeStore>,
    StorageSlotSummary,
    Option<crate::storage_health::RuntimeStoreLocator>,
);

/// Core's HeadCanonical pair on `store_path/runtime.sqlite`, composed as
/// Core's realm composes its own: the session store and the runtime store
/// share the one database, and the runtime store hosts sessions under the
/// realm's hosting paths ([`meerkat_store::realm_paths_in`]). Opening the
/// runtime store pins the head-canonical profile, so a composition runs the
/// startup checks ([`crate::native_startup`]) first. Fail-closed: an open
/// failure is a startup error, never an in-memory fallback.
///
/// # Errors
///
/// [`RuntimeStoreResolutionError`] when either store cannot be opened on
/// `store_path/runtime.sqlite`.
pub fn open_native_session_pair(
    store_path: &Path,
    realm: &meerkat_core::RealmId,
) -> Result<NativeSessionPair, RuntimeStoreResolutionError> {
    let runtime_db = store_path.join(crate::storage_layout::RUNTIME_DB_FILE_NAME);
    let unavailable = |message: String| RuntimeStoreResolutionError {
        path: runtime_db.clone(),
        message,
    };
    let session_store = meerkat_store::SqliteSessionStore::open(runtime_db.clone())
        .map_err(|error| unavailable(error.to_string()))?;
    let paths = meerkat_store::realm_paths_in(store_path, realm.as_str());
    let runtime_store =
        meerkat_runtime::store::SqliteRuntimeStore::new_head_canonical(runtime_db.clone())
            .map_err(|error| unavailable(error.to_string()))?
            .with_hosting_paths(meerkat_runtime::HostingPaths {
                hosting_lock_dir: paths.session_hosting_lock_dir(),
                cold_delivery_lock: paths.cold_delivery_lock_path(),
                database: None,
            });
    let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> = Arc::new(runtime_store);
    // A same-process successor's custody probe recognizes this store's lock
    // while the store is alive.
    crate::native_startup::record_own_runtime_store(store_path, &runtime_store);
    Ok((Arc::new(session_store), runtime_store))
}

#[cfg(test)]
static RUNTIME_TURN_TRACES: OnceLock<Mutex<Vec<RuntimeTurnTrace>>> = OnceLock::new();

#[cfg(test)]
fn runtime_turn_traces() -> &'static Mutex<Vec<RuntimeTurnTrace>> {
    RUNTIME_TURN_TRACES.get_or_init(|| Mutex::new(Vec::new()))
}

#[cfg(test)]
#[allow(clippy::expect_used)]
fn record_runtime_turn_trace(trace: RuntimeTurnTrace) {
    runtime_turn_traces()
        .lock()
        .expect("runtime turn traces mutex")
        .push(trace);
}

#[cfg(test)]
#[allow(dead_code)]
#[allow(clippy::expect_used)]
pub(crate) fn take_runtime_turn_traces() -> Vec<RuntimeTurnTrace> {
    std::mem::take(
        &mut *runtime_turn_traces()
            .lock()
            .expect("runtime turn traces mutex"),
    )
}

#[cfg(not(test))]
#[allow(dead_code)]
fn record_runtime_turn_trace(_trace: ()) {}

fn runtime_turn_diagnostics_enabled() -> bool {
    std::env::var("META_TRACE_RUNTIME_TURNS").is_ok_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

/// Whether the session factory should wire the image-generation substrate for
/// this definition. Meerkat owns the per-profile visibility decision via
/// `profile.tools.image_generation`; Meta only needs to make the runtime
/// machine available when a profile opts in, or when a realm profile may resolve
/// to an opt-in profile at spawn time.
pub fn mob_definition_may_use_image_generation(definition: &MobDefinition) -> bool {
    definition.profiles.values().any(|binding| {
        binding
            .as_inline()
            .is_none_or(|profile| profile.tools.image_generation)
    })
}

/// Whether the session factory should make the native shell dispatcher
/// available for profiles that opt into `profile.tools.shell`.
pub fn mob_definition_may_use_shell(definition: &MobDefinition) -> bool {
    definition.profiles.values().any(|binding| {
        binding
            .as_inline()
            .is_none_or(|profile| profile.tools.shell)
    })
}

fn normalize_runtime_turn_request(
    mut req: meerkat_core::service::StartTurnRequest,
) -> meerkat_core::service::StartTurnRequest {
    // Queue/Steer and render metadata are runtime-owned semantics. By the
    // time apply_runtime_turn() invokes the session service, the runtime
    // has already chosen the boundary and recorded the metadata it needs.
    // The direct agent/session path is queue-only, so forward a normalized
    // turn request to avoid re-injecting runtime-only semantics.
    req.runtime.handling_mode = meerkat_core::types::HandlingMode::Queue;
    // Meerkat 0.7: render metadata lives only on the typed turn-metadata
    // carrier; strip it there instead of the removed flat field.
    if let Some(metadata) = req.runtime.turn_metadata.as_mut() {
        metadata.render_metadata = None;
    }
    req
}

/// Apply a machine-authorized cooperative cancel as an idempotent quiescence
/// operation. Once the generated runtime machine has admitted the cancel,
/// absence of a live session (or absence of a running turn) proves that there
/// is no lower-plane work left to interrupt. Propagating either observation
/// back as a failed effect wedges the machine in its pre-cancel phase and can
/// make whole-mob shutdown retain otherwise releasable identity authority.
async fn cancel_after_boundary_with_machine_authority_if_live(
    service: &dyn MobSessionService,
    session_id: &meerkat_core::types::SessionId,
    expected_run_id: &meerkat_core::lifecycle::RunId,
    authority: meerkat_runtime::MachineSessionControlAuthority,
) -> Result<(), SessionError> {
    service
        .cancel_after_boundary_with_machine_authority(session_id, expected_run_id, authority)
        .await
        .or_else(|error| match error {
            SessionError::NotFound { .. } | SessionError::NotRunning { .. } => Ok(()),
            error => Err(error),
        })
}

/// A durable fork whose child would take one of the console's runtime-plane
/// identities is refused before the transcript is forked: Core seats that
/// child without asking a spawn customizer.
fn refuse_runtime_plane_fork(
    target: &meerkat_core::DurableSessionForkTarget,
) -> Result<(), SessionError> {
    crate::member_comms_id::refuse_runtime_plane_fork_target(target).map_err(|message| {
        SessionError::FailedWithData {
            message,
            data: serde_json::json!({ "error": "reserved_member_identity" }),
        }
    })
}

/// Implement all `MobSessionService` super-traits by delegating to `self.inner`,
/// overriding only `create_session` to apply the pre-build hook.
macro_rules! delegate_mob_session_service {
    ($wrapper:ty) => {
        #[async_trait]
        impl meerkat_core::service::SessionService for $wrapper {
            async fn read_tool_application_observations(
                &self,
                id: &meerkat_core::SessionId,
            ) -> Result<Vec<meerkat_core::ToolApplicationObservation>, SessionError> {
                self.inner.read_tool_application_observations(id).await
            }

            async fn tool_application(self: Arc<Self>, control: Arc<meerkat_core::ToolApplicationControlRequest>) -> Result<serde_json::Value, SessionError> {
                Arc::clone(&self.inner).tool_application(control).await
            }

            async fn create_session(
                &self,
                req: CreateSessionRequest,
            ) -> Result<meerkat_core::types::RunResult, SessionError> {
                let (req, context, capability_context) =
                    self.prepare_create_request(req).await?;
                let result = self.inner.create_session(req).await?;
                Ok(self
                    .complete_create(result, context, capability_context)
                    .await)
            }
            async fn start_turn(
                &self,
                id: &meerkat_core::types::SessionId,
                req: meerkat_core::service::StartTurnRequest,
            ) -> Result<meerkat_core::types::RunResult, SessionError> {
                self.inner.start_turn(id, req).await
            }
            async fn reconcile_runtime_compaction_projections(
                &self,
                id: &meerkat_core::types::SessionId,
                intents: Vec<meerkat_core::CompactionProjectionIntent>,
            ) -> Result<(), SessionError> {
                self.inner
                    .reconcile_runtime_compaction_projections(id, intents)
                    .await
            }
            // Forwarded: the trait default refuses, so an unforwarded
            // acknowledgement leaves the live actor behind the store's
            // compaction finalization and costs the member its next turn.
            async fn acknowledge_runtime_compaction_finalization(
                &self,
                id: &meerkat_core::types::SessionId,
                authority: &meerkat_core::CommittedSessionBoundaryAuthority,
            ) -> Result<(), SessionError> {
                self.inner
                    .acknowledge_runtime_compaction_finalization(id, authority)
                    .await
            }
            async fn abort_uncommitted_compaction_projections(
                &self,
                id: &meerkat_core::types::SessionId,
            ) -> Result<(), SessionError> {
                self.inner
                    .abort_uncommitted_compaction_projections(id)
                    .await
            }
            async fn abort_rejected_runtime_run_projections(
                &self,
                id: &meerkat_core::types::SessionId,
            ) -> Result<(), SessionError> {
                self.inner.abort_rejected_runtime_run_projections(id).await
            }
            async fn interrupt(
                &self,
                id: &meerkat_core::types::SessionId,
            ) -> Result<(), SessionError> {
                self.inner.interrupt(id).await
            }
            // meerkat 0.8.22 adds the exact-run hard cancel to `SessionService`
            // with an `Err(Unsupported)` default. This wrapper already forwards
            // the exact-run cooperative sibling
            // (`cancel_after_boundary_for_run`); without the same forward here
            // the decorator would answer "unsupported" on its own authority and
            // hide the inner service's real exact-run interrupt.
            async fn interrupt_run_if_current(
                &self,
                id: &meerkat_core::types::SessionId,
                expected_run_id: &meerkat_core::lifecycle::RunId,
            ) -> Result<bool, SessionError> {
                self.inner.interrupt_run_if_current(id, expected_run_id).await
            }
            async fn cancel_after_boundary(
                &self,
                id: &meerkat_core::types::SessionId,
            ) -> Result<(), SessionError> {
                self.inner.cancel_after_boundary(id).await
            }
            async fn cancel_after_boundary_for_run(
                &self,
                id: &meerkat_core::types::SessionId,
                expected_run_id: &meerkat_core::lifecycle::RunId,
            ) -> Result<(), SessionError> {
                self.inner
                    .cancel_after_boundary_for_run(id, expected_run_id)
                    .await
            }
            async fn set_session_client(
                &self,
                id: &meerkat_core::types::SessionId,
                client: Arc<dyn meerkat_core::AgentLlmClient>,
            ) -> Result<(), SessionError> {
                self.inner
                    .set_session_client(id, ReplaySanitizingAgentLlmClient::wrap(client))
                    .await
            }
            async fn hot_swap_session_llm_identity(
                &self,
                id: &meerkat_core::types::SessionId,
                client: Arc<dyn meerkat_core::AgentLlmClient>,
                identity: meerkat_core::session::SessionLlmIdentity,
                request_policy: meerkat_core::SessionLlmRequestPolicy,
            ) -> Result<(), SessionError> {
                self.inner
                    .hot_swap_session_llm_identity(
                        id,
                        ReplaySanitizingAgentLlmClient::wrap(client),
                        identity,
                        request_policy,
                    )
                    .await
            }
            async fn update_session_mob_authority_context(
                &self,
                id: &meerkat_core::types::SessionId,
                authority_context: Option<meerkat_core::service::MobToolAuthorityContext>,
            ) -> Result<(), SessionError> {
                self.inner
                    .update_session_mob_authority_context(id, authority_context)
                    .await
            }
            // The trait default is a typed `Unsupported`: unforwarded, a
            // member's durable MCP attachment intent change refuses through
            // this decorator instead of reaching the session's owner.
            async fn update_session_mcp_attachment_intent(
                &self,
                id: &meerkat_core::types::SessionId,
                mutation: meerkat_core::mcp_session_intent::McpAttachmentIntentMutation,
            ) -> Result<meerkat_core::mcp_session_intent::McpAttachmentIntentRecorded, SessionError>
            {
                self.inner
                    .update_session_mcp_attachment_intent(id, mutation)
                    .await
            }
            async fn has_live_session(
                &self,
                id: &meerkat_core::types::SessionId,
            ) -> Result<bool, SessionError> {
                self.inner.has_live_session(id).await
            }
            async fn set_session_tool_visibility_state(
                &self,
                id: &meerkat_core::types::SessionId,
                state: Option<meerkat_core::SessionToolVisibilityState>,
            ) -> Result<(), SessionError> {
                self.inner
                    .set_session_tool_visibility_state(id, state)
                    .await
            }
            async fn set_session_tool_filter(
                &self,
                id: &meerkat_core::types::SessionId,
                filter: meerkat_core::ToolFilter,
            ) -> Result<(), SessionError> {
                self.inner.set_session_tool_filter(id, filter).await
            }
            async fn read(
                &self,
                id: &meerkat_core::types::SessionId,
            ) -> Result<meerkat_core::service::SessionView, SessionError> {
                self.inner.read(id).await
            }
            async fn list(
                &self,
                query: meerkat_core::service::SessionQuery,
            ) -> Result<Vec<meerkat_core::service::SessionSummary>, SessionError> {
                self.inner.list(query).await
            }
            async fn archive(
                &self,
                id: &meerkat_core::types::SessionId,
            ) -> Result<(), SessionError> {
                self.inner.archive(id).await
            }
            async fn subscribe_session_events(
                &self,
                id: &meerkat_core::types::SessionId,
            ) -> Result<meerkat_core::comms::EventStream, meerkat_core::comms::StreamError> {
                meerkat_core::service::SessionService::subscribe_session_events(
                    self.inner.as_ref(),
                    id,
                )
                .await
            }
            async fn subscribe_session_events_from(
                &self,
                id: &meerkat_core::types::SessionId,
                cursor: meerkat_core::comms::SessionEventCursor,
            ) -> Result<
                meerkat_core::comms::SessionEventSubscription,
                meerkat_core::comms::StreamError,
            > {
                meerkat_core::service::SessionService::subscribe_session_events_from(
                    self.inner.as_ref(),
                    id,
                    cursor,
                )
                .await
            }
            async fn record_live_terminal_error(
                &self,
                id: &meerkat_core::types::SessionId,
                cause: meerkat_core::live_adapter::LiveAdapterErrorCode,
            ) -> Result<(), SessionError> {
                self.inner.record_live_terminal_error(id, cause).await
            }
            async fn record_live_output_audio_degraded(
                &self,
                id: &meerkat_core::types::SessionId,
                dropped: u64,
            ) -> Result<(), SessionError> {
                self.inner
                    .record_live_output_audio_degraded(id, dropped)
                    .await
            }
        }

        #[async_trait]
        impl meerkat_core::service::SessionServiceCommsExt for $wrapper {
            async fn comms_runtime(
                &self,
                id: &meerkat_core::types::SessionId,
            ) -> Option<Arc<dyn meerkat_core::agent::CommsRuntime>> {
                self.inner.comms_runtime(id).await
            }

            async fn event_injector(
                &self,
                id: &meerkat_core::types::SessionId,
            ) -> Option<Arc<dyn meerkat_core::EventInjector>> {
                self.inner.event_injector(id).await
            }

            async fn interaction_event_injector(
                &self,
                id: &meerkat_core::types::SessionId,
            ) -> Option<Arc<dyn meerkat_core::event_injector::SubscribableInjector>> {
                self.inner.interaction_event_injector(id).await
            }

            // Forwarded: the trait default sends on the raw comms runtime,
            // not through the inner service's own send.
            async fn send_comms(
                &self,
                id: &meerkat_core::types::SessionId,
                command: meerkat_core::CommsCommand,
            ) -> Option<Result<meerkat_core::SendReceipt, meerkat_core::SendError>> {
                self.inner.send_comms(id, command).await
            }
        }

        #[async_trait]
        impl meerkat_core::service::SessionServiceControlExt for $wrapper {
            async fn append_system_context(
                &self,
                id: &meerkat_core::types::SessionId,
                req: meerkat_core::service::AppendSystemContextRequest,
            ) -> Result<
                meerkat_core::service::AppendSystemContextResult,
                meerkat_core::service::SessionControlError,
            > {
                self.inner.append_system_context(id, req).await
            }
            // Forwarded: the trait defaults refuse both as unavailable, so
            // a governed append through the mob's service would fail.
            async fn append_authenticated_system_context(
                &self,
                control: Arc<meerkat_core::service::SystemContextControlRequest>,
            ) -> Result<
                meerkat_core::service::AppendSystemContextResult,
                meerkat_core::service::SessionControlError,
            > {
                self.inner.append_authenticated_system_context(control).await
            }
            async fn append_authorized_system_context(
                &self,
                prepared: meerkat_core::service::PreparedSystemContextAppend,
            ) -> Result<
                meerkat_core::service::AppendSystemContextResult,
                meerkat_core::service::SessionControlError,
            > {
                self.inner.append_authorized_system_context(prepared).await
            }
            async fn stage_tool_results(
                &self,
                id: &meerkat_core::types::SessionId,
                req: meerkat_core::service::StageToolResultsRequest,
            ) -> Result<meerkat_core::service::StageToolResultsResult, SessionError> {
                self.inner.stage_tool_results(id, req).await
            }
        }

        #[async_trait]
        impl meerkat_core::service::SessionServiceHistoryExt for $wrapper {
            async fn read_history(
                &self,
                id: &meerkat_core::types::SessionId,
                query: meerkat_core::service::SessionHistoryQuery,
            ) -> Result<meerkat_core::service::SessionHistoryPage, SessionError> {
                self.inner.read_history(id, query).await
            }
            async fn read_transcript_revision(
                &self,
                id: &meerkat_core::types::SessionId,
                query: meerkat_core::service::SessionTranscriptRevisionQuery,
            ) -> Result<meerkat_core::service::SessionTranscriptRevisionPage, SessionError> {
                self.inner.read_transcript_revision(id, query).await
            }
            async fn list_transcript_revisions(
                &self,
                id: &meerkat_core::types::SessionId,
                query: meerkat_core::service::SessionTranscriptRevisionListQuery,
            ) -> Result<meerkat_core::service::SessionTranscriptRevisionList, SessionError> {
                self.inner.list_transcript_revisions(id, query).await
            }
            // The trait default is a typed `Unsupported`: unforwarded, the
            // member instruction read refuses through this decorator.
            async fn read_instruction_activation_records(
                &self,
                id: &meerkat_core::types::SessionId,
                query: meerkat_core::InstructionActivationReadQuery,
            ) -> Result<meerkat_core::InstructionActivationReadPage, SessionError> {
                self.inner.read_instruction_activation_records(id, query).await
            }
        }

        #[async_trait]
        impl MobSessionService for $wrapper {
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

            // Forwarded exactly: the inner service answers from its live
            // status watch without waiting on a running turn. A wrapper-level
            // `read` would wait on the member's turn (meerkat 0.8.45 makes
            // this method required for that reason).
            async fn observe_member_status_view(
                &self,
                session_id: &meerkat_core::SessionId,
            ) -> Result<meerkat_mob::MemberStatusSessionView, SessionError> {
                self.inner.observe_member_status_view(session_id).await
            }

            // Forwarded exactly: the inner persistent service records the
            // interrupted-run notice in the durable transcript without a turn.
            // The trait default refuses, so behind this wrapper the notice
            // would stay owed instead of being recorded.
            async fn append_system_notice_under_runtime_turn_boundary(
                &self,
                session_id: &meerkat_core::SessionId,
                record: meerkat_core::types::SystemNoticeRecord,
            ) -> Result<(), SessionError> {
                self.inner
                    .append_system_notice_under_runtime_turn_boundary(session_id, record)
                    .await
            }

            // Forwarded exactly: the inner persistent service activates a
            // durable instruction under the runtime turn boundary. The trait
            // default refuses, so behind this wrapper activation would fail.
            async fn activate_instruction_under_runtime_turn_boundary(
                &self,
                session_id: &meerkat_core::SessionId,
                request: meerkat_core::InstructionActivationRequest,
                write_fence: Option<Arc<dyn meerkat_runtime::RuntimeStoreWriteFence>>,
            ) -> Result<meerkat_core::InstructionActivationMutation, SessionError> {
                self.inner
                    .activate_instruction_under_runtime_turn_boundary(
                        session_id,
                        request,
                        write_fence,
                    )
                    .await
            }

            // Forwarded exactly: the inner service observes the durable
            // source from authority, catalog and lifecycle rows without a
            // body read. Required since meerkat 0.8.47; a wrapper that
            // answered it itself would make every voice readiness poll a
            // whole-transcript read. It is also how a restored child mob's
            // owner resolves; an announcement test-fault build can answer
            // unobservable for one session (`mob_announcements::faults`).
            async fn observe_live_durable_source(
                &self,
                session_id: &meerkat_core::SessionId,
            ) -> Result<meerkat_mob::LiveDurableSourceObservation, SessionError> {
                if crate::mob_announcements::faults::owner_unobserved(session_id) {
                    return Err(SessionError::Store(Box::new(std::io::Error::other(
                        "announcement test fault: this session's durable source is not observed",
                    ))));
                }
                self.inner.observe_live_durable_source(session_id).await
            }

            // Forwarded exactly: a mob Stop or Shutdown awaits the inner
            // service's activity watch to learn that an interrupted member's
            // turn has ended. Required since meerkat 0.8.50.
            async fn subscribe_session_activity(
                &self,
                session_id: &meerkat_core::SessionId,
            ) -> Result<meerkat_mob::MemberSessionActivity, SessionError> {
                self.inner.subscribe_session_activity(session_id).await
            }

            #[cfg(feature = "openai-live")]
            async fn validate_live_bridge_member_eligibility(
                &self,
                session_id: &meerkat_core::SessionId,
            ) -> Result<(), SessionError> {
                self.inner.validate_live_bridge_member_eligibility(session_id).await
            }

            #[cfg(feature = "openai-live")]
            async fn capture_live_bridge_execution_snapshot(
                &self,
                session_id: &meerkat_core::SessionId,
                agent_identity: &str,
            ) -> Result<meerkat_mob::LiveBridgeExecutionSnapshot, SessionError> {
                self.inner
                    .capture_live_bridge_execution_snapshot(session_id, agent_identity)
                    .await
            }

            #[cfg(feature = "openai-live")]
            async fn start_live_bridge_member_operation(
                &self,
                request: meerkat_mob::LiveBridgeOperationRequest,
                cancellation: meerkat_mob::LiveBridgeOperationCancellationSignal,
            ) -> Result<meerkat_mob::LiveBridgeOperationTerminalFuture, meerkat_mob::LiveBridgeOperationStartError> {
                self.inner.start_live_bridge_member_operation(request, cancellation).await
            }

            async fn start_turn_with_admission_notification(
                &self,
                session_id: &meerkat_core::types::SessionId,
                req: meerkat_core::service::StartTurnRequest,
                admitted: tokio::sync::oneshot::Sender<()>,
            ) -> Result<meerkat_core::RunResult, SessionError> {
                self.inner
                    .start_turn_with_admission_notification(session_id, req, admitted)
                    .await
            }

            async fn load_session_for_resume(
                &self,
                session_id: &meerkat_core::types::SessionId,
            ) -> Result<meerkat_mob::ResumeSessionLoad, SessionError> {
                let load = self.inner.load_session_for_resume(session_id).await?;
                self.overlay_runtime_archived_terminal(load).await
            }
            // meerkat 0.8.22: the required `prepare_session_for_resume` hook
            // is DELETED from the trait. Its durable-tail convergence moved
            // inside `PersistentSessionService`'s override of
            // `materialize_session_resume_verdict`; the trait DEFAULT of that
            // method is the non-persistent composition (read-only
            // observations plus a `NonPersistent` receipt, no convergence).
            // So a wrapper that merely drops the old passthrough compiles
            // while recreating actors from stale committed authority whenever
            // the physical head is ahead after a power cut. This override
            // delegates the verdict to `inner` so its persistent convergence
            // is reached, then re-applies the archived-terminal overlay to
            // the authorized body. Under 0.8.21 the overlay rode along for
            // free because the provided default composed from THIS wrapper's
            // `load_session_for_resume`; the persistent route no longer calls
            // that method, so the projection has to be re-applied here or the
            // 0.8.6 "archived collapses into active" host confusion returns.
            //
            // `materialization == Revivable` is the exact 0.8.22 analogue of
            // the `ResumeSessionLoad::Revivable` arm the overlay used to key
            // on: `prepare_committed_boundary_resume` picks Revivable from
            // the same two store facts this overlay probes (catalog terminal
            // archived, or a Retired/Destroyed lifecycle row), so an `Active`
            // materialization from that store can never be a body the overlay
            // would have stamped. Widening the match would be a behaviour
            // change, not a port fix.
            async fn materialize_session_resume_verdict(
                &self,
                session_id: &meerkat_core::types::SessionId,
            ) -> Result<meerkat_mob::SessionResumeVerdict, SessionError> {
                let mut verdict = self
                    .inner
                    .materialize_session_resume_verdict(session_id)
                    .await?;
                if let meerkat_mob::SessionResumeVerdict::ResumeAuthorized {
                    materialization: meerkat_mob::SessionResumeMaterialization::Revivable,
                    session,
                    ..
                } = &mut verdict
                {
                    self.overlay_runtime_archived_terminal_in_place(session)
                        .await?;
                }
                Ok(verdict)
            }
            // meerkat 0.9.5: the verdict for a caller that holds the session's
            // turn-finalization boundary (a member host's materialization).
            // The trait default answers the plain verdict, whose persistent
            // preparation would wait for the boundary its caller holds and,
            // once a durable tail stands, refuse with `SessionError::Busy`.
            // The same archived-terminal overlay applies as above.
            async fn materialize_session_resume_verdict_under_runtime_turn_boundary(
                &self,
                session_id: &meerkat_core::types::SessionId,
            ) -> Result<meerkat_mob::SessionResumeVerdict, SessionError> {
                let mut verdict = self
                    .inner
                    .materialize_session_resume_verdict_under_runtime_turn_boundary(session_id)
                    .await?;
                if let meerkat_mob::SessionResumeVerdict::ResumeAuthorized {
                    materialization: meerkat_mob::SessionResumeMaterialization::Revivable,
                    session,
                    ..
                } = &mut verdict
                {
                    self.overlay_runtime_archived_terminal_in_place(session)
                        .await?;
                }
                Ok(verdict)
            }
            async fn observe_session_resume_authority(
                &self,
                session_id: &meerkat_core::types::SessionId,
            ) -> Result<meerkat_mob::SessionResumeAuthority, SessionError> {
                self.inner
                    .observe_session_resume_authority(session_id)
                    .await
            }
            // The wrapped persistent service settles the tail through its
            // cold-attach preparation, under this runtime owner's hosting
            // claim. The trait default would materialize the whole resume
            // verdict instead.
            async fn settle_durable_tail_before_cold_start(
                &self,
                session_id: &meerkat_core::types::SessionId,
            ) -> Result<(), SessionError> {
                self.inner
                    .settle_durable_tail_before_cold_start(session_id)
                    .await
            }
            // meerkat 0.9.5: the same settle, reporting what it left. The mob
            // start calls this one, and the trait default reports `Settled`,
            // so an unforwarded wrapper refuses to start a member whose open
            // run a graceful stop suspended (`MemberDurableTailUnprovable`):
            // only the wrapped persistent service can report that the
            // recovery continues the run.
            async fn settle_durable_tail_before_cold_start_reporting(
                &self,
                session_id: &meerkat_core::types::SessionId,
            ) -> Result<meerkat_mob::ColdStartTailSettlement, SessionError> {
                self.inner
                    .settle_durable_tail_before_cold_start_reporting(session_id)
                    .await
            }

            // NOT cfg-gated on meta's own `experimental-gpt-live`. The trait
            // requires this method whenever MEERKAT's feature is on, and
            // `--all-targets` dev-dependency unification turns meerkat's on while
            // meta's own flag stays off. Gating here on the meta flag deleted
            // the impl exactly when the trait demanded it.
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

            async fn create_session_under_runtime_turn_boundary(
                &self,
                req: meerkat_core::service::CreateSessionRequest,
            ) -> Result<meerkat_core::RunResult, SessionError> {
                let (req, context, capability_context) =
                    self.prepare_create_request(req).await?;
                let result = self
                    .inner
                    .create_session_under_runtime_turn_boundary(req)
                    .await?;
                Ok(self
                    .complete_create(result, context, capability_context)
                    .await)
            }
            // meerkat 0.8.22: exact-actor creation now carries the
            // owner-issued resume preparation receipt so persistent
            // materialization can consume the already-converged committed
            // boundary instead of repeating recovery. Forward it untouched.
            async fn create_session_with_actor_witness_under_runtime_turn_boundary(
                &self,
                req: meerkat_core::service::CreateSessionRequest,
                resume_preparation: Option<meerkat_mob::SessionResumePreparationReceipt>,
                actor_witness_slot: &meerkat_session::LiveSessionActorWitnessSlot,
            ) -> Result<meerkat_core::RunResult, SessionError> {
                let (req, context, capability_context) =
                    self.prepare_create_request(req).await?;
                let result = self
                    .inner
                    .create_session_with_actor_witness_under_runtime_turn_boundary(
                        req,
                        resume_preparation,
                        actor_witness_slot,
                    )
                    .await?;
                Ok(self
                    .complete_create(result, context, capability_context)
                    .await)
            }
            async fn create_session_with_machine_archived_resume_authority_under_runtime_turn_boundary(
                &self,
                req: meerkat_core::service::CreateSessionRequest,
                authorization: meerkat_runtime::ArchivedSessionActorMaterializationAuthorization,
            ) -> Result<meerkat_core::RunResult, SessionError> {
                let (req, context, capability_context) =
                    self.prepare_create_request(req).await?;
                let result = self
                    .inner
                    .create_session_with_machine_archived_resume_authority_under_runtime_turn_boundary(
                        req,
                        authorization,
                    )
                    .await?;
                Ok(self
                    .complete_create(result, context, capability_context)
                    .await)
            }
            // meerkat 0.8.22: same receipt threading as above, but the
            // machine-authorized archived route requires it unconditionally
            // (persistent materialization rejects a missing receipt).
            async fn create_session_with_machine_archived_resume_authority_and_actor_witness_under_runtime_turn_boundary(
                &self,
                req: meerkat_core::service::CreateSessionRequest,
                authorization: meerkat_runtime::ArchivedSessionActorMaterializationAuthorization,
                resume_preparation: meerkat_mob::SessionResumePreparationReceipt,
                actor_witness_slot: &meerkat_session::LiveSessionActorWitnessSlot,
            ) -> Result<meerkat_core::RunResult, SessionError> {
                let (req, context, capability_context) =
                    self.prepare_create_request(req).await?;
                let result = self
                    .inner
                    .create_session_with_machine_archived_resume_authority_and_actor_witness_under_runtime_turn_boundary(
                        req,
                        authorization,
                        resume_preparation,
                        actor_witness_slot,
                    )
                    .await?;
                Ok(self
                    .complete_create(result, context, capability_context)
                    .await)
            }
            fn supports_persistent_sessions(&self) -> bool {
                self.inner.supports_persistent_sessions()
            }
            // meerkat 0.9.5: whether a durable profile can spawn here. Only
            // the in-memory composition answers false for the persistent
            // service it wraps; every other layer forwards, since the trait
            // default (this wrapper's own `supports_persistent_sessions`)
            // would lose an inner layer's false.
            fn keeps_durable_member_sessions(&self) -> bool {
                !self.member_sessions_in_memory && self.inner.keeps_durable_member_sessions()
            }
            fn supports_runtime_turn_apply(&self) -> bool {
                self.inner.supports_runtime_turn_apply()
            }
            // meerkat 0.7.19: disposal routes on this fact. The trait default
            // is fail-closed (`true`); NOT forwarding it would swallow the
            // inner persistent service's real store read and resurrect the
            // ask-20 stranding for host-owned sessions (the external_tools
            // clobber class: wrappers MUST forward, not default).
            async fn session_known_to_archive_authority(
                &self,
                session_id: &meerkat_core::types::SessionId,
            ) -> Result<bool, SessionError> {
                self.inner.session_known_to_archive_authority(session_id).await
            }
            // An override is a default acquisition input, never a substitute
            // for the inner service's canonical owner validation or errors.
            fn acquire_runtime_adapter(
                &self,
                explicit: Option<Arc<meerkat_runtime::MeerkatMachine>>,
            ) -> Result<
                Option<Arc<meerkat_runtime::MeerkatMachine>>,
                meerkat_runtime::RuntimeDriverError,
            > {
                self.inner.acquire_runtime_adapter(
                    explicit.or_else(|| self.runtime_adapter_override.clone()),
                )
            }
            async fn interrupt_with_machine_authority(
                &self,
                session_id: &meerkat_core::types::SessionId,
                authority: meerkat_runtime::MachineSessionControlAuthority,
            ) -> Result<(), SessionError> {
                self.inner
                    .interrupt_with_machine_authority(session_id, authority)
                    .await
            }
            // meerkat 0.8.22: new exact-run machine-authorized hard cancel,
            // trait default `Err(Unsupported)`. The mob provisioner's
            // `CoreExecutorInterruptHandle::hard_cancel_run_if_current` calls
            // this on the service it was handed - i.e. THIS wrapper - and only
            // absorbs `NotRunning`, so an unforwarded default turns every
            // exact-run cancel into `control_failed_runtime`. Forward bare: the
            // `..._if_live` tolerance next door is specific to the idempotent
            // cooperative-quiescence seam, and inventing a second tolerance here
            // would mask a genuinely refused hard cancel.
            async fn interrupt_run_with_machine_authority(
                &self,
                session_id: &meerkat_core::types::SessionId,
                expected_run_id: &meerkat_core::lifecycle::RunId,
                authority: meerkat_runtime::MachineSessionControlAuthority,
            ) -> Result<bool, SessionError> {
                self.inner
                    .interrupt_run_with_machine_authority(session_id, expected_run_id, authority)
                    .await
            }
            // meerkat 0.9.5: the exact-run suspend of a graceful requeue stop,
            // trait default `Ok(false)` ("this service cannot suspend; leave
            // the stop to hard-cancel the run"). The mob provisioner's
            // `suspend_run_if_current` calls it on this wrapper, so an
            // unforwarded default hard-cancels a durable member's run that
            // the stop should keep for the member's next start.
            async fn suspend_run_with_machine_authority(
                &self,
                session_id: &meerkat_core::types::SessionId,
                expected_run_id: &meerkat_core::lifecycle::RunId,
                authority: meerkat_runtime::MachineSessionControlAuthority,
            ) -> Result<bool, SessionError> {
                self.inner
                    .suspend_run_with_machine_authority(session_id, expected_run_id, authority)
                    .await
            }
            async fn cancel_after_boundary_with_machine_authority(
                &self,
                session_id: &meerkat_core::types::SessionId,
                expected_run_id: &meerkat_core::lifecycle::RunId,
                authority: meerkat_runtime::MachineSessionControlAuthority,
            ) -> Result<(), SessionError> {
                cancel_after_boundary_with_machine_authority_if_live(
                    self.inner.as_ref(),
                    session_id,
                    expected_run_id,
                    authority,
                )
                .await
            }
            async fn cancel_current_after_boundary_with_machine_authority(
                &self,
                session_id: &meerkat_core::types::SessionId,
                authority: meerkat_runtime::MachineSessionControlAuthority,
            ) -> Result<(), SessionError> {
                self.inner
                    .cancel_current_after_boundary_with_machine_authority(session_id, authority)
                    .await
            }
            async fn live_session_actor_registered(
                &self,
                session_id: &meerkat_core::types::SessionId,
            ) -> Result<bool, SessionError> {
                self.inner.live_session_actor_registered(session_id).await
            }
            async fn session_projection_visible(
                &self,
                session_id: &meerkat_core::types::SessionId,
            ) -> Result<bool, SessionError> {
                self.inner.session_projection_visible(session_id).await
            }
            async fn session_belongs_to_mob(
                &self,
                session_id: &meerkat_core::types::SessionId,
                mob_id: &meerkat_mob::MobId,
            ) -> bool {
                self.inner.session_belongs_to_mob(session_id, mob_id).await
            }
            async fn load_persisted_session(
                &self,
                session_id: &meerkat_core::types::SessionId,
            ) -> Result<Option<meerkat_core::session::Session>, SessionError> {
                // The strict committed read, forwarded unchanged: a refusal
                // from the inner service (such as a provisional tail this
                // caller does not own) reaches the caller. The tolerant read
                // is the separate method below.
                self.inner.load_persisted_session(session_id).await
            }
            // meerkat 0.9.5: the committed read that tolerates the session's
            // own run's provisional tail, serving the committed snapshot
            // under it. The trait default is the strict
            // `load_persisted_session`, which refuses mid-run on a
            // HeadCanonical store, so an unforwarded wrapper keeps that
            // refusal for every reader of a Meta-hosted mob.
            async fn load_persisted_session_tolerating_tail(
                &self,
                session_id: &meerkat_core::types::SessionId,
            ) -> Result<Option<meerkat_core::session::Session>, SessionError> {
                self.inner
                    .load_persisted_session_tolerating_tail(session_id)
                    .await
            }
            // Forwarded: for a service that keeps durable sessions the trait
            // default refuses, so an unforwarded check refuses every resume
            // spawn over the persistent inner service.
            async fn provisional_tail_stands(
                &self,
                session_id: &meerkat_core::types::SessionId,
            ) -> Result<bool, SessionError> {
                meerkat_mob::MobSessionService::provisional_tail_stands(
                    self.inner.as_ref(),
                    session_id,
                )
                .await
            }
            // meerkat 0.7.29 revival seam (ask 31): the trait defaults answer
            // `Ok(None)`/`Unsupported`, which masks the inner persistent
            // service's real revival support and leaves Bug I victims
            // unresumable through this wrapper (same forwarding class as
            // `session_known_to_archive_authority` above).
            async fn load_revivable_retired_session(
                &self,
                session_id: &meerkat_core::types::SessionId,
            ) -> Result<Option<meerkat_core::session::Session>, SessionError> {
                self.inner.load_revivable_retired_session(session_id).await
            }
            // meerkat 0.8.22: durable transcript fork moved onto the session
            // service with an `Err(Unsupported)` default, and the mob handle's
            // fork-based spawn calls it on the service it holds. Same forwarding
            // class as `load_revivable_retired_session` above: not forwarding
            // would make this wrapper claim the inner persistent service has no
            // fork authority.
            async fn fork_persisted_session(
                &self,
                source_session_id: &meerkat_core::types::SessionId,
                message_count: Option<usize>,
                tool_access_policy: Option<meerkat_core::ops::ToolAccessPolicy>,
                target: meerkat_core::DurableSessionForkTarget,
            ) -> Result<meerkat_core::SessionForkResult, SessionError> {
                refuse_runtime_plane_fork(&target)?;
                self.inner
                    .fork_persisted_session(
                        source_session_id,
                        message_count,
                        tool_access_policy,
                        target,
                    )
                    .await
            }

            // meerkat 0.8.41 made `fork_persisted_session_at_turn_boundary` REQUIRED so
            // every wrapper forwards the boundary-then-fork as ONE contract to the owner;
            // a default that took the boundary and then called `fork_persisted_session`
            // self-deadlocks on the persistent owner's non-reentrant boundary.
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

            // Forwarded exactly: the inner persistent service commits a
            // re-presented live transcript at the turn boundary. The trait
            // default refuses (meerkat 0.8.50).
            async fn commit_live_delegation_represented_transcript_at_turn_boundary(
                &self,
                machine: &meerkat_runtime::MeerkatMachine,
                session_id: &meerkat_core::types::SessionId,
                provisional: meerkat_core::ProvisionalLiveHandoff,
                final_event: meerkat_core::RealtimeTranscriptEvent,
                represented: Vec<meerkat_core::RepresentedLiveUserRow>,
                bound: std::time::Duration,
            ) -> Result<meerkat_core::LiveFinalTranscriptCommitAtTurnBoundary, SessionError> {
                self.inner
                    .commit_live_delegation_represented_transcript_at_turn_boundary(
                        machine,
                        session_id,
                        provisional,
                        final_event,
                        represented,
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
                refuse_runtime_plane_fork(&target)?;
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
            async fn load_persisted_session_metadata(
                &self,
                session_id: &meerkat_core::types::SessionId,
            ) -> Result<Option<meerkat_core::PersistedSessionMetadataView>, SessionError> {
                self.inner.load_persisted_session_metadata(session_id).await
            }
            // Retained metadata includes archived sessions and is read only
            // from the inner authority, never through ordinary session reads.
            async fn load_retained_session_metadata(
                &self,
                session_id: &meerkat_core::types::SessionId,
            ) -> Result<Option<meerkat_core::PersistedSessionMetadataView>, SessionError> {
                self.inner.load_retained_session_metadata(session_id).await
            }
            // The trait default is a typed `Unsupported`: unforwarded, every
            // member respawn through this decorator is refused before
            // anything is retired.
            async fn predecessor_tool_restrictions(
                &self,
                session_id: &meerkat_core::types::SessionId,
            ) -> Result<meerkat_core::PredecessorToolRestrictions, SessionError> {
                self.inner.predecessor_tool_restrictions(session_id).await
            }
            // The trait default answers that the source is not live:
            // unforwarded, a fork child of a member behind this decorator is
            // never told its source's tools.
            async fn fork_source_tools(
                &self,
                session_id: &meerkat_core::types::SessionId,
            ) -> meerkat_core::fork_notice::ForkSeedSourceTools {
                self.inner.fork_source_tools(session_id).await
            }
            async fn authorize_revivable_retired_session(
                &self,
                session_id: &meerkat_core::types::SessionId,
                authority: meerkat_runtime::PreparedArchivedResumeCommitLease,
            ) -> Result<meerkat_runtime::AuthorizedArchivedResumeCommitLease, SessionError> {
                self.inner
                    .authorize_revivable_retired_session(session_id, authority)
                    .await
            }
            async fn create_session_with_machine_archived_resume_authority(
                &self,
                req: meerkat_core::service::CreateSessionRequest,
                authorization: meerkat_runtime::ArchivedSessionActorMaterializationAuthorization,
            ) -> Result<meerkat_core::RunResult, SessionError> {
                let (req, context, capability_context) =
                    self.prepare_create_request(req).await?;
                let result = self
                    .inner
                    .create_session_with_machine_archived_resume_authority(req, authorization)
                    .await?;
                Ok(self
                    .complete_create(result, context, capability_context)
                    .await)
            }
            async fn subscribe_session_events(
                &self,
                session_id: &meerkat_core::types::SessionId,
            ) -> Result<meerkat_core::comms::EventStream, meerkat_core::comms::StreamError> {
                meerkat_mob::MobSessionService::subscribe_session_events(
                    self.inner.as_ref(),
                    session_id,
                )
                .await
            }
            async fn subscribe_agent_session_events_from(
                &self,
                session_id: &meerkat_core::types::SessionId,
                cursor: meerkat_core::comms::SessionEventCursor,
            ) -> Result<meerkat_mob::AgentEventSubscription, meerkat_core::comms::StreamError>
            {
                meerkat_mob::MobSessionService::subscribe_agent_session_events_from(
                    self.inner.as_ref(),
                    session_id,
                    cursor,
                )
                .await
            }
            async fn archive_with_mob_lifecycle_authority(
                &self,
                session_id: &meerkat_core::types::SessionId,
            ) -> Result<(), SessionError> {
                match self
                    .inner
                    .archive_with_mob_lifecycle_authority(session_id)
                    .await
                {
                    Err(err)
                        if is_stopped_session_archive_retire_rejection(&err.to_string()) =>
                    {
                        // meerkat 0.7.1: the archive protocol commits the
                        // durable archive document FIRST, then drives the
                        // machine `Retire` realization — which the session
                        // machine of an idle (stopped-between-turns) member
                        // rejects from `Stopped`. meerkat-mob's own archive
                        // helper treats `Stopped` as already-retired; mirror
                        // that tolerance here so member retire/respawn
                        // disposal completes instead of wedging the roster
                        // anchor in `retiring` (any disposal retry on the
                        // retained anchor stalls the mob actor).
                        tracing::warn!(
                            session_id = %session_id,
                            error = %err,
                            "archive: tolerating Retire rejection for stopped idle session; archive document committed"
                        );
                        Ok(())
                    }
                    other => other,
                }
            }
            async fn archive_with_mob_lifecycle_authority_under_runtime_turn_boundary(
                &self,
                session_id: &meerkat_core::types::SessionId,
            ) -> Result<(), SessionError> {
                self.inner
                    .archive_with_mob_lifecycle_authority_under_runtime_turn_boundary(session_id)
                    .await
            }
            // meerkat 0.8.22: member-session disposal moved from the boundary
            // archive above to this deadline-aware sibling, whose trait default
            // is `Err(Unsupported)`. The provisioner routes only `Ok(())` and
            // `NotFound` specially, so an unforwarded default makes EVERY member
            // retirement archive fail and wedges the roster anchor in `retiring`.
            // Bare forward on purpose: this mirrors the non-deadline boundary
            // variant above, which is exactly the seam disposal used at 0.8.21.
            // The stopped-session Retire tolerance lives only on
            // `archive_with_mob_lifecycle_authority`, and it did not cover the
            // disposal path before either - widening it here would be a
            // behaviour change, not a port fix.
            async fn archive_with_mob_lifecycle_authority_under_runtime_turn_boundary_before(
                &self,
                session_id: &meerkat_core::types::SessionId,
                deadline: meerkat_core::time_compat::Instant,
            ) -> Result<(), SessionError> {
                self.inner
                    .archive_with_mob_lifecycle_authority_under_runtime_turn_boundary_before(
                        session_id, deadline,
                    )
                    .await
            }
            // The hook-bearing lane rides only a recovered-operation handoff at
            // retirement (meerkat-mob provisioner). Its trait DEFAULT refuses
            // typed whenever a hook is present, so a decorator that forgets
            // this forward turns every such destructive reset into
            // "session service cannot run a pre-retire archive hook" - seen as
            // a flake in live_reset_takes_a_turn_and_shuts_down_within_horizon
            // on 2026-09-03, latent since 0.8.28.
            async fn archive_with_mob_lifecycle_authority_under_runtime_turn_boundary_and_hook_before(
                &self,
                session_id: &meerkat_core::types::SessionId,
                deadline: meerkat_core::time_compat::Instant,
                post_commit_hook: Option<Arc<dyn meerkat_runtime::MachineSessionArchivePostCommitHook>>,
            ) -> Result<(), SessionError> {
                self.inner
                    .archive_with_mob_lifecycle_authority_under_runtime_turn_boundary_and_hook_before(
                        session_id,
                        deadline,
                        post_commit_hook,
                    )
                    .await
            }
            // meerkat 0.9.6: the cold-predecessor settlement. Its trait default
            // refuses typed (`Unsupported`) and touches nothing, so a decorator
            // that does not forward it silently refuses every settlement of the
            // runtime work an earlier incarnation of a member session left.
            async fn settle_cold_predecessor_with_mob_lifecycle_authority_before(
                &self,
                session_id: &meerkat_core::types::SessionId,
                claim: &meerkat_core::session_hosting::HostingClaim,
                deadline: meerkat_core::time_compat::Instant,
            ) -> Result<
                meerkat_runtime::ColdPredecessorSettlement,
                meerkat_session::ColdPredecessorSettleError,
            > {
                self.inner
                    .settle_cold_predecessor_with_mob_lifecycle_authority_before(
                        session_id, claim, deadline,
                    )
                    .await
            }

            // The remaining defaulted MobSessionService methods a transparent
            // decorator must forward. Each default is a refusal or a downgrade
            // (see the 2026-09-03 audit in the PR); the conformance gate
            // scripts/verify-session-service-decorators.py fails fmt-lint if a
            // production decorator ever lacks one again.
            fn forked_participant_source_runtime(
                self: Arc<Self>,
            ) -> Option<Arc<dyn meerkat_mob::ForkedParticipantSourceRuntime>> {
                Arc::clone(&self.inner).forked_participant_source_runtime()
            }
            fn persisted_session_authority_read_cost(
                &self,
            ) -> meerkat_mob::runtime::PersistedSessionAuthorityReadCost {
                self.inner.persisted_session_authority_read_cost()
            }
            async fn observe_persisted_session_authority(
                &self,
                session_id: &meerkat_core::types::SessionId,
            ) -> Result<Option<meerkat_mob::IdentitySessionStoreAuthority>, SessionError> {
                self.inner
                    .observe_persisted_session_authority(session_id)
                    .await
            }
            async fn revalidate_session_resume_authority(
                &self,
                session_id: &meerkat_core::types::SessionId,
                expected: &meerkat_mob::SessionResumeAuthority,
            ) -> Result<Result<(), meerkat_mob::SessionResumeRejection>, SessionError> {
                self.inner
                    .revalidate_session_resume_authority(session_id, expected)
                    .await
            }
            async fn discard_live_session_actor_after_durability_reload_required(
                &self,
                witness: &meerkat_session::LiveSessionActorWitness,
            ) -> Result<bool, SessionError> {
                self.inner
                    .discard_live_session_actor_after_durability_reload_required(witness)
                    .await
            }
            async fn execution_snapshot(
                &self,
                session_id: &meerkat_core::types::SessionId,
            ) -> Result<Option<meerkat_core::agent::AgentExecutionSnapshot>, SessionError> {
                self.inner.execution_snapshot(session_id).await
            }
            async fn tool_scope_snapshot(
                &self,
                session_id: &meerkat_core::types::SessionId,
            ) -> Result<Option<meerkat_core::ToolScopeSnapshot>, SessionError> {
                self.inner.tool_scope_snapshot(session_id).await
            }
            // Forwarded exactly, like its tool-scope siblings: the trait
            // default reports no view, so behind this wrapper the inner
            // service's catalog view would read as unavailable.
            async fn tool_catalog_view(
                &self,
                session_id: &meerkat_core::types::SessionId,
            ) -> Result<Option<meerkat_core::ToolCatalogView>, SessionError> {
                self.inner.tool_catalog_view(session_id).await
            }
            async fn live_visible_tool_defs(
                &self,
                session_id: &meerkat_core::types::SessionId,
            ) -> Result<Option<Vec<meerkat_core::ToolDef>>, SessionError> {
                self.inner.live_visible_tool_defs(session_id).await
            }
            async fn external_tool_surface_snapshot(
                &self,
                session_id: &meerkat_core::types::SessionId,
            ) -> Result<Option<meerkat_core::ExternalToolSurfaceSnapshot>, SessionError> {
                self.inner.external_tool_surface_snapshot(session_id).await
            }
            async fn peer_ingress_runtime_snapshot(
                &self,
                session_id: &meerkat_core::types::SessionId,
            ) -> Result<Option<meerkat_core::PeerIngressRuntimeSnapshot>, SessionError> {
                self.inner.peer_ingress_runtime_snapshot(session_id).await
            }
            async fn apply_runtime_turn(
                &self,
                session_id: &meerkat_core::types::SessionId,
                run_id: meerkat_core::lifecycle::RunId,
                req: meerkat_core::service::StartTurnRequest,
                boundary: meerkat_core::lifecycle::run_primitive::RunApplyBoundary,
                contributing_input_ids: Vec<meerkat_core::lifecycle::InputId>,
            ) -> Result<meerkat_core::lifecycle::core_executor::CoreApplyOutput, SessionError> {
                #[cfg(test)]
                let boundary_name = format!("{boundary:?}");
                #[cfg(test)]
                let contributing_count = contributing_input_ids.len();
                let run_id_for_log = run_id.to_string();
                if runtime_turn_diagnostics_enabled() {
                    tracing::warn!(
                        session_id = %session_id,
                        run_id = %run_id_for_log,
                        boundary = ?boundary,
                        contributing_inputs = contributing_input_ids.len(),
                        "meta runtime turn start"
                    );
                }
                let result = self
                    .inner
                    .apply_runtime_turn(
                        session_id,
                        run_id,
                        normalize_runtime_turn_request(req),
                        boundary,
                        contributing_input_ids,
                    )
                    .await;
                #[cfg(test)]
                record_runtime_turn_trace(RuntimeTurnTrace {
                    session_id: session_id.to_string(),
                    boundary: boundary_name,
                    contributing_input_count: contributing_count,
                    outcome: match &result {
                        Ok(_) => "ok".to_string(),
                        Err(error) => format!("err:{error}"),
                    },
                });
                if runtime_turn_diagnostics_enabled() {
                    match &result {
                        Ok(_) => tracing::warn!(
                            session_id = %session_id,
                            run_id = %run_id_for_log,
                            "meta runtime turn ok"
                        ),
                        Err(_) => tracing::error!(
                            session_id = %session_id,
                            run_id = %run_id_for_log,
                            "meta runtime turn error"
                        ),
                    }
                }
                result
            }
            async fn prepare_turn_boundary_delivery_for_active_turn(
                &self,
                session_id: &meerkat_core::types::SessionId,
                expected_run_id: &meerkat_core::lifecycle::RunId,
                delivery: meerkat_core::TurnBoundaryDelivery,
            ) -> Result<meerkat_core::CoreBoundaryStageOutput, meerkat_core::CoreBoundaryStageError>
            {
                self.inner
                    .prepare_turn_boundary_delivery_for_active_turn(
                        session_id,
                        expected_run_id,
                        delivery,
                    )
                    .await
            }
            async fn acknowledge_committed_runtime_session_boundary_under_turn_finalization_boundary(
                &self,
                session_id: &meerkat_core::types::SessionId,
                authority: &meerkat_core::CommittedSessionBoundaryAuthority,
            ) -> Result<(), SessionError> {
                self.inner
                    .acknowledge_committed_runtime_session_boundary_under_turn_finalization_boundary(
                        session_id,
                        authority,
                    )
                    .await
            }
            async fn acquire_runtime_turn_finalization_guard(
                &self,
                session_id: &meerkat_core::types::SessionId,
            ) -> Result<
                Box<dyn meerkat_core::lifecycle::CoreExecutorTurnFinalizationGuard>,
                SessionError,
            > {
                self.inner
                    .acquire_runtime_turn_finalization_guard(session_id)
                    .await
            }
            async fn checkpoint_committed_runtime_session_snapshot_under_turn_finalization_boundary(
                &self,
                session_id: &meerkat_core::types::SessionId,
                session_snapshot: Arc<Vec<u8>>,
            ) -> Result<(), SessionError> {
                self.inner
                    .checkpoint_committed_runtime_session_snapshot_under_turn_finalization_boundary(
                        session_id,
                        session_snapshot,
                    )
                    .await
            }
            async fn discard_live_session_after_runtime_stop_terminalized(
                &self,
                session_id: &meerkat_core::types::SessionId,
            ) -> Result<(), SessionError> {
                self.inner
                    .discard_live_session_after_runtime_stop_terminalized(session_id)
                    .await
            }
            async fn discard_live_session_after_runtime_stop_terminalized_under_turn_finalization_boundary(
                &self,
                session_id: &meerkat_core::types::SessionId,
            ) -> Result<(), SessionError> {
                self.inner
                    .discard_live_session_after_runtime_stop_terminalized_under_turn_finalization_boundary(
                        session_id,
                    )
                    .await
            }
            // meerkat 0.8.22: terminal publication is keyed by the exact
            // service-minted actor incarnation, not by SessionId. Retirement
            // drops its mutation gate before calling publication code, so a
            // delayed predecessor callback resolved only by id could target a
            // successor actor. Forward the witness verbatim.
            async fn publish_boundary_appends_discarded_for_actor(
                &self,
                actor_witness: &meerkat_session::LiveSessionActorWitness,
                discarded: &meerkat_core::event::BoundaryAppendsDiscarded,
            ) -> Result<(), SessionError> {
                self.inner.publish_boundary_appends_discarded_for_actor(actor_witness, discarded).await
            }
            async fn publish_settled_run_terminal_for_actor(
                &self,
                actor_witness: &meerkat_session::LiveSessionActorWitness,
                settled: &meerkat_core::lifecycle::SettledRunTerminal,
            ) -> Result<(), SessionError> {
                self.inner.publish_settled_run_terminal_for_actor(actor_witness, settled).await
            }
            async fn publish_interaction_terminals_for_actor(
                &self,
                actor_witness: &meerkat_session::LiveSessionActorWitness,
                events: &[meerkat_core::event::AgentEvent],
            ) -> Result<
                Vec<
                    meerkat_core::lifecycle::core_executor::CoreInteractionTerminalPublicationReceipt,
                >,
                SessionError,
            > {
                self.inner
                    .publish_interaction_terminals_for_actor(actor_witness, events)
                    .await
            }
            async fn discard_live_session(
                &self,
                session_id: &meerkat_core::types::SessionId,
            ) -> Result<(), SessionError> {
                self.inner.discard_live_session(session_id).await
            }
            async fn discard_live_session_under_runtime_turn_boundary(
                &self,
                session_id: &meerkat_core::types::SessionId,
            ) -> Result<(), SessionError> {
                self.inner
                    .discard_live_session_under_runtime_turn_boundary(session_id)
                    .await
            }
            async fn discard_live_session_actor_under_runtime_turn_boundary(
                &self,
                witness: &meerkat_session::LiveSessionActorWitness,
            ) -> Result<bool, SessionError> {
                self.inner
                    .discard_live_session_actor_under_runtime_turn_boundary(witness)
                    .await
            }
            // The trait default answers `Unsupported` on the decorator's own
            // authority. Forwarded with the exact actor, so a held member
            // carries what its actor's actual pinned client reported.
            async fn controller_route_under_runtime_turn_boundary(
                &self,
                actor: &meerkat_session::LiveSessionActorWitness,
            ) -> Result<meerkat_mob::ControllerRouteObservation, SessionError> {
                self.inner
                    .controller_route_under_runtime_turn_boundary(actor)
                    .await
            }
            // The trait default refuses with `Unsupported`. This wrapper
            // forwards `acquire_runtime_turn_finalization_guard`, and a
            // service implements that boundary and this pin both or neither,
            // so the pin goes to the inner service whose boundary the caller
            // holds, with the exact actor.
            async fn pin_controller_client_under_runtime_turn_boundary(
                &self,
                actor: &meerkat_session::LiveSessionActorWitness,
            ) -> Result<meerkat_core::ControllerModelClient, SessionError> {
                self.inner
                    .pin_controller_client_under_runtime_turn_boundary(actor)
                    .await
            }
            // The trait default returns at once. Forwarded, so a caller that
            // waits for a session's removed actors to exit (and release what
            // their agents owned) waits on the inner service's actors.
            async fn await_removed_actor_exit(&self, session_id: &meerkat_core::types::SessionId) {
                self.inner.await_removed_actor_exit(session_id).await;
            }
            async fn await_event_projection_drain(
                &self,
                session_id: &meerkat_core::types::SessionId,
            ) -> Result<bool, SessionError> {
                self.inner.await_event_projection_drain(session_id).await
            }
            async fn checkpoint_committed_runtime_session_snapshot(
                &self,
                session_id: &meerkat_core::types::SessionId,
                session_snapshot: Arc<Vec<u8>>,
            ) -> Result<(), SessionError> {
                self.inner
                    .checkpoint_committed_runtime_session_snapshot(session_id, session_snapshot)
                    .await
            }
            async fn cancel_all_checkpointers(&self) {
                self.inner.cancel_all_checkpointers().await;
            }
            async fn rearm_all_checkpointers(&self) {
                self.inner.rearm_all_checkpointers().await;
            }
        }
    };
}

delegate_mob_session_service!(PreBuildMobSessionService);

/// Wraps a `MobSessionService` to fire an `AfterCreateHook` after each
/// successful `create_session`. Unlike `PreBuildMobSessionService`, this
/// captures context **after** the inner service (including any pre-build hooks)
/// has finished, so the context reflects all mutations.
struct AfterCreateMobSessionService {
    inner: Arc<dyn MobSessionService>,
    after_hook: AfterCreateHook,
}

impl AfterCreateMobSessionService {
    fn prepare_create_request(
        &self,
        mut req: CreateSessionRequest,
    ) -> (CreateSessionRequest, SessionCreatedContext) {
        sanitize_create_session_request_llm_override(&mut req);
        let context = SessionCreatedContext {
            model: req.model.clone(),
            labels: req.labels.clone().unwrap_or_default(),
            system_prompt: req.system_prompt.as_set_prompt().map(ToString::to_string),
        };
        (req, context)
    }

    async fn complete_create(
        &self,
        result: meerkat_core::types::RunResult,
        context: SessionCreatedContext,
    ) -> meerkat_core::types::RunResult {
        (self.after_hook)(result.session_id.clone(), context).await;
        result
    }
}

#[async_trait]
impl meerkat_core::service::SessionService for AfterCreateMobSessionService {
    async fn read_tool_application_observations(
        &self,
        id: &meerkat_core::SessionId,
    ) -> Result<Vec<meerkat_core::ToolApplicationObservation>, SessionError> {
        self.inner.read_tool_application_observations(id).await
    }

    async fn tool_application(
        self: Arc<Self>,
        control: Arc<meerkat_core::ToolApplicationControlRequest>,
    ) -> Result<serde_json::Value, SessionError> {
        Arc::clone(&self.inner).tool_application(control).await
    }

    async fn create_session(
        &self,
        req: CreateSessionRequest,
    ) -> Result<meerkat_core::types::RunResult, SessionError> {
        let (req, context) = self.prepare_create_request(req);
        let result = self.inner.create_session(req).await?;
        Ok(self.complete_create(result, context).await)
    }
    async fn start_turn(
        &self,
        id: &meerkat_core::types::SessionId,
        req: meerkat_core::service::StartTurnRequest,
    ) -> Result<meerkat_core::types::RunResult, SessionError> {
        self.inner.start_turn(id, req).await
    }
    async fn reconcile_runtime_compaction_projections(
        &self,
        id: &meerkat_core::types::SessionId,
        intents: Vec<meerkat_core::CompactionProjectionIntent>,
    ) -> Result<(), SessionError> {
        self.inner
            .reconcile_runtime_compaction_projections(id, intents)
            .await
    }
    // Forwarded, as in `delegate_mob_session_service!`: the trait default
    // refuses the acknowledgement.
    async fn acknowledge_runtime_compaction_finalization(
        &self,
        id: &meerkat_core::types::SessionId,
        authority: &meerkat_core::CommittedSessionBoundaryAuthority,
    ) -> Result<(), SessionError> {
        self.inner
            .acknowledge_runtime_compaction_finalization(id, authority)
            .await
    }
    async fn abort_uncommitted_compaction_projections(
        &self,
        id: &meerkat_core::types::SessionId,
    ) -> Result<(), SessionError> {
        self.inner
            .abort_uncommitted_compaction_projections(id)
            .await
    }
    async fn abort_rejected_runtime_run_projections(
        &self,
        id: &meerkat_core::types::SessionId,
    ) -> Result<(), SessionError> {
        self.inner.abort_rejected_runtime_run_projections(id).await
    }
    async fn interrupt(&self, id: &meerkat_core::types::SessionId) -> Result<(), SessionError> {
        self.inner.interrupt(id).await
    }
    // meerkat 0.8.22 exact-run hard cancel, default `Err(Unsupported)` (see the
    // same seam in `delegate_mob_session_service!`).
    async fn interrupt_run_if_current(
        &self,
        id: &meerkat_core::types::SessionId,
        expected_run_id: &meerkat_core::lifecycle::RunId,
    ) -> Result<bool, SessionError> {
        self.inner
            .interrupt_run_if_current(id, expected_run_id)
            .await
    }
    async fn cancel_after_boundary(
        &self,
        id: &meerkat_core::types::SessionId,
    ) -> Result<(), SessionError> {
        self.inner.cancel_after_boundary(id).await
    }
    async fn cancel_after_boundary_for_run(
        &self,
        id: &meerkat_core::types::SessionId,
        expected_run_id: &meerkat_core::lifecycle::RunId,
    ) -> Result<(), SessionError> {
        self.inner
            .cancel_after_boundary_for_run(id, expected_run_id)
            .await
    }
    async fn set_session_client(
        &self,
        id: &meerkat_core::types::SessionId,
        client: Arc<dyn meerkat_core::AgentLlmClient>,
    ) -> Result<(), SessionError> {
        self.inner
            .set_session_client(id, ReplaySanitizingAgentLlmClient::wrap(client))
            .await
    }
    async fn hot_swap_session_llm_identity(
        &self,
        id: &meerkat_core::types::SessionId,
        client: Arc<dyn meerkat_core::AgentLlmClient>,
        identity: meerkat_core::session::SessionLlmIdentity,
        request_policy: meerkat_core::SessionLlmRequestPolicy,
    ) -> Result<(), SessionError> {
        self.inner
            .hot_swap_session_llm_identity(
                id,
                ReplaySanitizingAgentLlmClient::wrap(client),
                identity,
                request_policy,
            )
            .await
    }
    async fn update_session_mob_authority_context(
        &self,
        id: &meerkat_core::types::SessionId,
        authority_context: Option<meerkat_core::service::MobToolAuthorityContext>,
    ) -> Result<(), SessionError> {
        self.inner
            .update_session_mob_authority_context(id, authority_context)
            .await
    }
    // Default `Err(Unsupported)` (see the same seam in
    // `delegate_mob_session_service!`).
    async fn update_session_mcp_attachment_intent(
        &self,
        id: &meerkat_core::types::SessionId,
        mutation: meerkat_core::mcp_session_intent::McpAttachmentIntentMutation,
    ) -> Result<meerkat_core::mcp_session_intent::McpAttachmentIntentRecorded, SessionError> {
        self.inner
            .update_session_mcp_attachment_intent(id, mutation)
            .await
    }
    async fn has_live_session(
        &self,
        id: &meerkat_core::types::SessionId,
    ) -> Result<bool, SessionError> {
        self.inner.has_live_session(id).await
    }
    async fn set_session_tool_visibility_state(
        &self,
        id: &meerkat_core::types::SessionId,
        state: Option<meerkat_core::SessionToolVisibilityState>,
    ) -> Result<(), SessionError> {
        self.inner
            .set_session_tool_visibility_state(id, state)
            .await
    }
    async fn set_session_tool_filter(
        &self,
        id: &meerkat_core::types::SessionId,
        filter: meerkat_core::ToolFilter,
    ) -> Result<(), SessionError> {
        self.inner.set_session_tool_filter(id, filter).await
    }
    async fn read(
        &self,
        id: &meerkat_core::types::SessionId,
    ) -> Result<meerkat_core::service::SessionView, SessionError> {
        self.inner.read(id).await
    }
    async fn list(
        &self,
        query: meerkat_core::service::SessionQuery,
    ) -> Result<Vec<meerkat_core::service::SessionSummary>, SessionError> {
        self.inner.list(query).await
    }
    async fn archive(&self, id: &meerkat_core::types::SessionId) -> Result<(), SessionError> {
        self.inner.archive(id).await
    }
    async fn subscribe_session_events(
        &self,
        id: &meerkat_core::types::SessionId,
    ) -> Result<meerkat_core::comms::EventStream, meerkat_core::comms::StreamError> {
        meerkat_core::service::SessionService::subscribe_session_events(self.inner.as_ref(), id)
            .await
    }
    async fn subscribe_session_events_from(
        &self,
        id: &meerkat_core::types::SessionId,
        cursor: meerkat_core::comms::SessionEventCursor,
    ) -> Result<meerkat_core::comms::SessionEventSubscription, meerkat_core::comms::StreamError>
    {
        meerkat_core::service::SessionService::subscribe_session_events_from(
            self.inner.as_ref(),
            id,
            cursor,
        )
        .await
    }
    async fn record_live_terminal_error(
        &self,
        id: &meerkat_core::types::SessionId,
        cause: meerkat_core::live_adapter::LiveAdapterErrorCode,
    ) -> Result<(), SessionError> {
        self.inner.record_live_terminal_error(id, cause).await
    }
    async fn record_live_output_audio_degraded(
        &self,
        id: &meerkat_core::types::SessionId,
        dropped: u64,
    ) -> Result<(), SessionError> {
        self.inner
            .record_live_output_audio_degraded(id, dropped)
            .await
    }
}

#[async_trait]
impl meerkat_core::service::SessionServiceCommsExt for AfterCreateMobSessionService {
    async fn comms_runtime(
        &self,
        id: &meerkat_core::types::SessionId,
    ) -> Option<Arc<dyn meerkat_core::agent::CommsRuntime>> {
        self.inner.comms_runtime(id).await
    }

    async fn event_injector(
        &self,
        id: &meerkat_core::types::SessionId,
    ) -> Option<Arc<dyn meerkat_core::EventInjector>> {
        self.inner.event_injector(id).await
    }

    async fn interaction_event_injector(
        &self,
        id: &meerkat_core::types::SessionId,
    ) -> Option<Arc<dyn meerkat_core::event_injector::SubscribableInjector>> {
        self.inner.interaction_event_injector(id).await
    }

    // Forwarded, as in `delegate_mob_session_service!`.
    async fn send_comms(
        &self,
        id: &meerkat_core::types::SessionId,
        command: meerkat_core::CommsCommand,
    ) -> Option<Result<meerkat_core::SendReceipt, meerkat_core::SendError>> {
        self.inner.send_comms(id, command).await
    }
}

#[async_trait]
impl meerkat_core::service::SessionServiceControlExt for AfterCreateMobSessionService {
    async fn append_system_context(
        &self,
        id: &meerkat_core::types::SessionId,
        req: meerkat_core::service::AppendSystemContextRequest,
    ) -> Result<
        meerkat_core::service::AppendSystemContextResult,
        meerkat_core::service::SessionControlError,
    > {
        self.inner.append_system_context(id, req).await
    }
    // Forwarded, as in `delegate_mob_session_service!`.
    async fn append_authenticated_system_context(
        &self,
        control: Arc<meerkat_core::service::SystemContextControlRequest>,
    ) -> Result<
        meerkat_core::service::AppendSystemContextResult,
        meerkat_core::service::SessionControlError,
    > {
        self.inner
            .append_authenticated_system_context(control)
            .await
    }
    async fn append_authorized_system_context(
        &self,
        prepared: meerkat_core::service::PreparedSystemContextAppend,
    ) -> Result<
        meerkat_core::service::AppendSystemContextResult,
        meerkat_core::service::SessionControlError,
    > {
        self.inner.append_authorized_system_context(prepared).await
    }
    async fn stage_tool_results(
        &self,
        id: &meerkat_core::types::SessionId,
        req: meerkat_core::service::StageToolResultsRequest,
    ) -> Result<meerkat_core::service::StageToolResultsResult, SessionError> {
        self.inner.stage_tool_results(id, req).await
    }
}

#[async_trait]
impl meerkat_core::service::SessionServiceHistoryExt for AfterCreateMobSessionService {
    async fn read_history(
        &self,
        id: &meerkat_core::types::SessionId,
        query: meerkat_core::service::SessionHistoryQuery,
    ) -> Result<meerkat_core::service::SessionHistoryPage, SessionError> {
        self.inner.read_history(id, query).await
    }
    async fn read_transcript_revision(
        &self,
        id: &meerkat_core::types::SessionId,
        query: meerkat_core::service::SessionTranscriptRevisionQuery,
    ) -> Result<meerkat_core::service::SessionTranscriptRevisionPage, SessionError> {
        self.inner.read_transcript_revision(id, query).await
    }
    async fn list_transcript_revisions(
        &self,
        id: &meerkat_core::types::SessionId,
        query: meerkat_core::service::SessionTranscriptRevisionListQuery,
    ) -> Result<meerkat_core::service::SessionTranscriptRevisionList, SessionError> {
        self.inner.list_transcript_revisions(id, query).await
    }
    async fn read_instruction_activation_records(
        &self,
        id: &meerkat_core::types::SessionId,
        query: meerkat_core::InstructionActivationReadQuery,
    ) -> Result<meerkat_core::InstructionActivationReadPage, SessionError> {
        self.inner
            .read_instruction_activation_records(id, query)
            .await
    }
}

#[async_trait]
impl MobSessionService for AfterCreateMobSessionService {
    // Forwarded exactly, as in `delegate_mob_session_service!`: a mob Stop
    // awaits the inner service's activity watch for an interrupted turn.
    async fn subscribe_session_activity(
        &self,
        session_id: &meerkat_core::SessionId,
    ) -> Result<meerkat_mob::MemberSessionActivity, SessionError> {
        self.inner.subscribe_session_activity(session_id).await
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

    // Forwarded exactly, as in `delegate_mob_session_service!`: the inner
    // service answers from its live status watch without waiting on a turn.
    async fn observe_member_status_view(
        &self,
        session_id: &meerkat_core::SessionId,
    ) -> Result<meerkat_mob::MemberStatusSessionView, SessionError> {
        self.inner.observe_member_status_view(session_id).await
    }

    // Forwarded exactly, as in `delegate_mob_session_service!`: the typed
    // system-notice append the runtime uses for interrupted-run notices.
    async fn append_system_notice_under_runtime_turn_boundary(
        &self,
        session_id: &meerkat_core::SessionId,
        record: meerkat_core::types::SystemNoticeRecord,
    ) -> Result<(), SessionError> {
        self.inner
            .append_system_notice_under_runtime_turn_boundary(session_id, record)
            .await
    }

    // Forwarded exactly, as in `delegate_mob_session_service!`: durable
    // instruction activation under the runtime turn boundary.
    async fn activate_instruction_under_runtime_turn_boundary(
        &self,
        session_id: &meerkat_core::SessionId,
        request: meerkat_core::InstructionActivationRequest,
        write_fence: Option<Arc<dyn meerkat_runtime::RuntimeStoreWriteFence>>,
    ) -> Result<meerkat_core::InstructionActivationMutation, SessionError> {
        self.inner
            .activate_instruction_under_runtime_turn_boundary(session_id, request, write_fence)
            .await
    }

    // Forwarded exactly: the body-free durable-source observation behind
    // voice readiness. (`delegate_mob_session_service!` forwards it the same
    // way, after the announcement test-fault check of a test-fault build.)
    async fn observe_live_durable_source(
        &self,
        session_id: &meerkat_core::SessionId,
    ) -> Result<meerkat_mob::LiveDurableSourceObservation, SessionError> {
        self.inner.observe_live_durable_source(session_id).await
    }

    #[cfg(feature = "openai-live")]
    async fn validate_live_bridge_member_eligibility(
        &self,
        session_id: &meerkat_core::SessionId,
    ) -> Result<(), SessionError> {
        self.inner
            .validate_live_bridge_member_eligibility(session_id)
            .await
    }

    #[cfg(feature = "openai-live")]
    async fn capture_live_bridge_execution_snapshot(
        &self,
        session_id: &meerkat_core::SessionId,
        agent_identity: &str,
    ) -> Result<meerkat_mob::LiveBridgeExecutionSnapshot, SessionError> {
        self.inner
            .capture_live_bridge_execution_snapshot(session_id, agent_identity)
            .await
    }

    #[cfg(feature = "openai-live")]
    async fn start_live_bridge_member_operation(
        &self,
        request: meerkat_mob::LiveBridgeOperationRequest,
        cancellation: meerkat_mob::LiveBridgeOperationCancellationSignal,
    ) -> Result<
        meerkat_mob::LiveBridgeOperationTerminalFuture,
        meerkat_mob::LiveBridgeOperationStartError,
    > {
        self.inner
            .start_live_bridge_member_operation(request, cancellation)
            .await
    }

    async fn start_turn_with_admission_notification(
        &self,
        session_id: &meerkat_core::types::SessionId,
        req: meerkat_core::service::StartTurnRequest,
        admitted: tokio::sync::oneshot::Sender<()>,
    ) -> Result<meerkat_core::RunResult, SessionError> {
        self.inner
            .start_turn_with_admission_notification(session_id, req, admitted)
            .await
    }

    async fn load_session_for_resume(
        &self,
        session_id: &meerkat_core::types::SessionId,
    ) -> Result<meerkat_mob::ResumeSessionLoad, SessionError> {
        self.inner.load_session_for_resume(session_id).await
    }
    // meerkat 0.8.22: the required `prepare_session_for_resume` hook is
    // DELETED; durable-tail convergence now lives inside
    // `PersistentSessionService`'s override of
    // `materialize_session_resume_verdict`, whose trait DEFAULT converges
    // nothing. This wrapper adds no resume-side projection of its own, so the
    // whole verdict delegates to `inner` - dropping the old passthrough and
    // relying on the default would compile while silently resuming from stale
    // committed authority.
    async fn materialize_session_resume_verdict(
        &self,
        session_id: &meerkat_core::types::SessionId,
    ) -> Result<meerkat_mob::SessionResumeVerdict, SessionError> {
        self.inner
            .materialize_session_resume_verdict(session_id)
            .await
    }
    // meerkat 0.9.5: the verdict for a caller that holds the session's
    // turn-finalization boundary (a member host's materialization). The
    // trait default answers the plain verdict, whose persistent preparation
    // would wait for the boundary its caller holds and, once a durable tail
    // stands, refuse with `SessionError::Busy`.
    async fn materialize_session_resume_verdict_under_runtime_turn_boundary(
        &self,
        session_id: &meerkat_core::types::SessionId,
    ) -> Result<meerkat_mob::SessionResumeVerdict, SessionError> {
        self.inner
            .materialize_session_resume_verdict_under_runtime_turn_boundary(session_id)
            .await
    }
    // Forwarded for the same reason as in the delegating wrappers: the
    // wrapped persistent service settles the tail through its cold-attach
    // preparation, not through the whole resume verdict.
    async fn settle_durable_tail_before_cold_start(
        &self,
        session_id: &meerkat_core::types::SessionId,
    ) -> Result<(), SessionError> {
        self.inner
            .settle_durable_tail_before_cold_start(session_id)
            .await
    }
    // meerkat 0.9.5 reporting settle the mob start calls; the default
    // reports `Settled` and refuses a member whose open run a graceful stop
    // suspended (see the same seam in `delegate_mob_session_service!`).
    async fn settle_durable_tail_before_cold_start_reporting(
        &self,
        session_id: &meerkat_core::types::SessionId,
    ) -> Result<meerkat_mob::ColdStartTailSettlement, SessionError> {
        self.inner
            .settle_durable_tail_before_cold_start_reporting(session_id)
            .await
    }
    async fn observe_session_resume_authority(
        &self,
        session_id: &meerkat_core::types::SessionId,
    ) -> Result<meerkat_mob::SessionResumeAuthority, SessionError> {
        self.inner
            .observe_session_resume_authority(session_id)
            .await
    }

    // NOT cfg-gated on meta's own `experimental-gpt-live`. The trait
    // requires this method whenever MEERKAT's feature is on, and
    // `--all-targets` dev-dependency unification turns meerkat's on while
    // meta's own flag stays off. Gating here on the meta flag deleted
    // the impl exactly when the trait demanded it.
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

    async fn create_session_under_runtime_turn_boundary(
        &self,
        req: meerkat_core::service::CreateSessionRequest,
    ) -> Result<meerkat_core::RunResult, SessionError> {
        let (req, context) = self.prepare_create_request(req);
        let result = self
            .inner
            .create_session_under_runtime_turn_boundary(req)
            .await?;
        Ok(self.complete_create(result, context).await)
    }

    // meerkat 0.8.22: forward the owner-issued resume preparation receipt
    // (see the same seam in `delegate_mob_session_service!`).
    async fn create_session_with_actor_witness_under_runtime_turn_boundary(
        &self,
        req: meerkat_core::service::CreateSessionRequest,
        resume_preparation: Option<meerkat_mob::SessionResumePreparationReceipt>,
        actor_witness_slot: &meerkat_session::LiveSessionActorWitnessSlot,
    ) -> Result<meerkat_core::RunResult, SessionError> {
        let (req, context) = self.prepare_create_request(req);
        let result = self
            .inner
            .create_session_with_actor_witness_under_runtime_turn_boundary(
                req,
                resume_preparation,
                actor_witness_slot,
            )
            .await?;
        Ok(self.complete_create(result, context).await)
    }

    async fn create_session_with_machine_archived_resume_authority_under_runtime_turn_boundary(
        &self,
        req: meerkat_core::service::CreateSessionRequest,
        authorization: meerkat_runtime::ArchivedSessionActorMaterializationAuthorization,
    ) -> Result<meerkat_core::RunResult, SessionError> {
        let (req, context) = self.prepare_create_request(req);
        let result = self
            .inner
            .create_session_with_machine_archived_resume_authority_under_runtime_turn_boundary(
                req,
                authorization,
            )
            .await?;
        Ok(self.complete_create(result, context).await)
    }

    async fn create_session_with_machine_archived_resume_authority_and_actor_witness_under_runtime_turn_boundary(
        &self,
        req: meerkat_core::service::CreateSessionRequest,
        authorization: meerkat_runtime::ArchivedSessionActorMaterializationAuthorization,
        resume_preparation: meerkat_mob::SessionResumePreparationReceipt,
        actor_witness_slot: &meerkat_session::LiveSessionActorWitnessSlot,
    ) -> Result<meerkat_core::RunResult, SessionError> {
        let (req, context) = self.prepare_create_request(req);
        let result = self
            .inner
            .create_session_with_machine_archived_resume_authority_and_actor_witness_under_runtime_turn_boundary(
                req,
                authorization,
                resume_preparation,
                actor_witness_slot,
            )
            .await?;
        Ok(self.complete_create(result, context).await)
    }

    fn supports_persistent_sessions(&self) -> bool {
        self.inner.supports_persistent_sessions()
    }
    // meerkat 0.9.5: forwarded so an inner in-memory composition's false
    // survives this layer (see `delegate_mob_session_service!`).
    fn keeps_durable_member_sessions(&self) -> bool {
        self.inner.keeps_durable_member_sessions()
    }

    fn supports_runtime_turn_apply(&self) -> bool {
        self.inner.supports_runtime_turn_apply()
    }

    // meerkat 0.7.19 disposal-routing seam — forwarded for the same reason
    // as in `delegate_mob_session_service!` above.
    async fn session_known_to_archive_authority(
        &self,
        session_id: &meerkat_core::types::SessionId,
    ) -> Result<bool, SessionError> {
        self.inner
            .session_known_to_archive_authority(session_id)
            .await
    }
    fn acquire_runtime_adapter(
        &self,
        explicit: Option<Arc<meerkat_runtime::MeerkatMachine>>,
    ) -> Result<Option<Arc<meerkat_runtime::MeerkatMachine>>, meerkat_runtime::RuntimeDriverError>
    {
        self.inner.acquire_runtime_adapter(explicit)
    }
    async fn interrupt_with_machine_authority(
        &self,
        session_id: &meerkat_core::types::SessionId,
        authority: meerkat_runtime::MachineSessionControlAuthority,
    ) -> Result<(), SessionError> {
        self.inner
            .interrupt_with_machine_authority(session_id, authority)
            .await
    }
    // meerkat 0.8.22 exact-run machine-authorized hard cancel, default
    // `Err(Unsupported)` (see the same seam in
    // `delegate_mob_session_service!`).
    async fn interrupt_run_with_machine_authority(
        &self,
        session_id: &meerkat_core::types::SessionId,
        expected_run_id: &meerkat_core::lifecycle::RunId,
        authority: meerkat_runtime::MachineSessionControlAuthority,
    ) -> Result<bool, SessionError> {
        self.inner
            .interrupt_run_with_machine_authority(session_id, expected_run_id, authority)
            .await
    }
    // meerkat 0.9.5 exact-run suspend of a graceful requeue stop, default
    // `Ok(false)`, which hard-cancels the run (see the same seam in
    // `delegate_mob_session_service!`).
    async fn suspend_run_with_machine_authority(
        &self,
        session_id: &meerkat_core::types::SessionId,
        expected_run_id: &meerkat_core::lifecycle::RunId,
        authority: meerkat_runtime::MachineSessionControlAuthority,
    ) -> Result<bool, SessionError> {
        self.inner
            .suspend_run_with_machine_authority(session_id, expected_run_id, authority)
            .await
    }
    async fn cancel_after_boundary_with_machine_authority(
        &self,
        session_id: &meerkat_core::types::SessionId,
        expected_run_id: &meerkat_core::lifecycle::RunId,
        authority: meerkat_runtime::MachineSessionControlAuthority,
    ) -> Result<(), SessionError> {
        cancel_after_boundary_with_machine_authority_if_live(
            self.inner.as_ref(),
            session_id,
            expected_run_id,
            authority,
        )
        .await
    }
    async fn cancel_current_after_boundary_with_machine_authority(
        &self,
        session_id: &meerkat_core::types::SessionId,
        authority: meerkat_runtime::MachineSessionControlAuthority,
    ) -> Result<(), SessionError> {
        self.inner
            .cancel_current_after_boundary_with_machine_authority(session_id, authority)
            .await
    }
    async fn live_session_actor_registered(
        &self,
        session_id: &meerkat_core::types::SessionId,
    ) -> Result<bool, SessionError> {
        self.inner.live_session_actor_registered(session_id).await
    }
    async fn session_projection_visible(
        &self,
        session_id: &meerkat_core::types::SessionId,
    ) -> Result<bool, SessionError> {
        self.inner.session_projection_visible(session_id).await
    }
    async fn session_belongs_to_mob(
        &self,
        session_id: &meerkat_core::types::SessionId,
        mob_id: &meerkat_mob::MobId,
    ) -> bool {
        self.inner.session_belongs_to_mob(session_id, mob_id).await
    }
    async fn load_persisted_session(
        &self,
        session_id: &meerkat_core::types::SessionId,
    ) -> Result<Option<meerkat_core::session::Session>, SessionError> {
        self.inner.load_persisted_session(session_id).await
    }
    // meerkat 0.9.5 tolerant committed read; the default is the strict read,
    // which refuses mid-run (see the same seam in
    // `delegate_mob_session_service!`).
    async fn load_persisted_session_tolerating_tail(
        &self,
        session_id: &meerkat_core::types::SessionId,
    ) -> Result<Option<meerkat_core::session::Session>, SessionError> {
        self.inner
            .load_persisted_session_tolerating_tail(session_id)
            .await
    }
    // Forwarded, as in `delegate_mob_session_service!`: for a service that
    // keeps durable sessions the trait default refuses every resume spawn.
    async fn provisional_tail_stands(
        &self,
        session_id: &meerkat_core::types::SessionId,
    ) -> Result<bool, SessionError> {
        meerkat_mob::MobSessionService::provisional_tail_stands(self.inner.as_ref(), session_id)
            .await
    }
    // meerkat 0.7.29 revival seam (ask 31): forward, never default — the
    // trait defaults mask the inner service's revival support (Bug I victims
    // stay unresumable through the wrapper).
    async fn load_revivable_retired_session(
        &self,
        session_id: &meerkat_core::types::SessionId,
    ) -> Result<Option<meerkat_core::session::Session>, SessionError> {
        self.inner.load_revivable_retired_session(session_id).await
    }
    // meerkat 0.8.22 durable transcript fork, default `Err(Unsupported)` (see
    // the same seam in `delegate_mob_session_service!`).
    async fn fork_persisted_session(
        &self,
        source_session_id: &meerkat_core::types::SessionId,
        message_count: Option<usize>,
        tool_access_policy: Option<meerkat_core::ops::ToolAccessPolicy>,
        target: meerkat_core::DurableSessionForkTarget,
    ) -> Result<meerkat_core::SessionForkResult, SessionError> {
        self.inner
            .fork_persisted_session(source_session_id, message_count, tool_access_policy, target)
            .await
    }

    // meerkat 0.8.41 made `fork_persisted_session_at_turn_boundary` REQUIRED so
    // every wrapper forwards the boundary-then-fork as ONE contract to the owner;
    // a default that took the boundary and then called `fork_persisted_session`
    // self-deadlocks on the persistent owner's non-reentrant boundary.
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

    // Forwarded exactly, as in `delegate_mob_session_service!`.
    async fn commit_live_delegation_represented_transcript_at_turn_boundary(
        &self,
        machine: &meerkat_runtime::MeerkatMachine,
        session_id: &meerkat_core::types::SessionId,
        provisional: meerkat_core::ProvisionalLiveHandoff,
        final_event: meerkat_core::RealtimeTranscriptEvent,
        represented: Vec<meerkat_core::RepresentedLiveUserRow>,
        bound: std::time::Duration,
    ) -> Result<meerkat_core::LiveFinalTranscriptCommitAtTurnBoundary, SessionError> {
        self.inner
            .commit_live_delegation_represented_transcript_at_turn_boundary(
                machine,
                session_id,
                provisional,
                final_event,
                represented,
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
    async fn load_persisted_session_metadata(
        &self,
        session_id: &meerkat_core::types::SessionId,
    ) -> Result<Option<meerkat_core::PersistedSessionMetadataView>, SessionError> {
        self.inner.load_persisted_session_metadata(session_id).await
    }
    // Preserve the inner retained authority, including archived metadata,
    // absence, unsupported authority and read faults.
    async fn load_retained_session_metadata(
        &self,
        session_id: &meerkat_core::types::SessionId,
    ) -> Result<Option<meerkat_core::PersistedSessionMetadataView>, SessionError> {
        self.inner.load_retained_session_metadata(session_id).await
    }
    // Forwarded, as in `delegate_mob_session_service!`: the trait default
    // refuses, which would refuse every member respawn through this wrapper.
    async fn predecessor_tool_restrictions(
        &self,
        session_id: &meerkat_core::types::SessionId,
    ) -> Result<meerkat_core::PredecessorToolRestrictions, SessionError> {
        self.inner.predecessor_tool_restrictions(session_id).await
    }
    // Forwarded, as in `delegate_mob_session_service!`: the trait default
    // answers that the source is not live.
    async fn fork_source_tools(
        &self,
        session_id: &meerkat_core::types::SessionId,
    ) -> meerkat_core::fork_notice::ForkSeedSourceTools {
        self.inner.fork_source_tools(session_id).await
    }
    async fn authorize_revivable_retired_session(
        &self,
        session_id: &meerkat_core::types::SessionId,
        authority: meerkat_runtime::PreparedArchivedResumeCommitLease,
    ) -> Result<meerkat_runtime::AuthorizedArchivedResumeCommitLease, SessionError> {
        self.inner
            .authorize_revivable_retired_session(session_id, authority)
            .await
    }
    async fn create_session_with_machine_archived_resume_authority(
        &self,
        req: meerkat_core::service::CreateSessionRequest,
        authorization: meerkat_runtime::ArchivedSessionActorMaterializationAuthorization,
    ) -> Result<meerkat_core::RunResult, SessionError> {
        let (req, context) = self.prepare_create_request(req);
        let result = self
            .inner
            .create_session_with_machine_archived_resume_authority(req, authorization)
            .await?;
        Ok(self.complete_create(result, context).await)
    }
    async fn subscribe_session_events(
        &self,
        session_id: &meerkat_core::types::SessionId,
    ) -> Result<meerkat_core::comms::EventStream, meerkat_core::comms::StreamError> {
        meerkat_mob::MobSessionService::subscribe_session_events(self.inner.as_ref(), session_id)
            .await
    }
    async fn subscribe_agent_session_events_from(
        &self,
        session_id: &meerkat_core::types::SessionId,
        cursor: meerkat_core::comms::SessionEventCursor,
    ) -> Result<meerkat_mob::AgentEventSubscription, meerkat_core::comms::StreamError> {
        meerkat_mob::MobSessionService::subscribe_agent_session_events_from(
            self.inner.as_ref(),
            session_id,
            cursor,
        )
        .await
    }
    async fn archive_with_mob_lifecycle_authority(
        &self,
        session_id: &meerkat_core::types::SessionId,
    ) -> Result<(), SessionError> {
        self.inner
            .archive_with_mob_lifecycle_authority(session_id)
            .await
    }
    async fn archive_with_mob_lifecycle_authority_under_runtime_turn_boundary(
        &self,
        session_id: &meerkat_core::types::SessionId,
    ) -> Result<(), SessionError> {
        self.inner
            .archive_with_mob_lifecycle_authority_under_runtime_turn_boundary(session_id)
            .await
    }
    // meerkat 0.8.22 deadline-aware disposal archive, default
    // `Err(Unsupported)` (see the same seam in
    // `delegate_mob_session_service!`).
    async fn archive_with_mob_lifecycle_authority_under_runtime_turn_boundary_before(
        &self,
        session_id: &meerkat_core::types::SessionId,
        deadline: meerkat_core::time_compat::Instant,
    ) -> Result<(), SessionError> {
        self.inner
            .archive_with_mob_lifecycle_authority_under_runtime_turn_boundary_before(
                session_id, deadline,
            )
            .await
    }
    // Same seam as `delegate_mob_session_service!`: the hook-bearing lane's
    // trait default refuses when a hook is present; forward it.
    async fn archive_with_mob_lifecycle_authority_under_runtime_turn_boundary_and_hook_before(
        &self,
        session_id: &meerkat_core::types::SessionId,
        deadline: meerkat_core::time_compat::Instant,
        post_commit_hook: Option<Arc<dyn meerkat_runtime::MachineSessionArchivePostCommitHook>>,
    ) -> Result<(), SessionError> {
        self.inner
            .archive_with_mob_lifecycle_authority_under_runtime_turn_boundary_and_hook_before(
                session_id,
                deadline,
                post_commit_hook,
            )
            .await
    }
    // meerkat 0.9.6 cold-predecessor settlement, default typed refusal (see
    // the same seam in `delegate_mob_session_service!`).
    async fn settle_cold_predecessor_with_mob_lifecycle_authority_before(
        &self,
        session_id: &meerkat_core::types::SessionId,
        claim: &meerkat_core::session_hosting::HostingClaim,
        deadline: meerkat_core::time_compat::Instant,
    ) -> Result<
        meerkat_runtime::ColdPredecessorSettlement,
        meerkat_session::ColdPredecessorSettleError,
    > {
        self.inner
            .settle_cold_predecessor_with_mob_lifecycle_authority_before(
                session_id, claim, deadline,
            )
            .await
    }

    // Same seam as `delegate_mob_session_service!`: forward every remaining
    // defaulted method; the conformance gate keeps this list complete.
    fn forked_participant_source_runtime(
        self: Arc<Self>,
    ) -> Option<Arc<dyn meerkat_mob::ForkedParticipantSourceRuntime>> {
        Arc::clone(&self.inner).forked_participant_source_runtime()
    }
    fn persisted_session_authority_read_cost(
        &self,
    ) -> meerkat_mob::runtime::PersistedSessionAuthorityReadCost {
        self.inner.persisted_session_authority_read_cost()
    }
    async fn observe_persisted_session_authority(
        &self,
        session_id: &meerkat_core::types::SessionId,
    ) -> Result<Option<meerkat_mob::IdentitySessionStoreAuthority>, SessionError> {
        self.inner
            .observe_persisted_session_authority(session_id)
            .await
    }
    async fn revalidate_session_resume_authority(
        &self,
        session_id: &meerkat_core::types::SessionId,
        expected: &meerkat_mob::SessionResumeAuthority,
    ) -> Result<Result<(), meerkat_mob::SessionResumeRejection>, SessionError> {
        self.inner
            .revalidate_session_resume_authority(session_id, expected)
            .await
    }
    async fn discard_live_session_actor_after_durability_reload_required(
        &self,
        witness: &meerkat_session::LiveSessionActorWitness,
    ) -> Result<bool, SessionError> {
        self.inner
            .discard_live_session_actor_after_durability_reload_required(witness)
            .await
    }
    async fn execution_snapshot(
        &self,
        session_id: &meerkat_core::types::SessionId,
    ) -> Result<Option<meerkat_core::agent::AgentExecutionSnapshot>, SessionError> {
        self.inner.execution_snapshot(session_id).await
    }
    async fn tool_scope_snapshot(
        &self,
        session_id: &meerkat_core::types::SessionId,
    ) -> Result<Option<meerkat_core::ToolScopeSnapshot>, SessionError> {
        self.inner.tool_scope_snapshot(session_id).await
    }
    // Forwarded exactly, like its tool-scope siblings (see the delegating
    // wrapper above).
    async fn tool_catalog_view(
        &self,
        session_id: &meerkat_core::types::SessionId,
    ) -> Result<Option<meerkat_core::ToolCatalogView>, SessionError> {
        self.inner.tool_catalog_view(session_id).await
    }
    async fn live_visible_tool_defs(
        &self,
        session_id: &meerkat_core::types::SessionId,
    ) -> Result<Option<Vec<meerkat_core::ToolDef>>, SessionError> {
        self.inner.live_visible_tool_defs(session_id).await
    }
    async fn external_tool_surface_snapshot(
        &self,
        session_id: &meerkat_core::types::SessionId,
    ) -> Result<Option<meerkat_core::ExternalToolSurfaceSnapshot>, SessionError> {
        self.inner.external_tool_surface_snapshot(session_id).await
    }
    async fn peer_ingress_runtime_snapshot(
        &self,
        session_id: &meerkat_core::types::SessionId,
    ) -> Result<Option<meerkat_core::PeerIngressRuntimeSnapshot>, SessionError> {
        self.inner.peer_ingress_runtime_snapshot(session_id).await
    }
    async fn apply_runtime_turn(
        &self,
        session_id: &meerkat_core::types::SessionId,
        run_id: meerkat_core::lifecycle::RunId,
        req: meerkat_core::service::StartTurnRequest,
        boundary: meerkat_core::lifecycle::run_primitive::RunApplyBoundary,
        contributing_input_ids: Vec<meerkat_core::lifecycle::InputId>,
    ) -> Result<meerkat_core::lifecycle::core_executor::CoreApplyOutput, SessionError> {
        self.inner
            .apply_runtime_turn(session_id, run_id, req, boundary, contributing_input_ids)
            .await
    }
    async fn prepare_turn_boundary_delivery_for_active_turn(
        &self,
        session_id: &meerkat_core::types::SessionId,
        expected_run_id: &meerkat_core::lifecycle::RunId,
        delivery: meerkat_core::TurnBoundaryDelivery,
    ) -> Result<meerkat_core::CoreBoundaryStageOutput, meerkat_core::CoreBoundaryStageError> {
        self.inner
            .prepare_turn_boundary_delivery_for_active_turn(session_id, expected_run_id, delivery)
            .await
    }
    async fn acknowledge_committed_runtime_session_boundary_under_turn_finalization_boundary(
        &self,
        session_id: &meerkat_core::types::SessionId,
        authority: &meerkat_core::CommittedSessionBoundaryAuthority,
    ) -> Result<(), SessionError> {
        self.inner
            .acknowledge_committed_runtime_session_boundary_under_turn_finalization_boundary(
                session_id, authority,
            )
            .await
    }
    async fn acquire_runtime_turn_finalization_guard(
        &self,
        session_id: &meerkat_core::types::SessionId,
    ) -> Result<Box<dyn meerkat_core::lifecycle::CoreExecutorTurnFinalizationGuard>, SessionError>
    {
        self.inner
            .acquire_runtime_turn_finalization_guard(session_id)
            .await
    }
    async fn checkpoint_committed_runtime_session_snapshot_under_turn_finalization_boundary(
        &self,
        session_id: &meerkat_core::types::SessionId,
        session_snapshot: Arc<Vec<u8>>,
    ) -> Result<(), SessionError> {
        self.inner
            .checkpoint_committed_runtime_session_snapshot_under_turn_finalization_boundary(
                session_id,
                session_snapshot,
            )
            .await
    }
    async fn discard_live_session_after_runtime_stop_terminalized(
        &self,
        session_id: &meerkat_core::types::SessionId,
    ) -> Result<(), SessionError> {
        self.inner
            .discard_live_session_after_runtime_stop_terminalized(session_id)
            .await
    }
    async fn discard_live_session_after_runtime_stop_terminalized_under_turn_finalization_boundary(
        &self,
        session_id: &meerkat_core::types::SessionId,
    ) -> Result<(), SessionError> {
        self.inner
            .discard_live_session_after_runtime_stop_terminalized_under_turn_finalization_boundary(
                session_id,
            )
            .await
    }
    // meerkat 0.8.22: keyed by the exact actor incarnation, not SessionId
    // (see the same seam in `delegate_mob_session_service!`).
    async fn publish_boundary_appends_discarded_for_actor(
        &self,
        actor_witness: &meerkat_session::LiveSessionActorWitness,
        discarded: &meerkat_core::event::BoundaryAppendsDiscarded,
    ) -> Result<(), SessionError> {
        self.inner
            .publish_boundary_appends_discarded_for_actor(actor_witness, discarded)
            .await
    }
    async fn publish_settled_run_terminal_for_actor(
        &self,
        actor_witness: &meerkat_session::LiveSessionActorWitness,
        settled: &meerkat_core::lifecycle::SettledRunTerminal,
    ) -> Result<(), SessionError> {
        self.inner
            .publish_settled_run_terminal_for_actor(actor_witness, settled)
            .await
    }
    async fn publish_interaction_terminals_for_actor(
        &self,
        actor_witness: &meerkat_session::LiveSessionActorWitness,
        events: &[meerkat_core::event::AgentEvent],
    ) -> Result<
        Vec<meerkat_core::lifecycle::core_executor::CoreInteractionTerminalPublicationReceipt>,
        SessionError,
    > {
        self.inner
            .publish_interaction_terminals_for_actor(actor_witness, events)
            .await
    }
    async fn discard_live_session(
        &self,
        session_id: &meerkat_core::types::SessionId,
    ) -> Result<(), SessionError> {
        self.inner.discard_live_session(session_id).await
    }
    async fn discard_live_session_under_runtime_turn_boundary(
        &self,
        session_id: &meerkat_core::types::SessionId,
    ) -> Result<(), SessionError> {
        self.inner
            .discard_live_session_under_runtime_turn_boundary(session_id)
            .await
    }
    async fn discard_live_session_actor_under_runtime_turn_boundary(
        &self,
        witness: &meerkat_session::LiveSessionActorWitness,
    ) -> Result<bool, SessionError> {
        self.inner
            .discard_live_session_actor_under_runtime_turn_boundary(witness)
            .await
    }
    // Default `Ok(Unsupported)`; forwarded with the exact actor (see the
    // same seam in `delegate_mob_session_service!`).
    async fn controller_route_under_runtime_turn_boundary(
        &self,
        actor: &meerkat_session::LiveSessionActorWitness,
    ) -> Result<meerkat_mob::ControllerRouteObservation, SessionError> {
        self.inner
            .controller_route_under_runtime_turn_boundary(actor)
            .await
    }
    // Default `Unsupported`; forwarded with the exact actor to the inner
    // service that owns the forwarded boundary (see the same seam in
    // `delegate_mob_session_service!`).
    async fn pin_controller_client_under_runtime_turn_boundary(
        &self,
        actor: &meerkat_session::LiveSessionActorWitness,
    ) -> Result<meerkat_core::ControllerModelClient, SessionError> {
        self.inner
            .pin_controller_client_under_runtime_turn_boundary(actor)
            .await
    }
    // Default returns at once (see the same seam in
    // `delegate_mob_session_service!`).
    async fn await_removed_actor_exit(&self, session_id: &meerkat_core::types::SessionId) {
        self.inner.await_removed_actor_exit(session_id).await;
    }
    async fn await_event_projection_drain(
        &self,
        session_id: &meerkat_core::types::SessionId,
    ) -> Result<bool, SessionError> {
        self.inner.await_event_projection_drain(session_id).await
    }
    async fn checkpoint_committed_runtime_session_snapshot(
        &self,
        session_id: &meerkat_core::types::SessionId,
        session_snapshot: Arc<Vec<u8>>,
    ) -> Result<(), SessionError> {
        self.inner
            .checkpoint_committed_runtime_session_snapshot(session_id, session_snapshot)
            .await
    }
    async fn cancel_all_checkpointers(&self) {
        self.inner.cancel_all_checkpointers().await;
    }
    async fn rearm_all_checkpointers(&self) {
        self.inner.rearm_all_checkpointers().await;
    }
}

/// Specification for bootstrapping a mob runtime from a definition, storage, and session service.
/// Typed live-composition inputs retained beside the erased session service.
///
/// `ConsoleVoiceController` and `live_wiring::attach_live` take the concrete
/// `PersistentSessionService`, the agent factory, and the host config. The
/// gateway binaries build their own service and hand these over directly;
/// the builder path retains them here so `UnifiedRuntime::compose_live` can
/// do the same composition for library embedders.
#[derive(Clone)]
pub(crate) struct LiveComposeInputs {
    pub(crate) service: Arc<meerkat_session::PersistentSessionService<FactoryAgentBuilder>>,
    pub(crate) machine: Arc<meerkat_runtime::MeerkatMachine>,
    pub(crate) factory: AgentFactory,
    pub(crate) config: Config,
}

/// The canonical runtime machine of a runtime-backed spec. With a supplied
/// native grant configuration (D50 S1), the machine is constructed with that
/// actual grant owner and its paired invocation policy installed; Meta never
/// issues grants itself. Without one, it is the unscoped machine as before.
pub(crate) fn persistent_runtime_machine(
    runtime_store: Arc<dyn meerkat_runtime::RuntimeStore>,
    blob_store: Arc<dyn meerkat_core::BlobStore>,
    grant_configuration: Option<meerkat_runtime::meerkat_machine::NativeGrantWorkConfiguration>,
) -> Result<meerkat_runtime::MeerkatMachine, meerkat_runtime::RuntimeDriverError> {
    match grant_configuration {
        Some(configuration) => {
            meerkat_runtime::MeerkatMachine::persistent_with_local_grant_authorization(
                runtime_store,
                Some(blob_store),
                configuration,
            )
        }
        None => meerkat_runtime::MeerkatMachine::persistent(runtime_store, blob_store),
    }
}

/// What a judgement of a spec's mob storage before preparation found
/// ([`MobBootstrapSpec::read_launch_definition`]).
enum LaunchDefinitionRead {
    /// The event log was empty: a fresh boot, which creates the mob from the
    /// supplied definition.
    Fresh,
    /// The event log held events: the resume, judged.
    Resume(Box<JudgedResume>),
}

/// A resume of a non-empty mob storage, judged against the supplied
/// composition by [`judge_resume`]: what the resume binds to, and what the
/// judgement leaves for preparation to apply.
struct JudgedResume {
    /// The canonical definition snapshot the resume binds to. `None` for a
    /// storage declared in-process whose definition was not read: that
    /// resume is not verified, and Core reads the definition itself.
    snapshot: Option<meerkat_mob::MobDefinitionSnapshot>,
    /// The storage this launch speaks for has no composition manifest beside
    /// it: preparation records the supplied definition as its composition.
    record_supplied_composition: bool,
    /// The fields in which the stored definition a candidate knowingly boots
    /// differs from the supplied one (empty when they match).
    candidate_diverged: Vec<String>,
}

/// The expected epoch a resume refuses with
/// (`MobError::MobDefinitionAuthorityChanged`) when the read before
/// preparation found no definition. Core numbers definition epochs from 1.
pub(crate) const NO_STORED_DEFINITION_EPOCH: u64 = 0;

/// Where a persistent mob composition's authority lives
/// ([`MobBootstrapSpec::persistent_inner_with_provider_stores`]).
pub(crate) enum PersistentAuthority {
    /// The state directory the composition is built over.
    StateDir,
    /// A host store realm (see [`crate::host_store`]); the directory the
    /// composition is built over is a per-host cache.
    HostStore {
        /// The cache directory the runtime owns, when the host named none:
        /// it lives as long as the runtime.
        cache_dir: Option<Arc<tempfile::TempDir>>,
    },
}

pub struct MobBootstrapSpec {
    pub definition: MobDefinition,
    pub storage: MobStorage,
    /// Whether `storage` carries a composition that outlives the process.
    ///
    /// Declared by the launch path, because `MobStorage` does not report its
    /// own durability. `bootstrap` uses it to decide whether the composition
    /// needs a provenance record at all: an ephemeral storage has no
    /// cross-restart composition to judge.
    pub mob_storage_provenance: crate::mob_composition_manifest::MobStorageProvenance,
    /// Whether this launch speaks for the durable composition of its mob
    /// storage. Defaults to authoritative; a candidate or certification pass
    /// declares itself non-authoritative so it neither creates the composition
    /// pin nor is refused by one.
    pub composition_authority: crate::mob_composition_manifest::CompositionAuthority,
    /// For a candidate (non-authoritative) resume: whether a supplied
    /// definition that differs from the stored one refuses (the default) or
    /// boots the stored definition knowingly. Ignored by authoritative
    /// launches, which the composition pin judges.
    ///
    /// A knowing boot logs the diverged fields at WARN and, when the spec
    /// declares a storage census ([`Self::resolved_storage`]), reports them
    /// in a degraded `mob_composition` slot.
    pub candidate_definition: crate::mob_composition_manifest::CandidateDefinition,
    /// What [`Self::read_launch_definition`] judged of [`Self::storage`]
    /// before preparation. Preparation applies that judgement and binds the
    /// resume to its snapshot instead of judging again; `None` when nothing
    /// judged it.
    launch_definition_read: Option<LaunchDefinitionRead>,
    pub session_service: Arc<dyn MobSessionService>,
    /// The host config the member builder resolves models with: the image
    /// checks judge a member's model the way Core's model registry does,
    /// self-hosted and config-declared models included. `None` until a stock
    /// constructor or [`Self::with_member_model_config`] records it; the
    /// static model catalog alone answers then.
    pub(crate) member_model_config: Option<Arc<meerkat::Config>>,
    pub binary_blob_store: Option<Arc<dyn BinaryBlobStore>>,
    pub(crate) agent_mob_mcp_state: Option<Arc<meerkat_mob_mcp::MobMcpState>>,
    runtime_delivery: Option<MobRuntimeDelivery>,
    pub(crate) implicit_delegate_retirement_overrides: Option<ImplicitDelegateRetirementOverrides>,
    pub(crate) agent_mob_default_llm_client_slot: Option<SharedDefaultLlmClientSlot>,
    pub(crate) console_spawn_sink_slot: Option<SharedConsoleSpawnSinkSlot>,
    pub(crate) identity_runtime_slot: Option<SharedIdentityRuntimeSlot>,
    pub options: MobBootstrapOptions,
    /// Strong runtime authority shared by the session service and every
    /// runtime host installed for it.
    ///
    /// Keeping the adapter here is significant for ephemeral services: their
    /// upstream adapter cache is weak, so an installed host would otherwise be
    /// lost before bootstrap asks the service for its adapter again.
    pub runtime_adapter: Option<Arc<meerkat_runtime::MeerkatMachine>>,
    /// The concrete inputs a live door (console voice, `meta/live/*`)
    /// composes over, retained only when the launch path built a
    /// persistent session service. The erased `session_service` above stays
    /// the mob's authority; this is the typed view the live wiring needs and
    /// that library embedders could not otherwise reach.
    pub(crate) live_compose_inputs: Option<LiveComposeInputs>,
    /// Pre-build customizer applied to every mob member spawn (classic-path
    /// agent memory rides here — see `crate::memory::spawn_customizer`).
    /// Forwarded to `MobBuilder::with_spawn_member_customizer`.
    pub(crate) spawn_member_customizer: Option<Arc<dyn meerkat_mob::SpawnMemberCustomizer>>,
    /// Host member registration hook (D50 S2). Forwarded to
    /// `MobBuilder::with_member_runtime_registration_hook` for the root mob
    /// and to the agent mob-tool state for child mobs.
    pub(crate) member_runtime_registration_hook:
        Option<Arc<dyn meerkat_mob::MemberRuntimeRegistrationHook>>,
    /// Restored members whose customizer tools no publication before the mob
    /// build covers: an early `customize_build` failure, every roster
    /// identity of a composition that publishes none before its build, and
    /// a member whose identity the roster no longer lists. Forwarded to
    /// `MobBuilder::hold_restored_member_run_starts` for
    /// `HostRunStartHoldReason::ToolsNotPublished`, so meerkat's restore
    /// starts no run for them (a queued input, a kickoff, a peer message)
    /// without their tools; the identity runtime releases each hold when the
    /// member's materialization publishes them.
    pub(crate) restored_members_awaiting_tools: Vec<meerkat_mob::AgentIdentity>,
    /// Process-local registry resolving exact application consequence-policy
    /// identities for member builds, forwarded to
    /// `MobBuilder::with_tool_consequence_policy_registry`.
    ///
    /// `None` leaves member builds on Meerkat's unmanaged/inherited default,
    /// which is the behaviour before this seam existed.
    pub(crate) tool_consequence_policy_registry:
        Option<Arc<meerkat_core::ToolConsequencePolicyRegistry>>,
    /// Mob-wide external-tools provider, forwarded to
    /// `MobBuilder::with_default_external_tools_provider`. Called at EVERY
    /// member spawn — including revival — so tools attached here survive
    /// `materialize_revived_member_session`, unlike the per-spawn
    /// `SpawnMemberSpec.external_tools` overlay, which revival drops
    /// (meerkat-studio ask K4/M2). NOTE: a profile's `tools.mcp` allowlist
    /// gates what this provider exposes to that member, and an EMPTY allowlist
    /// means the full surface, not none.
    pub(crate) default_external_tools_provider: Option<meerkat_mob::ExternalToolsProvider>,
    /// Named Rust tool bundles, forwarded to
    /// `MobBuilder::register_tool_bundle` for the profiles'
    /// `tools.rust_bundles`. They are registered on every build of this mob:
    /// create, resume (including the members it revives) and respawn. Mobs
    /// that agents create through the mob tools receive only the ones marked
    /// child-available in [`Self::child_tool_bundle_availability`].
    pub(crate) tool_bundles: BTreeMap<String, Arc<dyn meerkat_core::AgentToolDispatcher>>,
    /// Which registered bundles the host supplies to child mobs (agent
    /// `mob_create` and delegate's implicit mob). Absent means host-only.
    pub(crate) child_tool_bundle_availability:
        BTreeMap<String, meerkat_mob_mcp::ChildToolBundleAvailability>,
    /// The application tool policy every child mob member is built with,
    /// forwarded to the agent mob-tool state with the registry. With a
    /// registry installed and no child policy, meerkat refuses agent
    /// `mob_create` and delegate with a typed tool error until one is set.
    pub(crate) child_application_tool_policy: Option<meerkat_core::ApplicationToolPolicyBinding>,
    /// What the agent mob tools were installed with, so bootstrap can
    /// reinstall them with the final child policy.
    pub(crate) agent_mob_tools_install: Option<AgentMobToolsInstall>,
    /// Where agent-created child mobs keep their event logs, when the host
    /// chose. Recorded here and applied at the bootstrap boundary, so a
    /// durable selection that cannot open fails there and an explicit late
    /// [`ChildMobStorage::ProcessBound`](crate::ChildMobStorage::ProcessBound)
    /// never opens the child root at all. `None` resolves the default at that
    /// boundary (see [`MobBootstrapSpec::with_meta_storage_layout`]).
    pub(crate) child_mob_storage: Option<crate::child_mob_storage::ChildMobStorage>,
    /// The Meta layout this runtime is composed over, which the default
    /// child-mob storage resolves against.
    pub(crate) meta_storage_layout: Option<crate::storage_layout::MetaStorageLayout>,
    /// Exclusive ownership of the durable child root, taken once at the
    /// bootstrap boundary and handed to the runtime.
    pub(crate) child_mob_root_claim: Option<Arc<crate::child_mob_storage::ChildMobRootClaim>>,
    /// The custody its members' MCP routers are composed under, settled by
    /// the runtime's shutdown. Set via
    /// [`with_member_mcp_custody`](Self::with_member_mcp_custody).
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) member_mcp_custody: Option<Arc<crate::member_mcp_custody::MemberMcpCustody>>,
    #[cfg(feature = "extension-state")]
    pub(crate) before_activation: Option<meerkat_mob::MobBeforeActivation>,
    /// Releases what `before_activation` bound of a child mob the agent
    /// mob-tool state destroys, before that destroy's storage cleanup.
    #[cfg(feature = "extension-state")]
    pub(crate) after_destroy: Option<meerkat_mob_mcp::MobAfterDestroy>,
    /// Admits or refuses each mob the agent mob-tool state creates, before
    /// anything of it exists.
    #[cfg(feature = "extension-state")]
    pub(crate) before_create: Option<meerkat_mob_mcp::MobBeforeCreate>,
    /// The meerkat-level realm this spec's persistent stores were composed
    /// under (the fresh default or an adopted predecessor's recorded realm),
    /// with the state directory it was resolved from; `None` for a spec not
    /// composed over a persistent state directory.
    #[cfg(feature = "extension-state")]
    pub(crate) composed_meerkat_realm: Option<(PathBuf, meerkat_core::RealmId)>,
    /// Public document MCP descriptors retained until bootstrap installs them
    /// on the final agent mob-tool state, after child policy composition.
    #[cfg(feature = "extension-state")]
    pub(crate) child_mcp_servers: Option<meerkat_mob_mcp::ChildMcpServers>,
    /// Realm-scoped WorkGraph service, forwarded to
    /// `MobBuilder::with_workgraph_service` so every mob-executor turn gets
    /// apply-time attention overlay injection, and to the agent mob-tool
    /// state so child mobs inherit it. Set via
    /// [`with_workgraph_service`](Self::with_workgraph_service).
    pub(crate) workgraph_service: Option<meerkat::WorkGraphService>,
    /// Tool-plane admission slots (one per `install_workgraph_tools` call on
    /// a builder feeding this runtime). `MobRuntime::bootstrap` fills each
    /// with the runtime-wide [`WorkGraphAdmission`] so the agent tool plane's
    /// `workgraph_attention_reassign` runs the same duplicate-binding guard
    /// (and holds the same gate) as the RPC surfaces.
    pub(crate) workgraph_admission_slots: Vec<crate::workgraph_admission::WorkGraphAdmissionSlot>,
    /// Empties this spec's builder tool slots once nothing holds the
    /// composition (see [`CompositionRelease`]).
    pub(crate) composition_release: CompositionRelease,
    /// Cross-process admission sidecar path — set when the workgraph store is
    /// SQLite-backed (shareable by a gateway + library-mode runtime on one
    /// state dir), `None` for memory-backed runtimes (single-process by
    /// construction).
    pub(crate) workgraph_admission_sidecar: Option<PathBuf>,
    /// Whether bootstrap migrates member-bound attention bindings that
    /// predate meerkat's mob-realm rescoping (see
    /// [`crate::workgraph_realm::migrate_member_bindings_to_mob_realms`]).
    pub(crate) workgraph_realm_migration: crate::workgraph_realm::WorkGraphRealmMigrationMode,
    /// Composition-time storage durability resolution (H1/H2), surfaced by
    /// the runtime health surfaces. The stock constructors record it;
    /// externally-composed specs (`MobBootstrapSpec::new` — both gateway
    /// binaries roll their own session services) should set it beside their
    /// own store composition, and `None` renders as an absent declaration.
    pub resolved_storage: Option<ResolvedStorageSummary>,
    /// The composition's durable runtime store: the store the runtime
    /// machine and the session service persist through. The mob runtime
    /// reads it for evidence only that store can give (a notice attempt that
    /// settled, a session's committed boundary authority); `None` reports
    /// nothing.
    pub(crate) durable_runtime_store: Option<Arc<dyn meerkat_runtime::RuntimeStore>>,
    /// The session store the stock constructors composed with a durable
    /// runtime store (Core's pair on `runtime.sqlite`, or a storage
    /// provider's realm pair). The runtime builder hands it to the agent
    /// memory engines, which read the transcripts the runtime persists;
    /// `None` when the composition has no durable session store.
    pub(crate) durable_session_store: Option<Arc<dyn SessionStore>>,
    /// Committed-boundary heal authority for the identity-first continuity
    /// repair supervisor (2026-07-29 heal/re-Break incident). The stock
    /// persistent constructors record the concrete meerkat-backed recoverer;
    /// externally-composed specs may leave it `None` (no heal seam — the
    /// repair supervisor falls back to plain reconcile retries).
    pub committed_boundary_recoverer:
        Option<Arc<dyn crate::identity_first::bridge::CommittedBoundaryRecoverer>>,
    /// The concrete session owner's controller pin operation, captured before
    /// the session service is erased, for governed binding.
    pub controller_pin_source: Option<Arc<dyn crate::identity_first::bridge::ControllerPinSource>>,
    /// Late-bound §10.1 dispatch-time taint slot carried by the base
    /// session-service wrapper `Self::with_base_layer` installs; see
    /// [`Self::dispatch_taint_slot`].
    pub(crate) dispatch_taint_slot: crate::memory::dispatch_taint::DispatchTaintSlot,
    /// The host's fork instruction policy for this mob's member forks
    /// ([`Self::with_fork_instruction_policy`]). The mob does not persist
    /// it, so bootstrap applies it on create and on resume.
    pub(crate) fork_instruction_policy: meerkat_core::ForkInstructionPolicy,
    /// The provider credential store the member builder's agent factory
    /// resolves with, as a stock constructor left it (test builds only).
    #[cfg(test)]
    pub(crate) member_provider_auth_persistence: Option<meerkat::ProviderAuthPersistence>,
    /// Holds the ephemeral temp directory alive for the lifetime of the spec.
    /// Only populated when the builder creates an ephemeral runtime.
    pub(crate) _ephemeral_dir: Option<Arc<tempfile::TempDir>>,
    /// The state directories whose persistent stores this spec composed,
    /// each claimed for this runtime before any of its stores opened (see
    /// [`crate::state_dir_claim`]). The runtime keeps them until a clean
    /// stand-down or until it drops.
    pub(crate) state_dir_claims: Vec<Arc<crate::state_dir_claim::StateDirClaim>>,
}

impl MobBootstrapSpec {
    /// A spec over a session service composed outside Meta's stock
    /// constructors. No Meta pre-build hook runs inside it, so the base layer
    /// refuses a marked document MCP descriptor in a member's build
    /// ([`crate::document_mcp_marker`]): no document binding resolved it.
    ///
    /// The refusal sits in the spec's outermost layer, which runs before any
    /// pre-build hook inside the supplied service. A supplied service whose
    /// own hook would resolve a marked descriptor never gets to: the build is
    /// refused first.
    pub fn new(
        definition: MobDefinition,
        storage: MobStorage,
        session_service: Arc<dyn MobSessionService>,
    ) -> Self {
        Self::with_base_layer(
            definition,
            storage,
            session_service,
            refusing_unresolved_document_markers(None),
        )
    }

    /// [`Self::new`] with the base layer's pre-build hook. The stock
    /// constructors pass a no-op: their inner wrapper already refuses an
    /// unresolved marker after their own hook, which this outer layer runs
    /// before.
    fn with_base_layer(
        definition: MobDefinition,
        storage: MobStorage,
        session_service: Arc<dyn MobSessionService>,
        hook: PreBuildHook,
    ) -> Self {
        // Every spec construction path funnels through here (the stock
        // constructors call it with their wrapped service), so this
        // is the ONE layer that carries the dispatch-time taint slot: each
        // member create passes it exactly once, and later `with_*` re-wraps
        // never double-decorate.
        let dispatch_taint_slot = crate::memory::dispatch_taint::DispatchTaintSlot::default();
        let session_service = Arc::new(PreBuildMobSessionService {
            inner: session_service,
            hook,
            dispatch_taint: Some(dispatch_taint_slot.clone()),
            after_create_hook: None,
            runtime_adapter_override: None,
            archived_terminal_authority: None,
            member_sessions_in_memory: false,
        }) as Arc<dyn MobSessionService>;
        Self {
            definition,
            storage,
            // Unspecified, not DeclaredEphemeral: this constructor accepts
            // arbitrary storage and cannot know what it was handed. Claiming
            // ephemerality on the caller's behalf is what would let an
            // external embedder's durable storage resume unverified.
            composition_authority: crate::mob_composition_manifest::CompositionAuthority::default(),
            candidate_definition: crate::mob_composition_manifest::CandidateDefinition::default(),
            launch_definition_read: None,
            mob_storage_provenance: crate::mob_composition_manifest::MobStorageProvenance::default(
            ),
            session_service,
            member_model_config: None,
            binary_blob_store: None,
            agent_mob_mcp_state: None,
            runtime_delivery: None,
            implicit_delegate_retirement_overrides: None,
            agent_mob_default_llm_client_slot: None,
            console_spawn_sink_slot: None,
            identity_runtime_slot: None,
            options: MobBootstrapOptions {
                allow_ephemeral_sessions: true,
                notify_orchestrator_on_resume: true,
                default_llm_client: None,
            },
            runtime_adapter: None,
            live_compose_inputs: None,
            spawn_member_customizer: None,
            member_runtime_registration_hook: None,
            restored_members_awaiting_tools: Vec::new(),
            tool_consequence_policy_registry: None,
            default_external_tools_provider: None,
            tool_bundles: BTreeMap::new(),
            child_tool_bundle_availability: BTreeMap::new(),
            child_application_tool_policy: None,
            agent_mob_tools_install: None,
            child_mob_storage: None,
            meta_storage_layout: None,
            child_mob_root_claim: None,
            #[cfg(not(target_arch = "wasm32"))]
            member_mcp_custody: None,
            #[cfg(feature = "extension-state")]
            before_activation: None,
            #[cfg(feature = "extension-state")]
            after_destroy: None,
            #[cfg(feature = "extension-state")]
            before_create: None,
            #[cfg(feature = "extension-state")]
            composed_meerkat_realm: None,
            #[cfg(feature = "extension-state")]
            child_mcp_servers: None,
            workgraph_service: None,
            workgraph_admission_slots: Vec::new(),
            composition_release: CompositionRelease::default(),
            workgraph_admission_sidecar: None,
            workgraph_realm_migration: crate::workgraph_realm::WorkGraphRealmMigrationMode::default(
            ),
            resolved_storage: None,
            durable_runtime_store: None,
            durable_session_store: None,
            committed_boundary_recoverer: None,
            controller_pin_source: None,
            dispatch_taint_slot,
            fork_instruction_policy: meerkat_core::ForkInstructionPolicy::default(),
            #[cfg(test)]
            member_provider_auth_persistence: None,
            _ephemeral_dir: None,
            state_dir_claims: Vec::new(),
        }
    }

    /// The late-bound §10.1 dispatch-time taint slot every member session
    /// create built from this spec consults (see
    /// `crate::memory::dispatch_taint`). Compositions that assemble the full
    /// agent-memory stack fill the returned slot with the stack's
    /// [`crate::SessionTaintTracker`]; unfilled it costs nothing.
    pub fn dispatch_taint_slot(&self) -> crate::memory::dispatch_taint::DispatchTaintSlot {
        self.dispatch_taint_slot.clone()
    }

    /// Bind the session builder's own default agent-LLM-client decorator
    /// cell (`FactoryAgentBuilder::default_agent_llm_client_decorator`, the
    /// one its factory reads) for a session service composed outside this
    /// spec. Member creates then run that default once, then taint, when
    /// they carry no decorator of their own; without the binding the member
    /// taint decorator would take the request's empty slot and the factory
    /// would skip its default.
    #[must_use]
    pub fn with_host_default_agent_llm_client_decorator(
        self,
        source: crate::memory::dispatch_taint::HostDefaultDecoratorSource,
    ) -> Self {
        self.dispatch_taint_slot.bind_host_default_decorator(source);
        self
    }

    /// Record the composition-time storage durability resolution for a spec
    /// whose stores were composed externally (see
    /// [`resolved_storage`](Self::resolved_storage)).
    #[must_use]
    pub fn with_resolved_storage(mut self, summary: ResolvedStorageSummary) -> Self {
        self.resolved_storage = Some(summary);
        self
    }

    pub fn with_options(mut self, options: MobBootstrapOptions) -> Self {
        self.options = options;
        self
    }

    /// The custody this runtime's member factories compose their MCP routers
    /// under ([`MemberMcpCustody::install`](crate::MemberMcpCustody::install)).
    /// The runtime's shutdown closes every router in it, and waits for each
    /// close's native outcome, before it reports cleanup.
    #[cfg(not(target_arch = "wasm32"))]
    #[must_use]
    pub fn with_member_mcp_custody(
        mut self,
        custody: Arc<crate::member_mcp_custody::MemberMcpCustody>,
    ) -> Self {
        self.member_mcp_custody = Some(custody);
        self
    }

    /// Record the host config `session_service`'s member builder was given,
    /// so the image checks judge a self-hosted or config-declared model by its
    /// `vision` flag, as Core's model registry does. Without it they judge
    /// with the static model catalog alone, which knows neither kind. The
    /// stock constructors record it themselves.
    #[must_use]
    pub fn with_member_model_config(mut self, config: Arc<meerkat::Config>) -> Self {
        self.member_model_config = Some(config);
        self
    }

    /// Supply this host's explicit runtime delivery composition. The inbox and
    /// job store must be the same instances used by shell/callback producers.
    /// Call before installing agent mob tools. No store is inferred for a
    /// custom session service.
    #[must_use]
    pub fn with_runtime_delivery(mut self, delivery: MobRuntimeDelivery) -> Self {
        self.runtime_delivery = Some(delivery);
        self
    }

    /// Compose the host's own committed-result owner, for exactly its
    /// declared `Host` namespaces, into the job source of this spec's
    /// SELECTED runtime delivery (see
    /// [`MobRuntimeDelivery::with_host_job_source`]). An agent mob-tool state
    /// the constructor already bound is rebuilt and rebound with the composed
    /// source at bootstrap, before any restored input runs.
    ///
    /// # Errors
    /// [`MobRuntimeDeliveryError::MissingComposition`] when no runtime
    /// delivery is selected: a host never builds a replacement delivery here.
    pub fn with_host_job_source(
        mut self,
        host: meerkat_mob_mcp::HostJobSource,
    ) -> Result<Self, MobRuntimeDeliveryError> {
        let delivery = self
            .runtime_delivery
            .take()
            .ok_or(MobRuntimeDeliveryError::MissingComposition)?;
        self.runtime_delivery = Some(delivery.with_host_job_source(host.clone()));
        if let Some(install) = self.agent_mob_tools_install.as_mut() {
            install.delivery = install.delivery.clone().with_host_job_source(host);
            // The installed state bound its job source without the host's:
            // bootstrap must rebuild and rebind it.
            install.host_job_source_changed = true;
        }
        Ok(self)
    }

    /// Install the agent-facing mob tool surface (spawn/delegate + the
    /// schedule mob-target authority) for externally-constructed specs.
    ///
    /// The stock `persistent()`/ephemeral constructors do this internally;
    /// specs built via `MobBootstrapSpec::new` (both gateway binaries roll
    /// their own session services) previously skipped it, which left
    /// `agent_mob_mcp_state()` None — members still got mob tools through
    /// meerkat-mob's INTERNAL default state, but meta's schedule host had
    /// no mob authority: `spawn_schedule_host` fell back to the Noop mob
    /// host, so agent-authored schedules could neither rewrite to mob-member
    /// targets at authoring nor deliver identity/mob targets at fire time
    /// ("scheduled identity targets are not supported by this session host",
    /// the downstream app's 0.7.26 last-link failure).
    ///
    /// Call AFTER any session-service wrapping (`with_session_runtime_adapter`)
    /// so the installed tools hold the final wrapped service, and AFTER
    /// [`with_workgraph_service`](Self::with_workgraph_service) so child mobs
    /// inherit the workgraph authority. `mob_tools_slot` is the agent factory
    /// builder's `default_mob_tools` slot.
    ///
    /// # Errors
    ///
    /// The agent mob tools' runtime adapter could not be acquired from the
    /// session service.
    pub fn with_agent_mob_tools(
        mut self,
        mob_tools_slot: Arc<
            std::sync::RwLock<Option<Arc<dyn meerkat_core::service::MobToolsFactory>>>,
        >,
    ) -> Result<Self, MobRuntimeDeliveryError> {
        let acquired = self
            .session_service
            .acquire_runtime_adapter(self.runtime_adapter.clone())?;
        self.runtime_adapter = acquired;
        let delivery = self
            .runtime_delivery
            .clone()
            .ok_or(MobRuntimeDeliveryError::MissingComposition)?;
        delivery.validate_runtime(self.runtime_adapter.as_deref())?;
        let (
            agent_mob_mcp_state,
            implicit_delegate_retirement_overrides,
            agent_mob_default_llm_client_slot,
            console_spawn_sink_slot,
            identity_runtime_slot,
            agent_mob_tools_install,
        ) = install_agent_mob_tools(
            &self.definition,
            mob_tools_slot,
            Arc::clone(&self.session_service),
            self.runtime_adapter.clone(),
            self.workgraph_service.clone(),
            None,
            // No `store_path` in this builder path, so no durable council
            // store: councils stay process-bound, exactly as before.
            None,
            self.storage.forked_participant_store().cloned(),
            delivery,
        )?;
        self.agent_mob_mcp_state = Some(agent_mob_mcp_state);
        self.implicit_delegate_retirement_overrides = Some(implicit_delegate_retirement_overrides);
        self.agent_mob_default_llm_client_slot = Some(agent_mob_default_llm_client_slot);
        self.console_spawn_sink_slot = Some(console_spawn_sink_slot);
        self.identity_runtime_slot = Some(identity_runtime_slot);
        self.composition_release
            .register_slot(&agent_mob_tools_install.slot);
        self.agent_mob_tools_install = Some(agent_mob_tools_install);
        Ok(self)
    }

    /// Thread a realm-scoped WorkGraph service into the mob runtime.
    ///
    /// `MobRuntime::bootstrap` forwards it to
    /// `MobBuilder::with_workgraph_service`, which turns on apply-time
    /// attention overlay injection for every mob-executor turn. Call BEFORE
    /// [`with_agent_mob_tools`](Self::with_agent_mob_tools) — the agent mob
    /// state snapshots the service at install time so agent-spawned child
    /// mobs inherit it.
    #[must_use]
    pub fn with_workgraph_service(mut self, service: Option<meerkat::WorkGraphService>) -> Self {
        self.workgraph_service = service;
        self
    }

    /// The WorkGraph service this spec selected, read-only: a clone of the
    /// same service (and so the same store) the runtime will use, the
    /// pre-start counterpart of `UnifiedRuntime::workgraph_service`. A host's
    /// `prepare_bootstrap` reads it before restored inputs run; the store's
    /// durability declaration is already public through `resolved_storage`.
    /// `None` without a WorkGraph service.
    #[must_use]
    pub fn workgraph_service(&self) -> Option<meerkat::WorkGraphService> {
        self.workgraph_service.clone()
    }

    /// Choose where agent-created child mobs (agent `mob_create`, delegate's
    /// implicit mob) keep their event logs.
    ///
    /// The selection is recorded here and applied when the runtime
    /// bootstraps, before the agent mob-tool state is shared, so it may be
    /// set before or after [`with_agent_mob_tools`](Self::with_agent_mob_tools).
    /// With [`ChildMobStorage::Durable`](crate::ChildMobStorage::Durable),
    /// bootstrap claims the layout's child root exclusively, restores every
    /// persisted child before the runtime reports ready, and fails with
    /// [`MobRuntimeError::ChildMobStorage`] rather than falling back to
    /// memory. [`ChildMobStorage::ProcessBound`](crate::ChildMobStorage::ProcessBound)
    /// never opens the child root.
    ///
    /// Without a selection, child mobs are durable under the layout recorded
    /// with [`with_meta_storage_layout`](Self::with_meta_storage_layout) when
    /// the final root mob storage is declared persistent (bootstrap fails
    /// with [`ChildMobStorageError::LayoutNotRecorded`](crate::ChildMobStorageError::LayoutNotRecorded)
    /// when none was recorded), and process-bound otherwise. An explicit
    /// selection always wins.
    #[must_use]
    pub fn with_child_mob_storage(mut self, storage: crate::ChildMobStorage) -> Self {
        self.child_mob_storage = Some(storage);
        self
    }

    /// Record the Meta storage layout this runtime is composed over.
    ///
    /// With no [`with_child_mob_storage`](Self::with_child_mob_storage)
    /// selection, agent-created child mobs then default to
    /// [`ChildMobStorage::Durable`](crate::ChildMobStorage::Durable) under
    /// this layout whenever the final root mob storage is declared persistent
    /// (see [`with_mob_storage_provenance`](Self::with_mob_storage_provenance))
    /// and the layout is not declared ephemeral. The stock persistent
    /// constructors record their layout themselves.
    #[must_use]
    pub fn with_meta_storage_layout(mut self, layout: crate::MetaStorageLayout) -> Self {
        self.meta_storage_layout = Some(layout);
        self
    }

    /// The child-mob storage this spec bootstraps with: the explicit
    /// selection; else, over a root storage declared persistent, durable
    /// under the recorded launch layout, refused when none was recorded;
    /// else process-bound. A recorded layout that is declared ephemeral is
    /// refused where the durable selection is prepared, never replaced.
    fn resolved_child_mob_storage(
        &self,
    ) -> Result<crate::child_mob_storage::ChildMobStorage, MobRuntimeError> {
        use crate::child_mob_storage::{ChildMobStorage, ChildMobStorageError};
        if let Some(selected) = &self.child_mob_storage {
            return Ok(selected.clone());
        }
        if !self.mob_storage_provenance.is_persistent() {
            return Ok(ChildMobStorage::ProcessBound);
        }
        self.meta_storage_layout
            .clone()
            .map(ChildMobStorage::Durable)
            .ok_or(MobRuntimeError::ChildMobStorage(
                ChildMobStorageError::LayoutNotRecorded,
            ))
    }

    /// Choose whether bootstrap migrates member-bound WorkGraph attention
    /// bindings that predate meerkat's mob-realm rescoping into the realm
    /// their owner key names (default: apply). The report lands on
    /// [`MobRuntime::workgraph_realm_migration`] and `meta/capabilities`.
    #[must_use]
    pub fn with_workgraph_realm_migration(
        mut self,
        mode: crate::workgraph_realm::WorkGraphRealmMigrationMode,
    ) -> Self {
        self.workgraph_realm_migration = mode;
        self
    }

    /// Register a tool-plane admission slot (returned by
    /// `workgraph_wiring::install_workgraph_tools` /
    /// `attach_workgraph_tools*`) to be filled at bootstrap with the
    /// runtime-wide [`WorkGraphAdmission`](crate::workgraph_admission::WorkGraphAdmission).
    /// Every builder whose members can call `workgraph_attention_reassign`
    /// must have its slot registered here, or those members bypass the
    /// duplicate-binding admission guard the RPC surfaces enforce.
    #[must_use]
    pub fn with_workgraph_admission_slot(
        mut self,
        slot: crate::workgraph_admission::WorkGraphAdmissionSlot,
    ) -> Self {
        self.workgraph_admission_slots.push(slot);
        self
    }

    /// Serialize admissions cross-process through the sidecar lock database
    /// under `state_dir` (see
    /// [`workgraph_admission_sidecar_path`](crate::workgraph_admission::workgraph_admission_sidecar_path)).
    /// Call for SQLite-backed workgraph stores — the store file is shareable
    /// by a gateway and a library-mode runtime on one state dir, and each
    /// process's in-process gate cannot see the other. Memory-backed
    /// runtimes must not set this.
    #[must_use]
    pub fn with_workgraph_admission_sidecar(mut self, state_dir: &Path) -> Self {
        self.workgraph_admission_sidecar =
            Some(crate::workgraph_admission::workgraph_admission_sidecar_path(state_dir));
        self
    }

    /// Install a mob-wide external-tools provider (e.g. MCP-backed callback
    /// tools). Unlike the per-spawn `SpawnMemberSpec.external_tools` overlay —
    /// which member revival silently drops — this provider is consulted on
    /// every spawn AND every revival, so the tools are durable for the
    /// member's whole lifecycle. The profile's `tools.mcp` allowlist gates
    /// what each member sees; an empty allowlist means the full surface.
    pub fn with_default_external_tools_provider(
        mut self,
        provider: meerkat_mob::ExternalToolsProvider,
    ) -> Self {
        self.default_external_tools_provider = Some(provider);
        self
    }

    /// Register a named Rust tool bundle. A profile whose `tools.rust_bundles`
    /// names it gets the dispatcher's tools on every spawn, resume, revival
    /// and respawn; meerkat-mob owns that wiring. A profile naming a bundle
    /// that is not registered is refused when its member is built. The bundle
    /// is host-only: agent-created child mobs never receive it (see
    /// [`Self::register_tool_bundle_with_availability`]).
    ///
    /// Registering the same name twice replaces the earlier dispatcher.
    #[must_use]
    pub fn register_tool_bundle(
        self,
        name: impl Into<String>,
        dispatcher: Arc<dyn meerkat_core::AgentToolDispatcher>,
    ) -> Self {
        self.register_tool_bundle_with_availability(
            name,
            dispatcher,
            meerkat_mob_mcp::ChildToolBundleAvailability::HostOnly,
        )
    }

    /// Register a named Rust tool bundle with its availability to child mobs
    /// (agent `mob_create` and delegate's implicit mob). A
    /// [`meerkat_mob_mcp::ChildToolBundleAvailability::ChildAvailable`] bundle
    /// is supplied by the host to every inline profile of a child mob when the
    /// mob is created; agents never name bundles themselves, and can narrow
    /// what a child member may call with its profile's deny list.
    ///
    /// Registering the same name twice replaces the earlier registration.
    #[must_use]
    pub fn register_tool_bundle_with_availability(
        mut self,
        name: impl Into<String>,
        dispatcher: Arc<dyn meerkat_core::AgentToolDispatcher>,
        availability: meerkat_mob_mcp::ChildToolBundleAvailability,
    ) -> Self {
        let name = name.into();
        self.tool_bundles.insert(name.clone(), dispatcher);
        self.child_tool_bundle_availability
            .insert(name, availability);
        self
    }

    /// The application tool policy every child mob member (agent
    /// `mob_create`, delegate's implicit mob) is built with. An explicit
    /// `ApplicationToolPolicyBinding::Unmanaged` is a valid opt-out. With a
    /// consequence-policy registry installed and no child policy, agent
    /// `mob_create` and delegate are refused with a typed tool error
    /// (`child_tool_policy_required`) until one is set.
    #[must_use]
    pub fn with_child_application_tool_policy(
        mut self,
        binding: meerkat_core::ApplicationToolPolicyBinding,
    ) -> Self {
        self.child_application_tool_policy = Some(binding);
        self
    }

    /// [`Self::with_child_application_tool_policy`] when one was configured.
    #[must_use]
    pub fn with_optional_child_application_tool_policy(
        mut self,
        binding: Option<meerkat_core::ApplicationToolPolicyBinding>,
    ) -> Self {
        if binding.is_some() {
            self.child_application_tool_policy = binding;
        }
        self
    }

    /// The child policy the agent mob-tool state forwards.
    fn agent_mob_child_policy(&self) -> AgentMobChildPolicy {
        let mut bundles = meerkat_mob_mcp::ChildToolBundles::new();
        let mut has_child_bundles = false;
        for (name, dispatcher) in &self.tool_bundles {
            let availability = self
                .child_tool_bundle_availability
                .get(name)
                .copied()
                .unwrap_or_default();
            // An offered bundle is forwarded like an available one: child
            // mob creation checks every opt-in against the offer.
            has_child_bundles |= matches!(
                availability,
                meerkat_mob_mcp::ChildToolBundleAvailability::ChildAvailable
                    | meerkat_mob_mcp::ChildToolBundleAvailability::ChildOffered
            );
            bundles = bundles.register(name.clone(), Arc::clone(dispatcher), availability);
        }
        AgentMobChildPolicy {
            registry: self.tool_consequence_policy_registry.clone(),
            binding: self.child_application_tool_policy.clone(),
            bundles,
            has_child_bundles,
            registration_hook: self.member_runtime_registration_hook.clone(),
        }
    }

    /// Reinstall the agent mob tools with the final child policy. The
    /// constructors install them before a host can set the registry, the
    /// child policy or a child-available bundle, so bootstrap applies those
    /// here, once, before anything uses the state. Nothing has run yet, the
    /// shared state is side-effect free, and the slots already handed out
    /// are kept; the session service is the one the tools were first
    /// installed with. The participant store follows the final spec.storage,
    /// and the child-mob storage follows the final
    /// [`Self::with_child_mob_storage`] selection, including when either is
    /// the only changed input.
    fn apply_agent_mob_child_policy(&mut self) -> Result<(), MobRuntimeError> {
        let child_policy = self.agent_mob_child_policy();
        let Some(install) = self.agent_mob_tools_install.clone() else {
            return Ok(());
        };
        let forked_participant_store = self.storage.forked_participant_store().cloned();
        // Public spec.storage can be replaced after the tools were installed.
        // Handle identity only decides whether to rebuild the composition;
        // capability access and authorization remain in the native store.
        let same_store = match (
            install.forked_participant_store.as_ref(),
            forked_participant_store.as_ref(),
        ) {
            (Some(installed), Some(selected)) => Arc::ptr_eq(installed, selected),
            (None, None) => true,
            _ => false,
        };
        // The default resolves here, against the final root storage, and is
        // then fixed for the rest of this bootstrap.
        let child_mob_storage = self.resolved_child_mob_storage()?;
        self.child_mob_storage = Some(child_mob_storage.clone());
        let child_mob_root = match &child_mob_storage {
            crate::child_mob_storage::ChildMobStorage::ProcessBound => None,
            crate::child_mob_storage::ChildMobStorage::Durable(layout) => {
                Some(layout.state_dir().to_path_buf())
            }
        };
        let same_child_root = install.child_mob_root == child_mob_root;
        if child_policy.is_empty()
            && same_store
            && same_child_root
            && !install.host_job_source_changed
        {
            return Ok(());
        }
        // Validate, claim and open everything the durable selection needs
        // before the replacement state is built or bound.
        let durable =
            self.prepare_durable_child_mobs(&install, forked_participant_store.is_some())?;
        let existing_slots = match (&self.console_spawn_sink_slot, &self.identity_runtime_slot) {
            (Some(sink), Some(identity)) => Some((Arc::clone(sink), Arc::clone(identity))),
            _ => None,
        };
        let replaced = self
            .agent_mob_mcp_state
            .as_ref()
            .and_then(|state| state.continuation_binding_generation())
            .ok_or(MobRuntimeError::Delivery(
                MobRuntimeDeliveryError::MissingBinding,
            ))?;
        let council_store = match &durable {
            Some(durable) => Some(Arc::clone(&durable.council_store)),
            None => install.council_store.clone(),
        };
        let mut state = new_agent_mob_state(
            install.session_service,
            install.runtime_adapter,
            install.workgraph_service,
            council_store,
            forked_participant_store.clone(),
        )
        .map_err(MobRuntimeError::Delivery)?;
        if let Some(durable) = &durable {
            // After the explicit stores and before the realm-profile overlay:
            // the native root upgrades only the stores still at their default.
            state = state
                .try_with_persistent_storage_root(Some(durable.runtime_root.clone()))
                .map_err(|error| {
                    MobRuntimeError::ChildMobStorage(
                        crate::child_mob_storage::ChildMobStorageError::Native(error),
                    )
                })?;
        }
        let (state, overrides, _llm_slot, console_spawn_sink, identity_runtime) =
            install_agent_mob_tools_with(
                &self.definition,
                install.slot,
                state,
                Some(install.default_llm_client_slot),
                child_policy,
                existing_slots,
                &install.delivery,
                Some(replaced),
            )
            .map_err(MobRuntimeError::Delivery)?;
        self.agent_mob_mcp_state = Some(state);
        self.implicit_delegate_retirement_overrides = Some(overrides);
        self.console_spawn_sink_slot = Some(console_spawn_sink);
        self.identity_runtime_slot = Some(identity_runtime);
        if let Some(install) = self.agent_mob_tools_install.as_mut() {
            install.forked_participant_store = forked_participant_store;
            install.child_mob_root = child_mob_root;
            install.host_job_source_changed = false;
        }
        if durable.is_none() {
            self.child_mob_root_claim = None;
        }
        Ok(())
    }

    /// Resolve the durable child-mob selection: refuse a composition that
    /// cannot keep its promise, claim the child root, and open the canonical
    /// council store. `None` while child mobs are process-bound.
    fn prepare_durable_child_mobs(
        &mut self,
        install: &AgentMobToolsInstall,
        root_custody: bool,
    ) -> Result<Option<crate::child_mob_storage::DurableChildMobs>, MobRuntimeError> {
        use crate::child_mob_storage::{
            ChildMobRootClaim, ChildMobStorage, ChildMobStorageError, DurableChildMobs,
            open_required_council_store,
        };
        let Some(ChildMobStorage::Durable(layout)) = &self.child_mob_storage else {
            return Ok(None);
        };
        // A host store realm keeps no authority in a local directory: its
        // child mobs stay process-bound.
        if self.mob_storage_provenance.is_host_store() {
            return Err(MobRuntimeError::ChildMobStorage(
                ChildMobStorageError::HostStoreRealm,
            ));
        }
        let layout = layout.clone();
        if layout.is_declared_ephemeral() {
            return Err(MobRuntimeError::ChildMobStorage(
                ChildMobStorageError::DeclaredEphemeralLayout {
                    state_dir: layout.state_dir().to_path_buf(),
                },
            ));
        }
        // The root storage's participant custody is installed explicitly, so
        // the native root never upgrades it. It must already be durable, or a
        // restored child would advertise fork recovery its custody cannot
        // honour.
        if root_custody && !self.mob_storage_provenance.is_persistent() {
            return Err(MobRuntimeError::ChildMobStorage(
                ChildMobStorageError::ProcessBoundParticipantCustody,
            ));
        }
        let root = layout.child_mob_root();
        if self
            .child_mob_root_claim
            .as_ref()
            .is_none_or(|claim| claim.root() != root)
        {
            let claim =
                ChildMobRootClaim::acquire(&root).map_err(MobRuntimeError::ChildMobStorage)?;
            self.child_mob_root_claim = Some(Arc::new(claim));
        }
        // Under the claim and before Meerkat opens the root: the
        // predecessor's empty, unversioned realm profile store becomes a
        // database Meerkat initializes as new; anything else is untouched.
        crate::child_mob_storage::recognize_legacy_empty_realm_profiles(&root)
            .map_err(MobRuntimeError::ChildMobStorage)?;
        let council_store = match &install.council_store {
            Some(store) => Arc::clone(store),
            None => {
                open_required_council_store(&layout).map_err(MobRuntimeError::ChildMobStorage)?
            }
        };
        Ok(Some(DurableChildMobs {
            runtime_root: layout.state_dir().to_path_buf(),
            council_store,
        }))
    }

    /// Add a [`meerkat_mob::SpawnMemberCustomizer`]. meerkat-mob has a single
    /// slot, so this composes with one already installed (which runs first)
    /// instead of replacing it.
    /// Install the host's native member registration hook. The mob selects
    /// each member's destination through it and installs that destination on
    /// the member's exact runtime registration before the member's actor
    /// exists; attached recovery verifies it. Applies to root and child-mob
    /// members alike, whatever order this and `with_agent_mob_tools` run in.
    /// Without a hook, members register exactly as before.
    #[must_use]
    pub fn with_member_runtime_registration_hook(
        mut self,
        hook: Arc<dyn meerkat_mob::MemberRuntimeRegistrationHook>,
    ) -> Self {
        self.member_runtime_registration_hook = Some(hook);
        self
    }

    /// Hold the run starts of each member meerkat's own resume revives while
    /// this mob is built (a log left Running) for an identity whose
    /// customizer tools are `pending` then
    /// ([`meerkat_mob::HostRunStartHoldReason::ToolsNotPublished`]): it
    /// starts no run (a restored queued input, a kickoff, a peer message)
    /// until the identity runtime publishes its tools and releases the hold.
    /// The same `pending` set must be recorded on the identity runtime
    /// (`IdentityRuntime::record_customizer_tools_pending`), which releases
    /// a hold only for an identity it holds pending.
    #[must_use]
    pub(crate) fn with_restored_members_awaiting_tools(
        mut self,
        pending: &std::collections::BTreeMap<
            crate::identity_first::AgentIdentity,
            crate::identity_first::CustomizerToolsPending,
        >,
    ) -> Self {
        self.restored_members_awaiting_tools = pending
            .keys()
            .map(|identity| crate::member_comms_id::mob_member_id(identity.as_str()))
            .collect();
        self
    }

    pub fn with_spawn_member_customizer(
        mut self,
        customizer: Arc<dyn meerkat_mob::SpawnMemberCustomizer>,
    ) -> Self {
        self.spawn_member_customizer =
            Some(crate::identity_first::ComposedSpawnMemberCustomizer::over(
                self.spawn_member_customizer.take(),
                customizer,
            ));
        self
    }

    /// Install the process-local application consequence-policy registry.
    pub fn with_tool_consequence_policy_registry(
        mut self,
        registry: Arc<meerkat_core::ToolConsequencePolicyRegistry>,
    ) -> Self {
        self.tool_consequence_policy_registry = Some(registry);
        self
    }

    /// Declare that `storage` lives only for this process.
    ///
    /// The named form exists so in-process callers can say so explicitly
    /// instead of relying on a default claim. It permits resuming a pre-seeded
    /// in-memory storage, which carries no cross-restart composition risk
    /// because the storage was built in this same process.
    pub fn with_declared_ephemeral_mob_storage(mut self) -> Self {
        self.mob_storage_provenance =
            crate::mob_composition_manifest::MobStorageProvenance::declared_ephemeral();
        self
    }

    /// Carry the provenance returned alongside a composed mob storage.
    ///
    /// Takes the provenance rather than a bare path so the pair produced by
    /// [`crate::mob_composition_manifest::persistent_mob_storage`] threads
    /// through intact: a launch path cannot compose persistent mob storage and
    /// then forget to declare it.
    /// Declare that this launch does or does not speak for the durable
    /// composition. See [`crate::mob_composition_manifest::CompositionAuthority`].
    #[must_use]
    pub fn with_composition_authority(
        mut self,
        authority: crate::mob_composition_manifest::CompositionAuthority,
    ) -> Self {
        self.composition_authority = authority;
        self
    }

    /// See [`Self::candidate_definition`].
    pub fn with_candidate_definition(
        mut self,
        policy: crate::mob_composition_manifest::CandidateDefinition,
    ) -> Self {
        self.candidate_definition = policy;
        self
    }

    /// The definition this launch runs, judged before preparation: on a
    /// resume the one its event log holds, which preparation resumes whatever
    /// definition was supplied (the supplied one is only checked against it);
    /// on a fresh boot the supplied one, in the form preparation creates the
    /// mob with ([`auto_mark_declared_resume_overrides`]). Either way it is the
    /// definition the running mob holds, which later roster reads are handed.
    ///
    /// A resume is judged here exactly as preparation would judge it
    /// ([`judge_resume`]): an undeclared storage, a diverged composition and
    /// a refused candidate are refused now, so the definition returned has
    /// passed the composition checks before it reaches host code (the
    /// identity-first roster). Preparation applies the kept judgement and
    /// binds the resume to its snapshot, so what was resolved from the
    /// returned definition describes the mob that then runs: a definition
    /// epoch committed in between refuses the resume instead.
    pub(crate) async fn read_launch_definition(
        &mut self,
    ) -> Result<MobDefinition, MobRuntimeError> {
        let event_log_empty = self
            .storage
            .is_event_log_empty()
            .await
            .map_err(|err| MobRuntimeError::Mob(MobError::from(err)))?;
        if event_log_empty {
            self.launch_definition_read = Some(LaunchDefinitionRead::Fresh);
            let mut definition = self.definition.clone();
            auto_mark_declared_resume_overrides(&mut definition);
            return Ok(definition);
        }
        // The form preparation judges in (`auto_mark_declared_resume_overrides`
        // runs there before the resume).
        let mut normalized = self.definition.clone();
        auto_mark_declared_resume_overrides(&mut normalized);
        let judged = judge_resume(
            &self.storage,
            &self.mob_storage_provenance,
            self.composition_authority,
            self.candidate_definition,
            &normalized,
            &self.definition,
            true,
        )
        .await?;
        let definition = judged.snapshot.as_ref().map_or_else(
            || normalized.clone(),
            |snapshot| snapshot.definition().clone(),
        );
        self.launch_definition_read = Some(LaunchDefinitionRead::Resume(Box::new(judged)));
        Ok(definition)
    }

    /// The live members this spec's persistent mob store holds whose current
    /// incarnation the roster materialized for a durable identity
    /// ([`meerkat_mob::SpawnContinuityIntent::DurableIdentity`], with the
    /// roster's `agent_identity` label) that commands alone drive
    /// ([`meerkat_mob::MemberIdentityManagement::CommandDriven`]), read from
    /// the store's member binding history before the mob is built: the
    /// identity members a resume of a log left Running revives that the
    /// roster pass retires once the roster no longer lists them. A host-adopted
    /// identity (Core holds an identity intent row for it, and its reconciler
    /// drives it) or one whose intent row Core cannot read is left out: the
    /// roster pass never retires it. So is a durable member spawned at
    /// runtime (mob tools, delegate, a helper), which the roster never lists
    /// and never retires: its run resumes unheld. A persistent store is read
    /// by its path;
    /// a mob storage kept in a host store realm, which is open already, is
    /// read through its own identity member store, the same fold. Empty for
    /// a storage that does not outlive the process and for a store not
    /// created yet. A history the store cannot give fails, so a caller that
    /// holds these members fails closed.
    pub(crate) async fn stored_command_driven_identity_members(
        &self,
    ) -> Result<Vec<meerkat_mob::AgentIdentity>, MobRuntimeError> {
        let history = if let Some(path) = self.mob_storage_provenance.persistent_path() {
            match MobStorage::read_member_binding_history(path).await {
                Ok(history) => history,
                Err(meerkat_mob::MemberBindingHistoryReadError::StoreMissing { .. }) => {
                    return Ok(Vec::new());
                }
                Err(error) => {
                    return Err(MobRuntimeError::Mob(MobError::Internal(format!(
                        "reading the mob store's member bindings before the mob build: {error}"
                    ))));
                }
            }
        } else if self.mob_storage_provenance.is_host_store() {
            self.storage
                .member_binding_history()
                .await
                .map_err(|error| {
                    MobRuntimeError::Mob(MobError::Internal(format!(
                        "reading the host store mob storage's member bindings before the mob \
                         build: {error}"
                    )))
                })?
        } else {
            return Ok(Vec::new());
        };
        Ok(history
            .durable_identity_members()
            .filter(|binding| {
                binding.identity_management()
                    == meerkat_mob::MemberIdentityManagement::CommandDriven
                    && crate::member_comms_id::binding_materialized_by_roster(binding)
            })
            .map(|binding| binding.identity().clone())
            .collect())
    }

    /// The host's fork instruction policy for member forks of this mob:
    /// `fork_off`, delegation forks, council participants and host-driven
    /// forks ([`meerkat_mob::MobBuilder::with_fork_instruction_policy`]).
    /// Defaults to Meerkat's default,
    /// [`meerkat_core::ForkInstructionPolicy::RequireExplicitActivation`].
    #[must_use]
    pub fn with_fork_instruction_policy(
        mut self,
        policy: meerkat_core::ForkInstructionPolicy,
    ) -> Self {
        self.fork_instruction_policy = policy;
        self
    }

    pub fn with_mob_storage_provenance(
        mut self,
        provenance: crate::mob_composition_manifest::MobStorageProvenance,
    ) -> Self {
        self.mob_storage_provenance = provenance;
        self
    }

    /// Install the registry only when one was configured, so a caller that
    /// parsed no policies does not have to branch.
    pub fn with_optional_tool_consequence_policy_registry(
        mut self,
        registry: Option<Arc<meerkat_core::ToolConsequencePolicyRegistry>>,
    ) -> Self {
        if registry.is_some() {
            self.tool_consequence_policy_registry = registry;
        }
        self
    }

    /// Supply the persistent session owner's exact completion reader for
    /// operator maintenance. This is independent of the bootstrap adapter.
    /// Existing recovery behavior is preserved; an absent recoverer stays unsupported.
    pub fn with_runtime_completion_observer(
        mut self,
        observer: Arc<meerkat_runtime::MeerkatMachine>,
    ) -> Self {
        self.committed_boundary_recoverer = Some(Arc::new(
            crate::identity_first::bridge::CompletionObservedRecoverer {
                inner: self.committed_boundary_recoverer.take(),
                observer,
            },
        ));
        self
    }

    /// Expose a runtime adapter through the session-service facade.
    ///
    /// Custom embedders that construct their own `MobSessionService` still need
    /// Meta's session-service surface to report the same runtime authority
    /// that `MobBuilder::with_runtime_adapter(...)` receives. This keeps
    /// autonomous-host comms, runtime inspection, and control paths pointed at
    /// one machine without forcing embedders through the stock factory helpers.
    pub fn with_session_runtime_adapter(
        mut self,
        adapter: Arc<meerkat_runtime::MeerkatMachine>,
    ) -> Result<Self, meerkat_runtime::RuntimeDriverError> {
        let acquired = self
            .session_service
            .acquire_runtime_adapter(Some(adapter))?;
        self.session_service = Arc::new(PreBuildMobSessionService {
            inner: self.session_service,
            hook: no_op_pre_build_hook(),
            dispatch_taint: None,
            after_create_hook: None,
            runtime_adapter_override: acquired.clone(),
            archived_terminal_authority: None,
            member_sessions_in_memory: false,
        });
        self.runtime_adapter = acquired;
        Ok(self)
    }

    /// Wrap the session service with an after-create hook that fires after
    /// each successful `create_session`. The hook is best-effort: errors are
    /// not propagated. Uses `AfterCreateMobSessionService` which wraps the
    /// inner service without a pre-build hook, so any pre-build mutations
    /// from inner wrappers are fully reflected in the context.
    pub fn with_after_create_hook(mut self, hook: AfterCreateHook) -> Self {
        self.session_service = Arc::new(AfterCreateMobSessionService {
            inner: self.session_service,
            after_hook: hook,
        });
        self
    }

    /// Declare the composition's durable runtime store: hand it the SAME
    /// `Arc` given to `MeerkatMachine::persistent` and the session service.
    /// The mob runtime reads it for evidence only that store can give (a
    /// notice attempt that settled, a session's committed boundary
    /// authority).
    pub fn with_durable_runtime_store(
        mut self,
        store: &Arc<dyn meerkat_runtime::RuntimeStore>,
    ) -> Self {
        self.durable_runtime_store = Some(Arc::clone(store));
        self
    }

    /// Overlay the RuntimeStore-owned archived terminal onto resume-seam
    /// reads on an externally-composed spec (both gateway binaries roll
    /// their own session services, so the stock persistent constructor's
    /// wiring does not reach them).
    ///
    /// At meerkat 0.8.11 archive never rewrites session bodies; the
    /// absorbing terminal lives in the runtime store's catalog entry or its
    /// Retired/Destroyed lifecycle row. Without this overlay,
    /// `load_session_for_resume` on a runtime-archived session returns
    /// `Revivable` with no archived terminal and hosts rotate identities off
    /// intact preserved transcripts. Hand it the SAME store the machine and
    /// session service share.
    #[must_use]
    pub fn with_runtime_archived_terminal_authority(
        mut self,
        runtime_store: Arc<dyn meerkat_runtime::RuntimeStore>,
    ) -> Self {
        self.session_service = Arc::new(PreBuildMobSessionService {
            inner: self.session_service,
            hook: no_op_pre_build_hook(),
            dispatch_taint: None,
            after_create_hook: None,
            runtime_adapter_override: None,
            archived_terminal_authority: Some(runtime_store),
            member_sessions_in_memory: false,
        });
        self
    }

    /// Build an ephemeral session service with a correctly wired `AgentFactory`.
    ///
    /// If `session_store` is provided, it is set on the `FactoryAgentBuilder` so
    /// that agents use the given store instead of falling back to JSONL.
    ///
    /// It takes no host factory options (an MCP call-context provider, an
    /// operation refusal observer or a provider credential store). A host
    /// that needs them uses [`UnifiedRuntimeBuilder`](crate::UnifiedRuntimeBuilder),
    /// or builds a session service on its own factory and passes it to
    /// [`MobBootstrapSpec::new`].
    ///
    /// # Errors
    ///
    /// The runtime machine or the agent mob tools could not acquire their
    /// runtime.
    pub fn ephemeral(
        definition: MobDefinition,
        storage: MobStorage,
        store_path: PathBuf,
        max_sessions: usize,
        session_store: Option<Arc<dyn AgentSessionStore>>,
    ) -> Result<Self, StorageResolutionError> {
        Self::ephemeral_inner(
            definition,
            storage,
            store_path,
            max_sessions,
            session_store,
            None,
            CapabilityFlags::default(),
            None,
            None,
            None,
        )
    }

    /// Like [`ephemeral`](Self::ephemeral), but with a pre-build hook that is
    /// called before each agent is constructed. Use this to inject external
    /// tools, augment system prompts, or set per-agent labels.
    ///
    /// It takes no host factory options (an MCP call-context provider, an
    /// operation refusal observer or a provider credential store). A host
    /// that needs them uses [`UnifiedRuntimeBuilder`](crate::UnifiedRuntimeBuilder),
    /// or builds a session service on its own factory and passes it to
    /// [`MobBootstrapSpec::new`].
    ///
    /// # Errors
    ///
    /// The runtime machine or the agent mob tools could not acquire their
    /// runtime.
    pub fn ephemeral_with_hook(
        definition: MobDefinition,
        storage: MobStorage,
        store_path: PathBuf,
        max_sessions: usize,
        session_store: Option<Arc<dyn AgentSessionStore>>,
        hook: impl Fn(
            &mut CreateSessionRequest,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<(), SessionError>> + Send + '_>,
        > + Send
        + Sync
        + 'static,
    ) -> Result<Self, StorageResolutionError> {
        Self::ephemeral_inner(
            definition,
            storage,
            store_path,
            max_sessions,
            session_store,
            Some(Arc::new(hook)),
            CapabilityFlags::default(),
            None,
            None,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn ephemeral_inner(
        definition: MobDefinition,
        storage: MobStorage,
        store_path: PathBuf,
        max_sessions: usize,
        session_store: Option<Arc<dyn AgentSessionStore>>,
        hook: Option<PreBuildHook>,
        mut caps: CapabilityFlags,
        after_create_hook: Option<AfterCreateHook>,
        agent_config: Option<Config>,
        mcp_call_context_provider: Option<Arc<dyn meerkat_mcp::McpCallContextProvider>>,
    ) -> Result<Self, StorageResolutionError> {
        caps.image_generation |= mob_definition_may_use_image_generation(&definition);
        let binary_blob_store: Arc<dyn BinaryBlobStore> = Arc::new(ObjectStoreBlobStore::memory());
        let blob_store: Arc<dyn meerkat_core::BlobStore> =
            Arc::new(Base64BlobStoreAdapter::new(binary_blob_store.clone()));
        // Image tools use the builder's blob store; their machine authority
        // can remain ephemeral like the session service that adopts it.
        let runtime_adapter = caps
            .image_generation
            .then(|| Arc::new(meerkat_runtime::MeerkatMachine::ephemeral()));
        let mut factory = AgentFactory::new(&store_path)
            .builtins(caps.builtins)
            .shell(caps.shell)
            .mob(caps.mob)
            .comms(caps.comms)
            .memory(caps.memory)
            .mcp_apps(caps.mcp_apps);
        if let Some(provider) = mcp_call_context_provider {
            factory = factory.mcp_call_context_provider(provider);
        }
        if let Some(machine) = runtime_adapter.as_ref() {
            factory = factory.with_image_generation_machine(weak_image_generation_machine(machine));
        }
        let config = crate::compaction_policy::with_meta_compaction_defaults(
            agent_config.unwrap_or_default(),
        );
        let member_model_config = Arc::new(config.clone());
        let mut builder = FactoryAgentBuilder::new(factory, config);
        let host_default_decorator = Arc::clone(&builder.default_agent_llm_client_decorator);
        // Prepare each member's construction only after its configured MCP
        // servers have connected or failed, so the surface captured for it
        // carries them.
        builder.wait_for_mcp = true;
        builder.default_blob_store = Some(blob_store.clone());
        let job_store: Arc<dyn meerkat::DetachedJobStore> =
            Arc::new(meerkat::MemoryDetachedJobStore::new());
        builder.default_detached_job_store = Some(Arc::clone(&job_store));
        let delivery = MobRuntimeDelivery::declared_ephemeral(Arc::clone(&job_store));
        builder.default_shell_job_delivery_projector =
            Some(meerkat::JobOutboxProjector::new_for_realm(
                job_store,
                delivery.inbox(),
                crate::storage_provider::DEFAULT_MEERKAT_LEVEL_REALM_ID,
            ));
        if let Some(store) = session_store {
            builder.default_session_store = Some(store);
        }
        let (session_llm_reconfigure_blueprint, session_llm_default_client_slot) =
            session_llm_reconfigure_blueprint(&builder, &store_path);
        let mob_tools_slot = Arc::clone(&builder.default_mob_tools);
        // Ephemeral specs carry a memory-backed workgraph so profiles with
        // `tools.workgraph = true` build (the factory fails closed on an
        // enabled category with an empty dispatcher slot).
        let (workgraph_service, workgraph_admission_slot) =
            crate::workgraph_wiring::attach_workgraph_tools_ephemeral(
                &builder,
                definition.id.as_str(),
            );
        let concrete_session_service = Arc::new(meerkat_session::EphemeralSessionService::new(
            builder,
            max_sessions,
        ));
        let acquired = concrete_session_service
            .acquire_canonical_runtime_adapter(runtime_adapter)
            .map_err(StorageResolutionError::Runtime)?;
        session_llm_reconfigure_blueprint.install(&acquired, concrete_session_service.clone());
        let after_create_hook = if caps.image_generation {
            Some(register_runtime_after_create(
                acquired.clone(),
                after_create_hook,
            ))
        } else {
            after_create_hook
        };
        let effective_runtime_adapter = Some(acquired);
        // The erased MobSessionService carries no controller pin method, so
        // the concrete owner's pin operation is captured here.
        let controller_pin_source: Arc<dyn crate::identity_first::bridge::ControllerPinSource> =
            concrete_session_service.clone();
        let session_service: Arc<dyn MobSessionService> = concrete_session_service;
        let hook = refusing_unresolved_document_markers(hook);
        let session_service = Arc::new(PreBuildMobSessionService {
            inner: session_service,
            hook,
            dispatch_taint: None,
            after_create_hook,
            runtime_adapter_override: effective_runtime_adapter.clone(),
            archived_terminal_authority: None,
            member_sessions_in_memory: false,
        }) as Arc<dyn MobSessionService>;
        // Wrap first and install the agent mob tools on the spec's final
        // service: child mobs are built on the tools' session service, so
        // they get every layer member creates get (the gateway's
        // `with_agent_mob_tools` order).
        let mut spec =
            Self::with_base_layer(definition, storage, session_service, no_op_pre_build_hook());
        spec.member_model_config = Some(member_model_config);
        // The builder's own default decorator cell, so member taint composes
        // over the default this factory would apply.
        spec.dispatch_taint_slot
            .bind_host_default_decorator(host_default_decorator);
        spec.runtime_delivery = Some(delivery.clone());
        let (
            agent_mob_mcp_state,
            implicit_delegate_retirement_overrides,
            agent_mob_default_llm_client_slot,
            console_spawn_sink_slot,
            identity_runtime_slot,
            agent_mob_tools_install,
        ) = install_agent_mob_tools(
            &spec.definition,
            mob_tools_slot,
            Arc::clone(&spec.session_service),
            effective_runtime_adapter.clone(),
            Some(workgraph_service.clone()),
            Some(session_llm_default_client_slot),
            // Ephemeral mob, ephemeral councils. Writing a durable council
            // database from a mob that keeps nothing else would leave records
            // behind that no later boot of this mob can claim.
            None,
            spec.storage.forked_participant_store().cloned(),
            delivery,
        )
        .map_err(StorageResolutionError::Delivery)?;
        spec.agent_mob_mcp_state = Some(agent_mob_mcp_state);
        spec.implicit_delegate_retirement_overrides = Some(implicit_delegate_retirement_overrides);
        spec.agent_mob_default_llm_client_slot = Some(agent_mob_default_llm_client_slot);
        spec.console_spawn_sink_slot = Some(console_spawn_sink_slot);
        spec.identity_runtime_slot = Some(identity_runtime_slot);
        spec.composition_release
            .register_slot(&agent_mob_tools_install.slot);
        spec.agent_mob_tools_install = Some(agent_mob_tools_install);
        spec.runtime_adapter = effective_runtime_adapter;
        spec.controller_pin_source = Some(controller_pin_source);
        spec.binary_blob_store = Some(binary_blob_store);
        spec.workgraph_service = Some(workgraph_service);
        spec.workgraph_admission_slots
            .push(workgraph_admission_slot);
        // Ephemeral mode: in-memory blobs are the declared choice of the
        // mode itself; the ephemeral session service persists nothing, so
        // the incremental capability is not applicable.
        let mut slots = vec![
            StorageSlotSummary::declared_ephemeral(
                "sessions",
                "EphemeralSessionService",
                "declared by the ephemeral launch mode",
            ),
            blob_slot_summary(BlobDurability::DeclaredEphemeral),
            StorageSlotSummary::declared_ephemeral(
                "jobs",
                "MemoryDetachedJobStore",
                "declared by the ephemeral launch mode",
            ),
            StorageSlotSummary::declared_ephemeral(
                "runtime_delivery",
                "InMemoryRuntimeStore",
                "declared by the ephemeral launch mode",
            ),
        ];
        slots.extend(scratch_ring_buffer_slots());
        spec.resolved_storage = Some(
            ResolvedStorageSummary::new(BlobDurability::DeclaredEphemeral, None).with_slots(slots),
        );
        Ok(spec)
    }

    /// Build a persistent session service with a correctly wired `AgentFactory`.
    ///
    /// The session store and the runtime store are Core's HeadCanonical pair
    /// on `store_path/runtime.sqlite`. Before opening them the runtime
    /// refuses the state directory when another process hosts runtimes
    /// there, when its layout needs `meta_gateway storage-migrate-native`, or
    /// when identity bindings have lost their runtime store
    /// ([`crate::native_startup`]).
    ///
    /// `provenance` says what `storage` is: the pair
    /// [`crate::mob_composition_manifest::persistent_mob_storage`] returns
    /// for durable mob storage, or
    /// [`MobStorageProvenance::declared_ephemeral`](crate::mob_composition_manifest::MobStorageProvenance::declared_ephemeral)
    /// for in-memory mob storage.
    ///
    /// It takes no host factory options (an MCP call-context provider, an
    /// operation refusal observer or a provider credential store). A host
    /// that needs them uses [`UnifiedRuntimeBuilder`](crate::UnifiedRuntimeBuilder),
    /// or builds a session service on its own factory and passes it to
    /// [`MobBootstrapSpec::new`].
    ///
    /// # Errors
    ///
    /// Fails closed when `provenance` is unspecified
    /// ([`StorageLayoutError::MobStorageUndeclared`](crate::storage_layout::StorageLayoutError::MobStorageUndeclared)),
    /// when the state directory is refused, or when a store under
    /// `store_path` cannot be opened; persistent mode never silently falls
    /// back to in-memory stores.
    pub async fn persistent(
        definition: MobDefinition,
        storage: MobStorage,
        provenance: crate::mob_composition_manifest::MobStorageProvenance,
        store_path: PathBuf,
        max_sessions: usize,
    ) -> Result<Self, StorageResolutionError> {
        Self::persistent_inner(
            definition,
            storage,
            provenance,
            store_path,
            max_sessions,
            None,
            false,
            None,
            None,
            None,
            CapabilityFlags::default(),
            None,
            None,
        )
        .await
    }

    /// Like [`persistent`](Self::persistent), but with a pre-build hook that
    /// is called before each agent is constructed. Use this to inject external
    /// tools, augment system prompts, or set per-agent labels.
    ///
    /// It takes no host factory options (an MCP call-context provider, an
    /// operation refusal observer or a provider credential store). A host
    /// that needs them uses [`UnifiedRuntimeBuilder`](crate::UnifiedRuntimeBuilder),
    /// or builds a session service on its own factory and passes it to
    /// [`MobBootstrapSpec::new`].
    ///
    /// # Errors
    ///
    /// As [`persistent`](Self::persistent).
    pub async fn persistent_with_hook(
        definition: MobDefinition,
        storage: MobStorage,
        provenance: crate::mob_composition_manifest::MobStorageProvenance,
        store_path: PathBuf,
        max_sessions: usize,
        hook: impl Fn(
            &mut CreateSessionRequest,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<(), SessionError>> + Send + '_>,
        > + Send
        + Sync
        + 'static,
    ) -> Result<Self, StorageResolutionError> {
        Self::persistent_inner(
            definition,
            storage,
            provenance,
            store_path,
            max_sessions,
            None,
            false,
            None,
            None,
            Some(Arc::new(hook)),
            CapabilityFlags::default(),
            None,
            None,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn persistent_inner(
        definition: MobDefinition,
        storage: MobStorage,
        provenance: crate::mob_composition_manifest::MobStorageProvenance,
        store_path: PathBuf,
        max_sessions: usize,
        custom_blob_store: Option<BlobStoreInjection>,
        ephemeral_blobs: bool,
        schedule_store: Option<Arc<dyn meerkat::ScheduleStore>>,
        workgraph_store: Option<Arc<dyn meerkat::WorkGraphStore>>,
        hook: Option<PreBuildHook>,
        caps: CapabilityFlags,
        after_create_hook: Option<AfterCreateHook>,
        agent_config: Option<Config>,
    ) -> Result<Self, StorageResolutionError> {
        // The installation's meerkat-level realm, resolved once from the
        // layout of `store_path` (fresh default, or the adopted recorded
        // value; refuses typed otherwise).
        let meerkat_realm =
            crate::storage_layout::MetaStorageLayout::with_injected_roots(store_path.clone(), None)
                .meerkat_level_realm()
                .map_err(StorageResolutionError::Layout)?
                .clone();
        Self::persistent_inner_with_provider_stores(
            definition,
            storage,
            provenance,
            store_path,
            max_sessions,
            custom_blob_store,
            ephemeral_blobs,
            schedule_store,
            workgraph_store,
            hook,
            caps,
            after_create_hook,
            agent_config,
            None,
            meerkat_realm,
            None,
            None,
            None,
            None,
            PersistentAuthority::StateDir,
            None,
            None,
        )
        .await
    }

    /// [`persistent_inner`](Self::persistent_inner) with the composite
    /// storage provider's meerkat-level bundle (M4b): when present, the
    /// runtime and workgraph slots compose over the provider's stores
    /// instead of local SQLite files, so the advertised single bundle is
    /// not silently split across backends.
    ///
    /// `authority` says where the composition's authority lives. Over a
    /// state directory, `store_path` is that directory: it is claimed and
    /// checked before any store opens, and it holds the councils and the
    /// durable child mobs. Over a host store realm, the provider's bundle is
    /// the realm's and `store_path` is a per-host cache: nothing claims or
    /// checks it, councils and agent-created child mobs are process-bound,
    /// and no rkat repair scope names the runtime store.
    ///
    /// `session_identity_config_root` is the agent factory's user-global
    /// root when the host names one: the members' session comms identities
    /// then live under `<root>/.rkat/session_comms_identity`, and nowhere
    /// ambient.
    ///
    /// `provider_auth_persistence` is the host's provider credential store
    /// when it names one: the factory holds it in place of the process
    /// default before the member builder takes it, so every agent of the
    /// composition resolves and persists provider credentials there.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn persistent_inner_with_provider_stores(
        definition: MobDefinition,
        storage: MobStorage,
        provenance: crate::mob_composition_manifest::MobStorageProvenance,
        store_path: PathBuf,
        max_sessions: usize,
        custom_blob_store: Option<BlobStoreInjection>,
        ephemeral_blobs: bool,
        schedule_store: Option<Arc<dyn meerkat::ScheduleStore>>,
        workgraph_store: Option<Arc<dyn meerkat::WorkGraphStore>>,
        hook: Option<PreBuildHook>,
        mut caps: CapabilityFlags,
        after_create_hook: Option<AfterCreateHook>,
        agent_config: Option<Config>,
        provider_meerkat_stores: Option<crate::storage_provider::ProviderMeerkatStores>,
        meerkat_realm: meerkat_core::RealmId,
        grant_configuration: Option<meerkat_runtime::meerkat_machine::NativeGrantWorkConfiguration>,
        mcp_call_context_provider: Option<Arc<dyn meerkat_mcp::McpCallContextProvider>>,
        operation_refusal_observer: Option<Arc<dyn meerkat_core::OperationRefusalObserver>>,
        state_dir_claim: Option<Arc<crate::state_dir_claim::StateDirClaim>>,
        authority: PersistentAuthority,
        session_identity_config_root: Option<PathBuf>,
        provider_auth_persistence: Option<meerkat::ProviderAuthPersistence>,
    ) -> Result<Self, StorageResolutionError> {
        // The startup checks and the composition record need to know whether
        // the mob store outlives the process.
        if !provenance.is_declared() {
            return Err(StorageResolutionError::Layout(
                crate::storage_layout::StorageLayoutError::MobStorageUndeclared {
                    state_dir: store_path,
                },
            ));
        }
        let host_store = matches!(authority, PersistentAuthority::HostStore { .. });
        let state_dir_claim = if host_store {
            // The realm's fence is the exclusivity; the cache is no authority.
            None
        } else {
            // One Meta runtime per state directory, claimed before any of its
            // stores opens. A composition that already holds the claim (the
            // builder) hands it in; the public constructors claim here.
            let state_dir_claim = match state_dir_claim {
                Some(claim) => claim,
                None => Arc::new(
                    crate::state_dir_claim::StateDirClaim::acquire(&store_path)
                        .map_err(StorageResolutionError::Layout)?,
                ),
            };
            // Refuse, before any store opens: a process still hosting runtimes
            // on this directory, a layout that needs `storage-migrate-native`,
            // and identity bindings whose runtime store is gone. A provider's
            // meerkat-level bundle holds the runtime store outside this
            // directory, so its bindings have no `runtime.sqlite` here to
            // check.
            let runtime_store = if provider_meerkat_stores.is_some() {
                crate::native_startup::RuntimeStoreLocation::StorageProvider
            } else {
                crate::native_startup::RuntimeStoreLocation::StateDir
            };
            crate::native_startup::refuse_unready_state_dir_for(
                &store_path,
                provenance.persistent_path(),
                runtime_store,
            )
            .await
            .map_err(StorageResolutionError::Layout)?;
            Some(state_dir_claim)
        };
        caps.image_generation |= mob_definition_may_use_image_generation(&definition);
        // H1 fail-closed blob slot: the slot resolves to a configured
        // backend, an explicitly declared ephemeral choice, or a startup
        // error — never a silent in-memory fallback (the former warn +
        // `ObjectStoreBlobStore::memory()` arm here was the GKE
        // month-of-silent-data-loss hazard).
        let (binary_blob_store, blob_store, blob_durability): (
            Arc<dyn BinaryBlobStore>,
            Arc<dyn meerkat_core::BlobStore>,
            BlobDurability,
        ) = if let Some(injection) = custom_blob_store {
            let (binary_blob_store, blob_store) = injection.into_pair();
            let persistent = binary_blob_store.is_persistent();
            (
                binary_blob_store,
                blob_store,
                BlobDurability::Custom { persistent },
            )
        } else if ephemeral_blobs {
            let binary_blob_store: Arc<dyn BinaryBlobStore> =
                Arc::new(ObjectStoreBlobStore::memory());
            let blob_store: Arc<dyn meerkat_core::BlobStore> =
                Arc::new(Base64BlobStoreAdapter::new(binary_blob_store.clone()));
            (
                binary_blob_store,
                blob_store,
                BlobDurability::DeclaredEphemeral,
            )
        } else {
            let blob_path = store_path.join(crate::storage_layout::BLOB_ROOT_DIR_NAME);
            let binary_blob_store: Arc<dyn BinaryBlobStore> =
                match ObjectStoreBlobStore::local(blob_path.clone()) {
                    Ok(store) => Arc::new(store),
                    Err(err) => {
                        return Err(BlobStoreResolutionError::OpenFailed {
                            path: blob_path,
                            message: err.to_string(),
                        }
                        .into());
                    }
                };
            let blob_store: Arc<dyn meerkat_core::BlobStore> =
                Arc::new(Base64BlobStoreAdapter::new(binary_blob_store.clone()));
            (
                binary_blob_store,
                blob_store,
                BlobDurability::PersistentDisk,
            )
        };
        // Persistent mode with a resolved blob store that will not survive a
        // restart is only legal as an explicit declaration — this keeps
        // custom-injected stores honest too.
        if !binary_blob_store.is_persistent() && !ephemeral_blobs {
            return Err(BlobStoreResolutionError::NonPersistentUndeclared.into());
        }
        // Core's HeadCanonical pair on ONE database (`runtime.sqlite`): the
        // session store and the runtime store share it, as Core's own realm
        // composes them. A composite storage provider supplies its realm's
        // pair instead. Fail-closed (M4): an open failure is a startup error.
        let (
            session_store,
            session_store_kind,
            runtime_store,
            runtime_store_slot,
            runtime_store_locator,
        ): ResolvedSessionStores = if let Some(provider) = provider_meerkat_stores.as_ref() {
            (
                Arc::clone(&provider.session_store),
                PROVIDER_SESSION_STORE_KIND,
                Arc::clone(&provider.runtime_store),
                provider.runtime_slot_summary(),
                // A host store realm's runtime store is the host's own
                // database: no rkat repair scope opens it.
                if host_store {
                    None
                } else {
                    Some(crate::storage_health::RuntimeStoreLocator::MeerkatRealm {
                        state_root: store_path.clone(),
                        realm: provider.realm.as_str().to_string(),
                    })
                },
            )
        } else {
            let (session_store, runtime_store) =
                open_native_session_pair(&store_path, &meerkat_realm)?;
            (
                session_store,
                "SqliteSessionStore",
                runtime_store,
                StorageSlotSummary::persistent("runtime", "SqliteRuntimeStore"),
                Some(crate::storage_health::RuntimeStoreLocator::SqliteFile {
                    path: store_path.join(crate::storage_layout::RUNTIME_DB_FILE_NAME),
                }),
            )
        };
        // H2: duplicate the incremental-capability probe the session service
        // runs privately, so whole-blob degradation is loud and
        // health-visible instead of silent. A host store realm is WholeBlob
        // by design, so there it is reported, not warned about.
        let session_store_incremental = Some(if host_store {
            Arc::clone(&session_store).as_incremental().is_some()
        } else {
            probe_session_store_incremental(&session_store, session_store_kind)
        });
        let runtime_adapter = Arc::new(
            persistent_runtime_machine(
                Arc::clone(&runtime_store),
                Arc::clone(&blob_store),
                grant_configuration,
            )
            .map_err(StorageResolutionError::Runtime)?,
        );
        let mut factory = AgentFactory::new(&store_path)
            .builtins(caps.builtins)
            .shell(caps.shell)
            .mob(caps.mob)
            .comms(caps.comms)
            .memory(caps.memory)
            .mcp_apps(caps.mcp_apps);
        // The host's provider credential store replaces the process default
        // before the member builder takes the factory, so the clones made
        // from it (the live composition, the reconfigure blueprint) share it.
        if let Some(persistence) = provider_auth_persistence {
            factory = factory.with_provider_auth_persistence(persistence);
        }
        // The host's root for the members' session comms identities, as the
        // gateway's `session_identity_config_root` sets it.
        if let Some(root) = session_identity_config_root {
            factory = factory.user_config_root(root);
        }
        if let Some(provider) = mcp_call_context_provider {
            factory = factory.mcp_call_context_provider(provider);
        }
        if let Some(observer) = operation_refusal_observer {
            factory = factory.with_operation_refusal_observer(observer);
        }
        if caps.image_generation {
            factory = factory
                .with_image_generation_machine(weak_image_generation_machine(&runtime_adapter));
        }
        let config = crate::compaction_policy::with_meta_compaction_defaults(
            agent_config.unwrap_or_default(),
        );
        let member_model_config = Arc::new(config.clone());
        let mut builder = FactoryAgentBuilder::new(factory, config);
        // What the member builder's factory resolves provider credentials
        // with, for the rows that prove the host's store reached it.
        #[cfg(test)]
        let member_provider_auth_persistence = builder
            .factory()
            .resolution_provider_auth_persistence()
            .ok()
            .flatten();
        let host_default_decorator = Arc::clone(&builder.default_agent_llm_client_decorator);
        // Prepare each member's construction only after its configured MCP
        // servers (the profile's own and any document-bound one) have
        // connected or failed, so the surface captured for its host carries
        // them and native inherited visibility resolves against them.
        builder.wait_for_mcp = true;
        builder.default_session_store = Some(Arc::new(StoreAdapter::new(session_store.clone())));
        builder.default_blob_store = Some(blob_store.clone());
        let (job_store, job_store_slot): (Arc<dyn meerkat::DetachedJobStore>, StorageSlotSummary) =
            if let Some(provider) = provider_meerkat_stores.as_ref() {
                (Arc::clone(&provider.job_store), provider.job_slot_summary())
            } else {
                let path = meerkat_store::realm_paths_in(&store_path, meerkat_realm.as_str())
                    .jobs_sqlite_path;
                let store =
                    meerkat::SqliteDetachedJobStore::open(path.clone()).map_err(|error| {
                        crate::storage_health::JobStoreResolutionError {
                            path,
                            message: error.to_string(),
                        }
                    })?;
                (
                    Arc::new(store),
                    StorageSlotSummary::persistent("jobs", "SqliteDetachedJobStore"),
                )
            };
        builder.default_detached_job_store = Some(Arc::clone(&job_store));
        let delivery = MobRuntimeDelivery::new(Arc::clone(&runtime_store), Arc::clone(&job_store));
        builder.default_shell_job_delivery_projector =
            Some(meerkat::JobOutboxProjector::new_for_realm(
                job_store,
                delivery.inbox(),
                meerkat_realm.as_str(),
            ));
        let (session_llm_reconfigure_blueprint, session_llm_default_client_slot) =
            session_llm_reconfigure_blueprint(&builder, &store_path);
        let mob_tools_slot = Arc::clone(&builder.default_mob_tools);
        // Injected schedule store (M4 builder seam): attach the agent-facing
        // schedule tools over the caller's store. Library mode wires no
        // firing host — authored schedules become durable rows the embedder's
        // own driver (or a gateway pointed at the same store) fires.
        let schedule_slot = schedule_store.map(|store| {
            let _attached =
                crate::schedule_wiring::attach_schedule_tools_with_store(&builder, store);
            StorageSlotSummary::persistent("schedule", "custom schedule store").with_detail(
                "caller-injected store; durability rides with the injector. Library mode \
                 attaches schedule tools without a firing host",
            )
        });
        // Durable workgraph store: the composite provider's meerkat-level
        // bundle when installed (M4b single-bundle), otherwise local SQLite
        // beside runtime.sqlite (boot-without on open failure — a
        // sanctioned, health-visible degradation).
        let (workgraph_service, workgraph_admission_slot, workgraph_slot) =
            if let Some(store) = workgraph_store {
                // Per-slot injection (item 5) wins over the composite
                // provider's bundle. The ORDER is load-bearing: this fn is
                // `pub(crate)` and reachable WITHOUT `UnifiedRuntimeBuilder`'s
                // conflict check, so if the provider arm ran first an
                // explicitly injected store would be dropped silently rather
                // than refused.
                let (service, slot) = crate::workgraph_wiring::attach_workgraph_tools_with_store(
                    &builder,
                    store,
                    definition.id.as_str(),
                );
                (
                    Some(service),
                    Some(slot),
                    StorageSlotSummary::unverified(
                        "workgraph",
                        meerkat_core::DurabilityClass::Durable,
                        "custom workgraph store",
                    )
                    .with_detail("caller-injected store; no durability declaration"),
                )
            } else if let Some(provider) = provider_meerkat_stores.as_ref() {
                // Canonical mob realm via the one helper - see
                // `workgraph_wiring::scoped_workgraph_service`. A second
                // spelling here is what made every workgraph member unbuildable.
                let service = crate::workgraph_wiring::scoped_workgraph_service(
                    Arc::clone(&provider.workgraph_store),
                    definition.id.as_str(),
                );
                let slot = crate::workgraph_wiring::install_workgraph_tools(&builder, &service);
                (Some(service), Some(slot), provider.workgraph_slot_summary())
            } else {
                match crate::workgraph_wiring::attach_workgraph_tools_reporting(
                    &builder,
                    &store_path,
                    definition.id.as_str(),
                ) {
                    Ok((service, slot)) => (
                        Some(service),
                        Some(slot),
                        StorageSlotSummary::persistent("workgraph", "SqliteWorkGraphStore"),
                    ),
                    Err(error) => (
                        None,
                        None,
                        StorageSlotSummary::degraded(
                            "workgraph",
                            format!("workgraph store failed to open; workgraph disabled: {error}"),
                        ),
                    ),
                }
            };
        let durable_runtime_store = Arc::clone(&runtime_store);
        let durable_session_store = Arc::clone(&session_store);
        let archived_terminal_authority = Arc::clone(&runtime_store);
        let live_factory = builder.factory().clone();
        let live_config = builder.config().clone();
        let concrete_session_service = Arc::new(meerkat_session::PersistentSessionService::new(
            builder,
            max_sessions,
            session_store,
            runtime_store,
            blob_store,
        ));
        let runtime_adapter = concrete_session_service
            .acquire_canonical_runtime_adapter(Some(runtime_adapter))
            .map_err(StorageResolutionError::Runtime)?;
        // The session store is durable here, so the live doors compose over
        // this service exactly as over a provider's.
        let live_compose_inputs = LiveComposeInputs {
            service: Arc::clone(&concrete_session_service),
            machine: Arc::clone(&runtime_adapter),
            factory: live_factory,
            config: live_config,
        };
        let reconfigure_service: Arc<
            dyn meerkat::session_runtime::llm_reconfigure::SessionRuntimeLlmReconfigureService,
        > = concrete_session_service.clone();
        session_llm_reconfigure_blueprint.install(&runtime_adapter, reconfigure_service);
        // Heal seam (2026-07-29 incident): the CONCRETE persistent service is
        // the committed-boundary recoverer; the erased MobSessionService does
        // not carry the heal API, so the typed handle is captured here.
        let committed_boundary_recoverer: Arc<
            dyn crate::identity_first::bridge::CommittedBoundaryRecoverer,
        > = concrete_session_service.clone();
        let controller_pin_source: Arc<dyn crate::identity_first::bridge::ControllerPinSource> =
            concrete_session_service.clone();
        let session_service: Arc<dyn MobSessionService> = concrete_session_service;
        let hook = refusing_unresolved_document_markers(hook);
        let session_service = Arc::new(PreBuildMobSessionService {
            inner: session_service,
            hook,
            dispatch_taint: None,
            after_create_hook,
            // This default was already adopted by the concrete service;
            // every later acquisition still validates through that service.
            runtime_adapter_override: Some(Arc::clone(&runtime_adapter)),
            archived_terminal_authority: Some(archived_terminal_authority),
            member_sessions_in_memory: false,
        }) as Arc<dyn MobSessionService>;
        // Wrap first and install the agent mob tools on the spec's final
        // service: child mobs are built on the tools' session service, so
        // they get every layer member creates get (the gateway's
        // `with_agent_mob_tools` order).
        let mut spec =
            Self::with_base_layer(definition, storage, session_service, no_op_pre_build_hook())
                .with_mob_storage_provenance(provenance);
        spec.member_model_config = Some(member_model_config);
        #[cfg(test)]
        {
            spec.member_provider_auth_persistence = member_provider_auth_persistence;
        }
        // The builder's own default decorator cell, so member taint composes
        // over the default this factory would apply.
        spec.dispatch_taint_slot
            .bind_host_default_decorator(host_default_decorator);
        spec.runtime_delivery = Some(delivery.clone());
        let (
            agent_mob_mcp_state,
            implicit_delegate_retirement_overrides,
            agent_mob_default_llm_client_slot,
            console_spawn_sink_slot,
            identity_runtime_slot,
            agent_mob_tools_install,
        ) = install_agent_mob_tools(
            &spec.definition,
            mob_tools_slot,
            Arc::clone(&spec.session_service),
            Some(runtime_adapter.clone()),
            workgraph_service.clone(),
            Some(session_llm_default_client_slot),
            // Persistent mob: councils become durable, so a restart can see
            // unfinished records through `list_unfinished` instead of losing
            // them with the process.
            // Refuse the boot on an UNREADABLE store rather than degrading:
            // an unreadable custody store is not an empty one, and meerkat's
            // council recovery surfaces read/decode failure as a typed refusal.
            // Silently falling back to in-memory here would swallow that
            // signal at the boundary. An ABSENT store is still the ordinary
            // first-boot case and still degrades quietly. A host store realm
            // has no council adapter: its councils are process-bound.
            if host_store {
                None
            } else {
                crate::council_wiring::open_council_store(
                    &crate::council_wiring::council_db_for_store_path(&store_path),
                )?
            },
            spec.storage.forked_participant_store().cloned(),
            delivery,
        )
        .map_err(StorageResolutionError::Delivery)?;
        spec.durable_runtime_store = Some(durable_runtime_store);
        spec.durable_session_store = Some(durable_session_store);
        spec.live_compose_inputs = Some(live_compose_inputs);
        spec.committed_boundary_recoverer = Some(committed_boundary_recoverer);
        spec.controller_pin_source = Some(controller_pin_source);
        spec.agent_mob_mcp_state = Some(agent_mob_mcp_state);
        spec.implicit_delegate_retirement_overrides = Some(implicit_delegate_retirement_overrides);
        spec.agent_mob_default_llm_client_slot = Some(agent_mob_default_llm_client_slot);
        spec.console_spawn_sink_slot = Some(console_spawn_sink_slot);
        spec.identity_runtime_slot = Some(identity_runtime_slot);
        spec.composition_release
            .register_slot(&agent_mob_tools_install.slot);
        spec.agent_mob_tools_install = Some(agent_mob_tools_install);
        spec.runtime_adapter = Some(runtime_adapter);
        spec.binary_blob_store = Some(binary_blob_store);
        spec.workgraph_service = workgraph_service;
        if let Some(slot) = workgraph_admission_slot {
            // SQLite- or provider-backed store: the backend is shareable
            // across processes, so admissions additionally serialize through
            // the sidecar lock (a same-host guard).
            spec.workgraph_admission_slots.push(slot);
            spec.workgraph_admission_sidecar =
                Some(crate::workgraph_admission::workgraph_admission_sidecar_path(&store_path));
        }
        let mut slots = vec![
            StorageSlotSummary::persistent("sessions", session_store_kind).with_detail(
                if session_store_kind == "SqliteSessionStore" {
                    "builder-opened SQLite store under the state directory"
                } else {
                    "caller-injected store; durability rides with the injector"
                },
            ),
            runtime_store_slot,
            blob_slot_summary(blob_durability),
            workgraph_slot,
            job_store_slot,
        ];
        if let Some(slot) = schedule_slot {
            slots.push(slot);
        }
        slots.extend(scratch_ring_buffer_slots());
        spec.resolved_storage = Some(
            ResolvedStorageSummary::new(blob_durability, session_store_incremental)
                .with_slots(slots)
                .with_runtime_store_locator(runtime_store_locator),
        );
        match authority {
            PersistentAuthority::StateDir => {
                #[cfg(feature = "extension-state")]
                {
                    spec.composed_meerkat_realm = Some((store_path.clone(), meerkat_realm));
                }
                // The stock persistent composition is over this layout, so the
                // child-mob default resolves against it.
                spec.meta_storage_layout = Some(
                    crate::storage_layout::MetaStorageLayout::with_injected_roots(store_path, None),
                );
            }
            PersistentAuthority::HostStore { cache_dir } => {
                // Agent-created child mobs (mob_create, delegate's implicit
                // mob) keep their logs under a local directory, which is no
                // authority here: they are process-bound, explicitly.
                spec.child_mob_storage = Some(crate::ChildMobStorage::ProcessBound);
                spec._ephemeral_dir = cache_dir;
            }
        }
        spec.state_dir_claims.extend(state_dir_claim);
        Ok(spec)
    }

    /// Test fixtures only: production paths construct through
    /// `ephemeral_runtime_backed_with_provider_stores`, which carries the
    /// optional native grant configuration.
    #[cfg(test)]
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn ephemeral_runtime_backed_inner(
        definition: MobDefinition,
        storage: MobStorage,
        store_path: PathBuf,
        max_sessions: usize,
        custom_blob_store: Option<BlobStoreInjection>,
        schedule_store: Option<Arc<dyn meerkat::ScheduleStore>>,
        workgraph_store: Option<Arc<dyn meerkat::WorkGraphStore>>,
        hook: Option<PreBuildHook>,
        caps: CapabilityFlags,
        after_create_hook: Option<AfterCreateHook>,
        agent_config: Option<Config>,
    ) -> Result<Self, StorageResolutionError> {
        Self::ephemeral_runtime_backed_with_provider_stores(
            definition,
            storage,
            store_path,
            max_sessions,
            custom_blob_store,
            schedule_store,
            workgraph_store,
            hook,
            caps,
            after_create_hook,
            agent_config,
            None,
            None,
            None,
            None,
            None,
            None,
        )
    }

    /// Build an ephemeral runtime with the optional composite storage provider's
    /// meerkat-level bundle. When present, runtime and workgraph authority ride
    /// the provider's stores instead of process-local memory.
    ///
    /// `provider_auth_persistence` is the host's provider credential store
    /// when it names one, held in place of the process default as in
    /// [`persistent_inner_with_provider_stores`](Self::persistent_inner_with_provider_stores).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn ephemeral_runtime_backed_with_provider_stores(
        definition: MobDefinition,
        storage: MobStorage,
        store_path: PathBuf,
        max_sessions: usize,
        custom_blob_store: Option<BlobStoreInjection>,
        schedule_store: Option<Arc<dyn meerkat::ScheduleStore>>,
        workgraph_store: Option<Arc<dyn meerkat::WorkGraphStore>>,
        hook: Option<PreBuildHook>,
        mut caps: CapabilityFlags,
        after_create_hook: Option<AfterCreateHook>,
        agent_config: Option<Config>,
        provider_meerkat_stores: Option<crate::storage_provider::ProviderMeerkatStores>,
        grant_configuration: Option<meerkat_runtime::meerkat_machine::NativeGrantWorkConfiguration>,
        mcp_call_context_provider: Option<Arc<dyn meerkat_mcp::McpCallContextProvider>>,
        operation_refusal_observer: Option<Arc<dyn meerkat_core::OperationRefusalObserver>>,
        session_identity_config_root: Option<PathBuf>,
        provider_auth_persistence: Option<meerkat::ProviderAuthPersistence>,
    ) -> Result<Self, StorageResolutionError> {
        caps.image_generation |= mob_definition_may_use_image_generation(&definition);
        let config = crate::compaction_policy::with_meta_compaction_defaults(
            agent_config.unwrap_or_default(),
        );
        // A composite storage provider's realm supplies the session store with
        // its runtime store (Core's pair); otherwise both are process-local,
        // the declared choice of this mode.
        let provider_session_store = provider_meerkat_stores
            .as_ref()
            .map(|provider| Arc::clone(&provider.session_store));
        let has_provider_session_store = provider_session_store.is_some();
        let session_store: Arc<dyn SessionStore> = provider_session_store
            .clone()
            .unwrap_or_else(|| Arc::new(meerkat_store::MemoryStore::new()));
        // Ephemeral-by-design mode: in-memory blobs are the declared choice
        // of the mode itself (scratch/temp-dir launches), not an error-path
        // fallback. The declaration still surfaces through
        // `resolved_storage` because this mode also serves the hybrid shape
        // where a storage provider's realm durably persists sessions yet the
        // blobs stay ephemeral.
        let (binary_blob_store, blob_store, blob_durability): (
            Arc<dyn BinaryBlobStore>,
            Arc<dyn meerkat_core::BlobStore>,
            BlobDurability,
        ) = if let Some(injection) = custom_blob_store {
            let (binary_blob_store, blob_store) = injection.into_pair();
            let persistent = binary_blob_store.is_persistent();
            (
                binary_blob_store,
                blob_store,
                BlobDurability::Custom { persistent },
            )
        } else {
            let binary_blob_store: Arc<dyn BinaryBlobStore> =
                Arc::new(ObjectStoreBlobStore::memory());
            let blob_store: Arc<dyn meerkat_core::BlobStore> =
                Arc::new(Base64BlobStoreAdapter::new(binary_blob_store.clone()));
            (
                binary_blob_store,
                blob_store,
                BlobDurability::DeclaredEphemeral,
            )
        };
        // Only a provider-supplied session store needs a persistence capability
        // probe. The default MemoryStore is an explicit process-local choice.
        let session_store_incremental = provider_session_store
            .as_ref()
            .map(|store| probe_session_store_incremental(store, PROVIDER_SESSION_STORE_KIND));
        // A store-backed machine requires the service that owns that same
        // store. MemoryStore/InMemoryRuntimeStore preserve process-local mode;
        // the native persistent service supplies live comms and boundary
        // acknowledgement through its canonical runtime owner.
        let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> =
            if let Some(provider) = provider_meerkat_stores.as_ref() {
                // M4b single-bundle: runtime authority rides the composite
                // provider's meerkat-level bundle together with its session
                // store, not process-local memory.
                Arc::clone(&provider.runtime_store)
            } else {
                Arc::new(meerkat_runtime::InMemoryRuntimeStore::new())
            };
        let runtime_adapter = Arc::new(
            persistent_runtime_machine(
                Arc::clone(&runtime_store),
                Arc::clone(&blob_store),
                grant_configuration,
            )
            .map_err(StorageResolutionError::Runtime)?,
        );
        let mut factory = AgentFactory::new(&store_path)
            .builtins(caps.builtins)
            .shell(caps.shell)
            .mob(caps.mob)
            .comms(caps.comms)
            .memory(caps.memory)
            .mcp_apps(caps.mcp_apps);
        // The host's provider credential store replaces the process default
        // before the member builder takes the factory, so the clones made
        // from it (the live composition, the reconfigure blueprint) share it.
        if let Some(persistence) = provider_auth_persistence {
            factory = factory.with_provider_auth_persistence(persistence);
        }
        // The host's root for the members' session comms identities, as the
        // gateway's `session_identity_config_root` sets it.
        if let Some(root) = session_identity_config_root {
            factory = factory.user_config_root(root);
        }
        if let Some(provider) = mcp_call_context_provider {
            factory = factory.mcp_call_context_provider(provider);
        }
        if let Some(observer) = operation_refusal_observer {
            factory = factory.with_operation_refusal_observer(observer);
        }
        if caps.image_generation {
            factory = factory
                .with_image_generation_machine(weak_image_generation_machine(&runtime_adapter));
        }
        let member_model_config = Arc::new(config.clone());
        let mut builder = FactoryAgentBuilder::new(factory, config);
        // What the member builder's factory resolves provider credentials
        // with, for the rows that prove the host's store reached it.
        #[cfg(test)]
        let member_provider_auth_persistence = builder
            .factory()
            .resolution_provider_auth_persistence()
            .ok()
            .flatten();
        let host_default_decorator = Arc::clone(&builder.default_agent_llm_client_decorator);
        // Prepare each member's construction only after its configured MCP
        // servers (the profile's own and any document-bound one) have
        // connected or failed, so the surface captured for its host carries
        // them and native inherited visibility resolves against them.
        builder.wait_for_mcp = true;
        builder.default_session_store = Some(Arc::new(StoreAdapter::new(session_store.clone())));
        builder.default_blob_store = Some(blob_store.clone());
        let job_store: Arc<dyn meerkat::DetachedJobStore> = provider_meerkat_stores
            .as_ref()
            .map(|provider| Arc::clone(&provider.job_store))
            .unwrap_or_else(|| Arc::new(meerkat::MemoryDetachedJobStore::new()));
        builder.default_detached_job_store = Some(Arc::clone(&job_store));
        let delivery = MobRuntimeDelivery::new(Arc::clone(&runtime_store), Arc::clone(&job_store));
        builder.default_shell_job_delivery_projector =
            Some(meerkat::JobOutboxProjector::new_for_realm(
                job_store,
                delivery.inbox(),
                crate::storage_provider::DEFAULT_MEERKAT_LEVEL_REALM_ID,
            ));
        let (session_llm_reconfigure_blueprint, session_llm_default_client_slot) =
            session_llm_reconfigure_blueprint(&builder, &store_path);
        let mob_tools_slot = Arc::clone(&builder.default_mob_tools);
        // Injected schedule store (M4 builder seam): the operator shape — an
        // otherwise-ephemeral local runtime whose durable schedule rows live
        // in a caller-supplied remote store. Library mode wires no firing
        // host (see `UnifiedRuntimeBuilder::schedule_store`).
        let schedule_slot = schedule_store.map(|store| {
            let _attached =
                crate::schedule_wiring::attach_schedule_tools_with_store(&builder, store);
            StorageSlotSummary::persistent("schedule", "custom schedule store").with_detail(
                "caller-injected store; durability rides with the injector. Library mode \
                 attaches schedule tools without a firing host",
            )
        });
        let (workgraph_service, workgraph_admission_slot, workgraph_slot) =
            if let Some(store) = workgraph_store {
                // Per-slot injection wins over the provider bundle. Keep the
                // census with the exact selected source rather than the mode.
                let (service, slot) = crate::workgraph_wiring::attach_workgraph_tools_with_store(
                    &builder,
                    store,
                    definition.id.as_str(),
                );
                (
                    service,
                    slot,
                    StorageSlotSummary::unverified(
                        "workgraph",
                        meerkat_core::DurabilityClass::Durable,
                        "custom workgraph store",
                    )
                    .with_detail("caller-injected store; no durability declaration"),
                )
            } else if let Some(provider) = provider_meerkat_stores.as_ref() {
                // Use the canonical mob realm and the declaration of the
                // provider store actually selected by this branch.
                let service = crate::workgraph_wiring::scoped_workgraph_service(
                    Arc::clone(&provider.workgraph_store),
                    definition.id.as_str(),
                );
                let slot = crate::workgraph_wiring::install_workgraph_tools(&builder, &service);
                (service, slot, provider.workgraph_slot_summary())
            } else {
                let (service, slot) = crate::workgraph_wiring::attach_workgraph_tools_ephemeral(
                    &builder,
                    definition.id.as_str(),
                );
                (
                    service,
                    slot,
                    StorageSlotSummary::declared_ephemeral(
                        "workgraph",
                        "MemoryWorkGraphStore",
                        "declared by the ephemeral launch mode",
                    ),
                )
            };
        let live_factory = builder.factory().clone();
        let live_config = builder.config().clone();
        let concrete_session_service = Arc::new(meerkat_session::PersistentSessionService::new(
            builder,
            max_sessions,
            session_store,
            runtime_store.clone(),
            blob_store,
        ));
        let runtime_adapter = concrete_session_service
            .acquire_canonical_runtime_adapter(Some(runtime_adapter))
            .map_err(StorageResolutionError::Runtime)?;
        let reconfigure_service: Arc<
            dyn meerkat::session_runtime::llm_reconfigure::SessionRuntimeLlmReconfigureService,
        > = concrete_session_service.clone();
        session_llm_reconfigure_blueprint.install(&runtime_adapter, reconfigure_service);
        let live_compose_inputs = has_provider_session_store.then(|| LiveComposeInputs {
            service: Arc::clone(&concrete_session_service),
            machine: Arc::clone(&runtime_adapter),
            factory: live_factory,
            config: live_config,
        });
        let session_service: Arc<dyn MobSessionService> = concrete_session_service;
        let after_create_hook = Some(register_runtime_after_create(
            runtime_adapter.clone(),
            after_create_hook,
        ));
        let hook = refusing_unresolved_document_markers(hook);
        let session_service = Arc::new(PreBuildMobSessionService {
            inner: session_service,
            hook,
            dispatch_taint: None,
            after_create_hook,
            runtime_adapter_override: Some(runtime_adapter.clone()),
            archived_terminal_authority: None,
            // Without a storage provider the session and runtime stores are
            // process-local memory: the member sessions do not outlive the
            // process, so a durable profile is refused here.
            member_sessions_in_memory: !has_provider_session_store,
        }) as Arc<dyn MobSessionService>;
        // Wrap first and install the agent mob tools on the spec's final
        // service: child mobs are built on the tools' session service, so
        // they get every layer member creates get (the gateway's
        // `with_agent_mob_tools` order).
        let mut spec =
            Self::with_base_layer(definition, storage, session_service, no_op_pre_build_hook());
        spec.member_model_config = Some(member_model_config);
        #[cfg(test)]
        {
            spec.member_provider_auth_persistence = member_provider_auth_persistence;
        }
        // The builder's own default decorator cell, so member taint composes
        // over the default this factory would apply.
        spec.dispatch_taint_slot
            .bind_host_default_decorator(host_default_decorator);
        spec.runtime_delivery = Some(delivery.clone());
        let (
            agent_mob_mcp_state,
            implicit_delegate_retirement_overrides,
            agent_mob_default_llm_client_slot,
            console_spawn_sink_slot,
            identity_runtime_slot,
            agent_mob_tools_install,
        ) = install_agent_mob_tools(
            &spec.definition,
            mob_tools_slot,
            Arc::clone(&spec.session_service),
            Some(runtime_adapter.clone()),
            Some(workgraph_service.clone()),
            Some(session_llm_default_client_slot),
            // Persistent mob: councils become durable here too. This site
            // shares an argument shape with the ephemeral constructor, which
            // is how it first received the ephemeral arm - it compiled clean
            // and would have left persistent councils process-bound.
            // Ephemeral runtime-backed mob: councils stay IN-MEMORY.
            //
            // This site previously took a durable store, which contradicted
            // the documented design and shipped in 0.8.29-dev. It shares an
            // argument shape with the persistent constructor, and the guard
            // that was meant to catch exactly that mis-assignment could not
            // see this function at all - it is `pub(crate) fn`, which the
            // guard's detection missed, so the site was attributed to the
            // preceding `persistent_*` function and read as correct.
            None,
            spec.storage.forked_participant_store().cloned(),
            delivery,
        )
        .map_err(StorageResolutionError::Delivery)?;
        spec.agent_mob_mcp_state = Some(agent_mob_mcp_state);
        spec.implicit_delegate_retirement_overrides = Some(implicit_delegate_retirement_overrides);
        spec.agent_mob_default_llm_client_slot = Some(agent_mob_default_llm_client_slot);
        spec.console_spawn_sink_slot = Some(console_spawn_sink_slot);
        spec.identity_runtime_slot = Some(identity_runtime_slot);
        spec.composition_release
            .register_slot(&agent_mob_tools_install.slot);
        spec.agent_mob_tools_install = Some(agent_mob_tools_install);
        spec.runtime_adapter = Some(runtime_adapter);
        spec.durable_runtime_store = Some(runtime_store);
        spec.durable_session_store = provider_session_store;
        spec.live_compose_inputs = live_compose_inputs;
        spec.binary_blob_store = Some(binary_blob_store);
        spec.workgraph_service = Some(workgraph_service);
        spec.workgraph_admission_slots
            .push(workgraph_admission_slot);
        let mut slots = vec![
            if has_provider_session_store {
                StorageSlotSummary::persistent("sessions", PROVIDER_SESSION_STORE_KIND).with_detail(
                    "the storage provider realm's store; durability rides with the provider",
                )
            } else {
                StorageSlotSummary::declared_ephemeral(
                    "sessions",
                    "MemoryStore",
                    "declared by the ephemeral launch mode",
                )
            },
            if let Some(provider) = provider_meerkat_stores.as_ref() {
                provider.runtime_slot_summary()
            } else {
                StorageSlotSummary::declared_ephemeral(
                    "runtime",
                    "InMemoryRuntimeStore",
                    "declared by the ephemeral launch mode",
                )
            },
            blob_slot_summary(blob_durability),
            workgraph_slot,
            if let Some(provider) = provider_meerkat_stores.as_ref() {
                provider.job_slot_summary()
            } else {
                StorageSlotSummary::declared_ephemeral(
                    "jobs",
                    "MemoryDetachedJobStore",
                    "declared by the ephemeral launch mode",
                )
            },
        ];
        if let Some(slot) = schedule_slot {
            slots.push(slot);
        }
        slots.extend(scratch_ring_buffer_slots());
        spec.resolved_storage = Some(
            ResolvedStorageSummary::new(blob_durability, session_store_incremental)
                .with_slots(slots),
        );
        Ok(spec)
    }
}

/// The session service's runtime machine for an observation. A refused
/// acquisition is logged and observed as
/// no machine, which every caller already reads as inconclusive or
/// unavailable, never as a negative answer.
pub(crate) fn observed_runtime_adapter(
    service: &dyn MobSessionService,
) -> Option<Arc<meerkat_runtime::MeerkatMachine>> {
    match service.acquire_runtime_adapter(None) {
        Ok(adapter) => adapter,
        Err(error) => {
            tracing::warn!(%error, "runtime adapter acquisition refused; observation is inconclusive");
            None
        }
    }
}

/// Preserve native readiness classification at SessionError boundaries.
pub(crate) fn runtime_adapter_acquisition_session_error(
    error: meerkat_runtime::RuntimeDriverError,
) -> SessionError {
    match error {
        meerkat_runtime::RuntimeDriverError::ControllerReadinessUnavailable { reason } => {
            SessionError::RuntimeUnavailable { reason }
        }
        other => SessionError::Agent(meerkat_core::AgentError::InternalError(other.to_string())),
    }
}

/// Error returned by mob runtime operations.
#[derive(Debug)]
pub enum MobRuntimeError {
    Mob(MobError),
    InvalidInput(&'static str),
    /// A typed refusal on the explicit host-human lane (for example a skill
    /// selection sent to a member that has no native carrier for it).
    HostHumanInput(crate::identity_first::bridge::HostHumanInputError),
    InvalidConfig(String),
    /// A persistent mob storage path could not be proven to match the supplied
    /// composition. Raised before the mob actuates.
    CompositionProvenance(crate::mob_composition_manifest::MobCompositionProvenanceError),
    /// A teardown stop did not settle the mob's flow runs within its budget.
    MobStopFlowRunsUnsettled(Box<crate::unified_runtime::MobStopFlowRunsUnsettled>),
    /// The runtime machine could not be acquired or composed.
    Runtime(meerkat_runtime::RuntimeDriverError),
    Delivery(MobRuntimeDeliveryError),
    /// Startup acquired a handle but the native delivery owner refused. The
    /// full stop outcome states which members remain unsettled or unholdable.
    DeliveryStartup {
        error: Box<MobRuntimeDeliveryError>,
        stop: Box<Result<meerkat_mob::MobStopReport, MobError>>,
    },
    /// Durable child-mob storage could not be composed or restored.
    ChildMobStorage(crate::child_mob_storage::ChildMobStorageError),
}

impl std::fmt::Display for MobRuntimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Mob(err) => write!(f, "{err}"),
            Self::InvalidInput(message) => write!(f, "{message}"),
            Self::HostHumanInput(err) => write!(f, "{err}"),
            Self::InvalidConfig(message) => write!(f, "{message}"),
            Self::CompositionProvenance(err) => write!(f, "{err}"),
            Self::MobStopFlowRunsUnsettled(err) => write!(f, "{err}"),
            Self::Runtime(err) => write!(f, "{err}"),
            Self::Delivery(err) => write!(f, "{err}"),
            Self::DeliveryStartup { error, stop } => write!(
                f,
                "runtime delivery startup failed: {error}; acquired mob stop outcome: {stop:?}"
            ),
            Self::ChildMobStorage(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for MobRuntimeError {}

impl From<MobError> for MobRuntimeError {
    fn from(value: MobError) -> Self {
        Self::Mob(value)
    }
}

// Meta's `MobMemberSnapshot`, `MobReconcileReport`, `MobReconcileOptions`
// wrapper types were removed as part of the meerkat 0.6 thin-shell cleanup.
// Consumers now use `meerkat_mob::runtime::MobMemberListEntry` and
// `meerkat_mob::runtime::reconcile::{ReconcileReport, ReconcileOptions,
// MemberFilter}` directly.

/// Context delivered to [`SessionHook::after_create`] after a session is
/// successfully created.
#[derive(Clone, Debug)]
pub struct SessionCreatedContext {
    pub model: String,
    pub labels: std::collections::BTreeMap<String, String>,
    pub system_prompt: Option<String>,
}

/// Hook trait for customising session lifecycle.
///
/// - `before_create` — runs before `create_session`. Returning `Err` aborts
///   session creation (both Rust-native and Python/TS boundary).
/// - `after_create` — runs after session creation succeeds. Best-effort: errors
///   are logged at `warn`, not propagated. The session is already live.
///
/// `before_create` runs on every build of a session, including the rebuild
/// that restores a member after a restart, and an edit it makes applies to
/// that build only. A narrowing of the member's mob operator spawn scope (the
/// build's `mob_tool_authority_context`, for example with
/// `meerkat_runtime::mob_operator_authority::restrict_spawn_profiles_in_mob`)
/// is not restored as authority, and neither is a disable of the member's mob
/// tools made in the hook: the restore build reaches this hook with the
/// member's profile setting and default authority again. Make the edit on
/// every build, restores included, or a restored member returns to the
/// default authority.
#[async_trait]
pub trait SessionHook: Send + Sync {
    /// Called before session creation. Mutate the request to inject tools,
    /// augment prompts, set labels, override model, etc. Return `Err` to
    /// abort session creation.
    async fn before_create(&self, _req: &mut CreateSessionRequest) -> Result<(), SessionError> {
        Ok(())
    }

    /// Called after a session is successfully created. Best-effort — errors
    /// logged, not propagated.
    async fn after_create(
        &self,
        _session_id: &meerkat_core::types::SessionId,
        _ctx: &SessionCreatedContext,
    ) {
    }
}

/// Capability flags controlling which agent capabilities are enabled.
#[derive(Clone, Copy, Debug)]
pub struct CapabilityFlags {
    pub builtins: bool,
    pub shell: bool,
    pub mob: bool,
    pub comms: bool,
    pub memory: bool,
    pub image_generation: bool,
    /// Advertise standard MCP Apps support only when a presentation host exists.
    pub mcp_apps: bool,
}

impl Default for CapabilityFlags {
    fn default() -> Self {
        Self {
            builtins: true,
            shell: true,
            mob: true,
            comms: true,
            memory: true,
            image_generation: false,
            mcp_apps: false,
        }
    }
}

/// Backward-compatible alias for [`MobRuntime`].
pub type RealMobRuntime = MobRuntime;

/// Normalize declared resume overrides without turning ordinary profile fields
/// into synthetic operator intent.
///
/// Durable session metadata restores model/provider/provider_params on
/// resume. Model/provider overrides are therefore operator authority and must
/// come from the host's explicit `resume_overrides` list. Automatically adding
/// them for every profile made a durable `brain_swap` realization lose to the
/// original profile on cold restart.
///
/// **Model and provider are a COHERENT PAIR, never independently masked**
/// (production cutover incident, 2026-07-29): masking the model alone lets the
/// durable provider survive under a profile model it was never registered
/// for, and the resume is REJECTED typed ("model 'claude-fable-5' is
/// registered for provider 'anthropic', not 'openai'"). When the profile
/// declares no provider, the pair is resolved FROM the declared model — the
/// canonical catalog owner, else the definition's `[models.<id>]` entry —
/// and written onto the profile so both fields apply together on resume. Pair
/// completion runs only when the host explicitly names either field. With no
/// explicit model/provider override, the whole durable LLM identity wins.
///
/// Applies to inline profile bindings only: realm-ref profiles resolve inside
/// meerkat-mob at spawn time and never pass through this seam. NOTE: the
/// bridge resume-divergence tripwire cannot see realm-ref declarations either
/// (the `RealmProfileStore` is not threaded into `MobSessionBridge`), so a
/// realm-profile edit that loses to durable metadata is currently silent
/// (tracked follow-up).
pub fn auto_mark_declared_resume_overrides(definition: &mut MobDefinition) {
    let MobDefinition {
        profiles, models, ..
    } = definition;
    for binding in profiles.values_mut() {
        let Some(profile) = binding.as_inline_mut() else {
            continue;
        };
        let explicit_llm_identity_override = profile.resume_overrides.iter().any(|field| {
            matches!(
                field,
                meerkat_mob::ResumeOverrideField::Model
                    | meerkat_mob::ResumeOverrideField::Provider
            )
        });
        let mut declared = Vec::new();
        if explicit_llm_identity_override {
            let coherent_provider = profile
                .provider
                // A self-hosted server binding is only meaningful under the
                // self_hosted provider; adopt that reading rather than leaving
                // an incoherent half-declaration.
                .or_else(|| {
                    profile
                        .self_hosted_server_id
                        .as_ref()
                        .map(|_| Provider::SelfHosted)
                })
                .or_else(|| {
                    meerkat_models::canonical()
                        .infer_provider(&profile.model)
                        .or_else(|| models.get(&profile.model).map(|entry| entry.provider))
                        // A DERIVED self-hosted provider needs a server binding
                        // the profile does not declare, and Other names no
                        // concrete adapter: neither can be written back as a
                        // coherent pair. (A DECLARED self_hosted/other above is
                        // honored as written.)
                        .filter(|provider| {
                            !matches!(provider, Provider::SelfHosted | Provider::Other)
                        })
                });
            if let Some(provider) = coherent_provider {
                profile.provider = Some(provider);
                declared.push(meerkat_mob::ResumeOverrideField::Model);
                declared.push(meerkat_mob::ResumeOverrideField::Provider);
            }
        }
        if profile.provider_params.is_some() {
            declared.push(meerkat_mob::ResumeOverrideField::ProviderParams);
        }
        for field in declared {
            if !profile.resume_overrides.contains(&field) {
                profile.resume_overrides.push(field);
            }
        }
    }
}

/// Reproduce the exact profile normalization shipped through MobKit 0.8.28.
///
/// Persistent stores created by those releases contain these synthesized
/// provider and resume-mask fields in their canonical `MobCreated` definition.
/// New launches must recognize that exact historical representation so an
/// unchanged operator definition can boot and later advance through the typed
/// definition-update ceremony.
fn legacy_auto_mark_declared_resume_overrides(definition: &mut MobDefinition) {
    let MobDefinition {
        profiles, models, ..
    } = definition;
    for binding in profiles.values_mut() {
        let Some(profile) = binding.as_inline_mut() else {
            continue;
        };
        let mut declared = Vec::new();
        let coherent_provider = profile
            .provider
            .or_else(|| {
                profile
                    .self_hosted_server_id
                    .as_ref()
                    .map(|_| Provider::SelfHosted)
            })
            .or_else(|| {
                meerkat_models::canonical()
                    .infer_provider(&profile.model)
                    .or_else(|| models.get(&profile.model).map(|entry| entry.provider))
                    .filter(|provider| !matches!(provider, Provider::SelfHosted | Provider::Other))
            });
        if let Some(provider) = coherent_provider {
            profile.provider = Some(provider);
            declared.push(meerkat_mob::ResumeOverrideField::Model);
            declared.push(meerkat_mob::ResumeOverrideField::Provider);
        }
        if profile.provider_params.is_some() {
            declared.push(meerkat_mob::ResumeOverrideField::ProviderParams);
        }
        for field in declared {
            if !profile.resume_overrides.contains(&field) {
                profile.resume_overrides.push(field);
            }
        }
    }
}

/// Judges a resume of a non-empty mob storage against the supplied
/// composition before any of the mob is built: the checks the resume makes
/// before the mob can actuate. It only reads; preparation applies what it
/// decides to record and report.
///
/// `definition` is the supplied definition in the form preparation judges
/// (normalized by [`auto_mark_declared_resume_overrides`]); `raw_definition`
/// is the supplied one as given, from which the released-representation
/// allowance derives its legacy form. `bind_in_process` also reads, and binds
/// the resume to, the definition of a storage declared in-process, which is
/// otherwise resumed unverified as Core reads it.
async fn judge_resume(
    storage: &MobStorage,
    provenance: &crate::mob_composition_manifest::MobStorageProvenance,
    authority: crate::mob_composition_manifest::CompositionAuthority,
    candidate_definition: crate::mob_composition_manifest::CandidateDefinition,
    definition: &MobDefinition,
    raw_definition: &MobDefinition,
    bind_in_process: bool,
) -> Result<JudgedResume, MobRuntimeError> {
    // Fail closed on undeclared storage. `MobBootstrapSpec::new` accepts
    // arbitrary storage from any caller including external embedders, so a
    // non-empty log with nothing declared about it could be a durable
    // database opened directly, and resuming it would skip composition
    // verification entirely. An explicit in-process declaration is
    // permitted: a pre-seeded in-memory storage has no composition that
    // outlives the process to disagree with.
    if !provenance.permits_unverified_resume() {
        return Err(MobRuntimeError::CompositionProvenance(
            crate::mob_composition_manifest::MobCompositionProvenanceError::UnprovenStorage,
        ));
    }
    let manifest_location = provenance.manifest_location();
    // A launch that does not speak for the durable composition is exempt from
    // VERIFICATION: verifying it would refuse the candidate for exactly the
    // fields it exists to differ in, wedging a candidate/promote pipeline in
    // the other direction.
    //
    // It is NOT exempt from RECORDING. Skipping the record would leave a store
    // whose composition nobody has spoken for, and the authoritative resume
    // that follows would adopt its OWN definition as the record while
    // `for_resume` boots the rehearsal definition the event log holds - a
    // manifest certifying a composition that is not running. Recording the
    // creating authority instead lets that resume refuse with the real reason.
    let speaks_for_composition = authority.speaks_for_composition();
    let mut judged = JudgedResume {
        snapshot: None,
        record_supplied_composition: false,
        candidate_diverged: Vec::new(),
    };
    // The event log's current definition is authoritative. Verify the
    // supplied composition against a storage-minted snapshot; the resume
    // binds to that exact snapshot.
    if let Some(location) = manifest_location
        .as_ref()
        .filter(|_| speaks_for_composition)
    {
        let snapshot = canonical_definition_snapshot(storage).await?;
        match location
            .verify_before_resume(snapshot.definition(), definition)
            .await
        {
            Ok(()) => {}
            Err(crate::mob_composition_manifest::MobCompositionProvenanceError::Divergent {
                ..
            }) => {
                let mut legacy_definition = raw_definition.clone();
                legacy_auto_mark_declared_resume_overrides(&mut legacy_definition);
                location
                    .verify_legacy_synthesized_definition_before_resume(
                        snapshot.epoch(),
                        snapshot.definition(),
                        definition,
                        &legacy_definition,
                    )
                    .await
                    .map_err(MobRuntimeError::CompositionProvenance)?;
                tracing::info!(
                    mob_id = %definition.id,
                    epoch = snapshot.epoch(),
                    "resuming a canonical definition written with the MobKit 0.8.28 \
                     synthesized profile representation"
                );
            }
            // No manifest exists, so there is no recorded composition to
            // contradict. Refusing here reports a conflict that cannot be
            // shown and blocks a boot for a claim nobody made.
            //
            // What remains reachable is narrow, and deliberately so: a
            // store from a build that predates the manifest, or one whose
            // manifest an operator removed during recovery surgery. A
            // rehearsal-created store is NOT in that set any more - it
            // carries a manifest tagged non-authoritative, and
            // `verify_before_resume` refuses it by name. Without that tag
            // this branch would adopt a definition the event log does not
            // hold and certify it.
            //
            // THIS launch speaks for the composition, so it records one
            // rather than refusing to start.
            Err(crate::mob_composition_manifest::MobCompositionProvenanceError::Missing {
                ..
            }) => judged.record_supplied_composition = true,
            Err(other) => return Err(MobRuntimeError::CompositionProvenance(other)),
        }
        judged.snapshot = Some(snapshot);
    } else if !speaks_for_composition {
        // A candidate never writes or claims the pin, but it is not exempt
        // from presenting what it runs: the resume boots the stored
        // definition, so a supplied definition that differs refuses unless
        // the launch acknowledges the stored one.
        let snapshot = canonical_definition_snapshot(storage).await?;
        let mut diverged = crate::mob_composition_manifest::diverged_definition_fields(
            snapshot.definition(),
            definition,
        );
        // The same released-representation allowance the authoritative arm
        // makes: a store a 0.8.9-0.8.28 writer created (proven by its
        // manifest) carries that release's auto-marked resume overrides, so
        // an unchanged operator config is judged in that form.
        if !diverged.is_empty()
            && let Some(location) = manifest_location.as_ref()
        {
            let mut legacy_definition = raw_definition.clone();
            legacy_auto_mark_declared_resume_overrides(&mut legacy_definition);
            if location
                .verify_legacy_synthesized_definition_before_resume(
                    snapshot.epoch(),
                    snapshot.definition(),
                    definition,
                    &legacy_definition,
                )
                .await
                .is_ok()
            {
                diverged.clear();
            }
        }
        judged.candidate_diverged = crate::mob_composition_manifest::verify_candidate_resume(
            diverged,
            candidate_definition,
        )
        .map_err(MobRuntimeError::CompositionProvenance)?;
        judged.snapshot = Some(snapshot);
    } else if bind_in_process {
        // A storage declared in-process resumes unverified. A launch that
        // resolves anything from its definition before preparation still
        // binds the resume to the definition it read.
        judged.snapshot = storage
            .created_definition_snapshot()
            .await
            .map_err(|err| MobRuntimeError::Mob(MobError::from(err)))?;
    }
    Ok(judged)
}

/// The canonical definition a non-empty mob storage holds.
async fn canonical_definition_snapshot(
    storage: &MobStorage,
) -> Result<meerkat_mob::MobDefinitionSnapshot, MobRuntimeError> {
    storage
        .created_definition_snapshot()
        .await
        .map_err(|err| MobRuntimeError::Mob(MobError::from(err)))?
        .ok_or_else(|| {
            MobRuntimeError::Mob(MobError::Internal(
                "non-empty mob storage has no canonical definition".to_string(),
            ))
        })
}

/// Bound on [`MobRuntime::session_commit_pending`]'s runtime read. A read
/// still waiting for the session's runtime driver (held by a boundary commit
/// until it lands) when this elapses answers `None`.
pub const SESSION_COMMIT_PENDING_READ_BOUND: std::time::Duration =
    std::time::Duration::from_secs(2);

/// [`MobRuntime::session_commit_pending`]'s mapping: only a conclusive answer
/// within `bound` is `Some`; an error or a timeout is `None`.
async fn bounded_commit_pending<E>(
    read: impl std::future::Future<Output = Result<bool, E>>,
    bound: std::time::Duration,
) -> Option<bool> {
    match tokio::time::timeout(bound, read).await {
        Ok(Ok(pending)) => Some(pending),
        Ok(Err(_)) | Err(_) => None,
    }
}

/// Live mob runtime backed by a `MobHandle`.
#[derive(Clone)]
pub struct MobRuntime {
    handle: MobHandle,
    runtime_delivery: Option<Arc<RunningMobRuntimeDelivery>>,

    session_service: Option<Arc<dyn MobSessionService>>,
    /// See [`MobBootstrapSpec`]'s field of the same name.
    member_model_config: Option<Arc<meerkat::Config>>,
    agent_mob_mcp_state: Option<Arc<meerkat_mob_mcp::MobMcpState>>,
    implicit_delegate_retirement_overrides: Option<ImplicitDelegateRetirementOverrides>,
    binary_blob_store: Option<Arc<dyn BinaryBlobStore>>,
    baseline_member_specs: Arc<tokio::sync::RwLock<Vec<SpawnMemberSpec>>>,
    /// Slot shared with the agent mob-tool dispatchers. A console-bearing
    /// runtime fills it so agent-tool spawns project into the console.
    console_spawn_sink_slot: Option<SharedConsoleSpawnSinkSlot>,
    identity_runtime_slot: Option<SharedIdentityRuntimeSlot>,
    /// Empties the builder tool slots that point back at this runtime (see
    /// [`CompositionRelease`]): explicitly at shutdown, otherwise when the
    /// last clone of this runtime drops.
    composition_release: Option<CompositionRelease>,
    /// Realm-scoped WorkGraph service carried over from the bootstrap spec
    /// so `UnifiedRuntime` can expose it to the RPC/console surfaces.
    workgraph_service: Option<meerkat::WorkGraphService>,
    /// Admission authority for the workgraph duplicate-binding guards
    /// (goal/create, attention/resume, attention/reassign — RPC arms AND the
    /// agent tool plane's `workgraph_attention_reassign`). ONE per runtime:
    /// every surface built from this runtime must acquire this same
    /// admission, or concurrent creates race past the check and brick the
    /// member with upstream `MultipleActiveBindings`. Non-admitting
    /// consumers (MobBuilder overlays, the schedule host) use the bare
    /// service and are intentionally not serialized here.
    workgraph_admission: Arc<crate::workgraph_admission::WorkGraphAdmission>,
    /// Report of the bootstrap-time migration of member-bound attention
    /// bindings into their mob realm; `None` when no WorkGraph service is
    /// configured or the runtime was adopted from a bare handle.
    workgraph_realm_migration: Option<Arc<crate::workgraph_realm::WorkGraphRealmMigrationReport>>,
    /// Composition-time storage durability resolution carried over from the
    /// bootstrap spec so the health surfaces can report it.
    resolved_storage: Option<ResolvedStorageSummary>,
    /// The composition's durable runtime store, carried from the bootstrap
    /// spec (see [`MobBootstrapSpec::with_durable_runtime_store`]).
    durable_runtime_store: Option<Arc<dyn meerkat_runtime::RuntimeStore>>,
    /// Committed-boundary heal authority carried from the bootstrap spec so
    /// the identity-first wiring can inject it into the session bridge.
    committed_boundary_recoverer:
        Option<Arc<dyn crate::identity_first::bridge::CommittedBoundaryRecoverer>>,
    /// The concrete session owner's controller pin operation, carried from
    /// the bootstrap spec for the identity-first session bridge.
    controller_pin_source: Option<Arc<dyn crate::identity_first::bridge::ControllerPinSource>>,
    /// Whether the runtime machine was constructed with a host's native work
    /// authorization owner installed (a governed runtime).
    native_work_governed: bool,
    /// Keeps the ephemeral temp directory alive for the lifetime of the runtime.
    /// Dropped when the runtime is dropped, cleaning up the temp dir.
    _ephemeral_dir: Option<Arc<tempfile::TempDir>>,
    /// The state-directory claims carried from the bootstrap spec, released
    /// by a clean stand-down (see [`Self::stand_down_child_mobs`]) or when
    /// the last clone drops.
    state_dir_claims: Arc<std::sync::Mutex<Vec<Arc<crate::state_dir_claim::StateDirClaim>>>>,
    /// Whether this launch speaks for the durable composition. Retained past
    /// bootstrap because it gates more than the pin: a launch that does not
    /// speak for the composition must not AUTHOR durable state either.
    composition_authority: crate::mob_composition_manifest::CompositionAuthority,
    /// Exclusive ownership of the durable child-mob root and the settlement
    /// record [`Self::stand_down_child_mobs`] keeps across calls.
    child_mob_root: crate::child_mob_storage::ChildMobRootOwnership,
    /// See [`MobBootstrapSpec::with_member_mcp_custody`].
    #[cfg(not(target_arch = "wasm32"))]
    member_mcp_custody: Option<Arc<crate::member_mcp_custody::MemberMcpCustody>>,
}

/// How long a boot waits for the Stop an earlier process placed, which
/// Core's recovery finishes after resume, to land.
const RECOVERED_PLACED_STOP_SETTLE_BOUND: std::time::Duration = std::time::Duration::from_mins(1);

/// Whether Core's recovery finishes a Stop after resuming `mob_id` with this
/// event history, as Core's own recovery rule reads it: the open placed
/// lifecycle intent (the last quiesce start that no quiesce end or reset
/// closed) is `Stop`, the phase the history replays to is still running, and
/// no destroy was admitted (Core then retries the destroy instead). A Stop
/// that landed replays as stopped, and a resume closes the intent. Only the
/// mob's own events count.
fn recovery_finishes_a_placed_stop(
    mob_id: &meerkat_mob::MobId,
    events: &[meerkat_mob::event::MobEvent],
) -> bool {
    use meerkat_mob::MobEventKind as Kind;
    use meerkat_mob::machines::mob_machine::PlacedCompletionLifecycleIntentKind as Intent;
    let mut intent = None;
    let mut phase = meerkat_mob::MobState::Running;
    let mut destroy_admitted = false;
    for event in events.iter().filter(|event| &event.mob_id == mob_id) {
        match &event.kind {
            Kind::PlacedCompletionLifecycleQuiesceStarted { intent: recorded } => {
                intent = Some(*recorded);
            }
            Kind::PlacedCompletionLifecycleQuiesceEnded { intent: recorded } => {
                intent = None;
                if *recorded == Intent::Stop {
                    phase = meerkat_mob::MobState::Running;
                }
            }
            Kind::MobReset => {
                intent = None;
                phase = meerkat_mob::MobState::Running;
                destroy_admitted = false;
            }
            Kind::MobStopped => phase = meerkat_mob::MobState::Stopped,
            Kind::MobCompleted => phase = meerkat_mob::MobState::Completed,
            Kind::MobDestroying => {
                phase = meerkat_mob::MobState::Running;
                destroy_admitted = true;
            }
            Kind::MobDestroyStorageFinalizing => phase = meerkat_mob::MobState::Destroyed,
            _ => {}
        }
    }
    intent == Some(Intent::Stop) && phase == meerkat_mob::MobState::Running && !destroy_admitted
}

/// When Core's recovery finishes a Stop an earlier process placed on the
/// resumed mob, wait until it lands (`MobStopped`, or a quiesce end or reset
/// that closes it), so the boot reads the phase it leaves. The history read,
/// which queues behind that Stop on the mob actor, and the wait share one
/// bound. A Stop that does not land within it refuses the boot typed (a
/// stalled lifecycle operation): the mob would admit no work.
async fn settle_recovered_placed_stop(handle: &MobHandle) -> Result<(), MobRuntimeError> {
    let mob_id = handle.mob_id().clone();
    let settle = async {
        let events = handle.events();
        let history = events.replay_all().await.map_err(MobRuntimeError::Mob)?;
        if !recovery_finishes_a_placed_stop(&mob_id, &history) {
            return Ok(());
        }
        let after_cursor = history.last().map_or(0, |event| event.cursor);
        tracing::info!(
            mob_id = %mob_id,
            after_cursor,
            "the resumed mob's history ends with a Stop an earlier process placed; waiting \
             for it to land before the boot lifts the mob"
        );
        // Dropping the subscription cancels it, on every path out.
        let mut subscription = events
            .subscribe_after(after_cursor)
            .await
            .map_err(MobRuntimeError::Mob)?;
        while let Some(event) = subscription.event_rx.recv().await {
            if event.mob_id == mob_id
                && matches!(
                    event.kind,
                    meerkat_mob::MobEventKind::MobStopped
                        | meerkat_mob::MobEventKind::MobReset
                        | meerkat_mob::MobEventKind::PlacedCompletionLifecycleQuiesceEnded { .. }
                )
            {
                return Ok(());
            }
        }
        // The structural event stream ends only with the mob actor.
        Err(MobRuntimeError::Mob(MobError::ActorCommandChannelClosed))
    };
    match tokio::time::timeout(RECOVERED_PLACED_STOP_SETTLE_BOUND, settle).await {
        Ok(settled) => settled,
        // Core keeps the Stop durable and drives it again at the next start.
        Err(_) => Err(MobRuntimeError::Mob(
            MobError::LifecycleOperationProgressStalled {
                intent: "Stop".to_string(),
                member_id: None,
                stage: "boot awaiting a Stop an earlier process placed",
            },
        )),
    }
}

/// The right to lift a prepared mob from `Stopped` to `Running`, once.
///
/// A persistent log that records a completed explicit Stop replays as
/// `Stopped`; shutting down the actor alone preserves the durable mob phase.
/// Lifting a stopped mob is what makes MobMachine revive persisted sessions -
/// and revival needs the owning identity already registered, which only an
/// installed `IdentityRuntime` can do. So for an identity-first composition the
/// lift cannot happen where the mob is built; it has to wait until the identity
/// context exists.
///
/// This is that obligation made typed rather than left to a convention. It is
/// only produced when a lift is genuinely owed, it is consumed by value so it
/// cannot be discharged twice, and there is no boolean anywhere that says
/// "skip the lift".
#[must_use = "a prepared mob stays Stopped until this activation is consumed, and a Stopped mob cannot spawn"]
pub(crate) struct PendingMobActivation {
    handle: MobHandle,
    /// How long a stalled explicit resume is re-joined before activation
    /// fails. Read once, where the obligation is created.
    stall_policy: crate::mob_activation_retry::ActivationStallPolicy,
    /// Test-only resume outcomes returned, in order, instead of calling
    /// Meerkat. Once drained, the real `MobHandle::resume` runs.
    #[cfg(test)]
    injected_resume_outcomes: std::collections::VecDeque<MobError>,
}

impl PendingMobActivation {
    /// Perform the deferred `Stopped` -> `Running` transition.
    ///
    /// The host must first declare its composition. Identity compositions
    /// register current continuity owners before activation; classic
    /// compositions explicitly finalize without an identity context.
    pub(crate) async fn activate(self) -> Result<(), MobRuntimeError> {
        match self.handle.status().await? {
            // The only transition MobHandle::resume performs.
            meerkat_mob::MobState::Stopped => {
                // A stalled resume is not abandoned by Meerkat: the operation
                // stays current for this handle's command authority and a
                // later `resume` on the same handle joins it. So a stall is
                // re-joined here, within the policy, rather than failing the
                // boot and re-doing the slow work from scratch.
                let Self {
                    handle,
                    stall_policy,
                    #[cfg(test)]
                    mut injected_resume_outcomes,
                } = self;
                crate::mob_activation_retry::resume_joining_stalled_operation(stall_policy, || {
                    #[cfg(test)]
                    let injected = injected_resume_outcomes.pop_front();
                    let handle = handle.clone();
                    async move {
                        #[cfg(test)]
                        if let Some(error) = injected {
                            return Err(error);
                        }
                        handle.resume().await
                    }
                })
                .await?;
                Ok(())
            }
            // Something else already lifted it; the obligation is discharged.
            meerkat_mob::MobState::Running => Ok(()),
            state => Err(MobRuntimeError::InvalidConfig(format!(
                "a prepared mob reported state {} at activation, so it can no longer be lifted \
                 to Running",
                state.as_str()
            ))),
        }
    }

    #[cfg(test)]
    pub(crate) fn set_stall_policy_for_test(
        &mut self,
        policy: crate::mob_activation_retry::ActivationStallPolicy,
    ) {
        self.stall_policy = policy;
    }

    /// An activation obligation staged for a mob that is already running,
    /// so its lift is a no-op: a test drives the prepared bootstrap's steps
    /// before the lift without restarting a persistent mob.
    #[cfg(test)]
    pub(crate) fn already_running_for_test(handle: &MobHandle) -> Self {
        Self {
            handle: handle.observer(),
            stall_policy: crate::mob_activation_retry::ActivationStallPolicy::from_env(),
            injected_resume_outcomes: std::collections::VecDeque::new(),
        }
    }

    /// Queue resume outcomes returned before the real `MobHandle::resume`.
    #[cfg(test)]
    pub(crate) fn inject_resume_outcomes_for_test(&mut self, outcomes: Vec<MobError>) {
        self.injected_resume_outcomes.extend(outcomes);
    }
}

/// Stage a `ToolsNotPublished` run-start hold for the next registration of
/// every session `store` catalogs: what a candidate launch holds (see
/// [`MobRuntime::prepare`]). Staging only writes `machine`'s in-memory set:
/// every later registration of such a session applies the hold, and only a
/// host release lifts it, which a candidate never sends. A catalog that
/// cannot be read refuses the launch, since a candidate cannot hold what it
/// cannot list.
async fn hold_stored_sessions_for_candidate(
    machine: &meerkat_runtime::MeerkatMachine,
    store: &dyn meerkat_runtime::RuntimeStore,
) -> Result<(), MobRuntimeError> {
    let entries = meerkat_runtime::RuntimeStore::list_runtime_session_catalog_entries(
        store,
        meerkat_core::SessionFilter::default(),
    )
    .await
    .map_err(|error| {
        MobRuntimeError::Mob(MobError::Internal(format!(
            "a candidate launch holds every stored session's run starts, and the runtime \
             store's session catalog could not be read: {error}"
        )))
    })?;
    for entry in &entries {
        machine.stage_registration_run_start_hold_only(
            entry.session_id(),
            meerkat_runtime::RunStartHoldReason::ToolsNotPublished,
        );
    }
    tracing::info!(
        sessions = entries.len(),
        "candidate launch: every stored session registers with its run starts held; no run \
         the store left open resumes and no queued input runs here"
    );
    Ok(())
}

/// The options of a launch's terminal mob Shutdowns, bounded by
/// `deadline`. A candidate launch hands the store back as it found it: it
/// held every stored member and admitted nothing to them, so its Shutdown
/// keeps every member's inputs and open run exactly as the store holds them
/// (`QueuedInputsOnShutdown::Keep`). Any other launch takes Meerkat's
/// default, which keeps a durable member's work for the next start.
fn mob_shutdown_options_for(
    authority: crate::mob_composition_manifest::CompositionAuthority,
    deadline: std::time::Instant,
) -> meerkat_mob::ShutdownOptions {
    let options = meerkat_mob::ShutdownOptions::default().with_deadline(deadline);
    if authority.speaks_for_composition() {
        options
    } else {
        options.with_queued_inputs(meerkat_mob::QueuedInputsOnShutdown::Keep)
    }
}

/// Stop `handle`'s mob after a startup that failed once the mob was built,
/// returning the Stop's result for the startup error to carry. An
/// authoritative launch stops it as before. A candidate launch stops
/// nothing: a Stop, even one that leaves every run running, journals
/// `MobStopped` in the shared mob store and fails pending spawns, cancels
/// pending peer deliveries and closes kickoff origins. Its teardown is the
/// Shutdown its graceful path runs, with [`mob_shutdown_options_for`]'s
/// keep, so no member's inputs or open run change and nothing is journaled;
/// it answers an empty stop report, or the Shutdown's refusal.
async fn stop_after_failed_startup(
    handle: &MobHandle,
    authority: crate::mob_composition_manifest::CompositionAuthority,
) -> Result<meerkat_mob::MobStopReport, MobError> {
    if authority.speaks_for_composition() {
        return handle.stop().await;
    }
    let deadline =
        std::time::Instant::now() + crate::unified_runtime::lifecycle::MOB_TERMINAL_SHUTDOWN_BUDGET;
    let report = handle
        .shutdown_with_report(mob_shutdown_options_for(authority, deadline))
        .await?;
    if !report.is_clean() {
        tracing::warn!(
            members = ?report.members,
            "candidate launch: the keep shutdown after a failed startup left members it could \
             not settle"
        );
    }
    Ok(meerkat_mob::MobStopReport::default())
}

impl MobRuntime {
    /// Bootstrap a mob that is `Running` on return.
    ///
    /// The honest wrapper: any caller that gets a `MobRuntime` from here can
    /// spawn into it. Identity-first composition uses [`Self::prepare`] instead,
    /// because it must interpose registration before the lift.
    pub async fn bootstrap(spec: MobBootstrapSpec) -> Result<Self, MobRuntimeError> {
        let (runtime, pending) = Self::prepare(spec).await?;
        if let Some(pending) = pending {
            pending.activate().await?;
        }
        Ok(runtime)
    }

    /// Build the mob without committing to `Running`.
    ///
    /// Returns the runtime plus, when a lift is owed and an identity runtime
    /// slot permits late identity composition, the typed activation that the
    /// host must consume after choosing and installing its authority.
    pub(crate) async fn prepare(
        mut spec: MobBootstrapSpec,
    ) -> Result<(Self, Option<PendingMobActivation>), MobRuntimeError> {
        // Every Meta surface funnels its definition through here, so this
        // is the one ingress where explicit resume intent is normalized before
        // the definition reaches meerkat-mob.
        let raw_definition = spec.definition.clone();
        let effective_runtime_adapter = spec
            .session_service
            .acquire_runtime_adapter(spec.runtime_adapter.clone())
            .map_err(MobRuntimeError::Runtime)?;
        spec.runtime_adapter = effective_runtime_adapter.clone();
        // A candidate launch holds the run starts of every session its
        // runtime store holds before anything registers one: the mob's
        // resume, its child mobs' restore and every later wake of a stored
        // member register held, so no run the store left open resumes and no
        // queued input runs. Child mobs share this runtime, so their members
        // are held too. A candidate never releases the hold, and the hold is
        // never durable: the authoritative launch over the same store stages
        // none.
        if !spec.composition_authority.speaks_for_composition()
            && let (Some(machine), Some(store)) = (
                effective_runtime_adapter.as_deref(),
                spec.durable_runtime_store.as_deref(),
            )
        {
            hold_stored_sessions_for_candidate(machine, store).await?;
        }
        let native_work_governed = effective_runtime_adapter
            .as_ref()
            .is_some_and(|machine| machine.has_native_work_authorization_host());
        // Before anything reads the agent mob-tool state: child mobs get the
        // host's final application tool policy and child bundles.
        if let Some(install) = &spec.agent_mob_tools_install {
            let delivery = spec
                .runtime_delivery
                .as_ref()
                .ok_or(MobRuntimeError::Delivery(
                    MobRuntimeDeliveryError::MissingComposition,
                ))?;
            if !delivery.shares_composition(&install.delivery) {
                return Err(MobRuntimeError::Delivery(
                    MobRuntimeDeliveryError::CompositionChanged,
                ));
            }
        }
        if let Some(delivery) = &spec.runtime_delivery {
            delivery
                .validate_runtime(effective_runtime_adapter.as_deref())
                .map_err(MobRuntimeError::Delivery)?;
            delivery.preflight().map_err(MobRuntimeError::Delivery)?;
        }
        spec.apply_agent_mob_child_policy()?;
        #[cfg(feature = "extension-state")]
        if let Some(servers) = spec.child_mcp_servers.take() {
            let state = spec.agent_mob_mcp_state.as_ref().ok_or_else(|| {
                MobRuntimeError::InvalidConfig(
                    "document MCP child delivery requires an agent mob-tool state".into(),
                )
            })?;
            state.set_child_mcp_servers(servers);
        }
        #[cfg(feature = "extension-state")]
        if let (Some(state), Some(callback)) = (&spec.agent_mob_mcp_state, &spec.before_activation)
        {
            state
                .set_before_activation(Arc::clone(callback))
                .map_err(|error| MobRuntimeError::InvalidConfig(error.to_string()))?;
        }
        #[cfg(feature = "extension-state")]
        if let (Some(state), Some(hook)) = (&spec.agent_mob_mcp_state, &spec.after_destroy) {
            state
                .set_after_destroy(Arc::clone(hook))
                .map_err(|error| MobRuntimeError::InvalidConfig(error.to_string()))?;
        }
        #[cfg(feature = "extension-state")]
        if let (Some(state), Some(hook)) = (&spec.agent_mob_mcp_state, &spec.before_create) {
            state
                .set_before_create(Arc::clone(hook))
                .map_err(|error| MobRuntimeError::InvalidConfig(error.to_string()))?;
        }
        auto_mark_declared_resume_overrides(&mut spec.definition);
        let ephemeral_dir = spec._ephemeral_dir.clone();
        let state_dir_claims = std::mem::take(&mut spec.state_dir_claims);
        let child_mob_root_claim = spec.child_mob_root_claim.clone();
        let session_service = spec.session_service.clone();
        let member_model_config = spec.member_model_config.clone();
        let binary_blob_store = spec.binary_blob_store.clone();
        let mob_id = spec.definition.id.clone();
        let agent_mob_mcp_state = spec.agent_mob_mcp_state.clone();
        let implicit_delegate_retirement_overrides =
            spec.implicit_delegate_retirement_overrides.clone();
        let console_spawn_sink_slot = spec.console_spawn_sink_slot.clone();
        let identity_runtime_slot = spec.identity_runtime_slot.clone();
        let default_llm_client = spec
            .options
            .default_llm_client
            .clone()
            .map(ReplaySanitizingLlmClient::wrap)
            // Mob-wide default: serves every member on every provider the
            // definition resolves to (see `ProviderAgnosticLlmClient`).
            .map(ProviderAgnosticLlmClient::wrap);
        if let Some(slot) = spec.agent_mob_default_llm_client_slot.as_ref() {
            *slot
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = default_llm_client.clone();
        }

        // Create-vs-resume selection, in ONE place. An in-memory storage is
        // always empty at boot, so this single read serves both modes:
        // ephemeral launches always create, and only a persistent path that
        // already holds events resumes. No launch site needs to decide, which
        // is what stops the three launch paths from drifting apart again.
        //
        // `spec.definition` is already normalized here:
        // `auto_mark_declared_resume_overrides` ran above, so the composition
        // recorded on create and the one compared on resume are the same form.
        // Recording a pre-normalization definition would mismatch its own
        // first boot.
        let event_log_empty = spec
            .storage
            .is_event_log_empty()
            .await
            .map_err(|err| MobRuntimeError::Mob(MobError::from(err)))?;
        let manifest_location = spec.mob_storage_provenance.manifest_location();
        // Health-visible record of a candidate that knowingly boots a stored
        // definition differing from the supplied one.
        let mut candidate_composition_slot: Option<crate::storage_health::StorageSlotSummary> =
            None;
        let mut builder = if event_log_empty {
            if let Some(location) = manifest_location.as_ref() {
                // Refuse rather than leave behind a storage whose composition
                // the next restart cannot judge.
                location
                    .record_on_create(&spec.definition, spec.composition_authority)
                    .await
                    .map_err(MobRuntimeError::CompositionProvenance)?;
            }
            MobBuilder::new(spec.definition, spec.storage)
        } else {
            let read = spec.launch_definition_read.take();
            // A launch that read no definition here before preparation (an
            // empty log, or an in-process one holding none) resolved what it
            // read from the supplied definition (the identity-first roster).
            // A definition written since is not what it resolved from, so it
            // is refused rather than resumed.
            let read_no_definition = match &read {
                Some(LaunchDefinitionRead::Fresh) => true,
                Some(LaunchDefinitionRead::Resume(judged)) => judged.snapshot.is_none(),
                None => false,
            };
            if read_no_definition
                && let Some(written) = spec
                    .storage
                    .created_definition_snapshot()
                    .await
                    .map_err(|err| MobRuntimeError::Mob(MobError::from(err)))?
            {
                return Err(MobRuntimeError::Mob(
                    MobError::MobDefinitionAuthorityChanged {
                        expected_epoch: NO_STORED_DEFINITION_EPOCH,
                        expected_event_cursor: 0,
                        actual_epoch: Some(written.epoch()),
                        actual_event_cursor: Some(written.event_cursor()),
                    },
                ));
            }
            let judged = match read {
                // Judged before preparation
                // (`MobBootstrapSpec::read_launch_definition`): apply that
                // judgement, so what was resolved from its definition
                // describes the mob that runs.
                Some(LaunchDefinitionRead::Resume(judged)) => *judged,
                Some(LaunchDefinitionRead::Fresh) | None => {
                    judge_resume(
                        &spec.storage,
                        &spec.mob_storage_provenance,
                        spec.composition_authority,
                        spec.candidate_definition,
                        &spec.definition,
                        &raw_definition,
                        false,
                    )
                    .await?
                }
            };
            if judged.record_supplied_composition
                && let Some(location) = manifest_location.as_ref()
            {
                tracing::info!(
                    manifest = %location.manifest_label().display(),
                    "no composition manifest for this mob storage; adopting the supplied \
                     definition as its recorded composition"
                );
                location
                    .record_on_create(&spec.definition, spec.composition_authority)
                    .await
                    .map_err(MobRuntimeError::CompositionProvenance)?;
            }
            if !judged.candidate_diverged.is_empty() {
                tracing::warn!(
                    mob_id = %mob_id,
                    diverged_fields = %judged.candidate_diverged.join(", "),
                    "candidate launch boots the STORED mob definition, which differs from \
                     the supplied one (runtime_options.mob_composition.candidate_definition \
                     = \"stored\"); the supplied definition is not in effect"
                );
                candidate_composition_slot =
                    Some(crate::storage_health::StorageSlotSummary::degraded(
                        "mob_composition",
                        format!(
                            "candidate launch runs the stored mob definition, not the \
                             supplied one; diverged: {}",
                            judged.candidate_diverged.join(", ")
                        ),
                    ));
            }
            // Bind the builder to the judged snapshot, so a concurrent epoch
            // refuses before the mob can actuate.
            if let Some(snapshot) = judged.snapshot {
                MobBuilder::for_resume_verified(spec.storage, snapshot)
            } else {
                MobBuilder::for_resume(spec.storage)
            }
        };

        // MobActor's autonomous readiness/comms-drain path consults the
        // builder-published runtime adapter directly. For session services
        // that already embed a runtime adapter (definition-based ephemeral
        // and persistent-with-runtime-backed-service), forward that adapter
        // explicitly so autonomous members do not come up session-backed but
        // runtime-unattached.
        if let Some(adapter) = effective_runtime_adapter.clone() {
            builder = builder.with_runtime_adapter(adapter);
        }

        builder = builder
            .with_session_service(session_service.clone())
            .allow_ephemeral_sessions(spec.options.allow_ephemeral_sessions)
            // A candidate launch runs no orchestrator resume turn: the turn is
            // the authoritative launch's, which notifies as before.
            .notify_orchestrator_on_resume(
                spec.options.notify_orchestrator_on_resume
                    && spec.composition_authority.speaks_for_composition(),
            )
            // Not persisted by the mob: create and resume both apply it.
            .with_fork_instruction_policy(spec.fork_instruction_policy);

        // Every mob this runtime builds refuses runtime-plane member
        // identities, after the spec's own customizers have run.
        if let Some(customizer) = crate::member_comms_id::runtime_plane_guarded_customizer(
            &mob_id,
            spec.spawn_member_customizer.clone(),
        ) {
            builder = builder.with_spawn_member_customizer(customizer);
        }
        if let Some(hook) = spec.member_runtime_registration_hook.clone() {
            builder = builder.with_member_runtime_registration_hook(hook);
        }
        if !spec.restored_members_awaiting_tools.is_empty() {
            builder = builder.hold_restored_member_run_starts(
                meerkat_mob::HostRunStartHoldReason::ToolsNotPublished,
                spec.restored_members_awaiting_tools.iter().cloned(),
            );
        }
        if let Some(registry) = spec.tool_consequence_policy_registry.clone() {
            builder = builder.with_tool_consequence_policy_registry(registry);
        }

        if let Some(provider) = spec.default_external_tools_provider.clone() {
            builder = builder.with_default_external_tools_provider(Some(provider));
        }
        for (name, dispatcher) in &spec.tool_bundles {
            builder = builder.register_tool_bundle(name.clone(), Arc::clone(dispatcher));
        }
        #[cfg(feature = "extension-state")]
        if let Some(callback) = &spec.before_activation {
            builder = builder.before_activation(Arc::clone(callback));
        }

        // Apply-time WorkGraph attention overlays: the provisioner's
        // MobSessionRuntimeExecutor injects the scoped tool overlay before
        // apply_runtime_turn for every mob-executor turn iff the builder
        // carries the service.
        builder = builder.with_workgraph_service(spec.workgraph_service.clone());

        // Durable workers (members spawned with a durable identity: the
        // members of a `durable = true` profile, and every identity-first
        // member): before Meerkat resumes such a worker's interrupted run,
        // the gate checks over the same mob-realm service that the worker
        // still holds its WorkGraph work. Without a service there is no claim
        // to read, no gate is installed, and the run resumes unchecked. A
        // candidate launch installs none: Meerkat asks the gate before it
        // checks the run-start hold, and a superseded verdict would settle a
        // run the candidate holds for the authoritative launch.
        if let Some(gate) = crate::workgraph_resume_gate::launch_resume_gate(
            spec.workgraph_service.clone(),
            spec.composition_authority,
        ) {
            builder = builder.with_durable_resume_gate(Arc::new(gate));
        }

        if let Some(client) = default_llm_client {
            builder = builder.with_default_llm_client(client);
        }

        let mut pending_activation: Option<PendingMobActivation> = None;
        let handle = if event_log_empty {
            crate::boxed_future::boxed(|| builder.create()).await?
        } else {
            let handle = crate::boxed_future::boxed(|| builder.resume()).await?;
            // A Stop an earlier process placed and never finished is a durable
            // barrier: resume hands back a handle that still reads Running, and
            // Core's recovery finishes that Stop afterwards. The phase is read
            // once it has landed, or the boot would take the mob for running,
            // never lift it, and refuse every later send.
            settle_recovered_placed_stop(&handle).await?;
            // A completed explicit Stop records `MobStopped`, so replaying
            // that durable phase hands back a handle in `Stopped`. Actor
            // shutdown alone does not record a Stop. Identity restoration
            // cannot spawn into a stopped mob until its activation is consumed.
            //
            // `bootstrap` is an operational bootstrap: the caller asked for a
            // running mob. So lift it here, once, rather than leaving every
            // downstream actuator to discover a stopped mob and interpret it
            // for itself. Any failure stays typed.
            match handle.status().await? {
                // Defer the lift until the resolver has its handle and the
                // delivery owner is armed. Identity-first composition also
                // needs its owners registered before machine-authorized
                // revival of persisted sessions; its IdentityRuntime arrives
                // later via install_and_bootstrap_identity_first_context.
                meerkat_mob::MobState::Stopped => {
                    pending_activation = Some(PendingMobActivation {
                        // D67: the staged lift is the composition's own
                        // obligation, not a public owner. An owning handle
                        // here would keep a runtime whose activation was
                        // never consumed (a roster failure before the lift)
                        // owned forever, so its idle close could not fire.
                        handle: handle.observer(),
                        stall_policy: crate::mob_activation_retry::ActivationStallPolicy::from_env(
                        ),
                        #[cfg(test)]
                        injected_resume_outcomes: std::collections::VecDeque::new(),
                    });
                }
                meerkat_mob::MobState::Running => {}
                // Terminal upstream. Returning Ok here would hand identity
                // restore a mob it can never spawn into, which is the obscure
                // failure class this whole branch exists to remove: the comment
                // claiming a refusal has to BE the refusal.
                state @ (meerkat_mob::MobState::Completed | meerkat_mob::MobState::Destroyed) => {
                    return Err(MobRuntimeError::InvalidConfig(format!(
                        "the mob storage at this path holds a mob in terminal state {} and \
                         cannot be resumed into a running runtime (MobHandle::resume only \
                         lifts Stopped to Running); use a new mob storage path",
                        state.as_str()
                    )));
                }
                // Not a state a just-resumed handle should report. Say so
                // rather than succeeding quietly into a half-built runtime.
                meerkat_mob::MobState::Creating => {
                    return Err(MobRuntimeError::InvalidConfig(
                        "a resumed mob reported state Creating, which a replayed event log \
                         should never produce; refusing rather than installing an identity \
                         runtime against an unsettled mob"
                            .to_string(),
                    ));
                }
            }
            handle
        };
        if let Some(state) = agent_mob_mcp_state.as_ref() {
            state.mob_insert_handle(mob_id, handle.observer()).await;
            match child_mob_root_claim.as_ref() {
                // Durable child mobs: complete the native restore on the final
                // state now, before the managed set is complete and before
                // delivery arms, so a persisted child is never reported gone
                // for good while it is merely not restored yet. The restore
                // itself completes the managed set.
                Some(claim) => {
                    // From here a child actor may run, so the root stays owned
                    // for the rest of the process.
                    crate::child_mob_storage::retain_until_process_exit(claim);
                    if let Err(error) = state.mob_list().await {
                        // Children restored before the failure may still be
                        // running and are not reachable from here, so the claim
                        // stays retained for the rest of the process.
                        let stop =
                            stop_after_failed_startup(&handle, spec.composition_authority).await;
                        tracing::error!(
                            %error,
                            root_stop = ?stop,
                            child_root = %claim.root().display(),
                            "durable child mob restore failed; the child root stays owned \
                             until process exit"
                        );
                        return Err(MobRuntimeError::ChildMobStorage(
                            crate::child_mob_storage::ChildMobStorageError::Restore(error),
                        ));
                    }
                }
                // Process-bound child mobs: this state restores this spec's one
                // root handle and has no later restored-handle ingress. Other
                // hosts sharing the inbox keep custody of their own rows.
                None => state.declare_mob_set_restored(),
            }
        }
        // A candidate launch arms no runtime delivery owner; the delivery
        // records that decision as its owner state.
        let runtime_delivery = match spec.runtime_delivery {
            Some(delivery) => match delivery.arm_for_launch(
                spec.composition_authority,
                Arc::clone(&session_service),
                effective_runtime_adapter.ok_or(MobRuntimeError::Delivery(
                    MobRuntimeDeliveryError::MissingRuntime,
                ))?,
            ) {
                Ok(owner) => owner,
                Err(error) => {
                    let stop = Box::new(
                        stop_after_failed_startup(&handle, spec.composition_authority).await,
                    );
                    return Err(MobRuntimeError::DeliveryStartup {
                        error: Box::new(error),
                        stop,
                    });
                }
            },
            None => None,
        };
        // A restored classic mob starts only after its address resolver has
        // its handle and the native delivery owner has been armed. Identity
        // composition retains the activation until its owners are registered.
        if identity_runtime_slot.is_none()
            && let Some(pending) = pending_activation.take()
        {
            pending.activate().await?;
        }

        // One admission per runtime; the tool-plane dispatchers were built
        // before the mob (and thus the roster) existed, so their late-bound
        // slots are filled here with the same instance the RPC surfaces use.
        // The session service rides along as the admission's session→member
        // resolution fallback: member sessions carry their identity on
        // `session_metadata.mob_member_binding`, which co-processes sharing
        // the state dir can read even when their roster is blind.
        let workgraph_admission = Arc::new(crate::workgraph_admission::WorkGraphAdmission::new(
            handle.observer(),
            Some(Arc::clone(&session_service)),
            spec.workgraph_admission_sidecar,
        ));
        for slot in &spec.workgraph_admission_slots {
            spec.composition_release.register_slot(slot);
            *slot
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner) =
                Some(Arc::clone(&workgraph_admission));
        }
        // Member-bound attention bindings written before meerkat rescoped
        // member attention to the mob realm are moved there now, before any
        // member turn resolves its overlay. The report is exposed on
        // `meta/capabilities`; a failure is recorded there, not raised, because
        // the typed realm refusal already prevents new misplaced rows.
        let workgraph_realm_migration = match spec.workgraph_service.as_ref() {
            Some(service) => Some(Arc::new(
                crate::workgraph_realm::migrate_member_bindings_to_mob_realms(
                    service,
                    spec.workgraph_realm_migration,
                )
                .await,
            )),
            None => None,
        };
        // A member created under one of the console's runtime-plane
        // identities before every creation path refused them still comes up.
        crate::member_comms_id::warn_reserved_identity_members(&handle).await;
        Ok((
            Self {
                handle,
                runtime_delivery,
                session_service: Some(session_service),
                member_model_config,
                agent_mob_mcp_state,
                implicit_delegate_retirement_overrides,
                binary_blob_store,
                baseline_member_specs: Arc::new(tokio::sync::RwLock::new(Vec::new())),
                console_spawn_sink_slot,
                identity_runtime_slot,
                composition_release: Some(spec.composition_release),
                workgraph_service: spec.workgraph_service,
                workgraph_admission,
                workgraph_realm_migration,
                resolved_storage: match (spec.resolved_storage, candidate_composition_slot) {
                    (Some(mut summary), Some(slot)) => {
                        summary.slots.push(slot);
                        Some(summary)
                    }
                    (summary, _) => summary,
                },
                durable_runtime_store: spec.durable_runtime_store,
                committed_boundary_recoverer: spec.committed_boundary_recoverer,
                controller_pin_source: spec.controller_pin_source,
                native_work_governed,
                _ephemeral_dir: ephemeral_dir,
                state_dir_claims: Arc::new(std::sync::Mutex::new(state_dir_claims)),
                composition_authority: spec.composition_authority,
                child_mob_root: crate::child_mob_storage::ChildMobRootOwnership::new(
                    child_mob_root_claim,
                ),
                #[cfg(not(target_arch = "wasm32"))]
                member_mcp_custody: spec.member_mcp_custody,
            },
            pending_activation,
        ))
    }

    /// Adopt a handle built outside [`MobBootstrapSpec`]. Meta composes no
    /// spawn customizer into such a mob: its Core-side creation paths
    /// (definition, flow and policy members, Core's agent tools) refuse the
    /// console's runtime-plane member identities only when the handle's own
    /// builder appended
    /// [`RuntimePlaneIdentityGuard::customizer`](crate::RuntimePlaneIdentityGuard::customizer)
    /// last to its spawn customizers. Meta's creation surfaces check every
    /// name either way, and the console drops a member's events under those
    /// identities.
    pub fn from_handle(handle: MobHandle) -> Self {
        let workgraph_admission = Arc::new(crate::workgraph_admission::WorkGraphAdmission::new(
            handle.observer(),
            None,
            None,
        ));
        Self {
            // A runtime adopted from a bare handle made no declaration, so it
            // keeps the safe default: it speaks for the composition unless told
            // otherwise. A candidate must say so explicitly.
            composition_authority:
                crate::mob_composition_manifest::CompositionAuthority::Authoritative,
            handle,
            runtime_delivery: None,
            session_service: None,
            member_model_config: None,
            agent_mob_mcp_state: None,
            implicit_delegate_retirement_overrides: None,
            binary_blob_store: None,
            baseline_member_specs: Arc::new(tokio::sync::RwLock::new(Vec::new())),
            console_spawn_sink_slot: None,
            identity_runtime_slot: None,
            composition_release: None,
            workgraph_service: None,
            workgraph_admission,
            workgraph_realm_migration: None,
            resolved_storage: None,
            durable_runtime_store: None,
            committed_boundary_recoverer: None,
            controller_pin_source: None,
            native_work_governed: false,
            _ephemeral_dir: None,
            state_dir_claims: Arc::default(),
            child_mob_root: crate::child_mob_storage::ChildMobRootOwnership::default(),
            #[cfg(not(target_arch = "wasm32"))]
            member_mcp_custody: None,
        }
    }

    /// The custody this runtime's member MCP routers are composed under.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn member_mcp_custody(
        &self,
    ) -> Option<&Arc<crate::member_mcp_custody::MemberMcpCustody>> {
        self.member_mcp_custody.as_ref()
    }

    /// Whether this runtime owns a durable child-mob root.
    pub(crate) fn owns_child_mob_root(&self) -> bool {
        self.child_mob_root.owns_claim()
    }

    /// Stand down every durable child mob this runtime manages.
    ///
    /// Call it after the root mob has stopped; [`crate::UnifiedRuntime`]
    /// shutdown does. It is terminal: first the runtime's delivery owner is
    /// retired and joined within the deadline, or already stopped
    /// (`UnifiedRuntime`'s shutdown stops it, unjoined, once the mob stops),
    /// and the composition slots that point back at this runtime are
    /// emptied, as that shutdown does once the mob stops.
    /// It then closes the agent mob-tool state's admission
    /// (Meerkat's `MobMcpState::close_admission`) and waits for the creates,
    /// inserts, restores and council admissions already past their check, so
    /// the child snapshot it takes next is final. Each child then gets a
    /// terminal shutdown that keeps its durable phase, so the next runtime
    /// over the same layout restores it as it was. An unsettled child stays
    /// recorded across repeated calls; a closed actor channel is not a
    /// settlement report. A child that already shut down cleanly in the same
    /// incarnation is not asked again. A member's runtime unregister an
    /// earlier stand-down left running is followed only after every child
    /// incarnation hosted now was asked; when admission or the state did not
    /// settle, so that is not known, a later call follows it. A close sent
    /// while a mob's actor is
    /// still exiting from its shutdown can go unanswered in Meerkat 0.9.3 and
    /// earlier, so the admission close is sent again once the root's actor
    /// task ends, and each child's actor task is waited for, within the
    /// deadline, after its shutdown.
    ///
    /// It then stands the state down (`MobMcpState::stand_down`): it joins
    /// the work the state owns (council runs, completion custodians,
    /// teardown handoffs, live delegation deliveries) and aborts its loops.
    /// Only when admission and that work settled, the delivery owner was
    /// joined or already stopped (unjoined, by `UnifiedRuntime`'s shutdown),
    /// every child is clean, and no child appeared meanwhile, is the
    /// child-root claim released
    /// ([`crate::ChildMobStandDownOutcome::Released`]), so a successor over
    /// the same layout can claim it in this process; with no child at all
    /// that is the outcome too. Otherwise the root stays owned and a
    /// successor is refused with
    /// [`crate::ChildMobStorageError::AlreadyClaimed`]; a later call retries
    /// and process exit releases it.
    ///
    /// A clean stand-down also releases this runtime's claim on its state
    /// directory ([`crate::state_dir_claim`]), so the same-process successor
    /// can open it too. A `UnifiedRuntime` keeps a claim of its own until its
    /// shutdown has run to its end.
    pub async fn stand_down_child_mobs(
        &self,
        deadline: std::time::Instant,
    ) -> crate::child_mob_storage::ChildMobStandDownOutcome {
        let outcome = self.stand_down_child_mob_root(deadline).await;
        // A clean stand-down with its delivery owner retired leaves nothing of
        // this runtime using the state directory. Retiring again only reads
        // the owner the stand-down already stopped.
        if outcome.cleanup_complete() && self.retire_runtime_delivery(deadline).await.is_ok() {
            self.release_state_dir_claims();
        }
        outcome
    }

    /// The child-root half of [`Self::stand_down_child_mobs`].
    async fn stand_down_child_mob_root(
        &self,
        deadline: std::time::Instant,
    ) -> crate::child_mob_storage::ChildMobStandDownOutcome {
        // A stand-down is terminal, as `UnifiedRuntime`'s shutdown is: the
        // delivery owner retires and the composition slots that point back
        // at this runtime empty before the child root can be released. A
        // runtime torn down without `UnifiedRuntime` has nothing else that
        // stops them, and its owner kept reconciling over the runtime store
        // of a released root. The owner is joined, within the deadline: a
        // pass already in blocking store work finishes before the release,
        // and an owner still running at the deadline keeps the root owned.
        // Both are idempotent. `UnifiedRuntime`'s shutdown stopped the owner
        // already, without joining it, so here there is nothing left to join.
        let delivery_retired = self.retire_runtime_delivery(deadline).await;
        self.release_composition_slots();
        // A root already released (or never claimed) answers at once.
        if let Some(unclaimed) = self.child_mob_root.unclaimed_outcome() {
            return unclaimed;
        }
        // One stand-down at a time through every clone; a retry that waited
        // here sees the result of the one before it. The wait is bounded by
        // this call's deadline and never cancels the running stand-down.
        let Some(_stand_down) = self.child_mob_root.begin_stand_down(deadline).await else {
            tracing::warn!(
                "another stand-down of the durable child root was still running at this \
                 call's deadline; the child root stays owned"
            );
            return crate::child_mob_storage::ChildMobStandDownOutcome::InProgress;
        };
        if let Some(unclaimed) = self.child_mob_root.unclaimed_outcome() {
            return unclaimed;
        }
        let Some(state) = self.agent_mob_mcp_state.as_ref() else {
            // A claim is only taken for an installed agent mob-tool state.
            return self.child_mob_root.outcome(0);
        };
        // Admission closes before the snapshot: no caller admitted before
        // the close can add a child after it.
        let admission = self.close_state_admission(state, deadline).await;
        let mut stood_down = std::collections::BTreeSet::new();
        let mut child_mobs = 0_usize;
        if let Err(unsettled) = self
            .stand_down_new_children(state, deadline, &mut stood_down, &mut child_mobs)
            .await
        {
            return unsettled;
        }
        // Every child has been asked: join the work the state itself owns.
        let state_stand_down = state.stand_down(deadline).await;
        if state_stand_down.is_clean() {
            // The state is sealed: a child registered by its owned work
            // before the seal (a teardown) is shut down here.
            if let Err(unsettled) = self
                .stand_down_new_children(state, deadline, &mut stood_down, &mut child_mobs)
                .await
            {
                return unsettled;
            }
        }
        // A member unregister an earlier stand-down left running may have
        // settled since. It is followed only once every child incarnation
        // hosted now was asked: admission closed with nothing admitted still
        // running, and the state sealed, so both passes saw the final set.
        // An earlier incarnation's settlement then never runs while a later
        // incarnation of the same child may still hold that member's
        // registration; otherwise it is left for a later call. Meerkat's
        // settlement already leaves a registration a later owner took over
        // alone (it reports it handed over); this order does not rely on
        // that alone.
        if admission.is_clean() && state_stand_down.is_clean() {
            self.child_mob_root
                .settle_pending_unregisters(deadline)
                .await;
        }
        let outcome = self.child_mob_root.outcome(child_mobs);
        if !outcome.child_actors_settled()
            || !admission.is_clean()
            || !state_stand_down.is_clean()
            || delivery_retired.is_err()
        {
            tracing::warn!(
                ?outcome,
                ?admission,
                ?state_stand_down,
                ?delivery_retired,
                "durable child mobs, the agent mob-tool state or the runtime delivery owner \
                 did not all settle; the child root stays owned"
            );
            return outcome;
        }
        let released = self.child_mob_root.release(child_mobs);
        if !released.cleanup_complete() {
            tracing::warn!(
                ?released,
                "another holder of the child-root claim is still alive; the child root stays \
                 owned"
            );
        }
        released
    }

    /// Close the agent mob-tool state's admission
    /// (`MobMcpState::close_admission`), and close it again if the root mob's
    /// actor task ends while the first close still waits.
    ///
    /// The root has stopped, but its actor may still be dropping its command
    /// channel. Meerkat 0.9.3 and earlier drop it without waiting for a send
    /// already under way, so the root's close can stay queued with no reply
    /// and hold the stand-down to its deadline. Once the task has ended, the
    /// channel refuses a new close at once, which the state counts as closed.
    /// A live root answers the first close. The close is idempotent, and a
    /// close left waiting is abandoned, not undone.
    async fn close_state_admission(
        &self,
        state: &meerkat_mob_mcp::MobMcpState,
        deadline: std::time::Instant,
    ) -> meerkat_mob_mcp::MobStateStandDown {
        let mut root_actor = self.handle.actor_task_watch();
        {
            let close = state.close_admission(deadline);
            tokio::pin!(close);
            tokio::select! {
                biased;
                admission = &mut close => return admission,
                _ = root_actor.wait() => {}
            }
        }
        state.close_admission(deadline).await
    }

    /// Shut down every child mob in the state's snapshot not yet in
    /// `stood_down`, recording each answer. A child already clean, or owing
    /// a pending unregister, in the same incarnation is counted without being
    /// asked again.
    async fn stand_down_new_children(
        &self,
        state: &meerkat_mob_mcp::MobMcpState,
        deadline: std::time::Instant,
        stood_down: &mut std::collections::BTreeSet<meerkat_mob::MobId>,
        child_mobs: &mut usize,
    ) -> Result<(), crate::child_mob_storage::ChildMobStandDownOutcome> {
        use crate::child_mob_storage::{ChildIncarnation, ChildMobStandDownOutcome, ChildShutdown};
        let root_mob_id = self.handle.definition().id.clone();
        let enumeration_failed = |error: MobError| ChildMobStandDownOutcome::EnumerationFailed {
            detail: format!("child mobs could not be enumerated: {error}"),
        };
        // Every managed child, a destroyed one still managed included (the
        // retry anchor of a destroy whose native part is incomplete): its
        // actor may still run. Handle snapshots withhold destroyed mobs, so
        // the list names them and each handle is looked up.
        let children = state.mob_list().await.map_err(enumeration_failed)?;
        for (mob_id, _) in children {
            if mob_id == root_mob_id || !stood_down.insert(mob_id.clone()) {
                continue;
            }
            let handle = match state.handle_for(&mob_id).await {
                Ok(handle) => handle,
                // Removed since the list: its destroy completed.
                Err(MobError::MobNotFound(_)) => continue,
                Err(error) => return Err(enumeration_failed(error)),
            };
            *child_mobs += 1;
            let incarnation = ChildIncarnation::of(&handle);
            if self.child_mob_root.clean_in(mob_id.as_ref(), &incarnation)
                || self
                    .child_mob_root
                    .pending_in(mob_id.as_ref(), &incarnation)
            {
                continue;
            }
            let shutdown = handle
                .shutdown_with_report(self.mob_shutdown_options(deadline))
                .await;
            if shutdown.is_ok() {
                // The child's actor drops its command channel right after it
                // answers, and the state's stand-down sends every managed
                // child a close. Meerkat 0.9.3 and earlier can leave a
                // command sent into that drop unanswered: wait for the task
                // to end, so the close is refused at once.
                let _ = tokio::time::timeout_at(
                    tokio::time::Instant::from_std(deadline),
                    handle.actor_task_watch().wait(),
                )
                .await;
            }
            let outcome = match shutdown {
                Ok(report) if report.is_clean() => ChildShutdown::Clean,
                Ok(report) => unsettled_child_report(&report, deadline).await,
                Err(MobError::ActorCommandChannelClosed | MobError::ActorReplyChannelClosed) => {
                    ChildShutdown::ChannelClosed
                }
                Err(error) => ChildShutdown::Unsettled(error.to_string()),
            };
            self.child_mob_root
                .record(mob_id.as_ref(), incarnation, outcome);
        }
        Ok(())
    }

    /// Stop the native delivery owner now, even while clones of this runtime
    /// live. Teardown calls it after the mob stops; undelivered rows stay
    /// pending in the durable inbox for the next owner. It aborts the owner's
    /// task without joining it, so a pass already in blocking store work may
    /// still finish after this returns.
    pub fn stop_runtime_delivery(&self) {
        if let Some(delivery) = self.runtime_delivery.as_ref() {
            delivery.stop();
        }
    }

    /// Retire the native delivery owner and wait for it until `deadline`: no
    /// pass starts after the request, and one already running finishes
    /// first, so nothing it does outlives the call. `Err` when it was still
    /// running at the deadline; a later call joins the same owner. A runtime
    /// without delivery, or whose owner already stopped, answers `Ok` at
    /// once (that owner is not joined).
    pub(crate) async fn retire_runtime_delivery(
        &self,
        deadline: std::time::Instant,
    ) -> Result<(), meerkat::RuntimeDeliveryOwnerRetirementIncomplete> {
        match self.runtime_delivery.as_ref() {
            Some(delivery) => {
                delivery
                    .retire(deadline.saturating_duration_since(std::time::Instant::now()))
                    .await
            }
            None => Ok(()),
        }
    }

    /// Empty the builder tool slots that point back at this runtime once the
    /// mob is stopped. Their contents retain this runtime's session service,
    /// machine and mob handle (and so a host's native grant owner), while the
    /// slots live in that service's builder. Without this the whole
    /// composition outlives shutdown and drop.
    pub(crate) fn release_composition_slots(&self) {
        if let Some(release) = self.composition_release.as_ref() {
            release.release();
        }
    }

    /// Release this runtime's state-directory claims after a clean
    /// stand-down, so a successor in this process can open the directory
    /// without waiting for every clone of this runtime to drop.
    fn release_state_dir_claims(&self) {
        self.state_dir_claims
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
    }

    /// The state-directory claims this runtime still holds, for an owner
    /// that must keep the directory claimed past this runtime's stand-down.
    pub(crate) fn state_dir_claim_refs(&self) -> Vec<Arc<crate::state_dir_claim::StateDirClaim>> {
        self.state_dir_claims
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Whether this runtime still holds a state-directory claim: it has one
    /// and no clean stand-down has released it.
    pub(crate) fn holds_state_dir_claims(&self) -> bool {
        !self
            .state_dir_claims
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty()
    }

    pub fn runtime_delivery_pass(&self) -> Option<meerkat::RuntimeDeliveryPass> {
        self.runtime_delivery
            .as_ref()
            .map(|owner| owner.last_pass())
    }

    /// The announcer a host uses to tell this runtime's continuation
    /// delivery that the custody its bound sources read may have changed
    /// (see [`ContinuationSourceAnnouncer`]).
    ///
    /// `Some` means this runtime was composed with a delivery that supplied
    /// an announcer, not that its delivery owner is running now: the runtime
    /// keeps its delivery after [`Self::stop_runtime_delivery`] and still
    /// returns one. `None` when the runtime has no delivery composition, or
    /// when its launch, a candidate, armed no delivery owner.
    pub fn continuation_source_announcer(&self) -> Option<ContinuationSourceAnnouncer> {
        self.runtime_delivery
            .as_ref()
            .map(|delivery| delivery.source_announcer())
    }

    /// Whether this launch speaks for the durable composition.
    ///
    /// Gates two things, and the second is the one that is easy to forget: the
    /// composition pin, and whether this launch may AUTHOR durable state. A
    /// certification pass that adopted declarations would write them under a
    /// composition no pin will ever cover, which is the hole the pin exemption
    /// would otherwise open.
    #[must_use]
    pub fn speaks_for_composition(&self) -> bool {
        self.composition_authority.speaks_for_composition()
    }

    /// The options of this runtime's terminal mob Shutdowns (the root mob's
    /// and its child mobs'), bounded by `deadline`: see
    /// [`mob_shutdown_options_for`].
    pub(crate) fn mob_shutdown_options(
        &self,
        deadline: std::time::Instant,
    ) -> meerkat_mob::ShutdownOptions {
        mob_shutdown_options_for(self.composition_authority, deadline)
    }

    /// Stop the mob after a startup that failed once this runtime was
    /// built: see [`stop_after_failed_startup`].
    pub(crate) async fn stop_after_failed_startup(
        &self,
    ) -> Result<meerkat_mob::MobStopReport, MobError> {
        stop_after_failed_startup(&self.handle, self.composition_authority).await
    }

    pub fn handle(&self) -> MobHandle {
        self.handle.clone()
    }

    /// The same runtime over a non-owning observer handle (D67): holders that
    /// belong to the composition itself (its tasks, routers, bridges and tool
    /// state) use this, so only the public owner and the clones taken from it
    /// keep the mob publicly owned.
    pub fn observer(&self) -> Self {
        let mut observer = self.clone();
        observer.handle = self.handle.observer();
        observer
    }

    pub fn agent_mob_mcp_state(&self) -> Option<Arc<meerkat_mob_mcp::MobMcpState>> {
        self.agent_mob_mcp_state.clone()
    }

    /// The realm-scoped WorkGraph service the runtime was bootstrapped with.
    pub fn workgraph_service(&self) -> Option<meerkat::WorkGraphService> {
        self.workgraph_service.clone()
    }

    /// Report of the bootstrap-time migration of member-bound attention
    /// bindings into their mob realm, when a WorkGraph service is configured.
    pub fn workgraph_realm_migration(
        &self,
    ) -> Option<Arc<crate::workgraph_realm::WorkGraphRealmMigrationReport>> {
        self.workgraph_realm_migration.clone()
    }

    /// The runtime-wide admission authority for the workgraph
    /// duplicate-binding guards. Clones share the underlying instance, so
    /// every surface built from (a clone of) this runtime serializes against
    /// the same gate (and, for SQLite-backed stores, the same cross-process
    /// sidecar).
    pub(crate) fn workgraph_admission(
        &self,
    ) -> Arc<crate::workgraph_admission::WorkGraphAdmission> {
        Arc::clone(&self.workgraph_admission)
    }

    /// Install the console sink that agent-tool spawns project into. A no-op
    /// for runtimes built without agent mob tools (no slot to fill).
    pub(crate) fn install_console_spawn_sink(&self, sink: ConsoleSpawnSink) {
        if let Some(slot) = self.console_spawn_sink_slot.as_ref() {
            *slot
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(sink);
        }
    }

    /// The installed console spawn sink, if any.
    pub(crate) fn console_spawn_sink(&self) -> Option<ConsoleSpawnSink> {
        self.console_spawn_sink_slot.as_ref().and_then(|slot| {
            slot.read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        })
    }

    pub(crate) fn install_identity_runtime_authority(
        &self,
        identity_runtime: Arc<crate::identity_first::IdentityRuntime>,
    ) {
        if let Some(slot) = self.identity_runtime_slot.as_ref() {
            *slot
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(identity_runtime);
        }
    }

    /// The installed identity authority, if any.
    pub(crate) fn installed_identity_runtime(
        &self,
    ) -> Option<Arc<crate::identity_first::IdentityRuntime>> {
        self.identity_runtime_slot.as_ref().and_then(|slot| {
            slot.read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        })
    }

    /// Remove the late-bound identity authority after the mob has fully
    /// quiesced. IdentityRuntime owns a MobHandle in its runtime services, so
    /// retaining the reverse Arc here would form a shutdown cycle and keep
    /// persistent controller locks alive after failed construction.
    pub(crate) fn clear_identity_runtime_authority(&self) {
        if let Some(slot) = self.identity_runtime_slot.as_ref() {
            *slot
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        }
    }

    /// Console identity metadata registered by agent-tool spawns, keyed by
    /// console identity. Empty when no console sink is installed.
    pub(crate) async fn console_identity_labels(
        &self,
    ) -> BTreeMap<String, BTreeMap<String, String>> {
        match self.console_spawn_sink() {
            Some(sink) => sink.identity_labels_snapshot().await,
            None => BTreeMap::new(),
        }
    }

    pub(crate) fn implicit_delegate_retirement_overrides(
        &self,
    ) -> Option<ImplicitDelegateRetirementOverrides> {
        self.implicit_delegate_retirement_overrides.clone()
    }

    pub async fn set_baseline_member_specs(&self, specs: Vec<SpawnMemberSpec>) {
        *self.baseline_member_specs.write().await = specs;
    }

    pub async fn baseline_member_specs(&self) -> Vec<SpawnMemberSpec> {
        self.baseline_member_specs.read().await.clone()
    }

    /// The session-boundary authority Core holds for `session_id` in the
    /// composition's durable runtime store, when that store answers it as a
    /// bounded read. Equal answers prove the session's committed boundary did
    /// not move in between. `None` (no durable store, an unbounded read, a
    /// failed read, or no authority yet) means callers must read the session.
    pub(crate) async fn session_boundary_authority(
        &self,
        session_id_str: &str,
    ) -> Option<meerkat_runtime::RuntimeSessionAuthority> {
        let store = self.durable_runtime_store.as_ref()?;
        if store.session_boundary_authority_read_cost()
            != meerkat_runtime::store::RuntimeSessionAuthorityReadCost::Bounded
        {
            return None;
        }
        let session_id = meerkat_core::types::SessionId::parse(session_id_str).ok()?;
        let runtime_id = meerkat_runtime::LogicalRuntimeId::for_session(&session_id);
        store
            .load_session_boundary_authority(&runtime_id)
            .await
            .ok()
            .flatten()
    }

    pub async fn read_session_history(
        &self,
        session_id_str: &str,
        offset: usize,
        limit: Option<usize>,
    ) -> Result<SessionHistoryPage, MobRuntimeError> {
        if session_id_str.trim().is_empty() {
            return Err(MobRuntimeError::InvalidInput(
                "session_id must not be empty",
            ));
        }
        let Some(session_service) = self.session_service.as_ref() else {
            return Err(MobRuntimeError::InvalidInput(
                "session history unavailable for this runtime",
            ));
        };
        let session_id = meerkat_core::types::SessionId::parse(session_id_str)
            .map_err(|_| MobRuntimeError::InvalidInput("invalid session_id format"))?;
        SessionServiceHistoryExt::read_history(
            session_service.as_ref(),
            &session_id,
            SessionHistoryQuery { offset, limit },
        )
        .await
        .map_err(|err| MobRuntimeError::Mob(MobError::Internal(err.to_string())))
    }

    /// Read host-only observations from the exact native session owner.
    /// These records supply display data, never permission for new tool IO.
    pub async fn read_tool_application_observations(
        &self,
        session_id_str: &str,
    ) -> Result<Vec<meerkat_core::ToolApplicationObservation>, MobRuntimeError> {
        let service = self
            .session_service
            .as_ref()
            .ok_or(MobRuntimeError::InvalidInput(
                "tool application observations unavailable for this runtime",
            ))?;
        let session_id = meerkat_core::SessionId::parse(session_id_str)
            .map_err(|_| MobRuntimeError::InvalidInput("invalid session_id format"))?;
        service
            .read_tool_application_observations(&session_id)
            .await
            .map_err(|error| MobRuntimeError::Mob(MobError::Internal(error.to_string())))
    }

    /// Observe whether an exact previous append attempt can still be applying.
    /// This projects existing runtime authority; it does not advance an input.
    pub(crate) async fn settled_notice_attempts(
        &self,
        session_id_str: &str,
        attempts: &BTreeSet<(String, String)>,
    ) -> BTreeSet<(String, String)> {
        use meerkat_runtime::input_state::InputLifecycleState;
        use meerkat_runtime::service_ext::SessionServiceRuntimeExt;
        let mut settled = BTreeSet::new();
        let Ok(session_id) = meerkat_core::types::SessionId::parse(session_id_str) else {
            return settled;
        };
        // The hot driver may already reflect a transition whose persistence
        // failed. Only the composition's actual durable store grants negative
        // evidence here. Custom/ephemeral runtimes without it keep provisional
        // notices until an exact discard event or positive history arrives.
        let Some(store) = self.durable_runtime_store.as_ref() else {
            return settled;
        };
        let runtime_id = meerkat_runtime::LogicalRuntimeId::for_session(&session_id);
        let adapter = self
            .session_service
            .as_ref()
            .and_then(|service| observed_runtime_adapter(service.as_ref()));
        for (run_id, input_id) in attempts {
            let Ok(input_uuid) = uuid::Uuid::parse_str(input_id) else {
                continue;
            };
            let input = meerkat_core::lifecycle::InputId(input_uuid);
            let Ok(Some(state)) = store.load_input_state(&runtime_id, &input).await else {
                continue;
            };
            if state.state.input_id != input {
                continue;
            }
            let same_attempt = state
                .seed
                .last_run_id
                .as_ref()
                .is_some_and(|run| run.to_string() == *run_id);
            let newer_attempt = state.seed.last_run_id.is_some() && !same_attempt;
            let queued = state.seed.phase == InputLifecycleState::Queued;
            let terminal = matches!(
                state.seed.phase,
                InputLifecycleState::Consumed
                    | InputLifecycleState::Superseded
                    | InputLifecycleState::Coalesced
                    | InputLifecycleState::Abandoned
            );
            let finalized = if terminal && let Some(adapter) = adapter.as_ref() {
                matches!(
                    adapter.input_terminal_completion(&session_id, &input).await,
                    Ok(Some(_))
                )
            } else {
                false
            };
            if newer_attempt || queued || finalized {
                settled.insert((run_id.clone(), input_id.clone()));
            }
        }
        settled
    }

    /// Whether the composition's durable runtime store holds an input of
    /// `session_id` short of a terminal phase (accepted, queued, staged or
    /// applied): work the session owes even while no live runtime serves it.
    /// `None` without a durable store, when the read fails, or when a row does
    /// not decode: inconclusive, never a guessed `false`.
    pub(crate) async fn session_owes_durable_input(
        &self,
        session_id: &meerkat_core::types::SessionId,
    ) -> Option<bool> {
        let store = self.durable_runtime_store.as_ref()?;
        let runtime_id = meerkat_runtime::LogicalRuntimeId::for_session(session_id);
        let rows = store.load_input_states(&runtime_id).await.ok()?;
        durable_inputs_owe_work(&rows)
    }

    /// Whether a session holds run input its runtime has taken up but not yet
    /// committed at a run boundary: meerkat's
    /// `MeerkatMachine::session_has_uncommitted_run_input`, read within
    /// [`SESSION_COMMIT_PENDING_READ_BOUND`].
    ///
    /// - `Some(true)`: run input is staged, applied or pending consumption.
    /// - `Some(false)`: durability is ready, every input of the session's
    ///   current runtime driver was read, and none is. Queued input that has
    ///   not started does not count. It is not a promise that no later run
    ///   starts.
    /// - `None`: inconclusive, never a guessed `false`. No runtime machine, a
    ///   session id that does not parse, every meerkat error (no runtime holds
    ///   the session, its driver was replaced during the read, degraded
    ///   durability, an input without its phase), and a read that did not
    ///   answer within the bound. The read waits for the session driver, which
    ///   a boundary commit holds until it lands, so a timeout usually means a
    ///   commit is in flight.
    pub async fn session_commit_pending(&self, session_id: &str) -> Option<bool> {
        let runtime_adapter = observed_runtime_adapter(self.session_service.as_ref()?.as_ref())?;
        let session_id = meerkat_core::types::SessionId::parse(session_id).ok()?;
        bounded_commit_pending(
            runtime_adapter.session_has_uncommitted_run_input(&session_id),
            SESSION_COMMIT_PENDING_READ_BOUND,
        )
        .await
    }

    /// Whether `session_id` has any non-terminal input: queued behind the
    /// current run, staged, or applied and awaiting its boundary commit.
    /// `None` when the runtime adapter does not answer within the bound
    /// (inconclusive, never a guessed `false`).
    pub async fn session_has_active_inputs(&self, session_id: &str) -> Option<bool> {
        let runtime_adapter = observed_runtime_adapter(self.session_service.as_ref()?.as_ref())?;
        let session_id = meerkat_core::types::SessionId::parse(session_id).ok()?;
        bounded_commit_pending(
            async {
                meerkat_runtime::service_ext::SessionServiceRuntimeExt::list_active_inputs(
                    runtime_adapter.as_ref(),
                    &session_id,
                )
                .await
                .map(|inputs| !inputs.is_empty())
            },
            SESSION_COMMIT_PENDING_READ_BOUND,
        )
        .await
    }

    #[allow(dead_code)]
    pub(crate) async fn runtime_state_for_session(
        &self,
        session_id_str: &str,
    ) -> Result<Option<meerkat_runtime::RuntimeState>, MobRuntimeError> {
        if session_id_str.trim().is_empty() {
            return Err(MobRuntimeError::InvalidInput(
                "session_id must not be empty",
            ));
        }
        let Some(session_service) = self.session_service.as_ref() else {
            return Ok(None);
        };
        let Some(runtime_adapter) = session_service
            .acquire_runtime_adapter(None)
            .map_err(MobRuntimeError::Runtime)?
        else {
            return Ok(None);
        };
        let session_id = meerkat_core::types::SessionId::parse(session_id_str)
            .map_err(|_| MobRuntimeError::InvalidInput("invalid session_id format"))?;
        let state = meerkat_runtime::service_ext::SessionServiceRuntimeExt::runtime_state(
            runtime_adapter.as_ref(),
            &session_id,
        )
        .await
        .map_err(|err| MobRuntimeError::Mob(MobError::Internal(err.to_string())))?;
        Ok(Some(state))
    }

    #[allow(dead_code)]
    pub(crate) async fn comms_runtime_for_session(
        &self,
        session_id_str: &str,
    ) -> Result<Option<Arc<dyn CommsRuntime>>, MobRuntimeError> {
        if session_id_str.trim().is_empty() {
            return Err(MobRuntimeError::InvalidInput(
                "session_id must not be empty",
            ));
        }
        let Some(session_service) = self.session_service.as_ref() else {
            return Ok(None);
        };
        let session_id = meerkat_core::types::SessionId::parse(session_id_str)
            .map_err(|_| MobRuntimeError::InvalidInput("invalid session_id format"))?;
        Ok(
            meerkat_core::service::SessionServiceCommsExt::comms_runtime(
                session_service.as_ref(),
                &session_id,
            )
            .await,
        )
    }

    #[allow(dead_code)]
    pub(crate) async fn active_input_ids_for_session(
        &self,
        session_id_str: &str,
    ) -> Result<Option<Vec<String>>, MobRuntimeError> {
        if session_id_str.trim().is_empty() {
            return Err(MobRuntimeError::InvalidInput(
                "session_id must not be empty",
            ));
        }
        let Some(session_service) = self.session_service.as_ref() else {
            return Ok(None);
        };
        let Some(runtime_adapter) = session_service
            .acquire_runtime_adapter(None)
            .map_err(MobRuntimeError::Runtime)?
        else {
            return Ok(None);
        };
        let session_id = meerkat_core::types::SessionId::parse(session_id_str)
            .map_err(|_| MobRuntimeError::InvalidInput("invalid session_id format"))?;
        let input_ids = meerkat_runtime::service_ext::SessionServiceRuntimeExt::list_active_inputs(
            runtime_adapter.as_ref(),
            &session_id,
        )
        .await
        .map_err(|err| MobRuntimeError::Mob(MobError::Internal(err.to_string())))?;
        Ok(Some(
            input_ids.into_iter().map(|id| id.to_string()).collect(),
        ))
    }

    #[allow(dead_code)]
    pub(crate) async fn ensure_comms_drain_for_session(
        &self,
        session_id_str: &str,
    ) -> Result<Option<bool>, MobRuntimeError> {
        if session_id_str.trim().is_empty() {
            return Err(MobRuntimeError::InvalidInput(
                "session_id must not be empty",
            ));
        }
        let Some(session_service) = self.session_service.as_ref() else {
            return Ok(None);
        };
        let Some(runtime_adapter) = session_service
            .acquire_runtime_adapter(None)
            .map_err(MobRuntimeError::Runtime)?
        else {
            return Ok(None);
        };
        let session_id = meerkat_core::types::SessionId::parse(session_id_str)
            .map_err(|_| MobRuntimeError::InvalidInput("invalid session_id format"))?;
        let comms_runtime = meerkat_core::service::SessionServiceCommsExt::comms_runtime(
            session_service.as_ref(),
            &session_id,
        )
        .await;
        if let Some(comms) = comms_runtime {
            let _handle = meerkat_runtime::comms_drain::spawn_comms_drain(
                runtime_adapter.clone(),
                session_id,
                comms,
                None,
            );
            Ok(Some(true))
        } else {
            Ok(Some(false))
        }
    }

    /// Access the session service this runtime was bootstrapped with, if any.
    ///
    /// Present for `MobRuntime::bootstrap(...)`-produced runtimes; `None` for
    /// `MobRuntime::from_handle(...)`. HTTP handlers that need to read session
    /// history reach through this accessor.
    pub fn session_service(&self) -> Option<&Arc<dyn MobSessionService>> {
        self.session_service.as_ref()
    }

    /// The host config this runtime's member builder resolves models with,
    /// when the bootstrap spec recorded it
    /// ([`MobBootstrapSpec::with_member_model_config`]): the image checks'
    /// source for self-hosted and config-declared models.
    pub fn member_model_config(&self) -> Option<Arc<meerkat::Config>> {
        self.member_model_config.clone()
    }

    /// Access the committed-boundary heal authority recorded at bootstrap,
    /// if any (2026-07-29 incident: injected into the identity-first session
    /// bridge so the continuity repair supervisor heals for real).
    pub fn committed_boundary_recoverer(
        &self,
    ) -> Option<Arc<dyn crate::identity_first::bridge::CommittedBoundaryRecoverer>> {
        self.committed_boundary_recoverer.clone()
    }

    /// The concrete session owner's controller pin operation recorded at
    /// bootstrap, if any, for governed binding through the session bridge.
    pub fn controller_pin_source(
        &self,
    ) -> Option<Arc<dyn crate::identity_first::bridge::ControllerPinSource>> {
        self.controller_pin_source.clone()
    }

    /// Whether this runtime's machine has a host's native work authorization
    /// owner installed: its members admit only governed input. A composition
    /// observation, never a permission decision.
    pub fn native_work_governed(&self) -> bool {
        self.native_work_governed
    }

    pub fn binary_blob_store(&self) -> Option<Arc<dyn BinaryBlobStore>> {
        self.binary_blob_store.clone()
    }

    /// Composition-time storage durability resolution carried over from the
    /// bootstrap spec. `None` for externally-composed specs that did not
    /// declare it (see [`MobBootstrapSpec::resolved_storage`]).
    pub fn resolved_storage(&self) -> Option<ResolvedStorageSummary> {
        self.resolved_storage.clone()
    }
}

/// A child's shutdown report that names unsettled members. When every one is
/// a runtime unregister still running at Shutdown's deadline, their
/// settlements are followed until `deadline` (this stand-down's own): all
/// settled is a clean shutdown (a registration a later owner took over
/// counts as settled, see `PendingUnregisters::of`), a failed one is
/// specific debt, and any still running is a pending unregister a later
/// stand-down follows. Any other unsettled member is specific debt.
async fn unsettled_child_report(
    report: &meerkat_mob::MobShutdownReport,
    deadline: std::time::Instant,
) -> crate::child_mob_storage::ChildShutdown {
    use crate::child_mob_storage::{ChildShutdown, PendingUnregisters};
    // Only the members the native report names as not clean.
    let detail = format!(
        "members not settled: {:?}",
        report
            .members
            .iter()
            .filter(|(_, outcome)| !outcome.is_clean())
            .collect::<Vec<_>>()
    );
    let mut settlements = Vec::new();
    for outcome in report.members.values() {
        match outcome {
            outcome if outcome.is_clean() => {}
            meerkat_mob::MemberShutdownOutcome::UnregisterPending { settlement, .. } => {
                settlements.push(settlement.clone());
            }
            _ => return ChildShutdown::Unsettled(detail),
        }
    }
    let states = futures::future::join_all(
        settlements
            .iter_mut()
            .map(|settlement| settlement.settle_within(deadline)),
    )
    .await;
    match PendingUnregisters::of(&states) {
        PendingUnregisters::Settled => ChildShutdown::Clean,
        PendingUnregisters::Failed(failure) => {
            ChildShutdown::Unsettled(format!("{detail}; {failure}"))
        }
        PendingUnregisters::Pending => ChildShutdown::UnregisterPending {
            detail,
            settlements,
        },
    }
}

/// Project a meerkat `MobMemberListEntry` into meta's HTTP JSON shape.
///
/// Aligns with meerkat 0.6's lightweight-roster design: list entries do
/// not carry a bridge `session_id`. Callers needing the realtime session
/// for a member must use `meta/member_status`, which serializes
/// `MobMemberSnapshot.current_session_id` natively.
pub fn member_entry_to_json(entry: &meerkat_mob::runtime::MobMemberListEntry) -> serde_json::Value {
    let mut value = serde_json::to_value(entry).unwrap_or(serde_json::Value::Null);
    // Wire egress speaks the public alias space: roster ids are comms-safe
    // encodings (meerkat 0.7 MemberCommsName forbids `:` in member ids);
    // decode them back to the aliases consoles/SDKs address members by.
    if let Some(object) = value.as_object_mut() {
        if let Some(serde_json::Value::String(id)) = object.get_mut("agent_identity") {
            *id = crate::member_comms_id::runtime_alias_str(id).into_owned();
        }
        if let Some(serde_json::Value::Array(peers)) = object.get_mut("wired_to") {
            for peer in peers {
                if let serde_json::Value::String(peer_id) = peer {
                    *peer_id = crate::member_comms_id::runtime_alias_str(peer_id).into_owned();
                }
            }
        }
        // meerkat 0.7 replaced the roster-owned `state: MemberState` with the
        // machine-projected `status: MobMemberStatus`. Meta's wire contract
        // (and the published SDKs — Python `MemberSnapshot.from_dict` indexes
        // `data["state"]`) keeps the `state` key, so project `status` back
        // into the console state vocabulary alongside it.
        object.insert(
            "state".to_string(),
            serde_json::Value::String(member_status_state_string(entry.status)),
        );
    }
    value
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ResolvedToolsSnapshot {
    pub identity: String,
    pub session_id: String,
    pub tools: Vec<String>,
}

/// The member session's resolved tools: those its next model request
/// carries, as meerkat's catalog view projects them from the session's live
/// tool dispatcher under its active filters. The session's tool scope keeps
/// the base set its last model call refreshed, so tools published since (a
/// restored member's customizer tools, published after meerkat built the
/// member on an unclean stop's resume) would show there only once the member
/// has run. A service that reports no catalog view answers the tool scope.
pub async fn resolved_tools_for_session(
    session_service: Option<&Arc<dyn MobSessionService>>,
    identity: &str,
    session_id: meerkat_core::types::SessionId,
) -> Result<ResolvedToolsSnapshot, MobRuntimeError> {
    let Some(session_service) = session_service else {
        return Err(MobRuntimeError::InvalidInput(
            "resolved tools unavailable for this runtime",
        ));
    };
    let view = session_service
        .tool_catalog_view(&session_id)
        .await
        .map_err(|err| MobRuntimeError::Mob(MobError::Internal(err.to_string())))?;
    let mut tools = match view {
        Some(view) => view
            .entries
            .into_iter()
            .filter(|entry| entry.visibility == meerkat_core::ToolCatalogVisibility::Visible)
            .map(|entry| entry.name.into_string())
            .collect::<Vec<_>>(),
        None => session_service
            .tool_scope_snapshot(&session_id)
            .await
            .map_err(|err| MobRuntimeError::Mob(MobError::Internal(err.to_string())))?
            .ok_or(MobRuntimeError::InvalidInput(
                "identity tool scope is unavailable",
            ))?
            .visible_names
            .into_iter()
            .map(meerkat_core::types::ToolName::into_string)
            .collect::<Vec<_>>(),
    };
    tools.sort();
    Ok(ResolvedToolsSnapshot {
        identity: identity.to_string(),
        session_id: session_id.to_string(),
        tools,
    })
}

/// Per-identity model-routing status, as owned by meerkat's runtime machine.
///
/// The `routing` payload is `meerkat_core::image_generation::SessionModelRoutingStatus`
/// verbatim - meerkat's own `WireSessionModelRoutingStatus` is a type alias to
/// that struct, so flattening it here keeps Meta's wire shape identical to
/// meerkat's published contract with no Meta-side mirror to drift.
///
/// Read `routing.session_provider == None` as "the machine has no hydrated
/// session LLM identity yet" (pre-hydration). It is NOT an invitation to
/// re-derive a provider from `effective_model` through a catalog: meerkat
/// documents that re-derivation as silently wrong for `ModelRegistry`-owned
/// custom models, which is the entire reason the typed field exists.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct IdentityRoutingStatusSnapshot {
    pub identity: String,
    pub session_id: String,
    #[serde(flatten)]
    pub routing: meerkat_core::image_generation::SessionModelRoutingStatus,
}

/// Why a routing status could not be produced.
///
/// These are deliberately distinct rather than one stringly failure. A fleet
/// sweep over materialized identities hits `NoCurrentSession` for every
/// identity that has not been addressed yet - that is the EXPECTED post-restart
/// state, not a defect - while `SessionNotHeld` is a real one. Collapsing them
/// leaves a caller with a refusal it cannot act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoutingStatusUnavailable {
    /// The session service is absent, or exposes no runtime adapter. This is a
    /// configuration property of the runtime, not a property of the identity.
    RuntimeUnsupported,
    /// The roster read returned no current session for this identity.
    ///
    /// Named for the fact observed, not for a cause inferred from it. Meta's
    /// `member_status` answers an UNKNOWN member with a well-formed "unknown"
    /// status carrying no session rather than an error, so this one observation
    /// covers two different situations that this surface genuinely cannot tell
    /// apart:
    ///
    ///  - an identity that was materialized but never addressed (identity-first
    ///    activates on address, so this is the normal post-restart state), and
    ///  - an identity that does not exist at all, including a typo.
    ///
    /// An earlier spelling of this variant asserted the first of those. It was
    /// wrong for the second, and a caller sweeping a fleet would have read a
    /// mistyped identity as legitimately unaddressed. Distinguishing them would
    /// need a roster read that reports absence as absence; that is a property of
    /// `member_status`, not of routing status, and is deliberately not papered
    /// over here.
    NoCurrentSession,
    /// The mob could not resolve the member at all. This is a ROSTER failure,
    /// not a statement about any session: keeping it separate stops an unknown
    /// member from being reported as if meerkat had lost a session it held.
    MemberLookupFailed(String),
    /// A session was resolved AND meerkat answered `RuntimeDriverError::NotFound`
    /// for it. Only this exact pairing earns this verdict. Unlike the absences
    /// above, it is a defect.
    SessionNotHeld(String),
    /// A session was resolved and the upstream read failed some OTHER way.
    ///
    /// `RuntimeDriverError` is `#[non_exhaustive]`: `NotReady`, `Destroyed`,
    /// `RecoveryCorruption` and any variant added later are real conditions that
    /// are NOT "the machine does not hold this session". Asserting a cause we
    /// did not observe would be the same class of fabrication the typed
    /// `session_provider` exists to prevent, so this variant deliberately
    /// diagnoses nothing and just carries the upstream text.
    UpstreamReadFailed(String),
    /// The caller supplied no usable identity.
    InvalidIdentity(&'static str),
}

impl RoutingStatusUnavailable {
    /// Stable discriminator for the JSON-RPC error payload. Callers classify on
    /// this, never on the message text.
    pub fn reason_code(&self) -> &'static str {
        match self {
            Self::RuntimeUnsupported => "runtime_unsupported",
            Self::NoCurrentSession => "no_current_session",
            Self::MemberLookupFailed(_) => "member_lookup_failed",
            Self::SessionNotHeld(_) => "session_not_held",
            Self::UpstreamReadFailed(_) => "upstream_read_failed",
            Self::InvalidIdentity(_) => "invalid_identity",
        }
    }
}

impl std::fmt::Display for RoutingStatusUnavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RuntimeUnsupported => {
                f.write_str("model routing status requires a runtime-backed session service")
            }
            Self::NoCurrentSession => f.write_str(
                "the roster reports no current session for this identity; it may have been \
                 materialized and never addressed, or it may not exist at all - this surface \
                 cannot tell those apart",
            ),
            Self::MemberLookupFailed(err) => {
                write!(f, "the mob could not resolve this member: {err}")
            }
            Self::SessionNotHeld(err) => {
                write!(f, "runtime machine does not hold this session: {err}")
            }
            Self::UpstreamReadFailed(err) => {
                write!(f, "the routing-status read failed upstream: {err}")
            }
            Self::InvalidIdentity(msg) => f.write_str(msg),
        }
    }
}

/// Classify an upstream routing-status read failure.
///
/// Named rather than inlined so the rule is testable on its own: only
/// `RuntimeDriverError::NotFound` may become `SessionNotHeld`. `RuntimeDriverError`
/// is `#[non_exhaustive]`, so a wildcard is required anyway - and the wildcard is
/// the point. `NotReady`, `Destroyed`, `RecoveryCorruption` and any variant added
/// upstream later are real conditions that are NOT "the machine does not hold this
/// session", and must not be reported as one.
fn classify_routing_status_read_error(
    err: meerkat_runtime::RuntimeDriverError,
) -> RoutingStatusUnavailable {
    match err {
        meerkat_runtime::RuntimeDriverError::NotFound { .. } => {
            RoutingStatusUnavailable::SessionNotHeld(err.to_string())
        }
        other => RoutingStatusUnavailable::UpstreamReadFailed(other.to_string()),
    }
}

/// Read the machine-owned routing status for one already-resolved session.
///
/// This is a `SessionServiceRuntimeExt` read through the service's runtime
/// adapter. Two absences are deliberately distinguished rather than collapsed
/// into an empty-but-plausible status:
///
/// - no session service, or a service with no runtime adapter, is
///   `InvalidInput` ("unavailable on this runtime"), and
/// - a session the adapter's machine has never seen surfaces meerkat's own
///   `RuntimeControlPlaneError::NotFound` through `MobRuntimeError`.
///
/// That second case matters. `project_model_routing_status` fills
/// `baseline_model` with `unwrap_or_default()` and leaves `session_provider`
/// at `None`, and both of those fields are `skip_serializing_if`, so a status
/// for an unknown session would serialize byte-identically to a status for a
/// genuinely pre-hydration session. Meerkat errors instead of returning that
/// default (dispatch_control.rs, `SessionModelRoutingStatus` arm), so this
/// helper must propagate rather than soften it - degrading to a default here
/// would manufacture exactly the indistinguishable pair meerkat avoided.
pub async fn model_routing_status_for_session(
    session_service: Option<&Arc<dyn MobSessionService>>,
    identity: &str,
    session_id: meerkat_core::types::SessionId,
) -> Result<IdentityRoutingStatusSnapshot, RoutingStatusUnavailable> {
    let Some(session_service) = session_service else {
        return Err(RoutingStatusUnavailable::RuntimeUnsupported);
    };
    let Some(runtime_adapter) = session_service
        .acquire_runtime_adapter(None)
        .map_err(|error| RoutingStatusUnavailable::UpstreamReadFailed(error.to_string()))?
    else {
        return Err(RoutingStatusUnavailable::RuntimeUnsupported);
    };
    let routing =
        meerkat_runtime::service_ext::SessionServiceRuntimeExt::session_model_routing_status(
            runtime_adapter.as_ref(),
            &session_id,
        )
        .await
        // Match the VARIANT, never the message text. `RuntimeControlPlaneError::NotFound`
        // lowers to `RuntimeDriverError::NotFound` through
        // `driver_error_from_control_plane_error`, so the one condition that means
        // "the machine does not hold this session" is structurally observable.
        // Everything else - including variants added upstream after this was written
        // - falls through undiagnosed rather than being asserted as a missing session.
        .map_err(classify_routing_status_read_error)?;
    Ok(IdentityRoutingStatusSnapshot {
        identity: identity.to_string(),
        session_id: session_id.to_string(),
        routing,
    })
}

/// Resolve an identity to its current session through the mob handle, then
/// read that session's routing status. Mirrors `resolved_tools_for_member`.
pub async fn model_routing_status_for_member(
    handle: &MobHandle,
    session_service: Option<&Arc<dyn MobSessionService>>,
    member_id: &str,
) -> Result<IdentityRoutingStatusSnapshot, RoutingStatusUnavailable> {
    if member_id.trim().is_empty() {
        return Err(RoutingStatusUnavailable::InvalidIdentity(
            "identity must not be empty",
        ));
    }
    // `roster_member_id_for_supplied_id`, NOT `mob_member_id`. Since the
    // stable-identity lowering the roster is keyed by the encoded DURABLE
    // identity, so a CURRENT runtime alias (`rt:{identity}:{generation}`)
    // handed straight to `mob_member_id` builds a roster key nobody owns. That
    // does not fail loudly: `member_status` answers with a well-formed
    // "unknown" status carrying no session, and this method would report
    // `no_current_session` for a perfectly healthy addressed member. The
    // normalizer's own doc names this exact trap, and both sibling surfaces
    // (`http_console.rs` member_status, `rpc/mob_methods.rs`) already route
    // through it.
    //
    // Lookup is normalized; the SUPPLIED identity is what gets echoed in the
    // snapshot below, so a caller still sees the spelling it asked with.
    let mid = crate::member_comms_id::roster_member_id_for_supplied_id(member_id);
    // A member the mob does not know is not the same as a member with no
    // session, and neither is a session meerkat lost. Three distinct facts,
    // three distinct reasons: flattening any pair of them hands a fleet sweep
    // a verdict it cannot act on.
    let status = handle
        .member_status(&mid)
        .await
        .map_err(|err| RoutingStatusUnavailable::MemberLookupFailed(err.to_string()))?;
    let Some(session_id) = status.current_session_id else {
        return Err(RoutingStatusUnavailable::NoCurrentSession);
    };
    model_routing_status_for_session(session_service, member_id, session_id).await
}

/// Resolve a supplied member id to its current session through the mob
/// handle, then read that session's visible tools. Mirrors
/// `model_routing_status_for_member`.
pub async fn resolved_tools_for_member(
    handle: &MobHandle,
    session_service: Option<&Arc<dyn MobSessionService>>,
    member_id: &str,
) -> Result<ResolvedToolsSnapshot, MobRuntimeError> {
    if member_id.trim().is_empty() {
        return Err(MobRuntimeError::InvalidInput("identity must not be empty"));
    }
    // `roster_member_id_for_supplied_id`, NOT `mob_member_id`, for the same
    // reason as the routing-status lookup (#366): the roster is keyed by the
    // encoded DURABLE identity, so a current runtime alias
    // (`rt:{identity}:{generation}`) handed straight to `mob_member_id` names
    // a roster row nobody owns, and a healthy member reads as having no
    // session. The SUPPLIED identity is still what the snapshot echoes. A
    // stale alias is refused by the surfaces before this lookup.
    let mid = crate::member_comms_id::roster_member_id_for_supplied_id(member_id);
    let status = handle.member_status(&mid).await?;
    let Some(session_id) = status.current_session_id else {
        return Err(MobRuntimeError::InvalidInput(
            "identity has no current session",
        ));
    };
    resolved_tools_for_session(session_service, member_id, session_id).await
}

/// Project a meerkat `AgentEvent` into meta's console/SSE/event-log JSON
/// payload shape.
///
/// Every surface that serializes an `AgentEvent` for consoles or SDKs must
/// route through here (HTTP SSE, the unified-runtime event ingest, the
/// identity-first live console projection) so the wire shape stays uniform:
///
/// - tool events mirror `id` into `tool_call_id`;
/// - a tool's result body travels only on `ToolResultReceived` (its typed
///   `content` blocks); `ToolExecutionCompleted` is the execution fact for the
///   same id and carries no body. Meta's wire contract, and the published
///   SDKs, read the flat `result: String` of the result event, so derive it
///   from that event's text blocks here.
/// - meerkat 0.7 removed the flat `error` string from `RunFailed`
///   (the typed `error_report { class, reason, message }` is the sole
///   owner). Meta's consumers read `error` and `reason`, so every LLM
///   failure rendered as "Failed: error" with the cause unread one level
///   down. Derive `error` from the report's message and `reason` from its
///   stable `reason_type` here, once; the report stays beside them.
///   `InteractionFailed` mirrors its typed reason's `Display` into `error`
///   the same way.
pub fn console_agent_event_payload(event: &meerkat_core::AgentEvent) -> Value {
    use meerkat_core::AgentEvent;
    use meerkat_core::event::agent_event_type;

    let mut payload = serde_json::to_value(event).unwrap_or_else(|_| serde_json::json!({}));
    let record = match payload.as_object_mut() {
        Some(record) => record,
        None => return payload,
    };
    let is_tool_event = matches!(
        agent_event_type(event),
        "tool_call_requested"
            | "tool_result_received"
            | "tool_execution_started"
            | "tool_execution_completed"
            | "tool_execution_timed_out"
    );
    if is_tool_event
        && !record.contains_key("tool_call_id")
        && let Some(id) = record.get("id").cloned()
    {
        record.insert("tool_call_id".to_string(), id);
    }
    if let AgentEvent::ToolResultReceived { content, .. } = event
        && !record.contains_key("result")
    {
        record.insert(
            "result".to_string(),
            Value::String(meerkat_core::types::text_content(content)),
        );
    }
    if let AgentEvent::RunFailed { error_report, .. } = event {
        if !record.contains_key("error") {
            record.insert(
                "error".to_string(),
                Value::String(error_report.message.clone()),
            );
        }
        // Read the discriminator from the report already serialized into the
        // record rather than serializing the typed reason a second time.
        let reason_type = record
            .get("error_report")
            .and_then(|report| report.get("reason"))
            .and_then(|reason| reason.get("reason_type"))
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        if let Some(reason_type) = reason_type
            && !record.contains_key("reason")
        {
            record.insert("reason".to_string(), Value::String(reason_type));
        }
    }
    if let AgentEvent::InteractionFailed { reason, .. } = event
        && !record.contains_key("error")
    {
        record.insert("error".to_string(), Value::String(reason.to_string()));
    }
    payload
}

pub fn content_input_has_images(content: &meerkat_core::ContentInput) -> bool {
    match content {
        meerkat_core::ContentInput::Text(_) => false,
        meerkat_core::ContentInput::Blocks(blocks) => blocks
            .iter()
            .any(|block| matches!(block, meerkat_core::ContentBlock::Image { .. })),
    }
}

/// The model registry Core builds a member of `definition` against: the
/// static catalog, the `[models.<id>]` and self-hosted models of
/// `model_config` (the host config the member builder holds), and the mob
/// definition's own `[models.<id>]` entries. `None` without a host config, or
/// when that registry does not assemble, in which case Core refuses to build
/// the member at all.
pub(crate) fn member_model_registry(
    model_config: Option<&meerkat::Config>,
    definition: &MobDefinition,
) -> Option<meerkat_core::ModelRegistry> {
    meerkat_core::ModelRegistry::from_config_with_models(
        model_config?,
        &definition.models,
        meerkat_models::canonical(),
    )
    .ok()
}

/// The image capability of `model` as the static model catalog knows it.
/// Self-hosted and config-declared models are not in the catalog; Meta's own
/// surfaces judge them with the host config's model registry instead.
pub fn model_capabilities_for_model(
    provider: Provider,
    model: &str,
) -> crate::runtime::ConsoleModelCapabilities {
    model_capabilities_for_model_with_registry(None, provider, model)
}

/// [`model_capabilities_for_model`], with a model the static catalog does not
/// know (a self-hosted or config-declared one) answered by `registry`, whose
/// `vision` flag for it is the one Core admits images by. A catalog model
/// keeps its catalog answer.
pub(crate) fn model_capabilities_for_model_with_registry(
    registry: Option<&meerkat_core::ModelRegistry>,
    provider: Provider,
    model: &str,
) -> crate::runtime::ConsoleModelCapabilities {
    let image_input = meerkat_models::profile_for(provider, model)
        .or_else(|| registry?.profile_for_provider(provider, model))
        .map(|profile| profile.vision)
        .unwrap_or(false);
    crate::runtime::ConsoleModelCapabilities { image_input }
}

pub fn model_capabilities_for_profile(
    profile: &Profile,
) -> crate::runtime::ConsoleModelCapabilities {
    model_capabilities_for_profile_with_registry(None, profile)
}

/// [`model_capabilities_for_profile`], with a model the static catalog does
/// not know answered by `registry` for the provider Core's member build
/// resolves: the profile's declared provider, else the one the registry
/// registers the model for.
pub(crate) fn model_capabilities_for_profile_with_registry(
    registry: Option<&meerkat_core::ModelRegistry>,
    profile: &Profile,
) -> crate::runtime::ConsoleModelCapabilities {
    let image_input = meerkat_models::infer_provider(&profile.model)
        .and_then(|provider| meerkat_models::profile_for(provider, &profile.model))
        .or_else(|| {
            let registry = registry?;
            let provider = profile
                .provider
                .or_else(|| registry.entry(&profile.model).map(|entry| entry.provider))?;
            registry.profile_for_provider(provider, &profile.model)
        })
        .map(|profile| profile.vision)
        .unwrap_or(false);
    crate::runtime::ConsoleModelCapabilities { image_input }
}

pub fn model_capabilities_for_role(
    definition: &MobDefinition,
    role: &str,
) -> crate::runtime::ConsoleModelCapabilities {
    model_capabilities_for_role_with_registry(None, definition, role)
}

pub(crate) fn model_capabilities_for_role_with_registry(
    registry: Option<&meerkat_core::ModelRegistry>,
    definition: &MobDefinition,
    role: &str,
) -> crate::runtime::ConsoleModelCapabilities {
    let profile_name = ProfileName::from(role);
    definition
        .resolve_inline_profile(&profile_name)
        .map(|profile| model_capabilities_for_profile_with_registry(registry, profile))
        .unwrap_or(crate::runtime::ConsoleModelCapabilities { image_input: false })
}

pub fn model_capabilities_for_member_entry(
    definition: &MobDefinition,
    entry: &meerkat_mob::runtime::MobMemberListEntry,
) -> crate::runtime::ConsoleModelCapabilities {
    model_capabilities_for_role(definition, entry.role.as_str())
}

pub async fn model_capabilities_for_member(
    handle: &MobHandle,
    session_service: Option<&Arc<dyn MobSessionService>>,
    member_id: &meerkat_mob::ids::AgentIdentity,
) -> crate::runtime::ConsoleModelCapabilities {
    model_capabilities_for_member_with_config(handle, session_service, None, member_id).await
}

/// [`model_capabilities_for_member`], judged with `model_config`, the host
/// config the mob's members are built with
/// ([`MobRuntime::member_model_config`]): a self-hosted or config-declared
/// model accepts images when its `vision` flag says so, as it does in Core.
/// `None` judges with the static model catalog alone.
pub(crate) async fn model_capabilities_for_member_with_config(
    handle: &MobHandle,
    session_service: Option<&Arc<dyn MobSessionService>>,
    model_config: Option<&meerkat::Config>,
    member_id: &meerkat_mob::ids::AgentIdentity,
) -> crate::runtime::ConsoleModelCapabilities {
    let registry = member_model_registry(model_config, handle.definition());
    if let Some(service) = session_service
        && let Some(session_id) = handle.resolve_bridge_session_id(member_id).await
        && let Ok(view) = service.read(&session_id).await
    {
        return model_capabilities_for_model_with_registry(
            registry.as_ref(),
            view.state.provider,
            &view.state.model,
        );
    }

    // Capability projection is a read-only display hint: a faulted or absent
    // member lookup degrades to "no image input" rather than failing the read.
    handle
        .get_member(member_id)
        .await
        .ok()
        .flatten()
        .map(|member| {
            model_capabilities_for_role_with_registry(
                registry.as_ref(),
                handle.definition(),
                member.role.as_str(),
            )
        })
        .unwrap_or(crate::runtime::ConsoleModelCapabilities { image_input: false })
}

pub async fn assert_member_accepts_images(
    handle: &MobHandle,
    session_service: Option<&Arc<dyn MobSessionService>>,
    member_id: &str,
    content: &meerkat_core::ContentInput,
) -> Result<(), MobRuntimeError> {
    assert_member_accepts_images_with_config(handle, session_service, None, member_id, content)
        .await
}

/// [`assert_member_accepts_images`], judged with `model_config`, the host
/// config the mob's members are built with (see
/// [`model_capabilities_for_member_with_config`]).
pub(crate) async fn assert_member_accepts_images_with_config(
    handle: &MobHandle,
    session_service: Option<&Arc<dyn MobSessionService>>,
    model_config: Option<&meerkat::Config>,
    member_id: &str,
    content: &meerkat_core::ContentInput,
) -> Result<(), MobRuntimeError> {
    if !content_input_has_images(content) {
        return Ok(());
    }
    // A read: resolve the supplied id the way `resolved_tools_for_member`
    // does, so a current runtime alias or an encoded roster id reaches the
    // durable row it names instead of "member not found".
    let mid = crate::member_comms_id::roster_member_id_for_supplied_id(member_id);
    let Some(member) = handle
        .get_member(&mid)
        .await
        .map_err(|_| MobRuntimeError::InvalidInput("member lookup failed"))?
    else {
        return Err(MobRuntimeError::InvalidInput("member not found"));
    };
    let caps = model_capabilities_for_member_with_config(
        handle,
        session_service,
        model_config,
        &member.agent_identity,
    )
    .await;
    if caps.image_input {
        Ok(())
    } else {
        Err(MobRuntimeError::InvalidInput(
            "target member model cannot accept image input",
        ))
    }
}

/// Send content to a mob member and return the bridge session id that
/// accepted the injection.
///
/// Validates that `member_id` and `content` are non-empty, calls
/// `handle.member(&id).send(...)`, then queries the mob handle for the
/// currently-bound bridge session id. Meerkat 0.6 removed `session_id` from
/// `MemberDeliveryReceipt`; this helper is meta's glue for the
/// send-and-learn-what-session-took-it pattern used by HTTP/RPC handlers and
/// the scheduled-dispatch injection path.
pub async fn send_message_on_mob(
    handle: &MobHandle,
    member_id: &str,
    content: impl Into<meerkat_core::ContentInput>,
) -> Result<String, MobRuntimeError> {
    send_message_on_mob_with_mode(
        handle,
        member_id,
        content,
        meerkat_core::types::HandlingMode::Queue,
    )
    .await
}

/// Variant that accepts the console's `Queue`/`Steer` wire contract while
/// delivering through Meta's direct member-send path.
pub async fn send_message_on_mob_with_mode(
    handle: &MobHandle,
    member_id: &str,
    content: impl Into<meerkat_core::ContentInput>,
    handling_mode: meerkat_core::types::HandlingMode,
) -> Result<String, MobRuntimeError> {
    send_message_on_mob_inner(handle, member_id, content.into(), handling_mode, None).await
}

/// Direct member send whose runtime admission carries the host-minted
/// interaction id, so the run's typed lineage names the exact console
/// reservation `meta/send_message` made for it and that run's terminal
/// settles it. Delivery semantics otherwise match
/// [`send_message_on_mob_with_mode`]: the same external-origin work on the
/// same SubmitWork path.
pub(crate) async fn send_message_on_mob_with_interaction(
    handle: &MobHandle,
    member_id: &str,
    content: impl Into<meerkat_core::ContentInput>,
    handling_mode: meerkat_core::types::HandlingMode,
    interaction_id: meerkat_core::interaction::InteractionId,
) -> Result<String, MobRuntimeError> {
    send_message_on_mob_inner(
        handle,
        member_id,
        content.into(),
        handling_mode,
        Some(interaction_id),
    )
    .await
}

async fn send_message_on_mob_inner(
    handle: &MobHandle,
    member_id: &str,
    content: meerkat_core::ContentInput,
    handling_mode: meerkat_core::types::HandlingMode,
    interaction_id: Option<meerkat_core::interaction::InteractionId>,
) -> Result<String, MobRuntimeError> {
    if member_id.trim().is_empty() {
        return Err(MobRuntimeError::InvalidInput("member_id must not be empty"));
    }
    let is_empty = match &content {
        meerkat_core::ContentInput::Text(s) => s.trim().is_empty(),
        meerkat_core::ContentInput::Blocks(blocks) => blocks.is_empty(),
    };
    if is_empty {
        return Err(MobRuntimeError::InvalidInput("content must not be empty"));
    }
    // Wire member ids are public aliases; the roster id is the comms-safe
    // encoding (meerkat 0.7 MemberCommsName). Decode first, so an already
    // encoded roster id is not encoded twice. A generated runtime alias names
    // an incarnation that only the identity authority can pin, and this
    // direct send has no authority to ask; collapsing it to the durable row
    // could deliver an old generation's message to the current incarnation,
    // so it is refused like the stdio send without an identity runtime.
    let decoded = crate::member_comms_id::runtime_alias_str(member_id);
    if crate::member_comms_id::is_reserved_generated_alias(&decoded) {
        return Err(MobRuntimeError::InvalidInput(
            "generated member alias requires identity authority",
        ));
    }
    let mid = crate::member_comms_id::mob_member_id(&decoded);
    match interaction_id {
        None => {
            handle
                .member(&mid)
                .await?
                .send(content, handling_mode)
                .await?;
        }
        Some(interaction_id) => {
            // The member-send verb has no interaction carrier, so submit the
            // same external-origin work with the id on its WorkSpec, bound to
            // the current roster binding (the identity bridge's pattern).
            // No `member()` preflight: SubmitWork admission already refuses a
            // restore-failed (Broken) member with the typed MemberRestoreFailed.
            let spec =
                meerkat_mob::WorkSpec::new(content.clone(), meerkat_mob::WorkOrigin::External)
                    .with_interaction_id(interaction_id);
            let work_ref = meerkat_mob::WorkRef::new();
            let member_id = &mid;
            let submitted = submit_on_current_binding_with_stale_fence_retry(
                || async move {
                    match handle.get_member(member_id).await? {
                        Some(entry) => Ok((entry.agent_runtime_id, entry.fence_token)),
                        None => {
                            // A member the roster no longer lists may still
                            // carry a restore-failure diagnostic, which only
                            // `member()` reports.
                            handle.member(member_id).await?;
                            Err(MobError::MemberNotFound(member_id.clone()))
                        }
                    }
                },
                |runtime_id, fence_token| {
                    let work_ref = work_ref.clone();
                    let spec = spec.clone();
                    async move {
                        handle
                            .submit_work_with_mode(
                                runtime_id,
                                fence_token,
                                work_ref,
                                spec,
                                handling_mode,
                            )
                            .await
                            .map(|_receipt| ())
                    }
                },
            )
            .await;
            match submitted {
                Ok(()) => {}
                // Legacy peer-only members cannot represent a transcript
                // interaction id. Meerkat refuses the carrier before admission,
                // so nothing was delivered and the plain send is not a second
                // delivery; that run simply carries no reservation lineage.
                Err(MobError::UnsupportedForMode { .. }) => {
                    handle
                        .member(&mid)
                        .await?
                        .send(content, handling_mode)
                        .await?;
                }
                Err(err) => return Err(err.into()),
            }
        }
    }
    if let Some(session_id) = handle.resolve_bridge_session_id(&mid).await {
        return Ok(session_id.to_string());
    }

    let status = handle.member_status(&mid).await?;
    if status.external_member.is_some() {
        return Ok(String::new());
    }

    Err(MobRuntimeError::Mob(MobError::Internal(
        "member has no bridge session after send".to_string(),
    )))
}

/// Submit to a member's current roster binding, re-reading the binding and
/// retrying exactly once when the submit is refused with `StaleFenceToken`.
///
/// The binding read and the submit are separate actor round trips, so a
/// respawn that lands between them fences the submit. A stale fence means the
/// work was not admitted, so one retry against the re-read binding is not a
/// second delivery. A second stale fence (the member keeps churning) is
/// returned to the caller.
async fn submit_on_current_binding_with_stale_fence_retry<Read, ReadFut, Submit, SubmitFut>(
    mut read_binding: Read,
    mut submit: Submit,
) -> Result<(), MobError>
where
    Read: FnMut() -> ReadFut,
    ReadFut: std::future::Future<
            Output = Result<(meerkat_mob::AgentRuntimeId, meerkat_mob::FenceToken), MobError>,
        >,
    Submit: FnMut(meerkat_mob::AgentRuntimeId, meerkat_mob::FenceToken) -> SubmitFut,
    SubmitFut: std::future::Future<Output = Result<(), MobError>>,
{
    let (runtime_id, fence_token) = read_binding().await?;
    match submit(runtime_id, fence_token).await {
        Err(MobError::StaleFenceToken { .. }) => {
            let (runtime_id, fence_token) = read_binding().await?;
            submit(runtime_id, fence_token).await
        }
        other => other,
    }
}

/// Console-only local human submission pinned to the member snapshot resolved
/// before accepting the request. External members retain the existing remote
/// send protocol; that compatibility path makes no local transcript-role claim.
pub(crate) async fn send_console_human_on_mob(
    handle: &MobHandle,
    member: &meerkat_mob::runtime::MobMemberListEntry,
    content: meerkat_core::ContentInput,
    skill_references: &[meerkat_core::skills::SkillKey],
    handling_mode: meerkat_core::types::HandlingMode,
    accepted: &crate::console_aggregator::ConsoleInteractionAccepted,
) -> Result<String, MobRuntimeError> {
    let (runtime_id, fence_token) = member.binding_atoms().ok_or(MobRuntimeError::InvalidInput(
        "console human target has no current runtime binding",
    ))?;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
    let status = crate::member_status_observation::observe_member_status_until(
        handle,
        &member.agent_identity,
        deadline,
    )
    .await?;
    if status.external_member.is_some() {
        // Remote work has no native member-turn skill carrier: refuse a
        // selection instead of running the turn without it.
        if !skill_references.is_empty() {
            return Err(MobRuntimeError::HostHumanInput(
                crate::identity_first::bridge::HostHumanInputError::Unsupported(
                    "selected skills require a local session-backed member".to_string(),
                ),
            ));
        }
        // Keep remote work transport semantics, but use the original binding:
        // a stale local snapshot must never become a send to a replacement peer.
        handle
            .submit_work_with_mode_bounded(
                runtime_id,
                fence_token,
                meerkat_mob::WorkRef::new(),
                meerkat_mob::WorkSpec::new(content, meerkat_mob::WorkOrigin::Internal),
                handling_mode,
                deadline.into_std(),
            )
            .await?;
        return Ok(String::new());
    }
    let interaction = accepted
        .interaction_id
        .parse::<uuid::Uuid>()
        .map_err(|_| MobRuntimeError::InvalidInput("console interaction must be a UUID"))?;
    let delivery = meerkat_mob::MobDeliveryIdentity::new(
        accepted.input_frame_id.clone(),
        interaction.to_string(),
    )
    .map_err(MobError::from)?;
    // Addressability was already checked by the console policy. Internal
    // origin preserves support for console-addressable internal workers.
    let spec = meerkat_mob::WorkSpec::new(content, meerkat_mob::WorkOrigin::Internal)
        .with_interaction_id(meerkat_core::interaction::InteractionId(interaction));
    let options = if skill_references.is_empty() {
        meerkat_mob::MemberTurnOptions::new()
    } else {
        meerkat_mob::MemberTurnOptions::new().with_skill_references(skill_references.to_vec())
    };
    handle
        .submit_host_human_input_with_options_bounded(
            runtime_id,
            fence_token,
            spec,
            handling_mode,
            options,
            delivery,
            deadline.into_std(),
        )
        .await?;
    Ok(accepted.session_id.clone().unwrap_or_default())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    /// A mob event history from `kinds`, in order.
    fn mob_history(kinds: Vec<meerkat_mob::MobEventKind>) -> Vec<meerkat_mob::event::MobEvent> {
        kinds
            .into_iter()
            .enumerate()
            .map(|(index, kind)| meerkat_mob::event::MobEvent {
                cursor: index as u64 + 1,
                timestamp: chrono::Utc::now(),
                mob_id: meerkat_mob::MobId::from("recovered-stop"),
                kind,
            })
            .collect()
    }

    /// The boot waits for exactly the Stops Core's recovery finishes after
    /// resume: an open placed Stop on a mob whose history still replays as
    /// running (the shape an earlier process's interrupted shutdown leaves).
    /// A Stop that landed, a resume, a reset or another intent waits for
    /// nothing.
    #[test]
    fn the_boot_waits_only_for_a_placed_stop_core_recovery_finishes() {
        use meerkat_mob::MobEventKind as Kind;
        use meerkat_mob::machines::mob_machine::PlacedCompletionLifecycleIntentKind as Intent;
        let started = |intent| Kind::PlacedCompletionLifecycleQuiesceStarted { intent };
        let ended = |intent| Kind::PlacedCompletionLifecycleQuiesceEnded { intent };
        let cases: Vec<(&str, Vec<Kind>, bool)> = vec![
            ("no history", vec![], false),
            (
                "an interrupted shutdown Stop",
                vec![started(Intent::Stop)],
                true,
            ),
            (
                "a Stop that landed",
                vec![started(Intent::Stop), Kind::MobStopped],
                false,
            ),
            (
                "a landed Stop the next boot resumed",
                vec![started(Intent::Stop), Kind::MobStopped, ended(Intent::Stop)],
                false,
            ),
            (
                "a second Stop interrupted after a resume",
                vec![
                    started(Intent::Stop),
                    Kind::MobStopped,
                    ended(Intent::Stop),
                    started(Intent::Stop),
                ],
                true,
            ),
            (
                "a Stop marker on a mob already stopped",
                vec![Kind::MobStopped, started(Intent::Stop)],
                false,
            ),
            (
                "a reset closes it",
                vec![started(Intent::Stop), Kind::MobReset],
                false,
            ),
            ("another intent", vec![started(Intent::Reset)], false),
            (
                "a destroy was admitted: Core retries the destroy",
                vec![started(Intent::Stop), Kind::MobDestroying],
                false,
            ),
        ];
        for (name, kinds, waits) in cases {
            assert_eq!(
                recovery_finishes_a_placed_stop(
                    &meerkat_mob::MobId::from("recovered-stop"),
                    &mob_history(kinds)
                ),
                waits,
                "{name}"
            );
        }
        // Only the mob's own events count.
        let mut foreign = mob_history(vec![started(Intent::Stop)]);
        foreign[0].mob_id = meerkat_mob::MobId::from("another-mob");
        assert!(!recovery_finishes_a_placed_stop(
            &meerkat_mob::MobId::from("recovered-stop"),
            &foreign
        ));
    }

    // `session_commit_pending` answers only what the runtime read concluded
    // within the bound: one test per arm of its mapping. The real-machine
    // arms (a missing runtime, an unknown or gone session) are in
    // tests/bridge_completion_seam.rs.

    /// Bindings read by the stale-fence retry, in order, and the bindings
    /// each submit was fenced with.
    fn stale_fence_binding(fence: u64) -> (meerkat_mob::AgentRuntimeId, meerkat_mob::FenceToken) {
        (
            meerkat_mob::AgentRuntimeId::new(
                meerkat_mob::AgentIdentity::from("worker"),
                meerkat_mob::ids::Generation::INITIAL,
            ),
            meerkat_mob::FenceToken::new(fence),
        )
    }

    fn stale_fence_error(fence: u64) -> MobError {
        let (runtime_id, _) = stale_fence_binding(fence);
        MobError::StaleFenceToken {
            runtime_id,
            expected: meerkat_mob::FenceToken::new(fence + 1),
            actual: meerkat_mob::FenceToken::new(fence),
        }
    }

    /// Drive the retry with scripted binding reads and submit outcomes;
    /// returns the result, the number of reads, and the fences submitted.
    async fn drive_stale_fence_retry(
        submits: Vec<Result<(), MobError>>,
    ) -> (Result<(), MobError>, usize, Vec<u64>) {
        let reads = std::sync::atomic::AtomicUsize::new(0);
        let submitted = std::sync::Mutex::new(Vec::new());
        let outcomes = std::sync::Mutex::new(std::collections::VecDeque::from(submits));
        let result = submit_on_current_binding_with_stale_fence_retry(
            || {
                let fence = reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst) as u64 + 1;
                async move { Ok(stale_fence_binding(fence)) }
            },
            |_runtime_id, fence_token| {
                submitted.lock().unwrap().push(fence_token.get());
                let outcome = outcomes.lock().unwrap().pop_front().unwrap();
                async move { outcome }
            },
        )
        .await;
        let submitted = submitted.lock().unwrap().clone();
        (
            result,
            reads.load(std::sync::atomic::Ordering::SeqCst),
            submitted,
        )
    }

    /// #479: a respawn between the binding read and the submit fences the
    /// submit; the send re-reads the binding and retries once, against the
    /// successor.
    #[tokio::test]
    async fn send_message_retries_once_on_a_stale_fence_against_the_reread_binding() {
        let (result, reads, submitted) =
            drive_stale_fence_retry(vec![Err(stale_fence_error(1)), Ok(())]).await;
        assert!(result.is_ok(), "{result:?}");
        assert_eq!(reads, 2);
        assert_eq!(
            submitted,
            [1, 2],
            "the retry is fenced with the re-read binding"
        );
    }

    #[tokio::test]
    async fn send_message_returns_a_second_stale_fence() {
        let (result, reads, submitted) =
            drive_stale_fence_retry(vec![Err(stale_fence_error(1)), Err(stale_fence_error(2))])
                .await;
        assert!(
            matches!(result, Err(MobError::StaleFenceToken { .. })),
            "{result:?}"
        );
        assert_eq!(reads, 2, "exactly one retry");
        assert_eq!(submitted, [1, 2]);
    }

    #[tokio::test]
    async fn send_message_does_not_retry_other_submit_errors() {
        let (result, reads, submitted) = drive_stale_fence_retry(vec![Err(
            MobError::MemberNotFound(meerkat_mob::AgentIdentity::from("worker")),
        )])
        .await;
        assert!(
            matches!(result, Err(MobError::MemberNotFound(_))),
            "{result:?}"
        );
        assert_eq!(reads, 1);
        assert_eq!(submitted, [1]);
    }

    #[tokio::test]
    async fn session_commit_pending_reports_pending_input() {
        assert_eq!(
            bounded_commit_pending(
                async { Ok::<_, ()>(true) },
                SESSION_COMMIT_PENDING_READ_BOUND
            )
            .await,
            Some(true)
        );
    }

    #[tokio::test]
    async fn session_commit_pending_reports_no_pending_input() {
        assert_eq!(
            bounded_commit_pending(
                async { Ok::<_, ()>(false) },
                SESSION_COMMIT_PENDING_READ_BOUND
            )
            .await,
            Some(false)
        );
    }

    #[tokio::test]
    async fn session_commit_pending_fails_closed_on_a_runtime_error() {
        assert_eq!(
            bounded_commit_pending(
                async { Err::<bool, _>("session is not held by a runtime") },
                SESSION_COMMIT_PENDING_READ_BOUND
            )
            .await,
            None,
            "an error is inconclusive, never a guessed false"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn session_commit_pending_fails_closed_when_the_read_outlasts_its_bound() {
        let started = tokio::time::Instant::now();
        assert_eq!(
            bounded_commit_pending(
                std::future::pending::<Result<bool, ()>>(),
                SESSION_COMMIT_PENDING_READ_BOUND
            )
            .await,
            None,
            "a read still waiting (a boundary commit holds the driver) is inconclusive"
        );
        assert_eq!(started.elapsed(), SESSION_COMMIT_PENDING_READ_BOUND);
    }

    /// meerkat >= 0.7 `RunFailed` carries only the typed `error_report`; the
    /// projection derives the flat `error` and `reason` every Meta consumer
    /// (SDK `RunFailed.error`, the console failure summary, the event log)
    /// reads, and keeps the report beside them.
    #[test]
    fn console_agent_event_payload_names_run_failed_from_its_error_report() {
        use meerkat_core::event::{AgentErrorClass, AgentErrorReason, AgentErrorReport};

        let event = meerkat_core::AgentEvent::RunFailed {
            identity: Default::default(),
            session_id: meerkat_core::types::SessionId::new(),
            error_report: AgentErrorReport {
                class: AgentErrorClass::Llm,
                reason: Some(AgentErrorReason::LlmAuthError),
                message: "LLM error: authentication failed (401)".to_string(),
            },
            terminal_cause_kind: None,
        };
        let payload = console_agent_event_payload(&event);
        assert_eq!(payload["type"], "run_failed");
        assert_eq!(payload["error"], "LLM error: authentication failed (401)");
        assert_eq!(payload["reason"], "llm_auth_error");
        assert_eq!(payload["error_report"]["class"], "llm");
        assert_eq!(
            payload["error_report"]["reason"]["reason_type"],
            "llm_auth_error"
        );
        assert_eq!(
            payload["error_report"]["message"],
            "LLM error: authentication failed (401)"
        );
    }

    /// A report without a typed reason still names the failure through
    /// `error`; no `reason` key is fabricated for it.
    #[test]
    fn console_agent_event_payload_run_failed_without_typed_reason_carries_error_only() {
        use meerkat_core::event::{AgentErrorClass, AgentErrorReport};

        let event = meerkat_core::AgentEvent::RunFailed {
            identity: Default::default(),
            session_id: meerkat_core::types::SessionId::new(),
            error_report: AgentErrorReport {
                class: AgentErrorClass::Internal,
                reason: None,
                message: "internal: runner gave up".to_string(),
            },
            terminal_cause_kind: None,
        };
        let payload = console_agent_event_payload(&event);
        assert_eq!(payload["error"], "internal: runner gave up");
        assert!(
            payload.get("reason").is_none(),
            "no typed reason means no reason key: {payload}"
        );
    }

    /// `InteractionFailed` carries a typed `reason`; its `Display` is the
    /// human-readable text, mirrored into `error` so consumers read one key
    /// for both failure shapes while the typed reason stays intact.
    #[test]
    fn console_agent_event_payload_interaction_failed_mirrors_reason_display_into_error() {
        use meerkat_core::event::InteractionFailureReason;

        let reason = InteractionFailureReason::Cancelled;
        let expected = reason.to_string();
        let event = meerkat_core::AgentEvent::InteractionFailed {
            interaction_id: meerkat_core::InteractionId::new(),
            reason,
        };
        let payload = console_agent_event_payload(&event);
        assert_eq!(payload["type"], "interaction_failed");
        assert_eq!(payload["error"], expected);
        assert_eq!(payload["reason"]["kind"], "cancelled");
    }

    /// A tool's result body travels only on `ToolResultReceived`: the flat
    /// `result` the SDKs read is derived on that event, beside its typed
    /// `content`, and the execution fact (`ToolExecutionCompleted`) carries
    /// neither, only its status and duration. Both mirror `tool_call_id`.
    #[test]
    fn console_agent_event_payload_derives_the_tool_result_on_the_result_event_only() {
        let received = meerkat_core::AgentEvent::ToolResultReceived {
            id: "tool-1".to_string(),
            name: "search".to_string(),
            content: vec![meerkat_core::types::ContentBlock::Text {
                text: "done".to_string(),
            }],
            is_error: false,
        };
        let payload = console_agent_event_payload(&received);
        assert_eq!(payload["type"], "tool_result_received");
        assert_eq!(payload["tool_call_id"], "tool-1");
        assert_eq!(payload["result"], "done");
        assert_eq!(payload["content"][0]["text"], "done");

        let completed = meerkat_core::AgentEvent::ToolExecutionCompleted {
            id: "tool-1".to_string(),
            name: "search".to_string(),
            is_error: false,
            duration_ms: 12,
        };
        let payload = console_agent_event_payload(&completed);
        assert_eq!(payload["type"], "tool_execution_completed");
        assert_eq!(payload["tool_call_id"], "tool-1");
        assert_eq!(payload["duration_ms"], 12);
        assert!(
            payload.get("result").is_none() && payload.get("content").is_none(),
            "the execution fact carries no result body: {payload}"
        );
    }

    struct EmptyDispatcher;

    #[async_trait::async_trait]
    impl meerkat_core::AgentToolDispatcher for EmptyDispatcher {
        fn tools(&self) -> Arc<[Arc<meerkat_core::types::ToolDef>]> {
            Vec::<Arc<meerkat_core::types::ToolDef>>::new().into()
        }

        async fn dispatch(
            &self,
            call: meerkat_core::types::ToolCallView<'_>,
        ) -> Result<meerkat_core::ToolDispatchOutcome, meerkat_core::ToolError> {
            Err(meerkat_core::ToolError::not_found(call.name))
        }

        fn capabilities(&self) -> meerkat_core::agent::DispatcherCapabilities {
            meerkat_core::agent::DispatcherCapabilities::default()
        }
    }

    struct NamedDispatcher {
        tools: Arc<[Arc<meerkat_core::types::ToolDef>]>,
    }

    #[async_trait::async_trait]
    impl meerkat_core::AgentToolDispatcher for NamedDispatcher {
        fn tools(&self) -> Arc<[Arc<meerkat_core::types::ToolDef>]> {
            Arc::clone(&self.tools)
        }

        async fn dispatch(
            &self,
            call: meerkat_core::types::ToolCallView<'_>,
        ) -> Result<meerkat_core::ToolDispatchOutcome, meerkat_core::ToolError> {
            Err(meerkat_core::ToolError::not_found(call.name))
        }
    }

    /// The wrapped mob tools admit against the session's mob operator
    /// authority, so the wrapper passes the binding through unchanged.
    #[test]
    fn the_parent_mob_tool_wrapper_binds_the_session_mob_authority_on_its_tools() {
        use crate::tool_application_test_support::{MobAuthorityBindProbe, session_mob_authority};
        let inner = Arc::new(MobAuthorityBindProbe::default());
        let wrapper = AutoWireParentMobToolDispatcher {
            inner: inner.clone(),
            ..wrapper_with_overrides(ImplicitDelegateRetirementOverrides::default())
        };
        let authority = session_mob_authority();
        meerkat_core::AgentToolDispatcher::bind_session_mob_tool_authority(
            &wrapper,
            Some(Arc::clone(&authority)),
        );
        assert!(inner.last_bound_is(Some(&authority)));
        meerkat_core::AgentToolDispatcher::bind_session_mob_tool_authority(&wrapper, None);
        assert!(inner.last_bound_is(None));
    }

    fn wrapper_with_overrides(
        overrides: ImplicitDelegateRetirementOverrides,
    ) -> AutoWireParentMobToolDispatcher {
        AutoWireParentMobToolDispatcher {
            inner: Arc::new(EmptyDispatcher),
            implicit_delegate_retirement_overrides: overrides,
            console_spawn_sink: new_console_spawn_sink_slot(),
            identity_runtime: Arc::new(std::sync::RwLock::new(None)),
            protected_mob_id: "test-mob".to_string(),
            spawner_comms_name: None,
        }
    }

    #[test]
    fn auto_wire_wrapper_forwards_exactness_and_patches_the_catalog_like_tools() {
        struct ExactMobTools;
        #[async_trait::async_trait]
        impl meerkat_core::AgentToolDispatcher for ExactMobTools {
            fn tools(&self) -> Arc<[Arc<meerkat_core::types::ToolDef>]> {
                ["delegate", "mob_spawn_member", "mob_list_members"]
                    .into_iter()
                    .map(|name| {
                        Arc::new(meerkat_core::types::ToolDef {
                            audience: Default::default(),
                            name: name.into(),
                            description: String::new(),
                            input_schema: serde_json::json!({"type": "object", "properties": {}}),
                            provenance: None,
                        })
                    })
                    .collect::<Vec<_>>()
                    .into()
            }
            fn tool_catalog_capabilities(&self) -> meerkat_core::ToolCatalogCapabilities {
                meerkat_core::ToolCatalogCapabilities {
                    exact_catalog: true,
                    may_require_catalog_control_plane: false,
                }
            }
            async fn dispatch(
                &self,
                call: meerkat_core::types::ToolCallView<'_>,
            ) -> Result<meerkat_core::ToolDispatchOutcome, meerkat_core::ToolError> {
                Err(meerkat_core::ToolError::not_found(call.name))
            }
        }

        use meerkat_core::AgentToolDispatcher as _;

        let wrapper = AutoWireParentMobToolDispatcher {
            inner: Arc::new(ExactMobTools),
            ..wrapper_with_overrides(ImplicitDelegateRetirementOverrides::default())
        };
        assert!(wrapper.tool_catalog_capabilities().exact_catalog);
        let catalog = wrapper.tool_catalog();
        let tools = wrapper.tools();
        assert_eq!(catalog.len(), tools.len());
        for (entry, tool) in catalog.iter().zip(tools.iter()) {
            assert_eq!(entry.tool.name, tool.name);
            assert_eq!(
                entry.tool.input_schema, tool.input_schema,
                "the catalog must advertise the same patched definition as tools()"
            );
        }
        let patched = |name: &str| {
            catalog
                .iter()
                .find(|entry| entry.tool.name == name)
                .and_then(|entry| {
                    entry
                        .tool
                        .input_schema
                        .pointer("/properties/idle_retire_secs")
                })
                .is_some()
        };
        assert!(patched("delegate"));
        assert!(patched("mob_spawn_member"));
        assert!(!patched("mob_list_members"));
    }

    #[test]
    fn raw_mob_tools_detect_public_and_encoded_generated_aliases() {
        let encoded = crate::member_comms_id::mob_member_id_str("rt:worker:0").into_owned();
        assert_eq!(
            reserved_raw_member_tool_argument(
                "mob_spawn_member",
                &serde_json::json!({"member_id": "rt:worker:0"}),
            ),
            Some(("member_id", "rt:worker:0".to_string()))
        );
        assert_eq!(
            reserved_raw_member_tool_argument(
                "mob_wire",
                &serde_json::json!({"member_id": "classic", "peer": {"local": encoded}}),
            ),
            Some(("peer.local", "rt:worker:0".to_string()))
        );
        assert_eq!(
            reserved_raw_member_tool_argument(
                "delegate",
                &serde_json::json!({"member_id": "mk--victim"}),
            ),
            Some(("member_id", "victim".to_string()))
        );
        assert_eq!(
            reserved_raw_member_tool_argument(
                "fork_off",
                &serde_json::json!({"member_id": "rt:worker:0"}),
            ),
            Some(("member_id", "rt:worker:0".to_string()))
        );
        assert_eq!(
            reserved_raw_member_tool_argument(
                "spawn_many_members",
                &serde_json::json!({
                    "specs": [
                        {"profile": "worker", "member_id": "classic"},
                        {"profile": "worker", "member_id": encoded},
                    ]
                }),
            ),
            Some(("specs[].member_id", "rt:worker:0".to_string()))
        );
    }

    #[tokio::test]
    async fn raw_mob_tool_dispatch_fails_closed_before_lower_plane() {
        use meerkat_core::AgentToolDispatcher;

        let dispatcher = wrapper_with_overrides(ImplicitDelegateRetirementOverrides::default());
        let args = serde_json::value::RawValue::from_string(
            serde_json::json!({"mob_id": "m", "member_id": "rt:worker:0"}).to_string(),
        )
        .expect("raw args");
        let error = dispatcher
            .dispatch(meerkat_core::types::ToolCallView {
                id: "call-reserved",
                name: "mob_retire_member",
                args: &args,
            })
            .await
            .expect_err("reserved alias must not reach the raw dispatcher");

        assert!(error.to_string().contains("reserved rt:* / mk--"));

        let batch_args = serde_json::value::RawValue::from_string(
            serde_json::json!({
                "specs": [{"profile": "worker", "member_id": "rt:worker:0"}]
            })
            .to_string(),
        )
        .expect("raw args");
        let batch_error = dispatcher
            .dispatch(meerkat_core::types::ToolCallView {
                id: "call-reserved-batch",
                name: "spawn_many_members",
                args: &batch_args,
            })
            .await
            .expect_err("reserved batch member must not reach the raw dispatcher");
        assert!(batch_error.to_string().contains("specs[].member_id"));
    }

    #[tokio::test]
    async fn raw_operator_tool_rejects_registered_plain_durable_identity() {
        use meerkat_core::AgentToolDispatcher;

        let dispatcher = wrapper_with_overrides(ImplicitDelegateRetirementOverrides::default());
        let identity_runtime = Arc::new(crate::identity_first::IdentityRuntime::new(
            crate::identity_first::IdentityRuntimeConfig {
                continuity_store: Arc::new(
                    crate::identity_first::LocalContinuityStore::in_memory()
                        .expect("continuity store"),
                ),
                lease_provider: Arc::new(crate::identity_first::LocalLeaseProvider::new()),
                runtime_instance_id: "raw-operator-test".to_string(),
                has_runtime_store: true,
                durability_policy: crate::identity_first::DurabilityPolicy::SyncWriteThrough,
                bridge: None,
                default_timeout: None,
            },
        ));
        let identity =
            crate::identity_first::AgentIdentity::parse("lead").expect("durable identity");
        identity_runtime
            .register(crate::identity_first::DurableAgentSpec {
                identity,
                profile: meerkat_mob::ProfileName::from("worker"),
                addressability: crate::identity_first::AgentAddressability::Addressable,
                display_name: None,
                labels: BTreeMap::new(),
                context: None,
                additional_instructions: Vec::new(),
                initial_message: None,
                runtime_mode_override: None,
                backend: None,
                binding: None,
                placement: None,
            })
            .await;
        *dispatcher
            .identity_runtime
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(identity_runtime);

        let args = serde_json::value::RawValue::from_string(
            serde_json::json!({"member_id": "lead"}).to_string(),
        )
        .expect("raw args");
        let error = dispatcher
            .dispatch(meerkat_core::types::ToolCallView {
                id: "call-owned",
                name: "force_cancel_member",
                args: &args,
            })
            .await
            .expect_err("registered durable identity must not reach raw force cancel");
        assert!(
            error
                .to_string()
                .contains("owned by the attached IdentityRuntime")
        );

        let child_collision = serde_json::value::RawValue::from_string(
            serde_json::json!({
                "member_id": "lead"
            })
            .to_string(),
        )
        .expect("raw args");
        let error = dispatcher
            .dispatch(meerkat_core::types::ToolCallView {
                id: "fork-off-child-collision",
                name: "fork_off",
                args: &child_collision,
            })
            .await
            .expect_err("registered child identity must fail before lower-plane dispatch");
        assert!(
            error
                .to_string()
                .contains("member_id 'lead' is owned by the attached IdentityRuntime")
        );
    }

    #[test]
    fn delegate_tool_schema_exposes_idle_retire_secs() {
        let tool = meerkat_core::types::ToolDef::new(
            "delegate",
            "Delegate work",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "task": {"type": "string"}
                },
                "required": ["task"]
            }),
        );

        let patched = delegate_tool_def_with_idle_retire_secs(&tool);
        let idle_retire_secs = &patched.input_schema["properties"]["idle_retire_secs"];

        assert!(patched.description.contains("IDLE RETIREMENT:"));
        assert_eq!(idle_retire_secs["anyOf"][0]["type"], "integer");
        assert_eq!(idle_retire_secs["anyOf"][0]["minimum"], 0);
        assert_eq!(idle_retire_secs["anyOf"][1]["type"], "null");
        assert_null_described_as_omitting(&patched.description, idle_retire_secs);
    }

    /// The tool text tells the model null is the same as omitting, and no
    /// longer offers a model-facing way to disable retirement.
    fn assert_null_described_as_omitting(description: &str, schema: &Value) {
        let schema_description = schema["description"].as_str().expect("schema description");
        for text in [description, schema_description] {
            assert!(text.contains("null is the same as omitting it"), "{text}");
            assert!(!text.contains("disable"), "{text}");
        }
    }

    #[test]
    fn fork_off_tool_schema_exposes_idle_retire_secs_through_the_wrapper() {
        struct ForkOffOnly;
        #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
        #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
        impl meerkat_core::AgentToolDispatcher for ForkOffOnly {
            fn tools(&self) -> Arc<[Arc<meerkat_core::types::ToolDef>]> {
                vec![Arc::new(meerkat_core::types::ToolDef::new(
                    "fork_off",
                    "Fork the current transcript",
                    serde_json::json!({
                        "type": "object",
                        "properties": {"task": {"type": "string"}},
                        "required": ["task"]
                    }),
                ))]
                .into()
            }
            async fn dispatch(
                &self,
                call: meerkat_core::types::ToolCallView<'_>,
            ) -> Result<meerkat_core::ToolDispatchOutcome, meerkat_core::ToolError> {
                Err(meerkat_core::ToolError::execution_failed(format!(
                    "unexpected dispatch of {}",
                    call.name
                )))
            }
        }
        let dispatcher = AutoWireParentMobToolDispatcher {
            inner: Arc::new(ForkOffOnly),
            implicit_delegate_retirement_overrides: ImplicitDelegateRetirementOverrides::default(),
            console_spawn_sink: new_console_spawn_sink_slot(),
            identity_runtime: Arc::new(std::sync::RwLock::new(None)),
            protected_mob_id: "test-mob".to_string(),
            spawner_comms_name: None,
        };

        let tools = meerkat_core::AgentToolDispatcher::tools(&dispatcher);
        let fork_off = tools
            .iter()
            .find(|tool| tool.name == "fork_off")
            .expect("fork_off stays exposed");
        let idle_retire_secs = &fork_off.input_schema["properties"]["idle_retire_secs"];

        assert!(fork_off.description.contains("IDLE RETIREMENT:"));
        assert!(
            fork_off
                .description
                .contains("Omit idle_retire_secs to use the runtime default")
        );
        assert_eq!(idle_retire_secs["anyOf"][0]["type"], "integer");
        assert_eq!(idle_retire_secs["anyOf"][1]["type"], "null");
        assert_null_described_as_omitting(&fork_off.description, idle_retire_secs);
    }

    #[test]
    fn fork_off_idle_retire_secs_defaults_to_the_runtime_default() {
        let mut absent = serde_json::json!({"task": "inspect"});
        assert_eq!(
            fork_off_idle_retire_override_from_args("fork_off", &mut absent)
                .expect("absent arg is valid"),
            DelegateIdleRetireOverride::RuntimeDefault
        );

        let mut seconds = serde_json::json!({"task": "inspect", "idle_retire_secs": 42});
        assert_eq!(
            fork_off_idle_retire_override_from_args("fork_off", &mut seconds)
                .expect("integer arg is valid"),
            DelegateIdleRetireOverride::Seconds(42)
        );
        assert!(seconds.get("idle_retire_secs").is_none());

        // null is the same as omitting: models emit null for optional
        // fields, and a null must not pin the fork child forever.
        let mut null = serde_json::json!({"task": "inspect", "idle_retire_secs": null});
        assert_eq!(
            fork_off_idle_retire_override_from_args("fork_off", &mut null)
                .expect("null arg is valid"),
            DelegateIdleRetireOverride::RuntimeDefault
        );
        assert!(null.get("idle_retire_secs").is_none());

        let mut invalid = serde_json::json!({"task": "inspect", "idle_retire_secs": "soon"});
        assert!(fork_off_idle_retire_override_from_args("fork_off", &mut invalid).is_err());
    }

    #[test]
    fn mob_spawn_tool_schema_exposes_opt_in_idle_retire_secs() {
        let tool = meerkat_core::types::ToolDef::new(
            "mob_spawn_member",
            "Spawn member",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "profile": {"type": "string"},
                    "member_id": {"type": "string"}
                },
                "required": ["profile", "member_id"]
            }),
        );

        let patched = mob_spawn_tool_def_with_idle_retire_secs(&tool);
        let idle_retire_secs = &patched.input_schema["properties"]["idle_retire_secs"];

        assert!(
            patched
                .description
                .contains("Omit idle_retire_secs to leave this spawned member out")
        );
        assert_eq!(idle_retire_secs["anyOf"][0]["type"], "integer");
        assert_eq!(idle_retire_secs["anyOf"][0]["minimum"], 0);
        assert_eq!(idle_retire_secs["anyOf"][1]["type"], "null");
        assert_null_described_as_omitting(&patched.description, idle_retire_secs);
    }

    #[tokio::test]
    async fn auto_wire_wrapper_preserves_ops_lifecycle_binding() {
        use meerkat_core::AgentToolDispatcher;
        use std::sync::atomic::{AtomicBool, Ordering};

        struct BindAwareDispatcher {
            bound: Arc<AtomicBool>,
        }

        #[async_trait::async_trait]
        impl meerkat_core::AgentToolDispatcher for BindAwareDispatcher {
            fn tools(&self) -> Arc<[Arc<meerkat_core::types::ToolDef>]> {
                Vec::<Arc<meerkat_core::types::ToolDef>>::new().into()
            }

            async fn dispatch(
                &self,
                call: meerkat_core::types::ToolCallView<'_>,
            ) -> Result<meerkat_core::ToolDispatchOutcome, meerkat_core::ToolError> {
                Err(meerkat_core::ToolError::not_found(call.name))
            }

            fn capabilities(&self) -> meerkat_core::agent::DispatcherCapabilities {
                meerkat_core::agent::DispatcherCapabilities {
                    ops_lifecycle: true,
                }
            }

            fn bind_ops_lifecycle(
                self: Arc<Self>,
                _registry: Arc<dyn meerkat_core::ops_lifecycle::OpsLifecycleRegistry>,
                _owner_bridge_session_id: meerkat_core::types::SessionId,
            ) -> Result<meerkat_core::agent::BindOutcome, meerkat_core::agent::OpsLifecycleBindError>
            {
                self.bound.store(true, Ordering::SeqCst);
                Ok(meerkat_core::agent::BindOutcome::Bound(self))
            }
        }

        let bound = Arc::new(AtomicBool::new(false));
        let dispatcher = Arc::new(AutoWireParentMobToolDispatcher {
            inner: Arc::new(BindAwareDispatcher {
                bound: Arc::clone(&bound),
            }),
            implicit_delegate_retirement_overrides: ImplicitDelegateRetirementOverrides::default(),
            console_spawn_sink: new_console_spawn_sink_slot(),
            identity_runtime: Arc::new(std::sync::RwLock::new(None)),
            protected_mob_id: "test-mob".to_string(),
            spawner_comms_name: None,
        });

        assert!(dispatcher.capabilities().ops_lifecycle);
        let outcome = dispatcher
            .bind_ops_lifecycle(
                Arc::new(meerkat_runtime::ops_lifecycle::RuntimeOpsLifecycleRegistry::new()),
                meerkat_core::types::SessionId::new(),
            )
            .expect("wrapper should delegate ops lifecycle binding");

        assert!(outcome.was_bound());
        assert!(bound.load(Ordering::SeqCst));
        assert!(outcome.into_dispatcher().capabilities().ops_lifecycle);
    }

    #[tokio::test]
    async fn auto_wire_wrapper_preserves_objective_dispatch_context() {
        struct ContextAwareDispatcher {
            observed_objective: Arc<std::sync::Mutex<Option<String>>>,
        }

        #[async_trait::async_trait]
        impl meerkat_core::AgentToolDispatcher for ContextAwareDispatcher {
            fn tools(&self) -> Arc<[Arc<meerkat_core::types::ToolDef>]> {
                Vec::<Arc<meerkat_core::types::ToolDef>>::new().into()
            }

            async fn dispatch(
                &self,
                call: meerkat_core::types::ToolCallView<'_>,
            ) -> Result<meerkat_core::ToolDispatchOutcome, meerkat_core::ToolError> {
                Err(meerkat_core::ToolError::execution_failed(format!(
                    "plain dispatch unexpectedly used for {}",
                    call.name
                )))
            }

            async fn dispatch_with_context(
                &self,
                call: meerkat_core::types::ToolCallView<'_>,
                context: &meerkat_core::ToolDispatchContext,
            ) -> Result<meerkat_core::ToolDispatchOutcome, meerkat_core::ToolError> {
                *self
                    .observed_objective
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = context
                    .turn_metadata(meerkat_core::agent::TOOL_DISPATCH_OBJECTIVE_ID_KEY)
                    .and_then(Value::as_str)
                    .map(ToString::to_string);
                Ok(meerkat_core::ToolDispatchOutcome::sync_result(
                    meerkat_core::types::ToolResult::new(
                        call.id.to_string(),
                        "{}".to_string(),
                        false,
                    ),
                ))
            }
        }

        let observed_objective = Arc::new(std::sync::Mutex::new(None));
        let dispatcher = AutoWireParentMobToolDispatcher {
            inner: Arc::new(ContextAwareDispatcher {
                observed_objective: Arc::clone(&observed_objective),
            }),
            implicit_delegate_retirement_overrides: ImplicitDelegateRetirementOverrides::default(),
            console_spawn_sink: new_console_spawn_sink_slot(),
            identity_runtime: Arc::new(std::sync::RwLock::new(None)),
            protected_mob_id: "test-mob".to_string(),
            spawner_comms_name: None,
        };
        let objective_id = uuid::Uuid::new_v4().to_string();
        let context =
            meerkat_core::ToolDispatchContext::default().with_turn_metadata(BTreeMap::from([(
                meerkat_core::agent::TOOL_DISPATCH_OBJECTIVE_ID_KEY.to_string(),
                Value::String(objective_id.clone()),
            )]));
        let args = serde_json::value::RawValue::from_string(
            serde_json::json!({"task": "review"}).to_string(),
        )
        .expect("raw delegate args");

        meerkat_core::AgentToolDispatcher::dispatch_with_context(
            &dispatcher,
            meerkat_core::types::ToolCallView {
                id: "call-objective",
                name: "delegate",
                args: &args,
            },
            &context,
        )
        .await
        .expect("context-aware delegate dispatch");

        assert_eq!(
            observed_objective
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_deref(),
            Some(objective_id.as_str())
        );
    }

    #[test]
    fn delegate_idle_retire_secs_arg_is_stripped_and_parsed() {
        let mut args = serde_json::json!({
            "task": "inspect",
            "idle_retire_secs": 42
        });

        let parsed = delegate_idle_retire_override_from_args("delegate", &mut args)
            .expect("valid idle retire arg");

        assert_eq!(parsed, Some(DelegateIdleRetireOverride::Seconds(42)));
        assert!(args.get("idle_retire_secs").is_none());
    }

    #[test]
    fn delegate_idle_retire_secs_null_is_the_same_as_omitting() {
        let mut args = serde_json::json!({
            "task": "inspect",
            "idle_retire_secs": null
        });

        let parsed = delegate_idle_retire_override_from_args("delegate", &mut args)
            .expect("valid idle retire arg");

        assert_eq!(parsed, None);
        assert_eq!(args, serde_json::json!({"task": "inspect"}));
    }

    /// The model-facing `idle_retire_secs` argument, per tool: JSON null
    /// means "not set" exactly like an omitted field (as for meerkat's own
    /// `Option` tool args), an integer is that many seconds, and anything
    /// else is refused. The argument never reaches the inner tool.
    #[test]
    fn idle_retire_secs_arg_parses_per_tool_with_null_as_omitted() {
        /// What each tool's dispatch registers for one argument value.
        fn parse(
            tool: &str,
            args: &mut Value,
        ) -> Result<Option<DelegateIdleRetireOverride>, meerkat_core::ToolError> {
            match tool {
                "fork_off" => fork_off_idle_retire_override_from_args(tool, args).map(Some),
                _ => delegate_idle_retire_override_from_args(tool, args),
            }
        }

        for (tool, unset) in [
            // delegate: no override, the implicit mob's runtime default applies.
            ("delegate", None),
            // mob_spawn_member: not opted in, the member is kept.
            ("mob_spawn_member", None),
            // fork_off: opted in on the runtime default.
            ("fork_off", Some(DelegateIdleRetireOverride::RuntimeDefault)),
        ] {
            let mut omitted = serde_json::json!({"task": "inspect"});
            assert_eq!(
                parse(tool, &mut omitted).expect("omitted is valid"),
                unset,
                "{tool}: omitted"
            );
            assert_eq!(omitted, serde_json::json!({"task": "inspect"}));

            let mut null = serde_json::json!({"task": "inspect", "idle_retire_secs": null});
            assert_eq!(
                parse(tool, &mut null).expect("null is valid"),
                unset,
                "{tool}: null must mean the same as omitting"
            );
            assert_eq!(null, serde_json::json!({"task": "inspect"}));

            for seconds in [42_u64, 0] {
                let mut args = serde_json::json!({"task": "inspect", "idle_retire_secs": seconds});
                assert_eq!(
                    parse(tool, &mut args).expect("integer is valid"),
                    Some(DelegateIdleRetireOverride::Seconds(seconds)),
                    "{tool}: {seconds}"
                );
                assert_eq!(args, serde_json::json!({"task": "inspect"}));
            }

            for invalid in [
                serde_json::json!(-1),
                serde_json::json!("soon"),
                serde_json::json!("42"),
                serde_json::json!(1.5),
                serde_json::json!(true),
                serde_json::json!({"secs": 1}),
            ] {
                let mut args = serde_json::json!({"task": "inspect", "idle_retire_secs": invalid});
                let error = parse(tool, &mut args).expect_err("non-integer is refused");
                assert!(
                    error.to_string().contains(
                        "idle_retire_secs must be a non-negative integer, or null to use the default"
                    ),
                    "{tool}: {invalid}: {error}"
                );
            }
        }
    }

    /// Dispatch-level: `idle_retire_secs: null` on each tool yields that
    /// tool's default policy, the same as omitting it, and never reaches
    /// the inner tool. An integer still registers its override, which shows
    /// the registration path is live.
    #[tokio::test]
    async fn idle_retire_secs_null_dispatches_as_the_tools_default_policy() {
        use meerkat_core::AgentToolDispatcher as _;

        /// Stands in for meerkat's spawning tools: answers every call with
        /// the spawned member's typed result and records the forwarded args.
        struct SpawningTools(std::sync::Mutex<Vec<Value>>);
        #[async_trait::async_trait]
        impl meerkat_core::AgentToolDispatcher for SpawningTools {
            fn tools(&self) -> Arc<[Arc<meerkat_core::types::ToolDef>]> {
                Arc::from(Vec::new())
            }
            async fn dispatch(
                &self,
                call: meerkat_core::types::ToolCallView<'_>,
            ) -> Result<meerkat_core::ToolDispatchOutcome, meerkat_core::ToolError> {
                let args: Value = serde_json::from_str(call.args.get()).expect("args");
                let member = args["member_id"].as_str().expect("member_id").to_string();
                self.0
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(args);
                Ok(meerkat_core::ToolDispatchOutcome::sync_result(
                    meerkat_core::types::ToolResult::new(
                        call.id.to_string(),
                        serde_json::json!({"mob_id": "ops", "agent_identity": member}).to_string(),
                        false,
                    ),
                ))
            }
        }

        for (tool, unset) in [
            ("delegate", None),
            ("mob_spawn_member", None),
            ("fork_off", Some(DelegateIdleRetireOverride::RuntimeDefault)),
        ] {
            let overrides = ImplicitDelegateRetirementOverrides::default();
            let inner = Arc::new(SpawningTools(std::sync::Mutex::new(Vec::new())));
            let dispatcher = AutoWireParentMobToolDispatcher {
                inner: Arc::clone(&inner) as Arc<dyn meerkat_core::AgentToolDispatcher>,
                ..wrapper_with_overrides(overrides.clone())
            };
            for (member, idle_retire_secs) in [
                ("null-child", Value::Null),
                ("counted-child", serde_json::json!(90)),
            ] {
                let args = serde_json::value::RawValue::from_string(
                    serde_json::json!({
                        "mob_id": "ops",
                        "member_id": member,
                        "task": "inspect",
                        "idle_retire_secs": idle_retire_secs,
                    })
                    .to_string(),
                )
                .expect("raw args");
                dispatcher
                    .dispatch_with_context(
                        meerkat_core::types::ToolCallView {
                            id: "spawn-1",
                            name: tool,
                            args: &args,
                        },
                        &meerkat_core::ToolDispatchContext::default(),
                    )
                    .await
                    .unwrap_or_else(|error| panic!("{tool} dispatch: {error}"));
            }
            assert_eq!(
                overrides.get("ops", "null-child").await,
                unset,
                "{tool}: null must register the same policy as omitting"
            );
            assert_eq!(
                overrides.get("ops", "counted-child").await,
                Some(DelegateIdleRetireOverride::Seconds(90)),
                "{tool}: an integer still registers its override"
            );
            let forwarded = inner
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            assert_eq!(forwarded.len(), 2, "{tool}");
            assert!(
                forwarded
                    .iter()
                    .all(|args| args.get("idle_retire_secs").is_none()),
                "{tool}: idle_retire_secs is Meta's, never forwarded"
            );
        }
    }

    #[test]
    fn delegate_idle_retire_secs_omitted_inherits_runtime_default() {
        let mut args = serde_json::json!({"task": "inspect"});

        let parsed = delegate_idle_retire_override_from_args("delegate", &mut args)
            .expect("omitted idle retire arg");

        assert_eq!(parsed, None);
        assert_eq!(args, serde_json::json!({"task": "inspect"}));
    }

    #[test]
    fn delegate_idle_retire_secs_rejects_negative_or_fractional_values() {
        let mut negative = serde_json::json!({"task": "inspect", "idle_retire_secs": -1});
        let mut fractional = serde_json::json!({"task": "inspect", "idle_retire_secs": 1.5});

        assert!(delegate_idle_retire_override_from_args("delegate", &mut negative).is_err());
        assert!(delegate_idle_retire_override_from_args("delegate", &mut fractional).is_err());
    }

    #[test]
    fn mob_spawn_idle_retire_targets_use_args_when_result_omits_mob_id() {
        let args = serde_json::json!({
            "mob_id": "ops",
            "profile": "review-worker",
            "member_id": "review-worker-vibe-forward",
        });
        let fallback_targets = idle_retire_targets_from_spawn_args(&args);

        assert_eq!(
            fallback_targets,
            vec![IdleRetireTarget {
                mob_id: "ops".to_string(),
                member_id: "review-worker-vibe-forward".to_string(),
            }]
        );
        assert_eq!(
            idle_retire_targets_from_outcome_text(
                r#"{"agent_identity":"review-worker-vibe-forward","member_ref":"opaque"}"#,
                &fallback_targets,
            ),
            fallback_targets
        );
    }

    #[test]
    fn mob_spawn_idle_retire_targets_support_canonical_specs_shape() {
        let args = serde_json::json!({
            "mob_id": "ops",
            "specs": [
                {"profile": "person-worker", "agent_identity": "person-worker-a"},
                {"profile": "person-worker", "member_id": "person-worker-b", "mob_id": "other"}
            ]
        });
        let fallback_targets = idle_retire_targets_from_spawn_args(&args);

        assert_eq!(
            fallback_targets,
            vec![
                IdleRetireTarget {
                    mob_id: "ops".to_string(),
                    member_id: "person-worker-a".to_string(),
                },
                IdleRetireTarget {
                    mob_id: "other".to_string(),
                    member_id: "person-worker-b".to_string(),
                },
            ]
        );
        assert_eq!(
            idle_retire_targets_from_outcome_text(
                r#"{"members":[{"agent_identity":"person-worker-a"},{"agent_identity":"person-worker-b","mob_id":"other"}]}"#,
                &fallback_targets,
            ),
            fallback_targets
        );
    }

    #[tokio::test]
    async fn implicit_delegate_retirement_overrides_round_trip_per_member() {
        let overrides = ImplicitDelegateRetirementOverrides::default();

        overrides
            .set("mob-a", "worker-1", DelegateIdleRetireOverride::Seconds(12))
            .await;
        overrides
            .set("mob-a", "worker-2", DelegateIdleRetireOverride::Disabled)
            .await;

        assert_eq!(
            overrides.get("mob-a", "worker-1").await,
            Some(DelegateIdleRetireOverride::Seconds(12))
        );
        assert_eq!(
            overrides.get("mob-a", "worker-2").await,
            Some(DelegateIdleRetireOverride::Disabled)
        );
        assert_eq!(overrides.get("mob-a", "worker-3").await, None);
    }

    #[tokio::test]
    async fn mob_spawn_idle_retire_registration_uses_spawn_args_when_result_omits_mob_id() {
        let overrides = ImplicitDelegateRetirementOverrides::default();
        let dispatcher = wrapper_with_overrides(overrides.clone());
        let fallback_targets = idle_retire_targets_from_spawn_args(&serde_json::json!({
            "mob_id": "ops",
            "member_id": "review-worker-vibe-forward",
        }));
        let outcome =
            meerkat_core::ToolDispatchOutcome::sync_result(meerkat_core::types::ToolResult::new(
                "spawn-1".to_string(),
                r#"{"agent_identity":"review-worker-vibe-forward","member_ref":"opaque"}"#
                    .to_string(),
                false,
            ));

        dispatcher
            .register_idle_retire_override_from_outcome(
                &outcome,
                Some(DelegateIdleRetireOverride::Seconds(900)),
                &fallback_targets,
            )
            .await;

        assert_eq!(
            overrides.get("ops", "review-worker-vibe-forward").await,
            Some(DelegateIdleRetireOverride::Seconds(900))
        );
    }

    /// A runtime over the persistent mob `mob_id` in `dir` (its store at
    /// `mob.sqlite3`), launched with `authority`.
    async fn persistent_runtime_as(
        dir: &std::path::Path,
        mob_id: &str,
        authority: crate::mob_composition_manifest::CompositionAuthority,
    ) -> MobRuntime {
        let definition = meerkat_mob::MobDefinition::from_toml(&format!(
            "[mob]\nid = \"{mob_id}\"\n\n[profiles.worker]\nmodel = \"gpt-5.5\"\n"
        ))
        .expect("mob definition");
        let (storage, provenance) =
            crate::mob_composition_manifest::persistent_mob_storage(dir.join("mob.sqlite3"))
                .expect("mob storage");
        let mut spec =
            MobBootstrapSpec::persistent(definition, storage, provenance, dir.to_path_buf(), 4)
                .await
                .expect("persistent spec")
                .with_composition_authority(authority);
        spec.options.default_llm_client = Some(Arc::new(meerkat_client::TestClient::default()));
        MobRuntime::bootstrap(spec)
            .await
            .expect("bootstrap runtime")
    }

    /// How many `MobStopped` events the mob store in `dir` journals.
    fn mob_stopped_events(dir: &std::path::Path) -> i64 {
        rusqlite::Connection::open_with_flags(
            dir.join("mob.sqlite3"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .expect("open the mob store read-only")
        .query_row(
            "SELECT COUNT(*) FROM mob_events \
             WHERE CAST(event_json AS TEXT) LIKE '%\"type\":\"mob_stopped\"%'",
            [],
            |row| row.get(0),
        )
        .expect("count the MobStopped events")
    }

    /// A candidate's failed startup stops nothing: over a persistent mob the
    /// authoritative launch created and shut down, it only shuts the mob
    /// down with the keep, which closes the mob's admission, and the mob
    /// store journals no `MobStopped`. Control: an authoritative launch's
    /// failed startup stops the mob as before, journaling `MobStopped` and
    /// leaving its admission open.
    #[tokio::test]
    async fn a_failed_candidate_startup_journals_no_stop() {
        use crate::mob_composition_manifest::CompositionAuthority;

        let dir = tempfile::tempdir().expect("temp");
        let mob_id = "failed-startup-journal";
        let created =
            persistent_runtime_as(dir.path(), mob_id, CompositionAuthority::Authoritative).await;
        created
            .handle()
            .shutdown()
            .await
            .expect("graceful shutdown");
        drop(created);
        assert_eq!(
            mob_stopped_events(dir.path()),
            0,
            "a Shutdown journals no stop"
        );

        let candidate =
            persistent_runtime_as(dir.path(), mob_id, CompositionAuthority::NonAuthoritative).await;
        let stop = candidate.stop_after_failed_startup().await;
        assert!(stop.is_ok(), "{stop:?}");
        assert!(
            candidate.handle().admission_closed().borrow().is_some(),
            "the candidate's teardown shuts its mob down"
        );
        drop(candidate);
        assert_eq!(
            mob_stopped_events(dir.path()),
            0,
            "the candidate's failed startup journals no MobStopped"
        );

        let control =
            persistent_runtime_as(dir.path(), mob_id, CompositionAuthority::Authoritative).await;
        let stop = control.stop_after_failed_startup().await;
        assert!(stop.is_ok(), "{stop:?}");
        assert!(
            control.handle().admission_closed().borrow().is_none(),
            "an authoritative failed startup only stops its mob"
        );
        assert_eq!(
            mob_stopped_events(dir.path()),
            1,
            "an authoritative failed startup journals its stop"
        );
        let _ = control.handle().shutdown().await;
    }

    /// A candidate launch's mob Shutdowns keep every member's work as the
    /// store holds it, and so does a candidate's failed startup: it stops
    /// nothing and shuts the mob down with the keep, which closes the mob's
    /// admission. An authoritative launch's failed startup stops the mob as
    /// before and leaves its admission open, and its Shutdowns take
    /// Meerkat's default.
    #[tokio::test]
    async fn a_failed_candidate_startup_shuts_its_mob_down_with_the_keep() {
        use crate::mob_composition_manifest::CompositionAuthority;

        let deadline = std::time::Instant::now();
        let candidate =
            super::mob_shutdown_options_for(CompositionAuthority::NonAuthoritative, deadline);
        assert_eq!(
            (candidate.queued_inputs, candidate.deadline),
            (meerkat_mob::QueuedInputsOnShutdown::Keep, Some(deadline))
        );
        let authoritative =
            super::mob_shutdown_options_for(CompositionAuthority::Authoritative, deadline);
        assert_eq!(
            authoritative.queued_inputs,
            meerkat_mob::QueuedInputsOnShutdown::default()
        );

        for (mob_id, authority, shut_down) in [
            (
                "failed-candidate-startup",
                CompositionAuthority::NonAuthoritative,
                true,
            ),
            (
                "failed-authoritative-startup",
                CompositionAuthority::Authoritative,
                false,
            ),
        ] {
            let dir = tempfile::tempdir().expect("temp");
            let definition = meerkat_mob::MobDefinition::from_toml(&format!(
                "[mob]\nid = \"{mob_id}\"\n\n[profiles.worker]\nmodel = \"gpt-5.5\"\n"
            ))
            .expect("mob definition");
            let spec = MobBootstrapSpec::ephemeral(
                definition,
                meerkat_mob::MobStorage::in_memory(),
                dir.path().to_path_buf(),
                4,
                None,
            )
            .expect("bootstrap spec")
            .with_options(MobBootstrapOptions {
                allow_ephemeral_sessions: true,
                notify_orchestrator_on_resume: true,
                default_llm_client: Some(Arc::new(meerkat_client::TestClient::default())),
            })
            .with_composition_authority(authority);
            let runtime = MobRuntime::bootstrap(spec)
                .await
                .expect("bootstrap runtime");
            let stop = runtime.stop_after_failed_startup().await;
            assert!(stop.is_ok(), "{mob_id}: {stop:?}");
            assert_eq!(
                runtime.handle().admission_closed().borrow().is_some(),
                shut_down,
                "{mob_id}: a candidate's failed startup shuts its mob down; an authoritative \
                 one only stops it"
            );
        }
    }

    async fn bootstrap_opt_in_runtime(mob_id: &str, dir: &std::path::Path) -> MobRuntime {
        let definition = meerkat_mob::MobDefinition::from_toml(&format!(
            "[mob]\nid = \"{mob_id}\"\n\n[profiles.worker]\nmodel = \"gpt-5.5\"\n\n\
             [profiles.worker.tools]\ncomms = true\n"
        ))
        .expect("mob definition");
        let spec = MobBootstrapSpec::ephemeral(
            definition,
            meerkat_mob::MobStorage::in_memory(),
            dir.to_path_buf(),
            4,
            None,
        )
        .expect("bootstrap spec")
        .with_options(MobBootstrapOptions {
            allow_ephemeral_sessions: true,
            notify_orchestrator_on_resume: true,
            default_llm_client: Some(Arc::new(meerkat_client::TestClient::default())),
        });
        MobRuntime::bootstrap(spec)
            .await
            .expect("bootstrap runtime")
    }

    async fn seat_worker(handle: &MobHandle, member: &str) -> meerkat_core::types::SessionId {
        handle
            .ensure_member(meerkat_mob::SpawnMemberSpec::new(
                meerkat_mob::ProfileName::from("worker"),
                meerkat_mob::AgentIdentity::from(member),
            ))
            .await
            .expect("seat member");
        handle
            .resolve_bridge_session_id(&meerkat_mob::AgentIdentity::from(member))
            .await
            .expect("member session")
    }

    /// #366: every spelling of one member reaches the same resolved tools.
    /// The roster is keyed by the encoded durable identity, so the lookup has
    /// to decode a supplied id the way `model_routing_status_for_member` does.
    /// A current runtime alias (`rt:{identity}:{generation}`) handed straight
    /// to `mob_member_id` names a roster row nobody owns, and the healthy
    /// member reads as having no current session. Each snapshot echoes the
    /// spelling it was asked with.
    #[tokio::test]
    async fn resolved_tools_for_member_resolves_every_spelling_of_one_member() {
        const DURABLE: &str = "review:singleton";
        let dir = tempfile::tempdir().expect("tempdir");
        let runtime = bootstrap_opt_in_runtime("resolved-tools-member-alias", dir.path()).await;
        let handle = runtime.handle();
        let roster_id = crate::member_comms_id::mob_member_id_str(DURABLE).into_owned();
        let session_id = seat_worker(&handle, &roster_id).await;

        let durable = resolved_tools_for_member(&handle, runtime.session_service(), DURABLE)
            .await
            .expect("the durable identity resolves its seated member");
        assert_eq!(durable.session_id, session_id.to_string());
        assert!(
            !durable.tools.is_empty(),
            "the seated member exposes tools: {durable:?}"
        );

        for supplied in [format!("rt:{DURABLE}:0"), roster_id.clone()] {
            let resolved = resolved_tools_for_member(&handle, runtime.session_service(), &supplied)
                .await
                .map_err(|error| error.to_string());
            assert_eq!(
                resolved,
                Ok(ResolvedToolsSnapshot {
                    identity: supplied.clone(),
                    session_id: durable.session_id.clone(),
                    tools: durable.tools.clone(),
                }),
                "{supplied} must resolve the member {DURABLE} names, with the same session and tools"
            );
        }
        let _ = handle.shutdown().await;
    }

    /// The image-capability guard is a read: like `resolved_tools_for_member`
    /// it reaches the member a durable identity, its current runtime alias or
    /// its encoded roster id names. Before, the alias and the encoded id read
    /// as "member not found" for a healthy, image-capable member.
    #[tokio::test]
    async fn image_guard_reads_every_spelling_of_one_member() {
        const DURABLE: &str = "review:singleton";
        let dir = tempfile::tempdir().expect("tempdir");
        let runtime = bootstrap_opt_in_runtime("image-guard-member-alias", dir.path()).await;
        let handle = runtime.handle();
        let roster_id = crate::member_comms_id::mob_member_id_str(DURABLE).into_owned();
        seat_worker(&handle, &roster_id).await;
        let image = meerkat_core::ContentInput::Blocks(vec![meerkat_core::ContentBlock::Image {
            media_type: "image/png".to_string(),
            data: meerkat_core::types::ImageData::Inline {
                data: "iVBORw0KGgo=".to_string(),
            },
        }]);

        for supplied in [DURABLE.to_string(), format!("rt:{DURABLE}:0"), roster_id] {
            let verdict =
                assert_member_accepts_images(&handle, runtime.session_service(), &supplied, &image)
                    .await
                    .map_err(|error| error.to_string());
            assert_eq!(
                verdict,
                Ok(()),
                "{supplied} must reach the image-capable member {DURABLE} names"
            );
        }
        let _ = handle.shutdown().await;
    }

    /// The image checks' source: a catalog model keeps its catalog answer;
    /// a self-hosted or mob-declared model answers by its `vision` flag, and
    /// only through the host config's registry; a model no source registers,
    /// or a registered one under another provider, stays refused.
    #[test]
    fn member_model_registry_answers_self_hosted_and_declared_models() {
        let definition = meerkat_mob::MobDefinition::from_toml(
            r#"
[mob]
id = "image-capability-source"

[models.declared-vision]
provider = "openai"
vision = true

[profiles.catalog]
model = "gpt-5.5"

[profiles.seeing]
model = "seeing-model"
provider = "self_hosted"

[profiles.blind]
model = "blind-model"
provider = "self_hosted"

[profiles.declared]
model = "declared-vision"

[profiles.unknown]
model = "unknown-model"
provider = "self_hosted"
"#,
        )
        .expect("mob definition");
        let config: meerkat::Config = toml::from_str(
            r#"
[self_hosted]
default_model = "seeing-model"

[self_hosted.servers.loop]
base_url = "http://127.0.0.1:9"

[self_hosted.models.seeing-model]
server = "loop"
remote_model = "seeing-model"
display_name = "Seeing"
family = "seeing-model"
vision = true

[self_hosted.models.blind-model]
server = "loop"
remote_model = "blind-model"
display_name = "Blind"
family = "blind-model"
"#,
        )
        .expect("host config");
        let registry =
            member_model_registry(Some(&config), &definition).expect("the registry assembles");
        let judged = |role: &str| {
            model_capabilities_for_role_with_registry(Some(&registry), &definition, role)
                .image_input
        };
        let by_model = |provider: Provider, model: &str| {
            model_capabilities_for_model_with_registry(Some(&registry), provider, model).image_input
        };

        assert_eq!(
            judged("catalog"),
            model_capabilities_for_role(&definition, "catalog").image_input
        );
        assert_eq!(
            by_model(Provider::OpenAI, "gpt-5.5"),
            model_capabilities_for_model(Provider::OpenAI, "gpt-5.5").image_input
        );
        assert!(judged("seeing"));
        assert!(by_model(Provider::SelfHosted, "seeing-model"));
        assert!(!judged("blind"));
        assert!(!by_model(Provider::SelfHosted, "blind-model"));
        assert!(judged("declared"));
        assert!(by_model(Provider::OpenAI, "declared-vision"));
        assert!(!judged("unknown"));
        assert!(!by_model(Provider::SelfHosted, "unknown-model"));
        assert!(!by_model(Provider::OpenAI, "seeing-model"));

        // Without a host config the static catalog alone answers.
        assert!(member_model_registry(None, &definition).is_none());
        assert!(!model_capabilities_for_role(&definition, "seeing").image_input);
        assert!(!model_capabilities_for_role(&definition, "declared").image_input);
    }

    /// A direct send is an action with no identity authority to consult. It
    /// decodes an encoded roster id instead of encoding it twice, and it
    /// refuses a generated runtime alias, which only the identity authority
    /// can pin to a generation, instead of delivering to the durable row.
    #[tokio::test]
    async fn direct_send_decodes_roster_ids_and_refuses_generated_aliases() {
        const DURABLE: &str = "review:singleton";
        let dir = tempfile::tempdir().expect("tempdir");
        let definition = meerkat_mob::MobDefinition::from_toml(
            "[mob]\nid = \"direct-send-member-alias\"\n\n[profiles.worker]\nmodel = \"gpt-5.5\"\n\
             external_addressable = true\n\n[profiles.worker.tools]\ncomms = true\n",
        )
        .expect("mob definition");
        let spec = MobBootstrapSpec::ephemeral(
            definition,
            meerkat_mob::MobStorage::in_memory(),
            dir.path().to_path_buf(),
            4,
            None,
        )
        .expect("bootstrap spec")
        .with_options(MobBootstrapOptions {
            allow_ephemeral_sessions: true,
            notify_orchestrator_on_resume: true,
            default_llm_client: Some(Arc::new(meerkat_client::TestClient::default())),
        });
        let runtime = MobRuntime::bootstrap(spec)
            .await
            .expect("bootstrap runtime");
        let handle = runtime.handle();
        let roster_id = crate::member_comms_id::mob_member_id_str(DURABLE).into_owned();
        let session_id = seat_worker(&handle, &roster_id).await.to_string();

        for supplied in [DURABLE, roster_id.as_str()] {
            let sent = send_message_on_mob(&handle, supplied, "hello".to_string())
                .await
                .map_err(|error| error.to_string());
            assert_eq!(
                sent,
                Ok(session_id.clone()),
                "{supplied} must deliver to the member {DURABLE} names"
            );
        }
        let refused = send_message_on_mob(&handle, &format!("rt:{DURABLE}:0"), "hello".to_string())
            .await
            .map_err(|error| error.to_string());
        assert_eq!(
            refused,
            Err(MobRuntimeError::InvalidInput(
                "generated member alias requires identity authority"
            )
            .to_string()),
            "a direct send must not pick an incarnation for a generated alias"
        );
        let _ = handle.shutdown().await;
    }

    /// Review of #442 (hardening): a forker respawning its own child with the
    /// agent-facing `mob_respawn` tool keeps the child's idle-retire opt-in,
    /// like every other Meta respawn surface.
    #[tokio::test(flavor = "multi_thread")]
    async fn agent_mob_respawn_tool_carries_the_childs_opt_in() {
        use meerkat_core::AgentToolDispatcher as _;

        /// Stands in for meerkat's `mob_respawn`: respawns the named member.
        struct RespawningTools(MobHandle);
        #[async_trait::async_trait]
        impl meerkat_core::AgentToolDispatcher for RespawningTools {
            fn tools(&self) -> Arc<[Arc<meerkat_core::types::ToolDef>]> {
                Arc::from(Vec::new())
            }
            async fn dispatch(
                &self,
                call: meerkat_core::types::ToolCallView<'_>,
            ) -> Result<meerkat_core::ToolDispatchOutcome, meerkat_core::ToolError> {
                let args: Value = serde_json::from_str(call.args.get()).expect("args");
                let member = args["agent_identity"].as_str().expect("agent_identity");
                self.0
                    .respawn(meerkat_mob::AgentIdentity::from(member), None)
                    .await
                    .map_err(|error| {
                        meerkat_core::ToolError::invalid_arguments(call.name, error.to_string())
                    })?;
                Ok(meerkat_core::ToolDispatchOutcome::sync_result(
                    meerkat_core::types::ToolResult::new(
                        call.id.to_string(),
                        "{}".to_string(),
                        false,
                    ),
                ))
            }
        }

        const MOB: &str = "agent-respawn-carry";
        let dir = tempfile::tempdir().expect("tempdir");
        let runtime = bootstrap_opt_in_runtime(MOB, dir.path()).await;
        let handle = runtime.handle();
        let before = seat_worker(&handle, "fork-child").await;
        let overrides = runtime
            .implicit_delegate_retirement_overrides()
            .expect("overrides");
        let policy = DelegateIdleRetireOverride::Seconds(3600);
        overrides.set(MOB, "fork-child", policy).await;

        let dispatcher = AutoWireParentMobToolDispatcher {
            inner: Arc::new(RespawningTools(handle.clone())),
            ..wrapper_with_overrides(overrides.clone())
        };
        let args = serde_json::value::RawValue::from_string(
            serde_json::json!({"mob_id": MOB, "agent_identity": "fork-child"}).to_string(),
        )
        .expect("raw args");
        dispatcher
            .dispatch_with_context(
                meerkat_core::types::ToolCallView {
                    id: "respawn-1",
                    name: "mob_respawn",
                    args: &args,
                },
                &meerkat_core::ToolDispatchContext::default(),
            )
            .await
            .expect("mob_respawn");
        let after = handle
            .resolve_bridge_session_id(&meerkat_mob::AgentIdentity::from("fork-child"))
            .await
            .expect("respawned session");
        assert_ne!(after, before);
        let carried = overrides
            .get_bound(MOB, "fork-child")
            .await
            .expect("the opt-in survives the respawn");
        assert_eq!(carried.policy, policy);
        assert_eq!(carried.session_id, Some(after));
        let _ = runtime.handle().shutdown().await;
    }

    /// A `fork_off` result carries `mob_id` and `agent_identity` (upstream
    /// `ForkOffResult`), so the child registers on the runtime default with
    /// no fallback targets and no new parsing.
    #[tokio::test]
    async fn fork_off_registration_opts_the_child_into_runtime_default_retirement() {
        let overrides = ImplicitDelegateRetirementOverrides::default();
        let dispatcher = wrapper_with_overrides(overrides.clone());
        let fork_result = concat!(
            r#"{"mob_id":"ops","source_member_id":"lead","agent_identity":"lead-fork-1","#,
            r#""member_ref":"opaque","fork_session_id":"s-child","turn_session_id":"s-child","#,
            r#""cache_inheritance":{"status":"unavailable","message_count":4,"#,
            r#""reason":"target_identity_unresolved"},"bounded_result":{},"usage":{},"#,
            r#""turns":1,"tool_calls":0}"#
        );
        let outcome =
            meerkat_core::ToolDispatchOutcome::sync_result(meerkat_core::types::ToolResult::new(
                "fork-1".to_string(),
                fork_result.to_string(),
                false,
            ));

        dispatcher
            .register_idle_retire_override_from_outcome(
                &outcome,
                Some(DelegateIdleRetireOverride::RuntimeDefault),
                &[],
            )
            .await;
        assert_eq!(
            overrides.get("ops", "lead-fork-1").await,
            Some(DelegateIdleRetireOverride::RuntimeDefault)
        );

        // An explicit policy still wins, and a failed fork registers nothing.
        dispatcher
            .register_idle_retire_override_from_outcome(
                &outcome,
                Some(DelegateIdleRetireOverride::Disabled),
                &[],
            )
            .await;
        assert_eq!(
            overrides.get("ops", "lead-fork-1").await,
            Some(DelegateIdleRetireOverride::Disabled)
        );
        let failed =
            meerkat_core::ToolDispatchOutcome::sync_result(meerkat_core::types::ToolResult::new(
                "fork-2".to_string(),
                r#"{"mob_id":"ops","agent_identity":"lead-fork-2"}"#.to_string(),
                true,
            ));
        dispatcher
            .register_idle_retire_override_from_outcome(
                &failed,
                Some(DelegateIdleRetireOverride::RuntimeDefault),
                &[],
            )
            .await;
        assert_eq!(overrides.get("ops", "lead-fork-2").await, None);
    }

    #[test]
    fn image_generation_substrate_defaults_off_for_inline_profiles() {
        let definition = meerkat_mob::MobDefinition::from_toml(
            r#"
[mob]
id = "test"

[profiles.worker]
model = "gpt-5.5"

[profiles.worker.tools]
builtins = true
"#,
        )
        .unwrap_or_else(|e| panic!("{e}"));

        assert!(
            !mob_definition_may_use_image_generation(&definition),
            "inline profiles should not wire the image substrate unless a profile opts in"
        );
    }

    /// Normalization is IDEMPOTENT at the byte level, proven rather than argued.
    ///
    /// Piece 2 normalizes each definition once, before both the pre-prepare owner
    /// callback and MobRuntimeSpec construction. `MobRuntime::prepare` then
    /// applies `auto_mark_declared_resume_overrides` again on its own copy. If
    /// that second application changed anything, the callback and the runtime
    /// would disagree about the definition - the byte-identity violation that got
    /// the first wiring attempt rejected.
    ///
    /// An idempotency ARGUMENT is not evidence: a transform that happens to be
    /// idempotent for the cases I construct is not idempotent. So this compares
    /// exact serialized bytes.
    ///
    /// NON-VACUITY IS ASSERTED FIRST. If the first application changed nothing,
    /// once-versus-twice would be trivially equal and this test would pass while
    /// proving nothing about a transform that never fired. The fixture is chosen
    /// so the first pass DOES complete an explicit identity-pair override.
    #[test]
    fn definition_normalization_is_byte_identical_on_reapplication() {
        let base = meerkat_mob::MobDefinition::from_toml(
            r#"
[mob]
id = "normalization-idempotence"

[profiles.worker]
model = "gpt-5.5"
resume_overrides = ["provider"]

[profiles.worker.tools]
builtins = true
"#,
        )
        .unwrap_or_else(|e| panic!("{e}"));

        let raw_bytes = serde_json::to_vec(&base).unwrap_or_else(|e| panic!("{e}"));

        // `base` is not read after this: raw_bytes was already taken from it,
        // so move rather than clone.
        let mut once = base;
        auto_mark_declared_resume_overrides(&mut once);
        let once_bytes = serde_json::to_vec(&once).unwrap_or_else(|e| panic!("{e}"));

        assert_ne!(
            raw_bytes, once_bytes,
            "the fixture must be one the transform ACTUALLY changes, or once-vs-twice equality \
             proves nothing about idempotence"
        );

        let mut twice = once.clone();
        auto_mark_declared_resume_overrides(&mut twice);
        let twice_bytes = serde_json::to_vec(&twice).unwrap_or_else(|e| panic!("{e}"));

        assert_eq!(
            once_bytes, twice_bytes,
            "re-applying normalization must be byte-identical: prepare applies it again on its own \
             copy, so any change here means the owner callback and the runtime disagree about the \
             definition"
        );
    }

    #[test]
    fn image_generation_substrate_follows_profile_tool_config() {
        let definition = meerkat_mob::MobDefinition::from_toml(
            r#"
[mob]
id = "test"

[profiles.commander]
model = "gpt-5.5"

[profiles.commander.tools]
builtins = true
image_generation = true

[profiles.investigator]
model = "gpt-5.5"

[profiles.investigator.tools]
builtins = true
image_generation = false
"#,
        )
        .unwrap_or_else(|e| panic!("{e}"));

        let commander = definition.profiles["commander"].as_inline().unwrap();
        let investigator = definition.profiles["investigator"].as_inline().unwrap();
        assert!(commander.tools.image_generation);
        assert!(!investigator.tools.image_generation);
        assert!(
            mob_definition_may_use_image_generation(&definition),
            "one opt-in profile is enough to wire substrate; Meerkat gates visibility per profile"
        );
    }

    #[test]
    fn shell_substrate_defaults_off_for_inline_profiles() {
        let definition = meerkat_mob::MobDefinition::from_toml(
            r#"
[mob]
id = "test"

[profiles.worker]
model = "gpt-5.5"

[profiles.worker.tools]
builtins = true
"#,
        )
        .unwrap_or_else(|e| panic!("{e}"));

        assert!(
            !mob_definition_may_use_shell(&definition),
            "inline profiles should not wire the shell substrate unless a profile opts in"
        );
    }

    #[test]
    fn shell_substrate_follows_profile_tool_config() {
        let definition = meerkat_mob::MobDefinition::from_toml(
            r#"
[mob]
id = "test"

[profiles.domain]
model = "gpt-5.5"

[profiles.domain.tools]
builtins = true
shell = false

[profiles.security]
model = "gpt-5.5"

[profiles.security.tools]
builtins = true
shell = true
"#,
        )
        .unwrap_or_else(|e| panic!("{e}"));

        let domain = definition.profiles["domain"].as_inline().unwrap();
        let security = definition.profiles["security"].as_inline().unwrap();
        assert!(!domain.tools.shell);
        assert!(security.tools.shell);
        assert!(
            mob_definition_may_use_shell(&definition),
            "one opt-in profile is enough to wire substrate; Meerkat gates visibility per profile"
        );
    }

    fn shell_visibility_request(
        state: Option<meerkat_core::SessionToolVisibilityState>,
        initial_tool_filter: Option<meerkat_core::ToolFilter>,
    ) -> CreateSessionRequest {
        let resume_session = state.map(|state| {
            session_with_visibility_state(meerkat_core::types::SessionId::new(), state)
        });

        CreateSessionRequest {
            model: "gpt-5.5".to_string(),
            prompt: meerkat_core::ContentInput::Text("test".to_string()),
            system_prompt: meerkat_core::config::SystemPromptOverride::Inherit,
            max_tokens: None,
            event_tx: None,
            initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
            build: Some(meerkat_core::service::SessionBuildOptions {
                override_builtins: meerkat_core::ToolCategoryOverride::Disable,
                override_shell: meerkat_core::ToolCategoryOverride::Enable,
                initial_tool_filter,
                resume_session,
                ..Default::default()
            }),
            labels: None,
            deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::default(),
            injected_context: Vec::new(),
        }
    }

    fn session_with_visibility_state(
        session_id: meerkat_core::types::SessionId,
        state: meerkat_core::SessionToolVisibilityState,
    ) -> meerkat_core::Session {
        let mut value = serde_json::to_value(meerkat_core::Session::with_id(session_id))
            .expect("serialize session");
        value
            .as_object_mut()
            .expect("session document")
            .entry("metadata")
            .or_insert_with(|| serde_json::json!({}))
            .as_object_mut()
            .expect("session metadata")
            .insert(
                meerkat_core::SESSION_TOOL_VISIBILITY_STATE_KEY.to_string(),
                serde_json::to_value(state).expect("serialize visibility state"),
            );
        serde_json::from_value(value).expect("deserialize resumed session")
    }

    fn downstream_security_visibility_fixture(
        snapshot: &str,
    ) -> (
        meerkat_core::types::SessionId,
        serde_json::Value,
        meerkat_core::SessionToolVisibilityState,
    ) {
        use base64::Engine as _;

        const BUNDLE: &[u8] =
            include_bytes!("../tests/fixtures/security_idempotency/security-head-evolution.json");
        let bundle: serde_json::Value = serde_json::from_slice(BUNDLE).expect("fixture bundle");
        let table = &bundle[snapshot]["continuity_session_heads"];
        let columns = table["columns"].as_array().expect("fixture columns");
        let head_index = columns
            .iter()
            .position(|column| column.as_str() == Some("head_json"))
            .expect("head_json column");
        let encoded = table["rows"][0][head_index]["b64"]
            .as_str()
            .expect("head_json base64");
        let head_bytes = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .expect("decode head_json");
        let head: serde_json::Value =
            serde_json::from_slice(&head_bytes).expect("deserialize head_json");
        let session_id =
            meerkat_core::types::SessionId::parse(head["id"].as_str().expect("fixture session id"))
                .expect("parse fixture session id");
        let raw_visibility =
            head["metadata"][meerkat_core::SESSION_TOOL_VISIBILITY_STATE_KEY].clone();
        let visibility = serde_json::from_value(raw_visibility.clone())
            .expect("deserialize fixture visibility state");
        (session_id, raw_visibility, visibility)
    }

    fn legacy_shell_allow_filter(order_reversed: bool) -> meerkat_core::ToolFilter {
        let mut names = LEGACY_SHELL_AND_COMMS_TOOL_NAMES
            .iter()
            .map(|name| (*name).to_string())
            .collect::<Vec<_>>();
        if order_reversed {
            names.reverse();
        }
        meerkat_core::ToolFilter::Allow(names.into_iter().collect())
    }

    fn heal_embedded_visibility_fixture(req: &mut CreateSessionRequest) {
        let persisted = req
            .build
            .as_ref()
            .and_then(|build| build.resume_session.clone());
        heal_legacy_shell_tool_visibility(req, persisted.as_ref());
    }

    #[test]
    fn fresh_shell_tooling_keeps_native_category_independent_of_builtins() {
        let mut req = shell_visibility_request(None, None);

        heal_embedded_visibility_fixture(&mut req);

        let build = req.build.expect("build options");
        assert_eq!(
            build.override_shell,
            meerkat_core::ToolCategoryOverride::Enable
        );
        assert_eq!(
            build.override_builtins,
            meerkat_core::ToolCategoryOverride::Disable,
            "native shell tooling must not enable broad builtins"
        );
        assert_eq!(build.initial_tool_filter, None);
    }

    #[test]
    fn shell_and_builtins_enabled_request_is_unchanged() {
        let mut req = shell_visibility_request(None, None);
        req.build.as_mut().expect("build options").override_builtins =
            meerkat_core::ToolCategoryOverride::Enable;

        heal_embedded_visibility_fixture(&mut req);

        let build = req.build.expect("build options");
        assert_eq!(
            build.override_shell,
            meerkat_core::ToolCategoryOverride::Enable
        );
        assert_eq!(
            build.override_builtins,
            meerkat_core::ToolCategoryOverride::Enable
        );
        assert_eq!(build.initial_tool_filter, None);
    }

    #[tokio::test]
    async fn native_shell_resolved_catalog_preserves_mixed_external_surfaces() {
        let external_names = [
            "mcp__example__probe",
            "list_members",
            "memory_search",
            "meerkat_schedule_list",
            "generate_image",
        ];
        let tools = external_names
            .iter()
            .map(|name| {
                Arc::new(meerkat_core::types::ToolDef::new(
                    *name,
                    "mixed-surface fixture",
                    serde_json::json!({"type": "object"}),
                ))
            })
            .collect::<Vec<_>>()
            .into();
        let temp = tempfile::tempdir().expect("temp dir");
        let factory = meerkat::AgentFactory::new(temp.path());
        let mut builder = meerkat::FactoryAgentBuilder::new(factory, meerkat::Config::default());
        builder.default_llm_client = Some(Arc::new(meerkat_client::TestClient::default()));
        let service = meerkat_session::EphemeralSessionService::new(builder, 4);
        let mut req = shell_visibility_request(None, None);
        req.build.as_mut().expect("build options").external_tools =
            Some(Arc::new(NamedDispatcher { tools }));

        let result = meerkat_core::service::SessionService::create_session(&service, req)
            .await
            .expect("create deferred shell-only session");
        let snapshot = service
            .tool_scope_snapshot(&result.session_id)
            .await
            .expect("read tool scope")
            .expect("live tool scope");
        let visible = snapshot
            .visible_names
            .into_iter()
            .map(meerkat_core::types::ToolName::into_string)
            .collect::<std::collections::BTreeSet<_>>();
        let expected = NATIVE_SHELL_TOOL_NAMES
            .iter()
            .copied()
            .chain(external_names)
            .map(String::from)
            .collect::<std::collections::BTreeSet<_>>();

        assert_eq!(
            visible, expected,
            "native shell must add exactly four shell tools without filtering MCP, mob, memory, schedule, or image surfaces"
        );
        for broad_builtin in ["task_list", "task_create", "apply_patch", "browse_skills"] {
            assert!(!visible.contains(broad_builtin), "unexpected broad builtin");
        }
    }

    #[test]
    fn legacy_shell_visibility_heal_matches_set_order_and_preserves_base_filters() {
        let original_state = meerkat_core::SessionToolVisibilityState {
            capability_base_filter: meerkat_core::ToolFilter::Deny(
                ["view_image".to_string()].into_iter().collect(),
            ),
            inherited_base_filter: meerkat_core::ToolFilter::Deny(
                ["operator_only".to_string()].into_iter().collect(),
            ),
            active_filter: legacy_shell_allow_filter(false),
            staged_filter: legacy_shell_allow_filter(true),
            active_revision: 7,
            staged_revision: 8,
            ..Default::default()
        };
        let mut req = shell_visibility_request(Some(original_state.clone()), None);

        heal_embedded_visibility_fixture(&mut req);

        let build = req.build.as_ref().expect("build options");
        assert_eq!(
            build.initial_tool_filter,
            Some(meerkat_core::ToolFilter::All)
        );
        assert_eq!(
            build
                .resume_session
                .as_ref()
                .expect("resumed session")
                .try_tool_visibility_state()
                .expect("parse visibility state"),
            Some(original_state),
            "Meta must not rewrite capability, inherited, witness, or revision state"
        );

        heal_embedded_visibility_fixture(&mut req);
        assert_eq!(
            req.build.expect("build options").initial_tool_filter,
            Some(meerkat_core::ToolFilter::All),
            "the repair must be idempotent"
        );
    }

    #[test]
    fn legacy_shell_visibility_heal_accepts_omitted_staged_filter_default() {
        let state = meerkat_core::SessionToolVisibilityState {
            active_filter: legacy_shell_allow_filter(false),
            staged_filter: meerkat_core::ToolFilter::All,
            ..Default::default()
        };
        let mut req = shell_visibility_request(Some(state), None);

        heal_embedded_visibility_fixture(&mut req);

        assert_eq!(
            req.build.expect("build options").initial_tool_filter,
            Some(meerkat_core::ToolFilter::All)
        );
    }

    #[test]
    fn legacy_shell_visibility_heal_respects_explicit_current_filter() {
        let state = meerkat_core::SessionToolVisibilityState {
            active_filter: legacy_shell_allow_filter(false),
            staged_filter: legacy_shell_allow_filter(false),
            ..Default::default()
        };
        let explicit = meerkat_core::ToolFilter::Deny(
            ["deliberately_hidden".to_string()].into_iter().collect(),
        );
        let mut req = shell_visibility_request(Some(state), Some(explicit.clone()));

        heal_embedded_visibility_fixture(&mut req);

        assert_eq!(
            req.build.expect("build options").initial_tool_filter,
            Some(explicit)
        );
    }

    #[test]
    fn legacy_shell_visibility_heal_ignores_nonmatching_durable_filters() {
        let legacy_superset = LEGACY_SHELL_AND_COMMS_TOOL_NAMES
            .iter()
            .map(|name| (*name).to_string())
            .chain(std::iter::once("memory_search".to_string()))
            .collect();
        for state in [
            meerkat_core::SessionToolVisibilityState {
                active_filter: meerkat_core::ToolFilter::Allow(
                    ["shell".to_string()].into_iter().collect(),
                ),
                staged_filter: legacy_shell_allow_filter(false),
                ..Default::default()
            },
            meerkat_core::SessionToolVisibilityState {
                active_filter: meerkat_core::ToolFilter::Allow(legacy_superset),
                staged_filter: legacy_shell_allow_filter(false),
                ..Default::default()
            },
            meerkat_core::SessionToolVisibilityState {
                active_filter: meerkat_core::ToolFilter::Allow(
                    [
                        "alpha", "bravo", "charlie", "delta", "echo", "foxtrot", "golf", "hotel",
                    ]
                    .into_iter()
                    .map(String::from)
                    .collect(),
                ),
                staged_filter: legacy_shell_allow_filter(false),
                ..Default::default()
            },
            meerkat_core::SessionToolVisibilityState {
                active_filter: legacy_shell_allow_filter(false),
                staged_filter: meerkat_core::ToolFilter::Deny(
                    ["task_create".to_string()].into_iter().collect(),
                ),
                ..Default::default()
            },
            meerkat_core::SessionToolVisibilityState::default(),
            meerkat_core::SessionToolVisibilityState {
                capability_base_filter: meerkat_core::ToolFilter::Deny(
                    ["view_image".to_string()].into_iter().collect(),
                ),
                ..Default::default()
            },
        ] {
            let mut req = shell_visibility_request(Some(state), None);
            heal_embedded_visibility_fixture(&mut req);
            assert_eq!(req.build.expect("build options").initial_tool_filter, None);
        }
    }

    #[tokio::test]
    async fn pre_build_heal_reads_persisted_visibility_behind_id_only_resume_stub() {
        let (session_id, raw_visibility, state) =
            downstream_security_visibility_fixture("post_boot1_fresh");
        let active_order = raw_visibility["active_filter"]["Allow"]
            .as_array()
            .expect("active Allow array");
        let staged_order = raw_visibility["staged_filter"]["Allow"]
            .as_array()
            .expect("staged Allow array");
        assert_ne!(
            active_order, staged_order,
            "the captured specimen must retain differently ordered Allow arrays"
        );
        assert_eq!(
            raw_visibility["filter_witnesses"]
                .as_object()
                .expect("fixture witnesses")
                .len(),
            LEGACY_SHELL_AND_COMMS_TOOL_NAMES.len(),
            "the captured specimen must retain all legacy provenance witnesses"
        );

        let persisted = session_with_visibility_state(session_id.clone(), state.clone());
        let mut req = shell_visibility_request(None, None);
        req.build.as_mut().expect("build options").resume_session =
            Some(meerkat_core::Session::with_id(session_id));
        assert_eq!(
            req.build
                .as_ref()
                .and_then(|build| build.resume_session.as_ref())
                .expect("ID-only resume stub")
                .try_tool_visibility_state()
                .expect("parse stub visibility"),
            None,
            "the production actor route presents an ID-only resume stub"
        );

        let probe = Arc::new(AbsorberInnerProbe::new(persisted));
        let wrapped = PreBuildMobSessionService {
            inner: probe.clone(),
            hook: no_op_pre_build_hook(),
            dispatch_taint: None,
            after_create_hook: None,
            runtime_adapter_override: None,
            archived_terminal_authority: None,
            member_sessions_in_memory: false,
        };

        let (prepared, _, _) = wrapped
            .prepare_create_request(req)
            .await
            .expect("prepare authoritative persisted resume");

        assert_eq!(
            probe.loads.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "the heal must read the authoritative persisted head exactly once"
        );
        assert_eq!(
            prepared.build.expect("build options").initial_tool_filter,
            Some(meerkat_core::ToolFilter::All)
        );
        assert_eq!(
            probe
                .session
                .try_tool_visibility_state()
                .expect("parse persisted visibility"),
            Some(state),
            "Meta must not mutate the persisted visibility document directly"
        );
    }

    #[tokio::test]
    async fn pre_build_heal_rejects_embedded_visibility_spoof() {
        let session_id = meerkat_core::types::SessionId::new();
        let embedded_legacy = meerkat_core::SessionToolVisibilityState {
            active_filter: legacy_shell_allow_filter(false),
            staged_filter: legacy_shell_allow_filter(true),
            ..Default::default()
        };
        let persisted_current = meerkat_core::SessionToolVisibilityState::default();
        let mut req = shell_visibility_request(None, None);
        req.build.as_mut().expect("build options").resume_session = Some(
            session_with_visibility_state(session_id.clone(), embedded_legacy),
        );

        let probe = Arc::new(AbsorberInnerProbe::new(session_with_visibility_state(
            session_id,
            persisted_current,
        )));
        let wrapped = PreBuildMobSessionService {
            inner: probe.clone(),
            hook: no_op_pre_build_hook(),
            dispatch_taint: None,
            after_create_hook: None,
            runtime_adapter_override: None,
            archived_terminal_authority: None,
            member_sessions_in_memory: false,
        };

        let (prepared, _, _) = wrapped
            .prepare_create_request(req)
            .await
            .expect("prepare authoritative non-legacy resume");

        assert_eq!(
            probe.loads.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "the embedded session document must never replace the authoritative persisted read"
        );
        assert_eq!(
            prepared.build.expect("build options").initial_tool_filter,
            None,
            "request-carried legacy metadata must not trigger repair when durable authority disagrees"
        );
    }

    #[tokio::test]
    async fn pre_build_heal_is_noop_after_durable_transition() {
        let session_id = meerkat_core::types::SessionId::new();
        let persisted_current = meerkat_core::SessionToolVisibilityState {
            active_filter: meerkat_core::ToolFilter::All,
            staged_filter: meerkat_core::ToolFilter::All,
            active_revision: 2,
            staged_revision: 2,
            ..Default::default()
        };
        let mut req = shell_visibility_request(None, None);
        req.build.as_mut().expect("build options").resume_session =
            Some(meerkat_core::Session::with_id(session_id.clone()));

        let probe = Arc::new(AbsorberInnerProbe::new(session_with_visibility_state(
            session_id,
            persisted_current,
        )));
        let wrapped = PreBuildMobSessionService {
            inner: probe.clone(),
            hook: no_op_pre_build_hook(),
            dispatch_taint: None,
            after_create_hook: None,
            runtime_adapter_override: None,
            archived_terminal_authority: None,
            member_sessions_in_memory: false,
        };

        let (prepared, _, _) = wrapped
            .prepare_create_request(req)
            .await
            .expect("prepare already-healed resume");

        assert_eq!(probe.loads.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(
            prepared.build.expect("build options").initial_tool_filter,
            None,
            "a second resume must not restage a transition after durable authority reached All"
        );
    }

    #[test]
    fn image_generation_profiles_can_disable_builtins_with_meerkat_062() {
        let definition = meerkat_mob::MobDefinition::from_toml(
            r#"
[mob]
id = "test"

[profiles.commander]
model = "gpt-5.5"

[profiles.commander.tools]
builtins = false
image_generation = true
"#,
        )
        .unwrap_or_else(|e| panic!("{e}"));

        let commander = definition.profiles["commander"].as_inline().unwrap();
        assert!(!commander.tools.builtins);
        assert!(commander.tools.image_generation);
        assert!(
            mob_definition_may_use_image_generation(&definition),
            "image generation now has its own Meerkat tool gate"
        );
    }

    #[test]
    fn image_generation_substrate_is_conservative_for_realm_profile_refs() {
        let definition = meerkat_mob::MobDefinition::from_toml(
            r#"
[mob]
id = "test"

[profiles.worker]
realm_profile = "worker-v2"
"#,
        )
        .unwrap_or_else(|e| panic!("{e}"));

        assert!(
            mob_definition_may_use_image_generation(&definition),
            "realm profiles resolve at spawn time, so Meta wires substrate and lets Meerkat enforce profile policy"
        );
    }

    #[test]
    fn sanitize_llm_request_drops_replay_unsafe_server_tool_blocks() {
        let request = meerkat_client::LlmRequest::new(
            "gpt-5.5",
            vec![meerkat_core::Message::BlockAssistant(
                meerkat_core::BlockAssistantMessage::new(
                    vec![
                        meerkat_core::AssistantBlock::Text {
                            text: "done".to_string(),
                            meta: None,
                        },
                        meerkat_core::AssistantBlock::ServerToolContent {
                            id: Some("ws-stream".to_string()),
                            kind: meerkat_core::ServerToolKind::WebSearch,
                            content: serde_json::json!({
                                "type": "response.web_search_call.searching",
                                "item_id": "ws_123"
                            }),
                            meta: None,
                        },
                        meerkat_core::AssistantBlock::ServerToolContent {
                            id: Some("ws_123".to_string()),
                            kind: meerkat_core::ServerToolKind::ProviderNative {
                                name: "web_search_call".to_string(),
                            },
                            content: serde_json::json!({
                                "type": "web_search_call",
                                "id": "ws_123",
                                "status": "completed"
                            }),
                            meta: None,
                        },
                        meerkat_core::AssistantBlock::ServerToolContent {
                            id: None,
                            kind: meerkat_core::ServerToolKind::ProviderNative {
                                name: "web_search_annotations".to_string(),
                            },
                            content: serde_json::json!({
                                "type": "message_annotations",
                                "annotations": []
                            }),
                            meta: None,
                        },
                    ],
                    meerkat_core::StopReason::EndTurn,
                ),
            )],
        );

        let sanitized = sanitize_llm_request_for_stateless_replay(&request);
        let meerkat_core::Message::BlockAssistant(assistant) = &sanitized.messages[0] else {
            panic!("expected block assistant");
        };

        assert_eq!(assistant.blocks.len(), 2);
        assert!(matches!(
            assistant.blocks[0],
            meerkat_core::AssistantBlock::Text { .. }
        ));
        assert!(matches!(
            assistant.blocks[1],
            meerkat_core::AssistantBlock::ServerToolContent { ref kind, .. }
                if kind.provider_name() == "web_search_call"
        ));
    }

    #[test]
    fn sanitize_llm_request_preserves_generated_images_for_meerkat_062() {
        let request = meerkat_client::LlmRequest::new(
            "gpt-5.5",
            vec![meerkat_core::Message::BlockAssistant(
                meerkat_core::BlockAssistantMessage::new(
                    vec![
                        meerkat_core::AssistantBlock::Text {
                            text: "visible".to_string(),
                            meta: None,
                        },
                        generated_image_block_for_test(),
                    ],
                    meerkat_core::StopReason::EndTurn,
                ),
            )],
        );

        let sanitized = sanitize_llm_request_for_stateless_replay(&request);

        let meerkat_core::Message::BlockAssistant(original_assistant) = &request.messages[0] else {
            panic!("expected original block assistant");
        };
        assert!(
            original_assistant
                .blocks
                .iter()
                .any(|block| matches!(block, meerkat_core::AssistantBlock::Image { .. })),
            "request-view sanitization must not rewrite canonical caller-owned messages"
        );

        let meerkat_core::Message::BlockAssistant(sanitized_assistant) = &sanitized.messages[0]
        else {
            panic!("expected sanitized block assistant");
        };
        assert!(
            sanitized_assistant
                .blocks
                .iter()
                .any(|block| matches!(block, meerkat_core::AssistantBlock::Image { .. })),
            "Meerkat 0.6.2 owns provider replay projection for generated images"
        );
    }

    #[derive(Default)]
    struct CapturingLlmClient {
        projected_messages: std::sync::Mutex<Vec<meerkat_core::Message>>,
    }

    #[async_trait]
    impl LlmClient for CapturingLlmClient {
        fn project_replay_messages(
            &self,
            messages: &[meerkat_core::Message],
        ) -> Result<Vec<meerkat_core::Message>, meerkat_client::LlmError> {
            *self
                .projected_messages
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = messages.to_vec();
            Ok(messages.to_vec())
        }

        fn stream<'a>(&'a self, request: &'a LlmRequest) -> LlmStream<'a> {
            // meerkat 0.8.22 rejects a turn whose stream carried no normalized
            // provider accounting, so the terminal `Done` never travels alone.
            let [usage, done] = super::test_llm_usage::usage_then_done(
                request,
                meerkat_core::Provider::OpenAI,
                meerkat_core::StopReason::EndTurn,
            );
            Box::pin(futures::stream::iter(vec![Ok(usage), Ok(done)]))
        }

        fn provider(&self) -> meerkat_core::Provider {
            meerkat_core::Provider::OpenAI
        }

        async fn health_check(&self) -> Result<(), meerkat_client::LlmError> {
            Ok(())
        }
    }

    /// A raw provider client with a governed controller: its selection, plain
    /// route and credential maintenance, a replay projection carrying its own
    /// route witness, and a prepared entry recording what reaches it.
    #[derive(Default)]
    struct GovernedRawClient {
        credential_preparations: std::sync::atomic::AtomicUsize,
        dispatched: std::sync::Mutex<Option<GovernedRawDispatch>>,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    struct RawRouteWitness(u64);

    #[derive(Debug)]
    struct GovernedRawDispatch {
        messages: Vec<meerkat_core::Message>,
        authorized: bool,
        witness: Option<RawRouteWitness>,
    }

    const GOVERNED_RAW_ENDPOINT: &str = "https://governed-raw.invalid/v1/responses";

    #[async_trait]
    impl LlmClient for GovernedRawClient {
        async fn prepare_controller_credential(&self) -> Result<(), meerkat_core::auth::AuthError> {
            self.credential_preparations
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }

        fn controller_model_selection(&self) -> Option<meerkat_core::ControllerModelSelection> {
            Some(crate::memory::dispatch_taint::controller_test_support::selection())
        }

        fn plain_model_route(
            &self,
            logical_model: &str,
        ) -> Result<meerkat_llm_core::PlainModelRoute, meerkat_core::ControllerFactsUnavailable>
        {
            meerkat_llm_core::PlainModelRoute::new(GOVERNED_RAW_ENDPOINT, logical_model)
        }

        fn project_replay_request(
            &self,
            messages: &[meerkat_core::Message],
        ) -> Result<meerkat_llm_core::LlmReplayProjection, meerkat_client::LlmError> {
            Ok(
                meerkat_llm_core::LlmReplayProjection::new(messages.to_vec()).with_route_witness(
                    meerkat_llm_core::LlmRequestRouteWitness::new(RawRouteWitness(7)),
                ),
            )
        }

        fn project_replay_messages(
            &self,
            messages: &[meerkat_core::Message],
        ) -> Result<Vec<meerkat_core::Message>, meerkat_client::LlmError> {
            Ok(messages.to_vec())
        }

        fn stream<'a>(&'a self, _request: &'a LlmRequest) -> LlmStream<'a> {
            panic!("a governed request must reach the prepared entry")
        }

        fn stream_prepared<'a>(
            &'a self,
            request: &'a meerkat_llm_core::PreparedLlmRequest,
        ) -> LlmStream<'a> {
            *self
                .dispatched
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(GovernedRawDispatch {
                messages: request.request().messages.clone(),
                authorized: request.authorization().is_some(),
                witness: request.route_witness::<RawRouteWitness>().copied(),
            });
            let [usage, done] = super::test_llm_usage::usage_then_done(
                request.request(),
                meerkat_core::Provider::OpenAI,
                meerkat_core::StopReason::EndTurn,
            );
            Box::pin(futures::stream::iter(vec![Ok(usage), Ok(done)]))
        }

        fn provider(&self) -> meerkat_core::Provider {
            meerkat_core::Provider::OpenAI
        }

        async fn health_check(&self) -> Result<(), meerkat_client::LlmError> {
            Ok(())
        }
    }

    /// A replay transcript with one provider stream-event block the sanitizer
    /// removes and one visible text block it keeps.
    fn transcript_with_replay_unsafe_block() -> Vec<meerkat_core::Message> {
        vec![meerkat_core::Message::BlockAssistant(
            meerkat_core::BlockAssistantMessage::new(
                vec![
                    meerkat_core::AssistantBlock::Text {
                        text: "visible".to_string(),
                        meta: None,
                    },
                    meerkat_core::AssistantBlock::ServerToolContent {
                        id: Some("ws-stream".to_string()),
                        kind: meerkat_core::ServerToolKind::WebSearch,
                        content: serde_json::json!({
                            "type": "response.web_search_call.searching",
                            "item_id": "ws_123"
                        }),
                        meta: None,
                    },
                ],
                meerkat_core::StopReason::EndTurn,
            ),
        )]
    }

    fn only_visible_text(messages: &[meerkat_core::Message]) -> bool {
        matches!(
            messages,
            [meerkat_core::Message::BlockAssistant(assistant)]
                if matches!(
                    assistant.blocks.as_slice(),
                    [meerkat_core::AssistantBlock::Text { .. }]
                )
        )
    }

    /// Both raw wrappers, in both orders the builder composes them, carry the
    /// wrapped client's actual controller (selection, plain route and
    /// credential maintenance) while keeping the provider-agnostic claim. A
    /// governed prepared request reaches the client sanitized, with its
    /// admitted work and route witness intact, and the replay projection is
    /// the client's own, so its route witness reaches the prepared request.
    #[tokio::test]
    async fn raw_client_wrappers_carry_the_controller_and_sanitize_governed_dispatch() {
        use crate::memory::dispatch_taint::controller_test_support::{authorization, selection};
        let model = "controller-model";
        let probe = Arc::new(GovernedRawClient::default());
        let inner: Arc<dyn LlmClient> = probe.clone();
        let shapes: [(&str, Arc<dyn LlmClient>); 2] = [
            (
                "sanitizer over provider-agnostic",
                ReplaySanitizingLlmClient::wrap(ProviderAgnosticLlmClient::wrap(Arc::clone(
                    &inner,
                ))),
            ),
            (
                "provider-agnostic over sanitizer",
                ProviderAgnosticLlmClient::wrap(ReplaySanitizingLlmClient::wrap(Arc::clone(
                    &inner,
                ))),
            ),
        ];
        for (preparations, (shape, client)) in (1..).zip(shapes) {
            assert_eq!(
                client.controller_model_selection(),
                Some(selection()),
                "{shape}"
            );
            let route = client
                .plain_model_route(model)
                .expect("the wrapped client's plain route");
            assert_eq!(
                (route.endpoint(), route.wire_model()),
                (GOVERNED_RAW_ENDPOINT, model),
                "{shape}"
            );
            client
                .prepare_controller_credential()
                .await
                .expect("the wrapped client's credential maintenance");
            assert_eq!(
                probe
                    .credential_preparations
                    .load(std::sync::atomic::Ordering::SeqCst),
                preparations,
                "{shape}"
            );
            assert_eq!(
                client.provider(),
                meerkat_core::Provider::Other,
                "{shape}: the provider-agnostic claim stays"
            );

            let projection = client
                .project_replay_request(&transcript_with_replay_unsafe_block())
                .expect("the wrapped client's projection");
            let projected = meerkat_llm_core::PreparedLlmRequest::from_projection(
                LlmRequest::new(model, Vec::new()),
                projection,
            );
            assert!(
                only_visible_text(&projected.request().messages),
                "{shape}: the projection is sanitized"
            );
            assert_eq!(
                projected.route_witness::<RawRouteWitness>(),
                Some(&RawRouteWitness(7)),
                "{shape}: the wrapped client's own route witness"
            );

            let governed = meerkat_llm_core::PreparedLlmRequest::from_projection(
                LlmRequest::new(model, Vec::new()),
                meerkat_llm_core::LlmReplayProjection::new(transcript_with_replay_unsafe_block())
                    .with_route_witness(meerkat_llm_core::LlmRequestRouteWitness::new(
                        RawRouteWitness(9),
                    )),
            )
            .with_authorization(Some(authorization()));
            let events: Vec<_> = client.stream_prepared(&governed).collect().await;
            assert!(events.iter().all(Result::is_ok), "{shape}: {events:?}");
            let dispatched = probe
                .dispatched
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take()
                .expect("the governed request reached the prepared entry");
            assert!(
                only_visible_text(&dispatched.messages),
                "{shape}: the governed payload is sanitized: {dispatched:?}"
            );
            assert!(dispatched.authorized, "{shape}: the admitted work travels");
            assert_eq!(
                dispatched.witness,
                Some(RawRouteWitness(9)),
                "{shape}: the route witness travels"
            );
        }
    }

    #[test]
    fn replay_sanitizing_llm_client_delegates_provider_projection() {
        let capture = Arc::new(CapturingLlmClient::default());
        let inner: Arc<dyn LlmClient> = capture.clone();
        let wrapped = ReplaySanitizingLlmClient::new(inner);
        let messages = vec![meerkat_core::Message::BlockAssistant(
            meerkat_core::BlockAssistantMessage::new(
                vec![
                    meerkat_core::AssistantBlock::Text {
                        text: "visible".to_string(),
                        meta: None,
                    },
                    meerkat_core::AssistantBlock::ServerToolContent {
                        id: Some("ws-stream".to_string()),
                        kind: meerkat_core::ServerToolKind::WebSearch,
                        content: serde_json::json!({
                            "type": "response.web_search_call.searching",
                            "item_id": "ws_123"
                        }),
                        meta: None,
                    },
                ],
                meerkat_core::StopReason::EndTurn,
            ),
        )];

        let projected = wrapped
            .project_replay_messages(&messages)
            .expect("wrapped client should delegate provider projection");

        let seen = capture
            .projected_messages
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let meerkat_core::Message::BlockAssistant(assistant) = &seen[0] else {
            panic!("expected block assistant");
        };
        assert_eq!(
            assistant.blocks.len(),
            1,
            "Meta sanitization must happen before Meerkat provider projection"
        );
        assert!(matches!(
            assistant.blocks[0],
            meerkat_core::AssistantBlock::Text { .. }
        ));
        assert_eq!(
            serde_json::to_value(&projected).expect("projected messages serialize"),
            serde_json::to_value(&seen).expect("seen messages serialize")
        );
    }

    /// `ReplaySanitizingAgentLlmClient` must report the authority of the client
    /// it wraps, not its own.
    ///
    /// Paired with `taint_observer_forwards_inner_authority` in
    /// `memory::dispatch_taint`. BOTH production decorators need this and a fix
    /// applied to only one passes the other's test - which is why there are two
    /// tests rather than one, in the two files that hold the two wrappers.
    ///
    /// Covers wrapper CONSTRUCTION only. It does NOT cover the durable-resume
    /// composition path, where production measured the actual 72-identity failure: a
    /// wrapper can forward correctly here and the resume path can still install
    /// something that does not. That is verified downstream, not by this test.
    #[test]
    fn replay_sanitizer_forwards_inner_authority() {
        #[derive(Debug)]
        struct UnifiedInner;

        #[async_trait]
        impl meerkat_core::AgentLlmClient for UnifiedInner {
            fn provider(&self) -> meerkat_core::Provider {
                meerkat_core::Provider::OpenAI
            }

            fn model(&self) -> &'static str {
                "gpt-5.5"
            }

            async fn stream_response(
                &self,
                _messages: &[meerkat_core::Message],
                _tools: &[std::sync::Arc<meerkat_core::ToolDef>],
                _max_tokens: u32,
                _temperature: Option<f32>,
                _provider_params: Option<
                    &meerkat_core::lifecycle::run_primitive::ProviderParamsOverride,
                >,
            ) -> Result<meerkat_core::agent::LlmStreamResult, meerkat_core::AgentError>
            {
                unreachable!("authority forwarding never streams")
            }

            fn request_attempt_authority(&self) -> meerkat_core::RequestAttemptAuthority {
                meerkat_core::RequestAttemptAuthority::Unified
            }
        }

        let wrapped = ReplaySanitizingAgentLlmClient::new(std::sync::Arc::new(UnifiedInner));
        assert_eq!(
            meerkat_core::AgentLlmClient::request_attempt_authority(&wrapped),
            meerkat_core::RequestAttemptAuthority::Unified,
            "replay sanitizer reported its own authority instead of the wrapped \
             client's; the trait default would have made this LegacySplit and \
             silently downgraded every session it decorates",
        );
    }

    /// The replay sanitizer forwards the controller surface, and the pinned
    /// controller (the client governed controller requests actually run
    /// through) still sanitizes what it sends.
    #[tokio::test]
    async fn replay_sanitizer_forwards_the_controller_and_its_pin_stays_sanitizing() {
        use crate::memory::dispatch_taint::controller_test_support::{
            PinnedInner, authorization, selection,
        };
        use meerkat_core::AgentLlmClient as _;
        let inner = Arc::new(PinnedInner::default());
        let wrapped = Arc::new(ReplaySanitizingAgentLlmClient::new(inner.clone()));
        let selection = selection();

        assert_eq!(
            wrapped.controller_model_selection(),
            Some(selection.clone())
        );
        assert_eq!(
            wrapped
                .controller_model_facts()
                .expect("controller facts")
                .selection(),
            &selection
        );
        wrapped
            .prepare_controller_credential()
            .await
            .expect("controller credential maintenance reaches the wrapped client");
        // The same check `Agent::pin_controller_client` applies.
        let pin = Arc::clone(&wrapped)
            .pin_controller()
            .expect("the wrapped client's controller pin");
        assert_eq!(pin.selection(), &selection);
        assert_eq!(
            pin.client().controller_model_selection().as_ref(),
            Some(&selection)
        );
        Arc::clone(&wrapped)
            .prepare_request_attempt_authorized(
                Arc::new(Vec::new()),
                Arc::from(Vec::new()),
                128,
                None,
                None,
                Some(authorization()),
            )
            .expect("an authorized attempt is the wrapped client's decision");
        wrapped
            .fork_noncommitting_live_bridge()
            .expect("the wrapped client's live-bridge fork");

        let messages = vec![meerkat_core::Message::BlockAssistant(
            meerkat_core::BlockAssistantMessage::new(
                vec![
                    meerkat_core::AssistantBlock::Text {
                        text: "visible".to_string(),
                        meta: None,
                    },
                    meerkat_core::AssistantBlock::ServerToolContent {
                        id: Some("ws-stream".to_string()),
                        kind: meerkat_core::ServerToolKind::WebSearch,
                        content: serde_json::json!({
                            "type": "response.web_search_call.searching",
                            "item_id": "ws_123"
                        }),
                        meta: None,
                    },
                ],
                meerkat_core::StopReason::EndTurn,
            ),
        )];
        pin.client()
            .stream_response_authorized(&messages, &[], 128, None, None, Some(authorization()))
            .await
            .expect("authorized controller request");
        let seen = inner
            .seen_messages
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let meerkat_core::Message::BlockAssistant(assistant) = &seen[0] else {
            panic!("expected block assistant");
        };
        assert_eq!(
            assistant.blocks.len(),
            1,
            "a request through the pinned controller must still be sanitized"
        );
        assert_eq!(
            inner
                .authorized_calls
                .load(std::sync::atomic::Ordering::SeqCst),
            2,
            "both authorized calls reached the wrapped client"
        );
    }

    #[derive(Default)]
    struct CapturingAgentLlmClient {
        seen_messages: std::sync::Mutex<Vec<meerkat_core::Message>>,
        fallback_prepare_calls: std::sync::atomic::AtomicUsize,
        fallback_commit_calls: std::sync::atomic::AtomicUsize,
        fallback_schema_calls: std::sync::atomic::AtomicUsize,
        stream_observation_starts: std::sync::atomic::AtomicUsize,
    }

    #[async_trait]
    impl meerkat_core::AgentLlmClient for CapturingAgentLlmClient {
        async fn stream_response(
            &self,
            messages: &[meerkat_core::Message],
            _tools: &[Arc<meerkat_core::ToolDef>],
            _max_tokens: u32,
            _temperature: Option<f32>,
            _provider_params: Option<
                &meerkat_core::lifecycle::run_primitive::ProviderParamsOverride,
            >,
        ) -> Result<meerkat_core::agent::LlmStreamResult, meerkat_core::AgentError> {
            *self
                .seen_messages
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = messages.to_vec();
            Ok(meerkat_core::agent::LlmStreamResult::new(
                Vec::new(),
                meerkat_core::StopReason::EndTurn,
                // `LlmStreamResult::new` still takes a flat `Usage`, so a bare
                // `Usage::default()` compiles and then fails the turn closed
                // with `normalized_provider_accounting_unavailable`. At the
                // agent-client seam the factory pins `provider()`/`model()` to
                // the canonical identity, so they ARE the identity the commit
                // check compares against.
                super::test_llm_usage::agent_turn_usage(
                    self.provider(),
                    self.model(),
                    meerkat_core::Usage::default(),
                ),
            ))
        }

        fn provider(&self) -> meerkat_core::Provider {
            meerkat_core::Provider::OpenAI
        }

        fn model(&self) -> &'static str {
            "gpt-5.5"
        }

        fn prepare_model_fallback(
            &self,
            _failure: &meerkat_core::AgentError,
            _request: &meerkat_core::model_fallback::ModelFallbackRequest<'_>,
        ) -> Result<
            meerkat_core::AgentLlmFallbackSwitch,
            Vec<meerkat_core::AgentLlmFallbackSkippedTarget>,
        > {
            self.fallback_prepare_calls
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Err(Vec::new())
        }

        fn commit_model_fallback(
            &self,
            _previous_identity: &meerkat_core::SessionLlmIdentity,
            _target_identity: &meerkat_core::SessionLlmIdentity,
        ) -> Result<(), meerkat_core::AgentError> {
            self.fallback_commit_calls
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }

        fn active_model_fallback_identity(&self) -> Option<meerkat_core::SessionLlmIdentity> {
            Some(test_session_llm_identity("fallback-model"))
        }

        fn compile_model_fallback_schema(
            &self,
            _target_identity: &meerkat_core::SessionLlmIdentity,
            _output_schema: &meerkat_core::OutputSchema,
        ) -> Result<meerkat_core::schema::CompiledSchema, meerkat_core::AgentError> {
            self.fallback_schema_calls
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Err(meerkat_core::AgentError::ConfigError(
                "fallback schema probe".to_string(),
            ))
        }

        fn begin_stream_output_observation(&self) {
            self.stream_observation_starts
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }

        fn stream_output_observed(&self) -> bool {
            true
        }
    }

    fn test_session_llm_identity(model: &str) -> meerkat_core::SessionLlmIdentity {
        meerkat_core::SessionLlmIdentity {
            model: model.to_string(),
            provider: meerkat_core::Provider::OpenAI,
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: None,
        }
    }

    #[test]
    fn sanitize_agent_llm_client_forwards_fallback_and_stream_observation_state() {
        let capture = Arc::new(CapturingAgentLlmClient::default());
        let inner: Arc<dyn meerkat_core::AgentLlmClient> = capture.clone();
        let wrapped = ReplaySanitizingAgentLlmClient::new(inner);
        let previous = test_session_llm_identity("primary-model");
        let target = test_session_llm_identity("fallback-model");

        assert!(
            meerkat_core::AgentLlmClient::prepare_model_fallback(
                &wrapped,
                &meerkat_core::AgentError::ConfigError("probe".to_string()),
                &meerkat_core::model_fallback::ModelFallbackRequest {
                    messages: &[],
                    tools: &[],
                    max_tokens: 512,
                    temperature: None,
                    provider_params: None,
                    output_schema: None,
                    attempt: 3,
                },
            )
            .expect_err("no proposal should forward")
            .is_empty()
        );
        meerkat_core::AgentLlmClient::commit_model_fallback(&wrapped, &previous, &target)
            .expect("fallback activation should forward");
        assert_eq!(
            meerkat_core::AgentLlmClient::active_model_fallback_identity(&wrapped)
                .expect("active fallback identity should forward")
                .model,
            "fallback-model"
        );
        let schema = meerkat_core::OutputSchema::new(serde_json::json!({"type": "object"}))
            .expect("valid test schema");
        let error =
            meerkat_core::AgentLlmClient::compile_model_fallback_schema(&wrapped, &target, &schema)
                .expect_err("fallback schema probe error should forward");
        assert!(error.to_string().contains("fallback schema probe"));
        meerkat_core::AgentLlmClient::begin_stream_output_observation(&wrapped);
        assert!(meerkat_core::AgentLlmClient::stream_output_observed(
            &wrapped
        ));

        assert_eq!(
            capture
                .fallback_prepare_calls
                .load(std::sync::atomic::Ordering::SeqCst),
            1
        );
        assert_eq!(
            capture
                .fallback_commit_calls
                .load(std::sync::atomic::Ordering::SeqCst),
            1
        );
        assert_eq!(
            capture
                .fallback_schema_calls
                .load(std::sync::atomic::Ordering::SeqCst),
            1
        );
        assert_eq!(
            capture
                .stream_observation_starts
                .load(std::sync::atomic::Ordering::SeqCst),
            1
        );
    }

    #[tokio::test]
    async fn sanitize_agent_llm_client_drops_replay_unsafe_server_tool_blocks() {
        let capture = Arc::new(CapturingAgentLlmClient::default());
        let inner: Arc<dyn meerkat_core::AgentLlmClient> = capture.clone();
        let wrapped = ReplaySanitizingAgentLlmClient::wrap(inner);
        let messages = vec![meerkat_core::Message::BlockAssistant(
            meerkat_core::BlockAssistantMessage::new(
                vec![
                    meerkat_core::AssistantBlock::Text {
                        text: "visible".to_string(),
                        meta: None,
                    },
                    meerkat_core::AssistantBlock::ServerToolContent {
                        id: Some("ws-stream".to_string()),
                        kind: meerkat_core::ServerToolKind::WebSearch,
                        content: serde_json::json!({
                            "type": "response.web_search_call.searching",
                            "item_id": "ws_123"
                        }),
                        meta: None,
                    },
                    meerkat_core::AssistantBlock::ServerToolContent {
                        id: Some("ok".to_string()),
                        kind: meerkat_core::ServerToolKind::ProviderNative {
                            name: "web_search_call".to_string(),
                        },
                        content: serde_json::json!({
                            "type": "web_search_call",
                            "id": "ws_123",
                            "status": "completed"
                        }),
                        meta: None,
                    },
                ],
                meerkat_core::StopReason::EndTurn,
            ),
        )];
        let tools: Vec<Arc<meerkat_core::ToolDef>> = Vec::new();

        wrapped
            .stream_response(&messages, &tools, 512, None, None)
            .await
            .expect("wrapped client should delegate");

        let seen = capture
            .seen_messages
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let meerkat_core::Message::BlockAssistant(assistant) = &seen[0] else {
            panic!("expected block assistant");
        };
        assert_eq!(assistant.blocks.len(), 2);
        assert!(matches!(
            assistant.blocks[0],
            meerkat_core::AssistantBlock::Text { .. }
        ));
        assert!(matches!(
            assistant.blocks[1],
            meerkat_core::AssistantBlock::ServerToolContent { ref kind, .. }
                if kind.provider_name() == "web_search_call"
        ));
    }

    fn generated_image_block_for_test() -> meerkat_core::AssistantBlock {
        serde_json::from_value(serde_json::json!({
            "block_type": "image",
            "data": {
                "image_id": "00000000-0000-0000-0000-000000000051",
                "blob_ref": {
                    "blob_id": "sha256:test-generated-image",
                    "media_type": "image/png"
                },
                "media_type": "image/png",
                "width": 1024,
                "height": 1024,
                "revised_prompt": { "disposition": "not_requested" },
                "meta": { "provider": "not_emitted" }
            }
        }))
        .expect("test image block should deserialize")
    }

    #[test]
    fn sanitize_message_preserves_assistant_image_blocks() {
        let message =
            meerkat_core::Message::BlockAssistant(meerkat_core::BlockAssistantMessage::new(
                vec![
                    meerkat_core::AssistantBlock::Text {
                        text: "Here is the image.".to_string(),
                        meta: None,
                    },
                    generated_image_block_for_test(),
                ],
                meerkat_core::StopReason::EndTurn,
            ));

        let sanitized = sanitize_message_for_stateless_replay(message);
        let meerkat_core::Message::BlockAssistant(assistant) = sanitized else {
            panic!("expected block assistant");
        };

        assert_eq!(assistant.blocks.len(), 2);
        assert!(matches!(
            assistant.blocks[0],
            meerkat_core::AssistantBlock::Text { .. }
        ));
        assert!(
            matches!(
                assistant.blocks[1],
                meerkat_core::AssistantBlock::Image { .. }
            ),
            "generated image blocks should reach Meerkat's provider projection"
        );
    }

    /// Verify that persistent_with_hook wraps the session service with
    /// PreBuildMobSessionService (hook is Some).
    /// Every `install_agent_mob_tools` call inside a PERSISTENT constructor
    /// must supply a durable council store, and every ephemeral one must not.
    ///
    /// This is a source-shape assertion because the defect it guards compiled
    /// clean: two call sites share an argument shape, patching by pattern put
    /// the ephemeral `None` into `persistent_with_hook`, and nothing failed.
    /// A persistent mob would have kept process-bound councils, losing
    /// unfinished records on every restart with no signal at all.
    /// Only `NotFound` may be reported as "the machine does not hold this
    /// session". Every other upstream failure must degrade to a reason that
    /// diagnoses nothing.
    ///
    /// This is the constraint that makes the discriminator worth having: a
    /// consumer sweeping a fleet treats `session_not_held` as a defect to
    /// escalate. If a transient `NotReady` or a `Destroyed` runtime also
    /// rendered as `session_not_held`, that escalation would fire on states
    /// that are not missing sessions at all.
    ///
    /// Asserted as a POSITIVE mapping per variant rather than as "the code
    /// contains a match": a comment claiming the rule cannot fail, and a
    /// source-shape check would pass against a wildcard that swallowed
    /// everything.
    #[test]
    fn only_a_notfound_earns_the_session_not_held_verdict() {
        use meerkat_runtime::RuntimeDriverError;

        let not_found = RuntimeDriverError::NotFound {
            runtime_id: meerkat_runtime::LogicalRuntimeId::new("session:probe"),
        };
        assert_eq!(
            super::classify_routing_status_read_error(not_found).reason_code(),
            "session_not_held",
            "a genuine NotFound is the one condition that means the machine does not hold \
             the session"
        );

        // The negative half. Without it, a wildcard mapping EVERYTHING to
        // session_not_held would still satisfy the assertion above.
        for other in [
            RuntimeDriverError::Destroyed,
            RuntimeDriverError::ValidationFailed {
                reason: "probe".to_string(),
            },
            RuntimeDriverError::RecoveryCorruption {
                reason: "probe".to_string(),
            },
        ] {
            let rendered = format!("{other}");
            assert_eq!(
                super::classify_routing_status_read_error(other).reason_code(),
                "upstream_read_failed",
                "{rendered:?} is not a missing session and must not be reported as one"
            );
        }
    }

    #[test]
    fn persistent_constructors_supply_a_durable_council_store() {
        let source = include_str!("mob_handle_runtime.rs");
        let lines: Vec<&str> = source.lines().collect();

        let mut enclosing = "";
        let mut seen: Vec<(&str, bool)> = Vec::new();
        for (index, line) in lines.iter().enumerate() {
            let trimmed = line.trim_start();
            // Method-level fn at EXACTLY four spaces, in every visibility and
            // asyncness spelling. The first version of this guard matched only
            // `    pub fn ` and `    fn `, so it could not see
            // `pub(crate) fn ephemeral_runtime_backed_with_provider_stores` -
            // it silently kept the PREVIOUS function's name, attributed that
            // site to the preceding `persistent_*` constructor, and passed
            // while a durable store was wired into an ephemeral one. A guard
            // that cannot see a call site does not report it missing; it
            // reports someone else's.
            let is_method_fn = line.starts_with("    ")
                && !line.starts_with("     ")
                && (trimmed.starts_with("fn ")
                    || trimmed.starts_with("async fn ")
                    || trimmed.starts_with("pub fn ")
                    || trimmed.starts_with("pub async fn ")
                    || trimmed.starts_with("pub(crate) fn ")
                    || trimmed.starts_with("pub(crate) async fn ")
                    || trimmed.starts_with("pub(super) fn ")
                    || trimmed.starts_with("pub(super) async fn "));
            if is_method_fn {
                enclosing = trimmed
                    .rsplit(" fn ")
                    .next()
                    .unwrap_or("")
                    .split('(')
                    .next()
                    .unwrap_or("");
            }
            if trimmed.starts_with(") = install_agent_mob_tools(") {
                let window = lines[index..(index + 24).min(lines.len())].join("\n");
                seen.push((enclosing, window.contains("open_council_store")));
            }
        }

        assert_eq!(seen.len(), 4, "call-site count changed: {seen:?}");
        // Every site must have resolved to a NAMED function. An empty name
        // means the detection above failed to see a `fn` line and silently
        // attributed the site to whatever came before - which is exactly how
        // the ephemeral/durable mix-up passed review.
        for (function, _) in &seen {
            assert!(
                !function.is_empty(),
                "a call site resolved to no enclosing function: {seen:?}"
            );
        }
        for (function, durable) in &seen {
            let expected = function.contains("persistent");
            assert_eq!(
                *durable,
                expected,
                "{function} should {} supply a durable council store",
                if expected { "" } else { "NOT" },
            );
        }
    }

    #[tokio::test]
    async fn persistent_with_hook_wraps_session_service() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let store_path = dir.path().to_path_buf();
        let Ok(definition) = meerkat_mob::MobDefinition::from_toml("[mob]\nid = \"test\"\n") else {
            panic!("failed to parse minimal mob definition");
        };

        let hook_called = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let hook_called_clone = hook_called.clone();

        let spec = MobBootstrapSpec::persistent_with_hook(
            definition,
            meerkat_mob::MobStorage::in_memory(),
            crate::mob_composition_manifest::MobStorageProvenance::declared_ephemeral(),
            store_path.clone(),
            4,
            move |_req: &mut CreateSessionRequest| {
                hook_called_clone.store(true, std::sync::atomic::Ordering::Relaxed);
                Box::pin(async { Ok(()) })
            },
        )
        .await
        .unwrap_or_else(|e| panic!("{e}"));

        // The session service is wired with a SqliteRuntimeStore so that
        // both `load_persisted_session` (resume) and
        // `load_persisted_session_for_control` (archive/retire) succeed
        // across process restart. spec.runtime_adapter is also set
        // explicitly so the bootstrap path uses the same store. See
        // `persistent_bootstrap_uses_sqlite_runtime_store` for the full
        // regression coverage.
        assert!(
            spec.runtime_adapter.is_some(),
            "persistent_with_hook must provide a runtime adapter via spec.runtime_adapter"
        );
        assert!(
            spec.session_service
                .acquire_runtime_adapter(None)
                .expect("acquire runtime adapter")
                .is_some(),
            "session service must own a runtime_store so archive/retire don't \
             hit the store-only-projection rejection in meerkat-session"
        );
        assert!(
            store_path.join("runtime.sqlite").exists(),
            "persistent_inner must open a SqliteRuntimeStore at <store_path>/runtime.sqlite"
        );

        // The hook isn't called until create_session — verify the wrapper exists
        // by checking the service is not the raw PersistentSessionService (it
        // wraps it). We can't call create_session without a full LLM stack, but
        // we can verify the hook_called flag is false (not prematurely invoked).
        assert!(
            !hook_called.load(std::sync::atomic::Ordering::Relaxed),
            "hook must not be called before create_session"
        );
    }

    /// Verify that ephemeral_with_hook accepts and stores a hook.
    #[test]
    fn ephemeral_with_hook_creates_spec() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let store_path = dir.path().to_path_buf();
        let Ok(definition) = meerkat_mob::MobDefinition::from_toml("[mob]\nid = \"test\"\n") else {
            panic!("failed to parse minimal mob definition");
        };

        let hook_called = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let hook_called_clone = hook_called.clone();

        let spec = MobBootstrapSpec::ephemeral_with_hook(
            definition,
            meerkat_mob::MobStorage::in_memory(),
            store_path,
            4,
            None,
            move |_req: &mut CreateSessionRequest| {
                hook_called_clone.store(true, std::sync::atomic::Ordering::Relaxed);
                Box::pin(async { Ok(()) })
            },
        )
        .expect("bootstrap spec");

        let runtime_adapter = spec
            .runtime_adapter
            .as_ref()
            .unwrap_or_else(|| panic!("ephemeral_with_hook must retain its runtime adapter"));
        assert!(
            runtime_adapter.has_session_llm_reconfigure_host(),
            "ephemeral_with_hook must install the live LLM reconfiguration host"
        );

        // Hook not yet called.
        assert!(
            !hook_called.load(std::sync::atomic::Ordering::Relaxed),
            "hook must not be called before create_session"
        );
    }

    /// Verify that PreBuildMobSessionService applies the hook to the request
    /// in create_session. The hook mutates the model and adds labels; we
    /// verify by capturing the state inside the hook itself.
    #[tokio::test]
    async fn pre_build_hook_mutates_create_session_request() {
        use std::sync::Mutex;

        let captured = Arc::new(Mutex::new(None::<(String, Option<String>)>));
        let captured_clone = captured.clone();

        // Build a minimal ephemeral service as the inner.
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let factory = AgentFactory::new(dir.path()).builtins(true);
        let config = Config::default();
        let builder = FactoryAgentBuilder::new(factory, config);
        let inner: Arc<dyn MobSessionService> =
            Arc::new(meerkat_session::EphemeralSessionService::new(builder, 4));

        // Hook that mutates and captures the post-mutation state.
        let hook: PreBuildHook = Arc::new(move |req: &mut CreateSessionRequest| {
            req.model = "hooked-model".to_string();
            req.system_prompt =
                meerkat_core::config::SystemPromptOverride::Set("injected-prompt".to_string());
            let labels = req.labels.get_or_insert_with(Default::default);
            labels.insert("hook_label".to_string(), "hook_value".to_string());
            // Capture to prove the hook ran and mutated the request.
            let mut lock = captured_clone
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *lock = Some((
                req.model.clone(),
                req.system_prompt.as_set_prompt().map(ToString::to_string),
            ));
            Box::pin(async { Ok(()) })
        });
        let wrapped = PreBuildMobSessionService {
            inner,
            hook,
            dispatch_taint: None,
            after_create_hook: None,
            runtime_adapter_override: None,
            archived_terminal_authority: None,
            member_sessions_in_memory: false,
        };

        let req = CreateSessionRequest {
            model: "original-model".to_string(),
            prompt: meerkat_core::ContentInput::Text("test".to_string()),
            system_prompt: meerkat_core::config::SystemPromptOverride::Inherit,
            max_tokens: None,
            event_tx: None,
            initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
            build: None,
            labels: None,
            deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::default(),
            injected_context: Vec::new(),
        };

        // create_session will fail (no LLM) but the hook runs first.
        let _ = meerkat_core::service::SessionService::create_session(&wrapped, req).await;

        let (model, prompt) = captured
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .expect("hook must have been called");
        assert_eq!(model, "hooked-model", "hook must mutate the model");
        assert_eq!(
            prompt.as_deref(),
            Some("injected-prompt"),
            "hook must set the system prompt"
        );
    }

    /// Regression for meerkat 0.7.2 fix #1: a custom `Config.retry` must reach
    /// the agent's effective `RetryPolicy` through Meta's session-service build
    /// path, instead of silently falling back to the 3-retry / 30s default.
    ///
    /// We build through the same `FactoryAgentBuilder` Meta's session service
    /// uses (`build_*_session_service` -> `FactoryAgentBuilder::new(factory,
    /// config)`), then read the effective policy back through the public
    /// `Agent::retry_policy()` accessor. The stub `LlmClient` exists only so the
    /// offline `build_agent` succeeds; no turn is run and no provider behavior is
    /// faked. The assertion targets the *plumbed config value*, not retry timing.
    #[tokio::test]
    async fn config_retry_reaches_agent_effective_retry_policy() {
        use meerkat_session::SessionAgentBuilder as _;

        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let factory = AgentFactory::new(dir.path()).builtins(true);

        // Non-default retry config: max_retries away from the canonical 3.
        let mut config = Config::default();
        assert_ne!(
            config.retry.max_retries, 11,
            "test sentinel must differ from default"
        );
        config.retry.max_retries = 11;
        config.retry.initial_delay = std::time::Duration::from_millis(125);
        config.retry.max_delay = std::time::Duration::from_secs(7);
        config.retry.multiplier = 3.5;

        let mut builder = FactoryAgentBuilder::new(factory, config);
        // Build-only stub so the offline build_agent succeeds; never run.
        builder.default_llm_client = Some(Arc::new(CapturingLlmClient::default()));

        let req = CreateSessionRequest {
            model: "mock-model".to_string(),
            prompt: meerkat_core::ContentInput::Text("noop".to_string()),
            system_prompt: meerkat_core::config::SystemPromptOverride::Inherit,
            max_tokens: None,
            event_tx: None,
            initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
            build: None,
            labels: None,
            deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
            injected_context: Vec::new(),
        };

        let (event_tx, _event_rx) = tokio::sync::mpsc::channel(8);
        let factory_agent = builder
            .build_agent(&req, event_tx)
            .await
            .unwrap_or_else(|e| panic!("build_agent should succeed offline: {e}"));

        let effective = factory_agent.agent().retry_policy();
        assert_eq!(
            effective.max_retries, 11,
            "Config.retry.max_retries must be plumbed into the agent's effective \
             RetryPolicy, not the default 3"
        );
        assert_eq!(
            effective.initial_delay,
            std::time::Duration::from_millis(125),
            "Config.retry.initial_delay must be plumbed, not the 500ms default"
        );
        assert_eq!(
            effective.max_delay,
            std::time::Duration::from_secs(7),
            "Config.retry.max_delay must be plumbed, not the 30s default"
        );
        assert!(
            (effective.multiplier - 3.5).abs() < f64::EPSILON,
            "Config.retry.multiplier must be plumbed, not the 2.0 default"
        );
    }

    /// Inner-service stand-in for the absorber seam: `load_persisted_session`
    /// returns one fixed document and counts reads; everything else is inert.
    struct AbsorberInnerProbe {
        session: meerkat_core::session::Session,
        loads: std::sync::atomic::AtomicU64,
        catalog_reads: std::sync::atomic::AtomicU64,
        catalog_names: Vec<meerkat_core::types::ToolName>,
    }

    impl AbsorberInnerProbe {
        fn new(session: meerkat_core::session::Session) -> Self {
            Self {
                session,
                loads: std::sync::atomic::AtomicU64::new(0),
                catalog_reads: std::sync::atomic::AtomicU64::new(0),
                catalog_names: Vec::new(),
            }
        }

        fn with_catalog(mut self, names: impl IntoIterator<Item = &'static str>) -> Self {
            self.catalog_names = names
                .into_iter()
                .map(meerkat_core::types::ToolName::from)
                .collect();
            self
        }
    }

    #[async_trait]
    impl meerkat_core::service::SessionService for AbsorberInnerProbe {
        async fn create_session(
            &self,
            _req: CreateSessionRequest,
        ) -> Result<meerkat_core::types::RunResult, SessionError> {
            Ok(meerkat_core::types::RunResult {
                text: String::new(),
                session_id: self.session.id().clone(),
                usage: meerkat_core::types::Usage::default(),
                request_usage: Vec::new(),
                run_usage: None,
                turns: 0,
                tool_calls: 0,
                terminal_cause_kind: None,
                structured_output: None,
                extraction_error: None,
                schema_warnings: None,
                skill_diagnostics: None,
            })
        }

        async fn start_turn(
            &self,
            _id: &meerkat_core::types::SessionId,
            _req: meerkat_core::service::StartTurnRequest,
        ) -> Result<meerkat_core::types::RunResult, SessionError> {
            Err(SessionError::Unsupported("start_turn".to_string()))
        }

        async fn interrupt(
            &self,
            _id: &meerkat_core::types::SessionId,
        ) -> Result<(), SessionError> {
            Ok(())
        }

        async fn read(
            &self,
            id: &meerkat_core::types::SessionId,
        ) -> Result<meerkat_core::service::SessionView, SessionError> {
            Err(SessionError::NotFound { id: id.clone() })
        }

        async fn list(
            &self,
            _query: meerkat_core::service::SessionQuery,
        ) -> Result<Vec<meerkat_core::service::SessionSummary>, SessionError> {
            Ok(Vec::new())
        }

        async fn archive(&self, _id: &meerkat_core::types::SessionId) -> Result<(), SessionError> {
            Ok(())
        }
    }

    #[async_trait]
    impl meerkat_core::service::SessionServiceCommsExt for AbsorberInnerProbe {}

    #[async_trait]
    impl meerkat_core::service::SessionServiceControlExt for AbsorberInnerProbe {
        async fn append_system_context(
            &self,
            _id: &meerkat_core::types::SessionId,
            _req: meerkat_core::service::AppendSystemContextRequest,
        ) -> Result<
            meerkat_core::service::AppendSystemContextResult,
            meerkat_core::service::SessionControlError,
        > {
            Err(SessionError::Unsupported("append_system_context".to_string()).into())
        }

        async fn stage_tool_results(
            &self,
            _id: &meerkat_core::types::SessionId,
            _req: meerkat_core::service::StageToolResultsRequest,
        ) -> Result<meerkat_core::service::StageToolResultsResult, SessionError> {
            Err(SessionError::Unsupported("stage_tool_results".to_string()))
        }
    }

    #[async_trait]
    impl meerkat_core::service::SessionServiceHistoryExt for AbsorberInnerProbe {
        async fn read_history(
            &self,
            id: &meerkat_core::types::SessionId,
            _query: meerkat_core::service::SessionHistoryQuery,
        ) -> Result<meerkat_core::service::SessionHistoryPage, SessionError> {
            Err(SessionError::NotFound { id: id.clone() })
        }
    }

    #[async_trait]
    impl MobSessionService for AbsorberInnerProbe {
        // In-memory double: its sessions never report an active turn.
        async fn subscribe_session_activity(
            &self,
            _session_id: &meerkat_core::SessionId,
        ) -> Result<meerkat_mob::MemberSessionActivity, meerkat_core::SessionError> {
            Ok(meerkat_mob::MemberSessionActivity::inactive())
        }

        // In-memory double (meerkat 0.8.47): export visibility is its durable source.
        async fn observe_live_durable_source(
            &self,
            session_id: &meerkat_core::SessionId,
        ) -> Result<meerkat_mob::LiveDurableSourceObservation, meerkat_core::SessionError> {
            meerkat_mob::observe_live_durable_source_via_projection_visibility(self, session_id)
                .await
        }

        // In-memory double: `read` is its published state (meerkat 0.8.45).
        async fn observe_member_status_view(
            &self,
            session_id: &meerkat_core::SessionId,
        ) -> Result<meerkat_mob::MemberStatusSessionView, meerkat_core::SessionError> {
            meerkat_mob::observe_member_status_view_via_read(self, session_id).await
        }

        async fn commit_live_delegation_final_transcript(
            &self,
            _machine: &meerkat_runtime::MeerkatMachine,
            _session_id: &meerkat_core::SessionId,
            _provisional: meerkat_core::ProvisionalLiveHandoff,
            _final_event: meerkat_core::RealtimeTranscriptEvent,
        ) -> Result<meerkat_core::FinalLiveUserTranscriptCommitEvidence, SessionError> {
            Err(SessionError::Unsupported(
                "absorber probe has no Live commit authority".into(),
            ))
        }

        // meerkat 0.8.41 made `fork_persisted_session_at_turn_boundary` REQUIRED.
        // This double owns no durable transcript, so it refuses explicitly rather
        // than inheriting a forward it cannot honour.
        async fn commit_live_delegation_final_transcript_at_turn_boundary(
            &self,
            _machine: &meerkat_runtime::MeerkatMachine,
            _session_id: &meerkat_core::types::SessionId,
            _provisional: meerkat_core::ProvisionalLiveHandoff,
            _final_event: meerkat_core::RealtimeTranscriptEvent,
            _bound: std::time::Duration,
        ) -> Result<meerkat_core::LiveFinalTranscriptCommitAtTurnBoundary, SessionError> {
            Err(SessionError::Unsupported(
                "absorber probe has no live final transcript turn-boundary commit authority".into(),
            ))
        }

        async fn fork_persisted_session_at_turn_boundary(
            &self,
            _source_session_id: &meerkat_core::types::SessionId,
            _message_count: Option<usize>,
            _tool_access_policy: Option<meerkat_core::ops::ToolAccessPolicy>,
            _target: meerkat_core::DurableSessionForkTarget,
            _bound: std::time::Duration,
        ) -> Result<meerkat_core::DurableForkAtTurnBoundary, SessionError> {
            Err(SessionError::Unsupported(
                "absorber probe has no durable turn-boundary fork authority".into(),
            ))
        }

        // meerkat 0.8.30 made `enqueue_committed_parent_session_boundary_after_runtime_turn`
        // REQUIRED, deleting the default that returned `Unsupported` for a
        // persistent profile and `Ok(0)` otherwise. That default is exactly how
        // production wrappers silently inherited a hole that only a Turbo S
        // execution red found. This double is non-persistent, so `Ok(0)` is the
        // correct behaviour - but it is guarded rather than bare: a bare `Ok(0)`
        // is indistinguishable at the call site from "projected nothing because
        // there was nothing to project", so nothing could ever assert the
        // difference if this double later became persistent.
        async fn enqueue_committed_parent_session_boundary_after_runtime_turn(
            &self,
            session_id: &meerkat_core::types::SessionId,
            _runtime_adapter: &meerkat_runtime::MeerkatMachine,
        ) -> Result<usize, meerkat_core::service::SessionError> {
            if self.supports_persistent_sessions() {
                return Err(meerkat_core::service::SessionError::Unsupported(format!(
                    "AbsorberInnerProbe became persistent and must project the committed \
                     parent-session boundary through canonical session ownership \
                     for {session_id}"
                )));
            }
            Ok(0)
        }

        // 0.8.22 made `materialize_session_resume_verdict` REQUIRED, deliberately
        // without a default, so that a PERSISTENT decorator cannot inherit a
        // composition that converges nothing and silently resume from stale
        // committed authority after a power cut. This probe is non-persistent, so
        // it opts in EXPLICITLY through the public helper rather than hand-rolling
        // the composition. The helper is fail-closed: it refuses if
        // `supports_persistent_sessions()` is ever true here.
        async fn materialize_session_resume_verdict(
            &self,
            session_id: &meerkat_core::types::SessionId,
        ) -> Result<meerkat_mob::SessionResumeVerdict, meerkat_core::service::SessionError>
        {
            meerkat_mob::materialize_nonpersistent_session_resume_verdict(self, session_id).await
        }
        async fn observe_session_resume_authority(
            &self,
            _session_id: &meerkat_core::types::SessionId,
        ) -> Result<meerkat_mob::SessionResumeAuthority, SessionError> {
            // Test double: truthfully an empty authority bundle (the ephemeral
            // arm of the meerkat 0.8.22 resume-verdict contract). meerkat
            // 0.8.22 deleted `prepare_session_for_resume`; this double owns
            // nothing durable to converge, so the trait's non-persistent
            // `materialize_session_resume_verdict` default is the truthful
            // answer and is deliberately left un-overridden here.
            Ok(meerkat_mob::SessionResumeAuthority::default())
        }
        async fn acknowledge_committed_runtime_session_boundary_under_turn_finalization_boundary(
            &self,
            _session_id: &meerkat_core::types::SessionId,
            _authority: &meerkat_core::CommittedSessionBoundaryAuthority,
        ) -> Result<(), SessionError> {
            Err(SessionError::Unsupported(
                "test double does not acknowledge store-owned runtime boundaries".to_string(),
            ))
        }
        async fn load_session_for_resume(
            &self,
            session_id: &meerkat_core::types::SessionId,
        ) -> Result<meerkat_mob::ResumeSessionLoad, SessionError> {
            // Truthful derived answer: this double's resume visibility IS its
            // typed reads' visibility (the meerkat-mob test-double idiom).
            if let Some(session) = self.load_persisted_session(session_id).await? {
                return Ok(meerkat_mob::ResumeSessionLoad::Active(Box::new(session)));
            }
            if let Some(session) = self.load_revivable_retired_session(session_id).await? {
                return Ok(meerkat_mob::ResumeSessionLoad::Revivable(Box::new(session)));
            }
            Ok(meerkat_mob::ResumeSessionLoad::Absent)
        }

        async fn create_session_under_runtime_turn_boundary(
            &self,
            req: CreateSessionRequest,
        ) -> Result<meerkat_core::types::RunResult, SessionError> {
            meerkat_core::SessionService::create_session(self, req).await
        }

        async fn archive_with_mob_lifecycle_authority_under_runtime_turn_boundary(
            &self,
            session_id: &meerkat_core::types::SessionId,
        ) -> Result<(), SessionError> {
            meerkat_core::SessionService::archive(self, session_id).await
        }

        async fn discard_live_session_under_runtime_turn_boundary(
            &self,
            _session_id: &meerkat_core::types::SessionId,
        ) -> Result<(), SessionError> {
            Ok(())
        }

        async fn load_persisted_session(
            &self,
            session_id: &meerkat_core::types::SessionId,
        ) -> Result<Option<meerkat_core::session::Session>, SessionError> {
            self.loads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if session_id == self.session.id() {
                Ok(Some(self.session.clone()))
            } else {
                Ok(None)
            }
        }

        async fn tool_scope_snapshot(
            &self,
            _session_id: &meerkat_core::types::SessionId,
        ) -> Result<Option<meerkat_core::ToolScopeSnapshot>, SessionError> {
            self.catalog_reads
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(Some(meerkat_core::ToolScopeSnapshot {
                known_base_names: self.catalog_names.clone(),
                visible_names: self.catalog_names.clone(),
                capability_base_filter: meerkat_core::ToolFilter::All,
                policy_base_filter: meerkat_core::ToolFilter::All,
                base_filter: meerkat_core::ToolFilter::All,
                active_external_filter: meerkat_core::ToolFilter::All,
                active_turn_allow: None,
                active_turn_deny: Vec::new(),
                active_revision: meerkat_core::ToolScopeRevision(0),
                staged_external_filter: meerkat_core::ToolFilter::All,
                staged_revision: meerkat_core::ToolScopeRevision(0),
            }))
        }
    }

    fn invariant_member_request(
        session_id: Option<meerkat_core::types::SessionId>,
        role: &str,
    ) -> CreateSessionRequest {
        let mut req = shell_visibility_request(None, None);
        let build = req.build.as_mut().expect("build options");
        build.mob_member_binding = Some(meerkat_core::MobMemberBinding {
            mob_id: "capability-choke".to_string(),
            role: role.to_string(),
            member: format!("{role}-1"),
        });
        build.override_builtins = meerkat_core::ToolCategoryOverride::Enable;
        build.override_shell = meerkat_core::ToolCategoryOverride::Enable;
        build.override_comms = meerkat_core::ToolCategoryOverride::Enable;
        build.override_memory = meerkat_core::ToolCategoryOverride::Enable;
        build.override_schedule = meerkat_core::ToolCategoryOverride::Enable;
        build.override_workgraph = meerkat_core::ToolCategoryOverride::Enable;
        build.override_mob = meerkat_core::ToolCategoryOverride::Enable;
        build.override_image_generation = meerkat_core::ToolCategoryOverride::Enable;
        build.override_web_search = meerkat_core::ToolCategoryOverride::Enable;
        build.resume_session = session_id.map(meerkat_core::Session::with_id);
        req
    }

    /// `MobBootstrapSpec::new` is the common library, builder, and gateway
    /// composition choke. Inline and realm profiles both arrive here only
    /// after Meerkat has resolved them to the same typed `SessionBuildOptions`.
    /// Every call, including repeated resume materializations, must read the
    /// newly materialized session's actual catalog exactly once.
    #[tokio::test]
    async fn bootstrap_choke_observes_inline_realm_fresh_and_every_resume() {
        let session = meerkat_core::Session::with_id(meerkat_core::types::SessionId::new());
        let session_id = session.id().clone();
        let probe = Arc::new(AbsorberInnerProbe::new(session).with_catalog([
            "task_create",
            "shell",
            "send_message",
            "memory_search",
            "meerkat_schedule_list",
            "workgraph_get",
            "spawn_member",
            "generate_image",
            "web_search",
        ]));
        let definition = meerkat_mob::MobDefinition::from_toml(
            r#"
[mob]
id = "capability-choke"

[profiles.inline-worker]
model = "gpt-5.5"

[profiles.inline-worker.tools]
comms = true
"#,
        )
        .expect("definition parses");
        let spec = MobBootstrapSpec::new(
            definition,
            meerkat_mob::MobStorage::in_memory(),
            probe.clone(),
        );

        for (role, resume) in [
            ("inline-worker", None),
            ("inline-worker", Some(session_id.clone())),
            ("realm-worker", None),
            ("realm-worker", Some(session_id.clone())),
        ] {
            meerkat_core::service::SessionService::create_session(
                spec.session_service.as_ref(),
                invariant_member_request(resume, role),
            )
            .await
            .expect("materialize through bootstrap choke");
        }

        assert_eq!(
            probe
                .catalog_reads
                .load(std::sync::atomic::Ordering::SeqCst),
            4,
            "fresh and resumed materializations for both resolved profile provenances must each evaluate the live exact catalog"
        );
    }

    /// What `ForwardingProbe` answers for a member-status view, so a test can
    /// tell a forwarded answer from a wrapper-level read.
    const FORWARDED_STATUS_PREVIEW: &str = "committed preview from the inner service";
    const FORWARDED_STATUS_TOKENS: u64 = 4242;
    const FORWARDED_DURABLE_REVISION: u64 = 7;

    /// The archive authority's answer the probe gives.
    #[derive(Clone, Copy, Default)]
    enum ArchiveAuthorityAnswer {
        /// Unset: every session is known.
        #[default]
        KnowsEverySession,
        /// The read answers this.
        Answers(bool),
        /// The read fails.
        Fails,
    }

    /// A failed execution snapshot, and what the typed local reads answer
    /// for its session.
    #[derive(Clone, Copy)]
    struct FailedSnapshot {
        /// The live-actor read's answer; `None`: the read fails.
        live: Option<bool>,
        /// The durable store holds the session.
        persisted: bool,
    }

    #[derive(Default)]
    struct ForwardingProbe {
        /// Set: the execution snapshot fails and the typed local reads answer
        /// this.
        failed_snapshot: Mutex<Option<FailedSnapshot>>,
        acquisition_outcome: std::sync::atomic::AtomicU8,
        execution_snapshot_reads: Arc<std::sync::atomic::AtomicUsize>,
        acquisition_inputs: Mutex<Vec<Option<Arc<meerkat_runtime::MeerkatMachine>>>>,
        calls: Mutex<Vec<&'static str>>,
        boundary_deliveries: Mutex<
            Vec<(
                meerkat_core::SessionId,
                meerkat_core::RunId,
                meerkat_core::TurnBoundaryDelivery,
            )>,
        >,
        live_machine: Mutex<Option<Arc<meerkat_runtime::MeerkatMachine>>>,
        live_commit: Mutex<
            Option<(
                meerkat_core::SessionId,
                meerkat_core::ProvisionalLiveHandoff,
                meerkat_core::RealtimeTranscriptEvent,
            )>,
        >,
        cancel_outcome: std::sync::atomic::AtomicU8,
        turn_request: Mutex<
            Option<(
                meerkat_core::types::SessionId,
                meerkat_core::service::StartTurnRequest,
            )>,
        >,
        turn_admission: tokio::sync::Notify,
        turn_terminal: tokio::sync::Notify,
        turn_outcome: std::sync::atomic::AtomicU8,
        retained_metadata_reply:
            Mutex<Option<Result<Option<meerkat_core::PersistedSessionMetadataView>, SessionError>>>,
        retained_metadata_requests: Mutex<Vec<meerkat_core::SessionId>>,
        /// The controller route observation the probe answers; `None` answers
        /// as a lost actor.
        controller_route_answer: Mutex<Option<meerkat_mob::ControllerRouteObservation>>,
        /// The controller client the probe pins under the turn boundary;
        /// `None` answers as a lost actor.
        controller_pin_answer: Mutex<Option<meerkat_core::ControllerModelClient>>,
        /// The archive authority's answer.
        archive_authority_answer: Mutex<ArchiveAuthorityAnswer>,
        /// Every compaction-finalization acknowledgement the probe received,
        /// with the authority it carried.
        acknowledged_compaction_finalizations: Mutex<
            Vec<(
                meerkat_core::types::SessionId,
                meerkat_core::CommittedSessionBoundaryAuthority,
            )>,
        >,
        /// Every exact run the probe was asked to suspend.
        suspended_runs: Mutex<
            Vec<(
                meerkat_core::types::SessionId,
                meerkat_core::lifecycle::RunId,
            )>,
        >,
        /// Set: the strict read refuses with the transcript revision conflict,
        /// as it does while a run's provisional tail stands.
        strict_read_refuses: std::sync::atomic::AtomicBool,
        /// The committed snapshot the tolerant read serves; `None` serves an
        /// empty session.
        tolerant_read_session: Mutex<Option<meerkat_core::Session>>,
        /// Set: `create_session` accepts and records each request's initial
        /// tool filter instead of refusing.
        accept_creates: std::sync::atomic::AtomicBool,
        created_tool_filters: Mutex<Vec<Option<meerkat_core::ToolFilter>>>,
    }

    impl ForwardingProbe {
        fn record(&self, call: &'static str) {
            self.calls
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(call);
        }

        fn calls(&self) -> Vec<&'static str> {
            self.calls
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        }

        fn set_cancel_outcome(&self, outcome: u8) {
            self.cancel_outcome
                .store(outcome, std::sync::atomic::Ordering::Relaxed);
        }
    }

    #[async_trait]
    impl meerkat_core::service::SessionService for ForwardingProbe {
        async fn update_session_mcp_attachment_intent(
            &self,
            _id: &meerkat_core::types::SessionId,
            mutation: meerkat_core::mcp_session_intent::McpAttachmentIntentMutation,
        ) -> Result<meerkat_core::mcp_session_intent::McpAttachmentIntentRecorded, SessionError>
        {
            self.record("update_session_mcp_attachment_intent");
            let meerkat_core::mcp_session_intent::McpAttachmentIntentMutation::Withdraw {
                server_name,
            } = mutation
            else {
                panic!("the probe answers only the mutation its test sends");
            };
            Ok(
                meerkat_core::mcp_session_intent::McpAttachmentIntentRecorded {
                    server_name,
                    revision: Some(FORWARDED_DURABLE_REVISION),
                    changed: true,
                },
            )
        }

        async fn create_session(
            &self,
            req: CreateSessionRequest,
        ) -> Result<meerkat_core::types::RunResult, SessionError> {
            if !self
                .accept_creates
                .load(std::sync::atomic::Ordering::SeqCst)
            {
                return Err(SessionError::Unsupported("create_session".to_string()));
            }
            self.record("create_session");
            let session_id = req
                .build
                .as_ref()
                .and_then(|build| build.resume_session.as_ref())
                .map_or_else(meerkat_core::types::SessionId::new, |session| {
                    session.id().clone()
                });
            self.created_tool_filters
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(
                    req.build
                        .as_ref()
                        .and_then(|build| build.initial_tool_filter.clone()),
                );
            Ok(meerkat_core::RunResult {
                text: String::new(),
                session_id,
                usage: meerkat_core::types::Usage::default(),
                request_usage: Vec::new(),
                run_usage: None,
                turns: 0,
                tool_calls: 0,
                terminal_cause_kind: None,
                structured_output: None,
                extraction_error: None,
                schema_warnings: None,
                skill_diagnostics: None,
            })
        }

        async fn start_turn(
            &self,
            _id: &meerkat_core::types::SessionId,
            _req: meerkat_core::service::StartTurnRequest,
        ) -> Result<meerkat_core::types::RunResult, SessionError> {
            Err(SessionError::Unsupported("start_turn".to_string()))
        }

        async fn reconcile_runtime_compaction_projections(
            &self,
            _id: &meerkat_core::types::SessionId,
            _intents: Vec<meerkat_core::CompactionProjectionIntent>,
        ) -> Result<(), SessionError> {
            self.record("reconcile_runtime_compaction_projections");
            Ok(())
        }

        async fn acknowledge_runtime_compaction_finalization(
            &self,
            id: &meerkat_core::types::SessionId,
            authority: &meerkat_core::CommittedSessionBoundaryAuthority,
        ) -> Result<(), SessionError> {
            self.record("acknowledge_runtime_compaction_finalization");
            self.acknowledged_compaction_finalizations
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push((id.clone(), authority.clone()));
            Ok(())
        }

        async fn abort_uncommitted_compaction_projections(
            &self,
            _id: &meerkat_core::types::SessionId,
        ) -> Result<(), SessionError> {
            self.record("abort_uncommitted_compaction_projections");
            Ok(())
        }

        async fn abort_rejected_runtime_run_projections(
            &self,
            _id: &meerkat_core::types::SessionId,
        ) -> Result<(), SessionError> {
            self.record("abort_rejected_runtime_run_projections");
            Ok(())
        }

        async fn interrupt(
            &self,
            _id: &meerkat_core::types::SessionId,
        ) -> Result<(), SessionError> {
            self.record("interrupt");
            Ok(())
        }

        async fn read(
            &self,
            id: &meerkat_core::types::SessionId,
        ) -> Result<meerkat_core::service::SessionView, SessionError> {
            self.record("read");
            Err(SessionError::NotFound { id: id.clone() })
        }

        async fn list(
            &self,
            _query: meerkat_core::service::SessionQuery,
        ) -> Result<Vec<meerkat_core::service::SessionSummary>, SessionError> {
            Ok(Vec::new())
        }

        async fn archive(&self, _id: &meerkat_core::types::SessionId) -> Result<(), SessionError> {
            self.record("archive");
            Ok(())
        }

        async fn record_live_terminal_error(
            &self,
            _id: &meerkat_core::types::SessionId,
            _cause: meerkat_core::live_adapter::LiveAdapterErrorCode,
        ) -> Result<(), SessionError> {
            self.record("record_live_terminal_error");
            Ok(())
        }

        async fn record_live_output_audio_degraded(
            &self,
            _id: &meerkat_core::types::SessionId,
            _dropped: u64,
        ) -> Result<(), SessionError> {
            self.record("record_live_output_audio_degraded");
            Ok(())
        }
    }

    #[async_trait]
    impl meerkat_core::service::SessionServiceCommsExt for ForwardingProbe {}

    #[async_trait]
    impl meerkat_core::service::SessionServiceControlExt for ForwardingProbe {
        async fn append_system_context(
            &self,
            _id: &meerkat_core::types::SessionId,
            _req: meerkat_core::service::AppendSystemContextRequest,
        ) -> Result<
            meerkat_core::service::AppendSystemContextResult,
            meerkat_core::service::SessionControlError,
        > {
            self.record("append_system_context");
            Ok(meerkat_core::service::AppendSystemContextResult {
                status: meerkat_core::service::AppendSystemContextStatus::Applied,
            })
        }

        async fn stage_tool_results(
            &self,
            _id: &meerkat_core::types::SessionId,
            _req: meerkat_core::service::StageToolResultsRequest,
        ) -> Result<meerkat_core::service::StageToolResultsResult, SessionError> {
            self.record("stage_tool_results");
            Ok(meerkat_core::service::StageToolResultsResult {
                accepted_result_count: 7,
                // meerkat 0.8.8 added the durable-ingress disposition. This
                // probe records the forwarded call; `Staged` is the ordinary
                // accepted outcome.
                disposition: meerkat_core::service::StageToolResultsDisposition::Staged,
            })
        }
    }

    #[async_trait]
    impl meerkat_core::service::SessionServiceHistoryExt for ForwardingProbe {
        async fn read_history(
            &self,
            id: &meerkat_core::types::SessionId,
            _query: meerkat_core::service::SessionHistoryQuery,
        ) -> Result<meerkat_core::service::SessionHistoryPage, SessionError> {
            self.record("read_history");
            Err(SessionError::NotFound { id: id.clone() })
        }

        async fn read_transcript_revision(
            &self,
            id: &meerkat_core::types::SessionId,
            _query: meerkat_core::service::SessionTranscriptRevisionQuery,
        ) -> Result<meerkat_core::service::SessionTranscriptRevisionPage, SessionError> {
            self.record("read_transcript_revision");
            Err(SessionError::NotFound { id: id.clone() })
        }

        async fn list_transcript_revisions(
            &self,
            id: &meerkat_core::types::SessionId,
            _query: meerkat_core::service::SessionTranscriptRevisionListQuery,
        ) -> Result<meerkat_core::service::SessionTranscriptRevisionList, SessionError> {
            self.record("list_transcript_revisions");
            Err(SessionError::NotFound { id: id.clone() })
        }
    }

    /// The visible tools the forwarding probe reads for any fork source.
    fn probe_fork_source_tools() -> meerkat_core::fork_notice::ForkSeedSourceTools {
        meerkat_core::fork_notice::ForkSeedSourceTools::Read {
            names: vec![meerkat_core::ToolName::new("probe_tool")],
        }
    }

    /// The spawn-time restrictions the forwarding probe states for any
    /// predecessor session.
    fn probe_predecessor_restrictions() -> meerkat_core::PredecessorToolRestrictions {
        meerkat_core::PredecessorToolRestrictions::from_build_handoff(
            Some(meerkat_core::ops::ToolAccessPolicy::DenyList(
                ["shell"].into_iter().collect(),
            )),
            None,
        )
    }

    #[async_trait]
    impl MobSessionService for ForwardingProbe {
        async fn fork_source_tools(
            &self,
            _session_id: &meerkat_core::SessionId,
        ) -> meerkat_core::fork_notice::ForkSeedSourceTools {
            self.record("fork_source_tools");
            probe_fork_source_tools()
        }

        async fn provisional_tail_stands(
            &self,
            _session_id: &meerkat_core::SessionId,
        ) -> Result<bool, SessionError> {
            self.record("provisional_tail_stands");
            // The trait default never answers that a tail stands.
            Ok(true)
        }

        async fn predecessor_tool_restrictions(
            &self,
            _session_id: &meerkat_core::SessionId,
        ) -> Result<meerkat_core::PredecessorToolRestrictions, SessionError> {
            self.record("predecessor_tool_restrictions");
            Ok(probe_predecessor_restrictions())
        }

        async fn load_retained_session_metadata(
            &self,
            session_id: &meerkat_core::SessionId,
        ) -> Result<Option<meerkat_core::PersistedSessionMetadataView>, SessionError> {
            self.record("load_retained_session_metadata");
            self.retained_metadata_requests
                .lock()
                .expect("retained requests")
                .push(session_id.clone());
            self.retained_metadata_reply
                .lock()
                .expect("retained reply")
                .take()
                .unwrap_or_else(|| {
                    Err(SessionError::Unsupported(
                        "probe has no configured retained metadata authority".to_string(),
                    ))
                })
        }

        async fn controller_route_under_runtime_turn_boundary(
            &self,
            actor: &meerkat_session::LiveSessionActorWitness,
        ) -> Result<meerkat_mob::ControllerRouteObservation, SessionError> {
            self.record("controller_route_under_runtime_turn_boundary");
            self.controller_route_answer
                .lock()
                .expect("controller route answer")
                .clone()
                .ok_or_else(|| SessionError::NotFound {
                    id: actor.session_id().clone(),
                })
        }

        async fn pin_controller_client_under_runtime_turn_boundary(
            &self,
            actor: &meerkat_session::LiveSessionActorWitness,
        ) -> Result<meerkat_core::ControllerModelClient, SessionError> {
            self.record("pin_controller_client_under_runtime_turn_boundary");
            self.controller_pin_answer
                .lock()
                .expect("controller pin answer")
                .clone()
                .ok_or_else(|| SessionError::NotFound {
                    id: actor.session_id().clone(),
                })
        }

        async fn await_removed_actor_exit(&self, _session_id: &meerkat_core::types::SessionId) {
            self.record("await_removed_actor_exit");
        }

        async fn load_persisted_session_metadata(
            &self,
            _session_id: &meerkat_core::SessionId,
        ) -> Result<Option<meerkat_core::PersistedSessionMetadataView>, SessionError> {
            self.record("load_persisted_session_metadata");
            let persisted = self
                .failed_snapshot
                .lock()
                .expect("failed snapshot")
                .is_some_and(|failed| failed.persisted);
            if persisted {
                let session = meerkat_core::Session::new();
                return Ok(Some(
                    meerkat_core::PersistedSessionMetadataView::try_from_session(&session)
                        .expect("a fresh session's metadata view"),
                ));
            }
            Ok(None)
        }

        async fn load_persisted_session(
            &self,
            session_id: &meerkat_core::SessionId,
        ) -> Result<Option<meerkat_core::Session>, SessionError> {
            self.record("load_persisted_session");
            if self
                .strict_read_refuses
                .load(std::sync::atomic::Ordering::SeqCst)
            {
                return Err(SessionError::Store(Box::new(
                    meerkat_core::session_store::SessionStoreError::TranscriptRevisionConflict {
                        id: session_id.clone(),
                        expected: "committed-boundary".to_string(),
                        actual: "provisional-tail".to_string(),
                    },
                )));
            }
            Ok(None)
        }

        async fn load_persisted_session_tolerating_tail(
            &self,
            _session_id: &meerkat_core::SessionId,
        ) -> Result<Option<meerkat_core::Session>, SessionError> {
            self.record("load_persisted_session_tolerating_tail");
            // The trait default is the strict read, which this probe answers
            // `None` (and records as `load_persisted_session`).
            Ok(Some(
                self.tolerant_read_session
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone()
                    .unwrap_or_default(),
            ))
        }

        async fn subscribe_session_activity(
            &self,
            _session_id: &meerkat_core::SessionId,
        ) -> Result<meerkat_mob::MemberSessionActivity, meerkat_core::SessionError> {
            self.record("subscribe_session_activity");
            Ok(meerkat_mob::MemberSessionActivity::inactive())
        }

        async fn observe_live_durable_source(
            &self,
            _session_id: &meerkat_core::SessionId,
        ) -> Result<meerkat_mob::LiveDurableSourceObservation, meerkat_core::SessionError> {
            self.record("observe_live_durable_source");
            Ok(meerkat_mob::LiveDurableSourceObservation::Committed {
                revision: Some(FORWARDED_DURABLE_REVISION),
            })
        }

        async fn tool_catalog_view(
            &self,
            _session_id: &meerkat_core::SessionId,
        ) -> Result<Option<meerkat_core::ToolCatalogView>, meerkat_core::SessionError> {
            self.record("tool_catalog_view");
            Ok(Some(forwarded_catalog_view()))
        }

        async fn observe_member_status_view(
            &self,
            _session_id: &meerkat_core::SessionId,
        ) -> Result<meerkat_mob::MemberStatusSessionView, meerkat_core::SessionError> {
            self.record("observe_member_status_view");
            Ok(meerkat_mob::MemberStatusSessionView::live_watch(
                Some(FORWARDED_STATUS_PREVIEW.to_string()),
                FORWARDED_STATUS_TOKENS,
            ))
        }

        async fn commit_live_delegation_final_transcript(
            &self,
            machine: &meerkat_runtime::MeerkatMachine,
            session_id: &meerkat_core::SessionId,
            provisional: meerkat_core::ProvisionalLiveHandoff,
            final_event: meerkat_core::RealtimeTranscriptEvent,
        ) -> Result<meerkat_core::FinalLiveUserTranscriptCommitEvidence, SessionError> {
            assert!(std::ptr::eq(
                machine,
                self.live_machine
                    .lock()
                    .expect("live machine")
                    .as_deref()
                    .expect("expected machine"),
            ));
            self.record("commit_live_delegation_final_transcript");
            *self.live_commit.lock().expect("live commit") =
                Some((session_id.clone(), provisional, final_event));
            Err(SessionError::NotFound {
                id: session_id.clone(),
            })
        }

        // meerkat 0.8.41 made `fork_persisted_session_at_turn_boundary` REQUIRED.
        // This double owns no durable transcript, so it refuses explicitly rather
        // than inheriting a forward it cannot honour.
        async fn commit_live_delegation_final_transcript_at_turn_boundary(
            &self,
            _machine: &meerkat_runtime::MeerkatMachine,
            _session_id: &meerkat_core::types::SessionId,
            _provisional: meerkat_core::ProvisionalLiveHandoff,
            _final_event: meerkat_core::RealtimeTranscriptEvent,
            _bound: std::time::Duration,
        ) -> Result<meerkat_core::LiveFinalTranscriptCommitAtTurnBoundary, SessionError> {
            self.record("commit_live_delegation_final_transcript_at_turn_boundary");
            Err(SessionError::Unsupported(
                "fork_persisted_session_at_turn_boundary".into(),
            ))
        }

        async fn fork_persisted_session_at_turn_boundary(
            &self,
            _source_session_id: &meerkat_core::types::SessionId,
            _message_count: Option<usize>,
            _tool_access_policy: Option<meerkat_core::ops::ToolAccessPolicy>,
            _target: meerkat_core::DurableSessionForkTarget,
            _bound: std::time::Duration,
        ) -> Result<meerkat_core::DurableForkAtTurnBoundary, SessionError> {
            self.record("fork_persisted_session_at_turn_boundary");
            Err(SessionError::Unsupported(
                "forwarding probe has no durable turn-boundary fork authority".into(),
            ))
        }

        #[cfg(feature = "openai-live")]
        async fn validate_live_bridge_member_eligibility(
            &self,
            _session_id: &meerkat_core::SessionId,
        ) -> Result<(), SessionError> {
            self.record("validate_live_bridge_member_eligibility");
            Ok(())
        }

        #[cfg(feature = "openai-live")]
        async fn capture_live_bridge_execution_snapshot(
            &self,
            session_id: &meerkat_core::SessionId,
            _agent_identity: &str,
        ) -> Result<meerkat_mob::LiveBridgeExecutionSnapshot, SessionError> {
            self.record("capture_live_bridge_execution_snapshot");
            Err(SessionError::NotFound {
                id: session_id.clone(),
            })
        }

        async fn start_turn_with_admission_notification(
            &self,
            session_id: &meerkat_core::types::SessionId,
            req: meerkat_core::service::StartTurnRequest,
            admitted: tokio::sync::oneshot::Sender<()>,
        ) -> Result<meerkat_core::RunResult, SessionError> {
            self.record("start_turn_with_admission_notification");
            *self.turn_request.lock().expect("turn request") = Some((session_id.clone(), req));
            self.turn_admission.notified().await;
            let outcome = self.turn_outcome.load(std::sync::atomic::Ordering::Relaxed);
            if outcome == 1 {
                return Err(SessionError::NotRunning {
                    id: session_id.clone(),
                });
            }
            admitted.send(()).expect("original admission observer");
            self.turn_terminal.notified().await;
            if outcome == 2 {
                return Err(SessionError::NotFound {
                    id: session_id.clone(),
                });
            }
            Ok(meerkat_core::RunResult {
                text: "owner terminal result".to_string(),
                session_id: session_id.clone(),
                usage: meerkat_core::types::Usage::default(),
                request_usage: Vec::new(),
                run_usage: None,
                turns: 3,
                tool_calls: 2,
                terminal_cause_kind: None,
                structured_output: None,
                extraction_error: None,
                schema_warnings: None,
                skill_diagnostics: None,
            })
        }

        // 0.8.22 made `materialize_session_resume_verdict` REQUIRED, deliberately
        // without a default, so that a PERSISTENT decorator cannot inherit a
        // composition that converges nothing and silently resume from stale
        // committed authority after a power cut. This probe is non-persistent, so
        // it opts in EXPLICITLY through the public helper rather than hand-rolling
        // the composition. The helper is fail-closed: it refuses if
        // `supports_persistent_sessions()` is ever true here.
        async fn materialize_session_resume_verdict(
            &self,
            session_id: &meerkat_core::types::SessionId,
        ) -> Result<meerkat_mob::SessionResumeVerdict, meerkat_core::service::SessionError>
        {
            meerkat_mob::materialize_nonpersistent_session_resume_verdict(self, session_id).await
        }
        async fn settle_durable_tail_before_cold_start(
            &self,
            _session_id: &meerkat_core::types::SessionId,
        ) -> Result<(), SessionError> {
            self.record("settle_durable_tail_before_cold_start");
            Ok(())
        }
        async fn settle_durable_tail_before_cold_start_reporting(
            &self,
            _session_id: &meerkat_core::types::SessionId,
        ) -> Result<meerkat_mob::ColdStartTailSettlement, SessionError> {
            self.record("settle_durable_tail_before_cold_start_reporting");
            // The trait default settles through the plain settle and reports
            // `Settled`.
            Ok(meerkat_mob::ColdStartTailSettlement::ContinuesOpenRun {
                run_id: meerkat_core::lifecycle::RunId::new(),
                committed: Box::new(meerkat_core::Session::new()),
            })
        }
        async fn observe_session_resume_authority(
            &self,
            _session_id: &meerkat_core::types::SessionId,
        ) -> Result<meerkat_mob::SessionResumeAuthority, SessionError> {
            // Test double: truthfully an empty authority bundle (the ephemeral
            // arm of the meerkat 0.8.22 resume-verdict contract). meerkat
            // 0.8.22 deleted `prepare_session_for_resume`; this double owns
            // nothing durable to converge, so the trait's non-persistent
            // `materialize_session_resume_verdict` default is the truthful
            // answer and is deliberately left un-overridden here.
            Ok(meerkat_mob::SessionResumeAuthority::default())
        }
        // NOT cfg-gated on meta's own `experimental-gpt-live`: the trait requires
        // this whenever MEERKAT's feature is on, and --all-targets dev-dependency
        // unification enables meerkat's while meta's own flag stays off.
        async fn enqueue_committed_parent_session_boundary_after_runtime_turn(
            &self,
            _session_id: &meerkat_core::types::SessionId,
            _runtime_adapter: &meerkat_runtime::MeerkatMachine,
        ) -> Result<usize, SessionError> {
            self.record("enqueue_committed_parent_session_boundary_after_runtime_turn");
            Ok(7)
        }
        async fn acknowledge_committed_runtime_session_boundary_under_turn_finalization_boundary(
            &self,
            _session_id: &meerkat_core::types::SessionId,
            _authority: &meerkat_core::CommittedSessionBoundaryAuthority,
        ) -> Result<(), SessionError> {
            Err(SessionError::Unsupported(
                "test double does not acknowledge store-owned runtime boundaries".to_string(),
            ))
        }
        async fn load_session_for_resume(
            &self,
            session_id: &meerkat_core::types::SessionId,
        ) -> Result<meerkat_mob::ResumeSessionLoad, SessionError> {
            // Truthful derived answer: this double's resume visibility IS its
            // typed reads' visibility (the meerkat-mob test-double idiom).
            if let Some(session) = self.load_persisted_session(session_id).await? {
                return Ok(meerkat_mob::ResumeSessionLoad::Active(Box::new(session)));
            }
            if let Some(session) = self.load_revivable_retired_session(session_id).await? {
                return Ok(meerkat_mob::ResumeSessionLoad::Revivable(Box::new(session)));
            }
            Ok(meerkat_mob::ResumeSessionLoad::Absent)
        }

        async fn create_session_under_runtime_turn_boundary(
            &self,
            req: CreateSessionRequest,
        ) -> Result<meerkat_core::types::RunResult, SessionError> {
            meerkat_core::SessionService::create_session(self, req).await
        }

        fn supports_persistent_sessions(&self) -> bool {
            true
        }

        fn keeps_durable_member_sessions(&self) -> bool {
            // The trait default answers `supports_persistent_sessions`, true.
            false
        }

        async fn execution_snapshot(
            &self,
            session_id: &meerkat_core::SessionId,
        ) -> Result<Option<meerkat_core::AgentExecutionSnapshot>, SessionError> {
            self.execution_snapshot_reads
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if self
                .failed_snapshot
                .lock()
                .expect("failed snapshot")
                .is_some()
            {
                return Err(SessionError::NotFound {
                    id: session_id.clone(),
                });
            }
            Ok(None)
        }

        async fn live_session_actor_registered(
            &self,
            session_id: &meerkat_core::SessionId,
        ) -> Result<bool, SessionError> {
            let failed = *self.failed_snapshot.lock().expect("failed snapshot");
            match failed {
                Some(FailedSnapshot {
                    live: Some(live), ..
                }) => Ok(live),
                Some(FailedSnapshot { live: None, .. }) => Err(SessionError::Store(Box::new(
                    std::io::Error::other("probe: the live-actor read fails"),
                ))),
                None => {
                    <Self as meerkat_core::service::SessionService>::has_live_session(
                        self, session_id,
                    )
                    .await
                }
            }
        }

        fn acquire_runtime_adapter(
            &self,
            explicit: Option<Arc<meerkat_runtime::MeerkatMachine>>,
        ) -> Result<Option<Arc<meerkat_runtime::MeerkatMachine>>, meerkat_runtime::RuntimeDriverError>
        {
            self.acquisition_inputs
                .lock()
                .expect("acquisition inputs")
                .push(explicit.clone());
            match self
                .acquisition_outcome
                .load(std::sync::atomic::Ordering::Relaxed)
            {
                1 => Ok(None),
                2 => Err(
                    meerkat_runtime::RuntimeDriverError::ControllerReadinessUnavailable {
                        reason:
                            meerkat_runtime::traits::ControllerReadinessFailure::AuthorityChanged,
                    },
                ),
                3 => Err(meerkat_runtime::RuntimeDriverError::RecoveryCorruption {
                    reason: "acquisition probe".to_string(),
                }),
                _ => Ok(Some(explicit.unwrap_or_else(|| {
                    Arc::new(meerkat_runtime::MeerkatMachine::ephemeral())
                }))),
            }
        }

        async fn suspend_run_with_machine_authority(
            &self,
            session_id: &meerkat_core::types::SessionId,
            expected_run_id: &meerkat_core::lifecycle::RunId,
            _authority: meerkat_runtime::MachineSessionControlAuthority,
        ) -> Result<bool, SessionError> {
            self.record("suspend_run_with_machine_authority");
            self.suspended_runs
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push((session_id.clone(), expected_run_id.clone()));
            // The trait default answers `false`.
            Ok(true)
        }

        async fn cancel_after_boundary_with_machine_authority(
            &self,
            session_id: &meerkat_core::types::SessionId,
            _expected_run_id: &meerkat_core::lifecycle::RunId,
            _authority: meerkat_runtime::MachineSessionControlAuthority,
        ) -> Result<(), SessionError> {
            self.record("cancel_after_boundary_with_machine_authority");
            match self
                .cancel_outcome
                .load(std::sync::atomic::Ordering::Relaxed)
            {
                0 => Err(SessionError::NotFound {
                    id: session_id.clone(),
                }),
                1 => Err(SessionError::NotRunning {
                    id: session_id.clone(),
                }),
                2 => Err(SessionError::Unsupported(
                    "synthetic cancel rejection".to_string(),
                )),
                _ => Ok(()),
            }
        }

        async fn archive_with_mob_lifecycle_authority(
            &self,
            _session_id: &meerkat_core::types::SessionId,
        ) -> Result<(), SessionError> {
            self.record("archive_with_mob_lifecycle_authority");
            Ok(())
        }

        async fn archive_with_mob_lifecycle_authority_under_runtime_turn_boundary(
            &self,
            _session_id: &meerkat_core::types::SessionId,
        ) -> Result<(), SessionError> {
            self.record("archive_with_mob_lifecycle_authority_under_runtime_turn_boundary");
            Ok(())
        }

        async fn archive_with_mob_lifecycle_authority_under_runtime_turn_boundary_and_hook_before(
            &self,
            _session_id: &meerkat_core::types::SessionId,
            _deadline: meerkat_core::time_compat::Instant,
            post_commit_hook: Option<Arc<dyn meerkat_runtime::MachineSessionArchivePostCommitHook>>,
        ) -> Result<(), SessionError> {
            match post_commit_hook {
                Some(hook) => {
                    hook.after_runtime_retire_commit()
                        .await
                        .map_err(|e| SessionError::Store(Box::new(e)))?;
                    self.record("archive_and_hook_before(hook=some, fired)");
                }
                None => self.record("archive_and_hook_before(hook=none)"),
            }
            Ok(())
        }

        async fn settle_cold_predecessor_with_mob_lifecycle_authority_before(
            &self,
            session_id: &meerkat_core::types::SessionId,
            _claim: &meerkat_core::session_hosting::HostingClaim,
            _deadline: meerkat_core::time_compat::Instant,
        ) -> Result<
            meerkat_runtime::ColdPredecessorSettlement,
            meerkat_session::ColdPredecessorSettleError,
        > {
            self.record("settle_cold_predecessor_with_mob_lifecycle_authority_before");
            Ok(meerkat_runtime::ColdPredecessorSettlement::untouched(
                session_id.clone(),
            ))
        }

        fn forked_participant_source_runtime(
            self: Arc<Self>,
        ) -> Option<Arc<dyn meerkat_mob::ForkedParticipantSourceRuntime>> {
            self.record("forked_participant_source_runtime");
            None
        }

        fn persisted_session_authority_read_cost(
            &self,
        ) -> meerkat_mob::runtime::PersistedSessionAuthorityReadCost {
            self.record("persisted_session_authority_read_cost");
            meerkat_mob::runtime::PersistedSessionAuthorityReadCost::Bounded
        }

        async fn observe_persisted_session_authority(
            &self,
            _session_id: &meerkat_core::types::SessionId,
        ) -> Result<Option<meerkat_mob::IdentitySessionStoreAuthority>, SessionError> {
            self.record("observe_persisted_session_authority");
            Ok(None)
        }

        async fn revalidate_session_resume_authority(
            &self,
            _session_id: &meerkat_core::types::SessionId,
            _expected: &meerkat_mob::SessionResumeAuthority,
        ) -> Result<Result<(), meerkat_mob::SessionResumeRejection>, SessionError> {
            self.record("revalidate_session_resume_authority");
            Ok(Ok(()))
        }

        async fn discard_live_session_under_runtime_turn_boundary(
            &self,
            _session_id: &meerkat_core::types::SessionId,
        ) -> Result<(), SessionError> {
            self.record("discard_live_session_under_runtime_turn_boundary");
            Ok(())
        }

        async fn session_known_to_archive_authority(
            &self,
            _session_id: &meerkat_core::types::SessionId,
        ) -> Result<bool, SessionError> {
            self.record("session_known_to_archive_authority");
            match *self
                .archive_authority_answer
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
            {
                ArchiveAuthorityAnswer::KnowsEverySession => Ok(true),
                ArchiveAuthorityAnswer::Answers(known) => Ok(known),
                ArchiveAuthorityAnswer::Fails => Err(SessionError::PersistenceDisabled),
            }
        }

        async fn prepare_turn_boundary_delivery_for_active_turn(
            &self,
            session_id: &meerkat_core::types::SessionId,
            expected_run_id: &meerkat_core::lifecycle::RunId,
            delivery: meerkat_core::TurnBoundaryDelivery,
        ) -> Result<meerkat_core::CoreBoundaryStageOutput, meerkat_core::CoreBoundaryStageError>
        {
            self.record("prepare_turn_boundary_delivery_for_active_turn");
            self.boundary_deliveries
                .lock()
                .expect("boundary deliveries")
                .push((session_id.clone(), expected_run_id.clone(), delivery));
            Err(meerkat_core::CoreBoundaryStageError::unavailable(
                "probe has no boundary authority",
            ))
        }
    }

    // Un-gated to match the impls it covers. The forwarding methods are no
    // longer behind meta's own `experimental-gpt-live`, because the trait
    // requires them whenever MEERKAT's feature is on and dev-dependency
    // unification enables meerkat's independently. A gated test over un-gated
    // code means the default build compiles the forwards and asserts nothing
    // about them - which is the same silent-coverage shape the required-method
    // change exists to remove.
    #[tokio::test]
    async fn explicit_completion_observer_is_independent_of_bootstrap_and_recovery() {
        let definition = MobDefinition::from_toml(
            "[mob]\nid = \"completion-observer\"\n[profiles.worker]\nmodel = \"gpt-5.5\"\n",
        )
        .expect("definition");
        let bootstrap = Arc::new(meerkat_runtime::MeerkatMachine::ephemeral());
        let observer = Arc::new(meerkat_runtime::MeerkatMachine::ephemeral());
        let spec = MobBootstrapSpec::new(
            definition,
            MobStorage::in_memory(),
            Arc::new(ForwardingProbe::default()),
        )
        .with_session_runtime_adapter(bootstrap)
        .expect("acquire explicit bootstrap owner")
        .with_runtime_completion_observer(observer.clone());
        let owner = spec
            .committed_boundary_recoverer
            .expect("explicit observer facade");
        assert!(Arc::ptr_eq(
            &owner.runtime_completion_observer().expect("observer"),
            &observer,
        ));
        assert!(matches!(
            owner
                .recover_committed_boundary(&meerkat_core::SessionId::new())
                .await
                .expect("unsupported recovery"),
            crate::identity_first::CommittedBoundaryRepair::Unsupported,
        ));
    }

    /// Both production decorators, alone and stacked as the builder stacks
    /// them, forward the durable-tail settle to the service they wrap.
    /// Without the forward, the trait default materializes the whole resume
    /// verdict instead, which a persistent service answers `Busy` for a
    /// session one of its live actors hosts.
    #[tokio::test]
    async fn mob_session_wrappers_forward_the_claimed_durable_tail_settle() {
        let session_id = meerkat_core::types::SessionId::new();
        let probe = Arc::new(ForwardingProbe::default());
        for (shape, wrapper) in decorated_shapes(&probe) {
            probe.calls.lock().expect("probe calls").clear();
            let settled = wrapper
                .settle_durable_tail_before_cold_start(&session_id)
                .await;
            assert_eq!(
                probe.calls(),
                vec!["settle_durable_tail_before_cold_start"],
                "{shape}: the inner service settles the tail, exactly once"
            );
            settled.expect("the wrapped service settled the tail");
        }
    }

    /// Both production decorators, alone and stacked as the builder stacks
    /// them, forward the runtime's compaction-finalization acknowledgement,
    /// with the exact committed boundary authority, to the service they wrap.
    /// Without the forward the trait default refuses it: the live actor never
    /// adopts the store's metadata-only head commit after a memory compaction,
    /// and the member's next turn is lost.
    #[tokio::test]
    async fn mob_session_wrappers_forward_the_compaction_finalization_acknowledgement() {
        let session_id = meerkat_core::types::SessionId::new();
        let authority = meerkat_core::CommittedSessionBoundaryAuthority::HeadCanonical {
            session_id: session_id.clone(),
            committed_head_token: "probe-head-token".to_string(),
        };
        let probe = Arc::new(ForwardingProbe::default());
        for (shape, wrapper) in decorated_shapes(&probe) {
            probe.calls.lock().expect("probe calls").clear();
            probe
                .acknowledged_compaction_finalizations
                .lock()
                .expect("probe acknowledgements")
                .clear();
            meerkat_core::service::SessionService::acknowledge_runtime_compaction_finalization(
                wrapper.as_ref(),
                &session_id,
                &authority,
            )
            .await
            .unwrap_or_else(|error| panic!("{shape}: the wrapped service acknowledges: {error}"));
            assert_eq!(
                probe.calls(),
                vec!["acknowledge_runtime_compaction_finalization"],
                "{shape}: the inner service acknowledges, exactly once"
            );
            assert_eq!(
                *probe
                    .acknowledged_compaction_finalizations
                    .lock()
                    .expect("probe acknowledgements"),
                vec![(session_id.clone(), authority.clone())],
                "{shape}: the exact authority reaches the inner service"
            );
        }
    }

    /// Both production decorators, alone and stacked as the builder stacks
    /// them, forward the exact-run suspend of a graceful requeue stop to the
    /// service they wrap, with the exact run, and return its answer. Without
    /// the forward the trait default answers `false`: the stop hard-cancels a
    /// durable member's run instead of keeping it for the member's next start.
    #[tokio::test]
    async fn mob_session_wrappers_forward_the_exact_run_suspend_of_a_requeue_stop() {
        let session_id = meerkat_core::types::SessionId::new();
        let run_id = meerkat_core::lifecycle::RunId::new();
        let machine = meerkat_runtime::MeerkatMachine::ephemeral();
        let probe = Arc::new(ForwardingProbe::default());
        for (shape, wrapper) in decorated_shapes(&probe) {
            probe.calls.lock().expect("probe calls").clear();
            probe.suspended_runs.lock().expect("probe suspends").clear();
            let suspended = MobSessionService::suspend_run_with_machine_authority(
                wrapper.as_ref(),
                &session_id,
                &run_id,
                machine.session_control_authority(),
            )
            .await
            .unwrap_or_else(|error| panic!("{shape}: the wrapped service suspends: {error}"));
            assert!(
                suspended,
                "{shape}: the inner service's answer comes back, not the default"
            );
            assert_eq!(
                probe.calls(),
                vec!["suspend_run_with_machine_authority"],
                "{shape}: the inner service suspends, exactly once"
            );
            assert_eq!(
                *probe.suspended_runs.lock().expect("probe suspends"),
                vec![(session_id.clone(), run_id.clone())],
                "{shape}: the exact run reaches the inner service"
            );
        }
    }

    /// Both production decorators, alone and stacked as the builder stacks
    /// them, forward the cold-predecessor settlement to the service they wrap
    /// and return its settlement. Without the forward the trait default
    /// refuses typed (`Unsupported`) and touches nothing, so no settlement of
    /// what an earlier incarnation of a member session left could ever run
    /// through a Meta-hosted mob.
    #[tokio::test]
    async fn mob_session_wrappers_forward_the_cold_predecessor_settlement() {
        let session_id = meerkat_core::types::SessionId::new();
        let claim = meerkat_core::session_hosting::grant_session_hosting(
            &meerkat_core::session_hosting::HostingCapability::ProcessLocal,
            &meerkat_core::session_hosting::HostingOwner::mint(),
            &session_id,
        )
        .expect("a process-local grant");
        let probe = Arc::new(ForwardingProbe::default());
        for (shape, wrapper) in decorated_shapes(&probe) {
            probe.calls.lock().expect("probe calls").clear();
            let settlement =
                MobSessionService::settle_cold_predecessor_with_mob_lifecycle_authority_before(
                    wrapper.as_ref(),
                    &session_id,
                    &claim,
                    meerkat_core::time_compat::Instant::now() + std::time::Duration::from_mins(1),
                )
                .await
                .unwrap_or_else(|error| {
                    panic!("{shape}: the wrapped service settles, not the default: {error:?}")
                });
            assert_eq!(
                settlement.session_id, session_id,
                "{shape}: the inner service's settlement comes back"
            );
            assert_eq!(
                probe.calls(),
                vec!["settle_cold_predecessor_with_mob_lifecycle_authority_before"],
                "{shape}: the inner service settles once"
            );
        }
    }

    /// Both production decorators, alone and stacked as the builder stacks
    /// them, forward the reporting cold-start settle to the service they wrap
    /// and return its report. The mob start calls this method on the
    /// outermost wrapper. Without the forward the trait default settles
    /// through the plain settle and reports `Settled`, so the start refuses a
    /// member whose open run a graceful stop suspended
    /// (`MemberDurableTailUnprovable`) instead of restoring it over that run.
    #[tokio::test]
    async fn mob_session_wrappers_forward_the_reporting_cold_start_settle() {
        let session_id = meerkat_core::types::SessionId::new();
        let probe = Arc::new(ForwardingProbe::default());
        for (shape, wrapper) in decorated_shapes(&probe) {
            probe.calls.lock().expect("probe calls").clear();
            let report = MobSessionService::settle_durable_tail_before_cold_start_reporting(
                wrapper.as_ref(),
                &session_id,
            )
            .await
            .unwrap_or_else(|error| panic!("{shape}: the wrapped service settles: {error}"));
            assert!(
                matches!(
                    report,
                    meerkat_mob::ColdStartTailSettlement::ContinuesOpenRun { .. }
                ),
                "{shape}: the inner service's report comes back, not the default's Settled: \
                 {report:?}"
            );
            assert_eq!(
                probe.calls(),
                vec!["settle_durable_tail_before_cold_start_reporting"],
                "{shape}: the inner service settles once, through the reporting method"
            );
        }
    }

    /// Both production decorators, alone and stacked as the builder stacks
    /// them, forward the committed read that tolerates the session's own
    /// run's provisional tail to the service they wrap. Without the forward
    /// the trait default is the strict `load_persisted_session`, which
    /// refuses mid-run on a HeadCanonical store, so every reader of a
    /// Meta-hosted mob keeps that refusal.
    #[tokio::test]
    async fn mob_session_wrappers_forward_the_tolerant_committed_read() {
        let session_id = meerkat_core::types::SessionId::new();
        let probe = Arc::new(ForwardingProbe::default());
        for (shape, wrapper) in decorated_shapes(&probe) {
            probe.calls.lock().expect("probe calls").clear();
            let read = MobSessionService::load_persisted_session_tolerating_tail(
                wrapper.as_ref(),
                &session_id,
            )
            .await
            .unwrap_or_else(|error| panic!("{shape}: the wrapped service reads: {error}"));
            assert!(
                read.is_some(),
                "{shape}: the inner service's committed snapshot comes back, not the strict \
                 read's answer"
            );
            assert_eq!(
                probe.calls(),
                vec!["load_persisted_session_tolerating_tail"],
                "{shape}: the inner service reads once, never through the strict read"
            );
        }
    }

    /// A Resume create over a session whose open run continues past its
    /// committed boundary: the strict read refuses while the run's
    /// provisional tail stands, so the pre-create heal reads the committed
    /// snapshot through the tolerant read. The legacy shell visibility heals
    /// from that snapshot and the create reaches the wrapped service.
    #[tokio::test]
    async fn a_resume_create_over_a_continued_tail_heals_legacy_shell_visibility_from_the_snapshot()
    {
        let state = meerkat_core::SessionToolVisibilityState {
            active_filter: legacy_shell_allow_filter(false),
            staged_filter: meerkat_core::ToolFilter::All,
            ..Default::default()
        };
        let req = shell_visibility_request(Some(state.clone()), None);
        let session_id = req
            .build
            .as_ref()
            .and_then(|build| build.resume_session.as_ref())
            .map(|session| session.id().clone())
            .expect("the create resumes a session");
        let probe = Arc::new(ForwardingProbe::default());
        probe
            .strict_read_refuses
            .store(true, std::sync::atomic::Ordering::SeqCst);
        probe
            .accept_creates
            .store(true, std::sync::atomic::Ordering::SeqCst);
        *probe.tolerant_read_session.lock().expect("probe snapshot") =
            Some(session_with_visibility_state(session_id.clone(), state));
        let wrapper = PreBuildMobSessionService {
            inner: probe.clone(),
            hook: no_op_pre_build_hook(),
            dispatch_taint: None,
            after_create_hook: None,
            runtime_adapter_override: None,
            archived_terminal_authority: None,
            member_sessions_in_memory: false,
        };

        let created = meerkat_core::service::SessionService::create_session(&wrapper, req)
            .await
            .unwrap_or_else(|error| {
                panic!("the resume create goes through over the continued tail: {error}")
            });

        assert_eq!(created.session_id, session_id);
        assert_eq!(
            *probe.created_tool_filters.lock().expect("probe creates"),
            vec![Some(meerkat_core::ToolFilter::All)],
            "the legacy shell visibility heals from the committed snapshot"
        );
        let calls = probe.calls();
        assert!(
            calls.contains(&"load_persisted_session_tolerating_tail")
                && !calls.contains(&"load_persisted_session"),
            "the heal reads the committed snapshot, never the strict read: {calls:?}"
        );
    }

    /// Both production decorators, alone and stacked as the builder stacks
    /// them, forward whether the wrapped service's member sessions outlive
    /// the process. Without the forward the trait default answers the
    /// wrapper's own `supports_persistent_sessions`, so a durable profile
    /// would spawn on member sessions the wrapped service keeps in memory.
    #[test]
    fn mob_session_wrappers_forward_whether_member_sessions_outlive_the_process() {
        let probe = Arc::new(ForwardingProbe::default());
        for (shape, wrapper) in decorated_shapes(&probe) {
            assert!(
                wrapper.supports_persistent_sessions(),
                "{shape}: the persistent-session contract is forwarded"
            );
            assert!(
                !wrapper.keeps_durable_member_sessions(),
                "{shape}: the inner service's answer comes back, not the default's"
            );
        }
    }

    /// The in-memory composition's layer answers that its member sessions do
    /// not outlive the process, whatever the wrapped persistent service
    /// answers, and keeps the persistent-session contract for everything
    /// else. Any other layer forwards the wrapped service's answer.
    #[test]
    fn the_in_memory_composition_layer_answers_member_sessions_do_not_outlive_the_process() {
        let dir = tempfile::tempdir().expect("temp dir");
        let builder = FactoryAgentBuilder::new(
            AgentFactory::new(dir.path()).builtins(false),
            Config::default(),
        );
        // Core's persistent session service over in-memory stores: it keeps
        // the persistent-session contract and, by default, answers that its
        // member sessions outlive the process.
        let inner: Arc<dyn MobSessionService> =
            Arc::new(meerkat_session::PersistentSessionService::new(
                builder,
                4,
                Arc::new(meerkat_store::MemoryStore::new()),
                Arc::new(meerkat_runtime::InMemoryRuntimeStore::new()),
                Arc::new(meerkat_store::MemoryBlobStore::new()),
            ));
        assert!(inner.keeps_durable_member_sessions());
        let layer = |member_sessions_in_memory| PreBuildMobSessionService {
            inner: Arc::clone(&inner),
            hook: no_op_pre_build_hook(),
            dispatch_taint: None,
            after_create_hook: None,
            runtime_adapter_override: None,
            archived_terminal_authority: None,
            member_sessions_in_memory,
        };
        let in_memory = layer(true);
        assert!(in_memory.supports_persistent_sessions());
        assert!(!in_memory.keeps_durable_member_sessions());
        assert!(layer(false).keeps_durable_member_sessions());
    }

    #[tokio::test]
    async fn mob_session_wrappers_preserve_owner_turn_admission_and_terminal_result() {
        for after_create in [false, true] {
            for outcome in [0, 1, 2] {
                let probe = Arc::new(ForwardingProbe::default());
                probe
                    .turn_outcome
                    .store(outcome, std::sync::atomic::Ordering::Relaxed);
                let wrapper: Arc<dyn MobSessionService> = if after_create {
                    Arc::new(AfterCreateMobSessionService {
                        inner: probe.clone(),
                        after_hook: Arc::new(|_, _| {
                            panic!("starting a turn must not run an after-create hook")
                        }),
                    })
                } else {
                    Arc::new(PreBuildMobSessionService {
                        inner: probe.clone(),
                        hook: Arc::new(|_| panic!("starting a turn must not run a pre-build hook")),
                        dispatch_taint: None,
                        after_create_hook: None,
                        runtime_adapter_override: None,
                        archived_terminal_authority: None,
                        member_sessions_in_memory: false,
                    })
                };
                let session_id = meerkat_core::types::SessionId::new();
                let req = meerkat_core::service::StartTurnRequest {
                    prompt: meerkat_core::ContentInput::Text("unchanged prompt".to_string()),
                    injected_context: Vec::new(),
                    system_prompt: Some("unchanged system prompt".to_string()),
                    event_tx: None,
                    runtime: meerkat_core::service::StartTurnRuntimeSemantics {
                        work_authorization: None,
                        input_identity: None,
                        handling_mode: meerkat_core::types::HandlingMode::Steer,
                        turn_tool_overlay: None,
                        typed_turn_appends: Vec::new(),
                        turn_metadata: None,
                    },
                };
                let (sender, mut admitted) = tokio::sync::oneshot::channel();
                let mut turn = Box::pin(wrapper.start_turn_with_admission_notification(
                    &session_id,
                    req,
                    sender,
                ));
                assert!(futures::poll!(&mut turn).is_pending());
                assert!(matches!(
                    admitted.try_recv(),
                    Err(tokio::sync::oneshot::error::TryRecvError::Empty)
                ));
                let (forwarded_session, forwarded_req) = probe
                    .turn_request
                    .lock()
                    .expect("turn request")
                    .take()
                    .expect("owner received request");
                assert_eq!(forwarded_session, session_id);
                assert_eq!(
                    forwarded_req.prompt,
                    meerkat_core::ContentInput::Text("unchanged prompt".to_string())
                );
                assert_eq!(
                    forwarded_req.system_prompt.as_deref(),
                    Some("unchanged system prompt")
                );
                assert_eq!(
                    forwarded_req.runtime.handling_mode,
                    meerkat_core::types::HandlingMode::Steer,
                    "transparent forwarding must not normalize the owner's request"
                );
                assert_eq!(
                    probe.calls(),
                    vec!["start_turn_with_admission_notification"]
                );

                probe.turn_admission.notify_one();
                if outcome == 1 {
                    assert!(matches!(
                        turn.await,
                        Err(SessionError::NotRunning { id }) if id == session_id
                    ));
                    assert!(matches!(
                        admitted.try_recv(),
                        Err(tokio::sync::oneshot::error::TryRecvError::Closed)
                    ));
                    continue;
                }
                assert!(futures::poll!(&mut turn).is_pending());
                assert_eq!(admitted.try_recv(), Ok(()));
                assert!(
                    futures::poll!(&mut turn).is_pending(),
                    "owner admission is not terminal completion"
                );
                probe.turn_terminal.notify_one();
                let result = turn.await;
                if outcome == 2 {
                    assert!(matches!(
                        result,
                        Err(SessionError::NotFound { id }) if id == session_id
                    ));
                } else {
                    let result = result.expect("owner terminal result");
                    assert_eq!(result.session_id, session_id);
                    assert_eq!(result.text, "owner terminal result");
                    assert_eq!(result.turns, 3);
                    assert_eq!(result.tool_calls, 2);
                }
            }
        }
    }

    #[tokio::test]
    async fn mob_session_wrappers_forward_committed_parent_boundary_exactly_once() {
        let probe = Arc::new(ForwardingProbe::default());
        let session_id = meerkat_core::types::SessionId::new();
        let machine = Arc::new(meerkat_runtime::MeerkatMachine::ephemeral());

        let pre_build = PreBuildMobSessionService {
            inner: probe.clone(),
            hook: no_op_pre_build_hook(),
            dispatch_taint: None,
            after_create_hook: None,
            runtime_adapter_override: Some(Arc::clone(&machine)),
            archived_terminal_authority: None,
            member_sessions_in_memory: false,
        };
        assert_eq!(
            MobSessionService::enqueue_committed_parent_session_boundary_after_runtime_turn(
                &pre_build,
                &session_id,
                machine.as_ref(),
            )
            .await
            .expect("pre-build wrapper must forward the sealed boundary"),
            7,
        );

        let after_create = AfterCreateMobSessionService {
            inner: probe.clone(),
            after_hook: Arc::new(|_, _| Box::pin(async {})),
        };
        assert_eq!(
            MobSessionService::enqueue_committed_parent_session_boundary_after_runtime_turn(
                &after_create,
                &session_id,
                machine.as_ref(),
            )
            .await
            .expect("after-create wrapper must forward the sealed boundary"),
            7,
        );

        assert_eq!(
            probe.calls(),
            vec![
                "enqueue_committed_parent_session_boundary_after_runtime_turn",
                "enqueue_committed_parent_session_boundary_after_runtime_turn",
            ],
            "each wrapper must delegate exactly once and add no semantic step",
        );
    }

    /// Both Meta decorators over `probe`, alone and stacked as the builder
    /// stacks them. Their hooks must not run for a forwarded call.
    fn decorated_shapes(
        probe: &Arc<ForwardingProbe>,
    ) -> [(&'static str, Arc<dyn MobSessionService>); 3] {
        let pre_build = |inner: Arc<dyn MobSessionService>| -> Arc<dyn MobSessionService> {
            Arc::new(PreBuildMobSessionService {
                inner,
                hook: Arc::new(|_| panic!("a forwarded call must not run a pre-build hook")),
                dispatch_taint: None,
                after_create_hook: None,
                runtime_adapter_override: None,
                archived_terminal_authority: None,
                member_sessions_in_memory: false,
            })
        };
        let after_create = |inner: Arc<dyn MobSessionService>| -> Arc<dyn MobSessionService> {
            Arc::new(AfterCreateMobSessionService {
                inner,
                after_hook: Arc::new(|_, _| panic!("a forwarded call must not run a create hook")),
            })
        };
        let inner: Arc<dyn MobSessionService> = probe.clone();
        [
            ("pre-build", pre_build(Arc::clone(&inner))),
            ("after-create", after_create(Arc::clone(&inner))),
            (
                "after-create over pre-build",
                after_create(pre_build(inner)),
            ),
        ]
    }

    /// Both decorators, alone and stacked as the builder stacks them, forward
    /// a respawn's read of the predecessor's spawn-time restrictions to the
    /// inner service instead of answering the trait's refusing default, so a
    /// member respawned through them keeps its restrictions rather than being
    /// refused.
    #[tokio::test]
    async fn mob_session_wrappers_forward_the_predecessor_tool_restrictions() {
        let session_id = meerkat_core::types::SessionId::new();
        let probe = Arc::new(ForwardingProbe::default());
        for (shape, wrapper) in decorated_shapes(&probe) {
            probe.calls.lock().expect("probe calls").clear();
            let restrictions = wrapper
                .predecessor_tool_restrictions(&session_id)
                .await
                .expect("the inner service's restrictions");
            assert_eq!(restrictions, probe_predecessor_restrictions(), "{shape}");
            assert_eq!(
                probe.calls(),
                vec!["predecessor_tool_restrictions"],
                "{shape}: the inner service answers, exactly once"
            );
        }
    }

    /// Both decorators, alone and stacked as the builder stacks them, forward
    /// a resume spawn's durable tail presence check to the inner service.
    /// Without the forward, the trait default refuses the check for a
    /// service that keeps durable sessions (or, for one that does not,
    /// answers that no tail stands without asking), so every resume spawn
    /// over a persistent service behind them is refused.
    #[tokio::test]
    async fn mob_session_wrappers_forward_the_durable_tail_presence_check() {
        let session_id = meerkat_core::types::SessionId::new();
        let probe = Arc::new(ForwardingProbe::default());
        for (shape, wrapper) in decorated_shapes(&probe) {
            probe.calls.lock().expect("probe calls").clear();
            let stands = wrapper
                .provisional_tail_stands(&session_id)
                .await
                .map_err(|error| error.to_string());
            assert_eq!(
                (stands, probe.calls()),
                (Ok(true), vec!["provisional_tail_stands"]),
                "{shape}: the inner service answers, exactly once"
            );
        }
    }

    /// A retired member resumes onto its own session on a persistent mob
    /// whose session service is layered as the gateway layers it (an
    /// after-create hook over the bootstrap's pre-build layer): the path an
    /// operator re-attach and an identity's resume take. The resume spawn
    /// asks the session service whether a provisional durable tail stands;
    /// a layer that kept the trait default refused that check for a service
    /// that keeps durable sessions, so every such resume was refused.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_retired_member_resumes_onto_its_session_through_the_persistent_layers() {
        let dir = tempfile::tempdir().expect("state dir");
        let definition = MobDefinition::from_toml(
            "[mob]\nid = \"persistent-layers-resume\"\n\n[profiles.worker]\nmodel = \"gpt-5.5\"\n\n\
             [profiles.worker.tools]\ncomms = true\n",
        )
        .expect("mob definition");
        let (storage, provenance) =
            crate::mob_composition_manifest::persistent_mob_storage(dir.path().join("mob.sqlite3"))
                .expect("mob storage");
        let spec = MobBootstrapSpec::persistent(
            definition,
            storage,
            provenance,
            dir.path().to_path_buf(),
            4,
        )
        .await
        .expect("persistent spec")
        .with_options(MobBootstrapOptions {
            allow_ephemeral_sessions: true,
            notify_orchestrator_on_resume: true,
            default_llm_client: Some(Arc::new(meerkat_client::TestClient::default())),
        })
        .with_after_create_hook(Arc::new(|_session_id, _context| Box::pin(async {})));
        let runtime = Box::pin(MobRuntime::bootstrap(spec))
            .await
            .expect("bootstrap the mob runtime");
        let handle = runtime.handle();
        let member = meerkat_mob::AgentIdentity::from("worker-1");
        let worker = || SpawnMemberSpec::new(ProfileName::from("worker"), member.clone());
        handle
            .ensure_member(worker())
            .await
            .expect("seat the member");
        let session = handle
            .resolve_bridge_session_id(&member)
            .await
            .expect("the member's session");
        handle
            .retire(member.clone())
            .await
            .expect("retire the member");

        Box::pin(handle.spawn_spec(worker().with_launch_mode(
            meerkat_mob::MemberLaunchMode::Resume {
                bridge_session_id: session.clone(),
                resume_from_role: None,
            },
        )))
        .await
        .expect("the member resumes onto its session");
        assert_eq!(
            handle.resolve_bridge_session_id(&member).await,
            Some(session),
            "the member is back on its own session"
        );
        handle.shutdown().await.expect("shutdown");
    }

    /// Both decorators, alone and stacked as the builder stacks them, forward
    /// a fork's read of the source's visible tools to the inner service.
    /// Without the forward, the trait default answers that the source was not
    /// live, so a fork child of a member behind them is never told the
    /// source's tools.
    #[tokio::test]
    async fn mob_session_wrappers_forward_the_fork_source_tools() {
        let session_id = meerkat_core::types::SessionId::new();
        let probe = Arc::new(ForwardingProbe::default());
        for (shape, wrapper) in decorated_shapes(&probe) {
            probe.calls.lock().expect("probe calls").clear();
            let tools = wrapper.fork_source_tools(&session_id).await;
            assert_eq!(
                (tools, probe.calls()),
                (probe_fork_source_tools(), vec!["fork_source_tools"]),
                "{shape}: the inner service reads the source's tools, exactly once"
            );
        }
    }

    /// Both decorators, alone and stacked as the builder stacks them, forward
    /// a held member's controller route query for the exact actor to the
    /// inner service instead of answering the trait's `Unsupported` default:
    /// a report, each typed observation and a lost actor's error arrive
    /// unchanged, from exactly one inner call.
    #[tokio::test]
    async fn mob_session_wrappers_forward_the_controller_route_observation() {
        let dir = tempfile::tempdir().expect("fixture directory");
        let mut builder =
            FactoryAgentBuilder::new(AgentFactory::new(dir.path()), Config::default());
        builder.default_llm_client = Some(Arc::new(meerkat_client::TestClient::default()));
        let service = meerkat_session::EphemeralSessionService::new(builder, 4);
        let session_id = meerkat_core::service::SessionService::create_session(
            &service,
            shell_visibility_request(None, None),
        )
        .await
        .expect("a deferred session")
        .session_id;
        let actor = service
            .live_session_actor_witness(&session_id)
            .await
            .expect("its live actor");
        let report = meerkat::controller_route::ControllerRouteReport {
            key: meerkat_core::ControllerRouteKey::parse(&format!(
                "{}{}",
                meerkat_core::ControllerRouteKey::PREFIX,
                "cd".repeat(32)
            ))
            .expect("route key"),
            provider: "openai".into(),
            model: "gpt-5.5".into(),
            wire_model: "gpt-5.5".into(),
            endpoint: "https://wrapper-route.invalid/v1/responses".into(),
            credential_owner: "account wrapper/route".into(),
        };
        let answers = [
            Some(meerkat_mob::ControllerRouteObservation::Reported(report)),
            Some(meerkat_mob::ControllerRouteObservation::Unavailable),
            Some(meerkat_mob::ControllerRouteObservation::Unsupported),
            None,
        ];
        for answer in &answers {
            let probe = Arc::new(ForwardingProbe::default());
            for (shape, wrapper) in decorated_shapes(&probe) {
                probe
                    .controller_route_answer
                    .lock()
                    .expect("controller route answer")
                    .clone_from(answer);
                probe.calls.lock().expect("probe calls").clear();
                let observed = wrapper
                    .controller_route_under_runtime_turn_boundary(&actor)
                    .await;
                match answer {
                    Some(expected) => assert_eq!(
                        &observed.expect("the inner service's observation"),
                        expected,
                        "{shape}"
                    ),
                    None => assert!(
                        matches!(&observed, Err(SessionError::NotFound { id }) if id == &session_id),
                        "{shape}: {observed:?}"
                    ),
                }
                assert_eq!(
                    probe.calls(),
                    vec!["controller_route_under_runtime_turn_boundary"],
                    "{shape}: the inner service answers, exactly once"
                );
            }
        }
    }

    /// Both decorators, alone and stacked as the builder stacks them, forward
    /// the controller pin under the runtime turn boundary for the exact actor
    /// to the inner service instead of refusing with the trait's
    /// `Unsupported` default: the inner pin's own client (the same retained
    /// child, with its selection) and a lost actor's error arrive unchanged,
    /// from exactly one inner call. Every shape is observed before the
    /// comparison, so a refusal names each wrapper that answered it.
    #[tokio::test]
    async fn mob_session_wrappers_forward_the_controller_pin_under_the_turn_boundary() {
        use crate::memory::dispatch_taint::controller_test_support::{PinnedInner, selection};

        let dir = tempfile::tempdir().expect("fixture directory");
        let mut builder =
            FactoryAgentBuilder::new(AgentFactory::new(dir.path()), Config::default());
        builder.default_llm_client = Some(Arc::new(meerkat_client::TestClient::default()));
        let service = meerkat_session::EphemeralSessionService::new(builder, 4);
        let session_id = meerkat_core::service::SessionService::create_session(
            &service,
            shell_visibility_request(None, None),
        )
        .await
        .expect("a deferred session")
        .session_id;
        let actor = service
            .live_session_actor_witness(&session_id)
            .await
            .expect("its live actor");
        let child: Arc<dyn meerkat_core::AgentLlmClient> = Arc::new(PinnedInner::default());
        let probe = Arc::new(ForwardingProbe::default());
        *probe
            .controller_pin_answer
            .lock()
            .expect("controller pin answer") = Some(meerkat_core::ControllerModelClient::new(
            selection(),
            Arc::clone(&child),
        ));
        let mut pins = Vec::new();
        for (shape, wrapper) in decorated_shapes(&probe) {
            probe.calls.lock().expect("probe calls").clear();
            let pinned = wrapper
                .pin_controller_client_under_runtime_turn_boundary(&actor)
                .await
                .map(|client| {
                    Arc::ptr_eq(client.client(), &child) && client.selection() == &selection()
                })
                .map_err(|error| error.to_string());
            pins.push((shape, pinned, probe.calls()));
        }
        assert_eq!(
            pins,
            ["pre-build", "after-create", "after-create over pre-build"].map(|shape| (
                shape,
                Ok::<_, String>(true),
                vec!["pin_controller_client_under_runtime_turn_boundary"]
            )),
            "each shape answers the inner pin's own client, from exactly one inner call"
        );

        *probe
            .controller_pin_answer
            .lock()
            .expect("controller pin answer") = None;
        for (shape, wrapper) in decorated_shapes(&probe) {
            let lost = wrapper
                .pin_controller_client_under_runtime_turn_boundary(&actor)
                .await;
            assert!(
                matches!(&lost, Err(SessionError::NotFound { id }) if id == &session_id),
                "{shape}: {lost:?}"
            );
        }
    }

    /// The controller selection the pin rows' member reports, for the
    /// session's own model.
    fn pin_test_selection() -> meerkat_core::ControllerModelSelection {
        meerkat_core::ControllerModelSelection::new(
            meerkat_core::SessionLlmIdentity {
                model: "gpt-5.5".into(),
                provider: meerkat_core::Provider::OpenAI,
                self_hosted_server_id: None,
                provider_params: None,
                auth_binding: None,
            },
            serde_json::from_value(
                serde_json::json!({"realm": "meta-pin", "account": "pin-controller"}),
            )
            .expect("nonsecret identity of the fixture transport"),
            "pin-profile".into(),
            "pin-transport".into(),
        )
    }

    /// The raw transport the pin rows' member runs on: it reports
    /// [`pin_test_selection`] and serves no request (the rows run no turn).
    struct PinnableRawClient;

    #[async_trait::async_trait]
    impl meerkat_llm_core::LlmClient for PinnableRawClient {
        fn controller_model_selection(&self) -> Option<meerkat_core::ControllerModelSelection> {
            Some(pin_test_selection())
        }

        fn stream<'a>(
            &'a self,
            _request: &'a meerkat_llm_core::LlmRequest,
        ) -> meerkat_llm_core::LlmStream<'a> {
            Box::pin(futures::stream::empty())
        }

        fn provider(&self) -> meerkat_core::Provider {
            meerkat_core::Provider::OpenAI
        }

        async fn health_check(&self) -> Result<(), meerkat_llm_core::LlmError> {
            Ok(())
        }
    }

    /// A member agent client that pins itself as the controller of
    /// [`pin_test_selection`], as a governed member's selection owner does.
    struct PinnableAgentClient {
        inner: Arc<dyn meerkat_core::AgentLlmClient>,
    }

    #[async_trait::async_trait]
    impl meerkat_core::AgentLlmClient for PinnableAgentClient {
        fn controller_model_selection(&self) -> Option<meerkat_core::ControllerModelSelection> {
            Some(pin_test_selection())
        }

        fn controller_model_facts(
            &self,
        ) -> Result<meerkat_core::ControllerModelFacts, meerkat_core::ControllerFactsUnavailable>
        {
            Err(meerkat_core::ControllerFactsUnavailable)
        }

        fn pin_controller(self: Arc<Self>) -> Option<meerkat_core::ControllerModelClient> {
            Some(meerkat_core::ControllerModelClient::new(
                pin_test_selection(),
                self,
            ))
        }

        fn request_attempt_authority(&self) -> meerkat_core::RequestAttemptAuthority {
            self.inner.request_attempt_authority()
        }

        async fn stream_response(
            &self,
            messages: &[meerkat_core::Message],
            tools: &[Arc<meerkat_core::ToolDef>],
            max_tokens: u32,
            temperature: Option<f32>,
            provider_params: Option<
                &meerkat_core::lifecycle::run_primitive::ProviderParamsOverride,
            >,
        ) -> Result<meerkat_core::agent::LlmStreamResult, meerkat_core::AgentError> {
            self.inner
                .stream_response(messages, tools, max_tokens, temperature, provider_params)
                .await
        }

        fn provider(&self) -> meerkat_core::Provider {
            self.inner.provider()
        }

        fn model(&self) -> &str {
            self.inner.model()
        }
    }

    /// A deferred session whose member agent pins its controller client.
    fn pinnable_session_request() -> CreateSessionRequest {
        let mut request = shell_visibility_request(None, None);
        request
            .build
            .get_or_insert_with(Default::default)
            .agent_llm_client_decorator =
            Some(Arc::new(|inner| Arc::new(PinnableAgentClient { inner })));
        request
    }

    /// One session owner's non-waiting controller pin of `actor`
    /// ([`ControllerPinSource::try_pin_actor`](crate::identity_first::bridge::ControllerPinSource::try_pin_actor)).
    /// While `held` holds the session's turn boundary it refuses typed
    /// `TurnBoundaryHeld` at once: the bound below only turns a wait into a
    /// failure, because the boundary is never released while the pin runs.
    /// Once the boundary is free it pins the actor's own controller client,
    /// the one the waiting pin returns.
    async fn assert_the_owner_pins_without_waiting(
        owner: &dyn crate::identity_first::bridge::ControllerPinSource,
        actor: &meerkat_session::LiveSessionActorWitness,
        held: tokio::sync::OwnedMutexGuard<()>,
    ) {
        let refused = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            owner.try_pin_actor(actor),
        )
        .await
        .expect("the non-waiting pin answers while the turn boundary is held");
        assert!(
            matches!(
                refused,
                Err(meerkat_session::TryPinControllerError::TurnBoundaryHeld)
            ),
            "a held turn boundary refuses typed at once: {:?}",
            refused.map(|_| ())
        );
        drop(held);
        let pinned = owner
            .try_pin_actor(actor)
            .await
            .map_err(|error| error.to_string())
            .expect("a free actor pins");
        let waited = owner
            .pin_actor(actor)
            .await
            .expect("the waiting pin of the same actor");
        assert!(
            pinned.selection() == waited.selection() && pinned.selection() == &pin_test_selection(),
            "the non-waiting pin is the actor's own controller client"
        );
    }

    /// One session owner's non-waiting pin of an actor that is gone: refused
    /// typed as not found, never with a client.
    async fn assert_a_gone_actor_refuses_the_pin(
        owner: &dyn crate::identity_first::bridge::ControllerPinSource,
        actor: &meerkat_session::LiveSessionActorWitness,
        session_id: &meerkat_core::types::SessionId,
    ) {
        let refused = owner.try_pin_actor(actor).await;
        assert!(
            matches!(
                &refused,
                Err(meerkat_session::TryPinControllerError::Session(
                    SessionError::NotFound { id }
                )) if id == session_id
            ),
            "a gone actor refuses typed: {:?}",
            refused.map(|_| ())
        );
    }

    /// The ephemeral session owner's non-waiting controller pin: a held turn
    /// boundary refuses typed without waiting, a free actor pins, and a gone
    /// actor refuses typed.
    #[tokio::test]
    async fn the_ephemeral_owner_pins_a_free_actor_and_refuses_a_held_boundary_without_waiting() {
        use crate::identity_first::bridge::ControllerPinSource;

        let dir = tempfile::tempdir().expect("fixture directory");
        let mut builder =
            FactoryAgentBuilder::new(AgentFactory::new(dir.path()), Config::default());
        builder.default_llm_client = Some(Arc::new(PinnableRawClient));
        let service = meerkat_session::EphemeralSessionService::new(builder, 4);
        let session_id = meerkat_core::service::SessionService::create_session(
            &service,
            pinnable_session_request(),
        )
        .await
        .expect("a deferred session")
        .session_id;
        let actor = ControllerPinSource::live_actor(&service, &session_id)
            .await
            .expect("its live actor");
        let held = service
            .acquire_runtime_turn_finalization_guard(&session_id)
            .await;
        assert_the_owner_pins_without_waiting(&service, &actor, held).await;
        meerkat_core::service::SessionService::archive(&service, &session_id)
            .await
            .expect("archive the session");
        assert_a_gone_actor_refuses_the_pin(&service, &actor, &session_id).await;
    }

    /// The persistent session owner's non-waiting controller pin: a held
    /// turn boundary refuses typed without waiting, a free actor pins, and a
    /// gone actor refuses typed.
    #[tokio::test]
    async fn the_persistent_owner_pins_a_free_actor_and_refuses_a_held_boundary_without_waiting() {
        use crate::identity_first::bridge::ControllerPinSource;

        let dir = tempfile::tempdir().expect("fixture directory");
        let store: Arc<dyn meerkat_runtime::RuntimeStore> =
            Arc::new(meerkat_runtime::InMemoryRuntimeStore::new());
        let blobs: Arc<dyn meerkat_core::BlobStore> = Arc::new(Base64BlobStoreAdapter::new(
            Arc::new(ObjectStoreBlobStore::memory()),
        ));
        let mut builder =
            FactoryAgentBuilder::new(AgentFactory::new(dir.path()), Config::default());
        builder.default_llm_client = Some(Arc::new(PinnableRawClient));
        let service = meerkat_session::PersistentSessionService::new(
            builder,
            4,
            Arc::new(meerkat_store::MemoryStore::new()),
            store,
            blobs,
        );
        let session_id = meerkat_core::service::SessionService::create_session(
            &service,
            pinnable_session_request(),
        )
        .await
        .expect("a deferred session")
        .session_id;
        let actor = ControllerPinSource::live_actor(&service, &session_id)
            .await
            .expect("its live actor");
        let held = service
            .acquire_runtime_turn_finalization_guard(&session_id)
            .await;
        assert_the_owner_pins_without_waiting(&service, &actor, held).await;
        let lease = service
            .acquire_live_session_actor_turn_boundary_lease_exact(&actor)
            .await
            .expect("the actor's turn boundary")
            .expect("the actor is current");
        assert!(
            service
                .discard_live_session_actor(&lease)
                .await
                .expect("discard the actor"),
            "the exact actor is discarded"
        );
        drop(lease);
        assert_a_gone_actor_refuses_the_pin(&service, &actor, &session_id).await;
    }

    /// An owner that implements only the waiting pin keeps the trait's
    /// default non-waiting pin, and that default fails closed: a typed
    /// unsupported refusal, never a client.
    #[tokio::test]
    async fn an_owner_without_a_non_waiting_pin_refuses_it_typed_as_unsupported() {
        use crate::identity_first::bridge::ControllerPinSource;

        struct WaitingPinOnly;

        #[async_trait::async_trait]
        impl ControllerPinSource for WaitingPinOnly {
            async fn live_actor(
                &self,
                _session_id: &meerkat_core::types::SessionId,
            ) -> Option<meerkat_session::LiveSessionActorWitness> {
                None
            }

            async fn pin_actor(
                &self,
                _actor: &meerkat_session::LiveSessionActorWitness,
            ) -> Result<meerkat_core::ControllerModelClient, String> {
                Err("this row never takes the waiting pin".to_string())
            }
        }

        let dir = tempfile::tempdir().expect("fixture directory");
        let mut builder =
            FactoryAgentBuilder::new(AgentFactory::new(dir.path()), Config::default());
        builder.default_llm_client = Some(Arc::new(meerkat_client::TestClient::default()));
        let service = meerkat_session::EphemeralSessionService::new(builder, 4);
        let session_id = meerkat_core::service::SessionService::create_session(
            &service,
            shell_visibility_request(None, None),
        )
        .await
        .expect("a deferred session")
        .session_id;
        let actor = service
            .live_session_actor_witness(&session_id)
            .await
            .expect("a live actor to name");
        let refused = WaitingPinOnly.try_pin_actor(&actor).await;
        assert!(
            matches!(
                &refused,
                Err(meerkat_session::TryPinControllerError::Session(
                    SessionError::Unsupported(_)
                ))
            ),
            "the default refuses typed as unsupported: {:?}",
            refused.map(|_| ())
        );
    }

    /// Both decorators, alone and stacked, forward the removed-actor exit
    /// wait and the durable MCP attachment intent change to the inner
    /// service instead of answering their trait defaults (an immediate
    /// return and a typed `Unsupported`).
    #[tokio::test]
    async fn mob_session_wrappers_forward_removed_actor_exit_and_mcp_attachment_intent() {
        let session_id = meerkat_core::types::SessionId::new();
        let probe = Arc::new(ForwardingProbe::default());
        for (shape, wrapper) in decorated_shapes(&probe) {
            probe.calls.lock().expect("probe calls").clear();
            wrapper.await_removed_actor_exit(&session_id).await;
            let recorded = wrapper
                .update_session_mcp_attachment_intent(
                    &session_id,
                    meerkat_core::mcp_session_intent::McpAttachmentIntentMutation::Withdraw {
                        server_name: "docs".to_string(),
                    },
                )
                .await
                .expect("the inner service's record");
            assert_eq!(
                recorded,
                meerkat_core::mcp_session_intent::McpAttachmentIntentRecorded {
                    server_name: "docs".to_string(),
                    revision: Some(FORWARDED_DURABLE_REVISION),
                    changed: true,
                },
                "{shape}"
            );
            assert_eq!(
                probe.calls(),
                vec![
                    "await_removed_actor_exit",
                    "update_session_mcp_attachment_intent"
                ],
                "{shape}: each reaches the inner service exactly once"
            );
        }
    }

    #[cfg(feature = "openai-live")]
    #[tokio::test]
    async fn console_voice_mob_session_wrappers_forward_transcript_commit_and_bridge_preflight() {
        let probe = Arc::new(ForwardingProbe::default());
        let machine = Arc::new(meerkat_runtime::MeerkatMachine::ephemeral());
        *probe.live_machine.lock().expect("live machine") = Some(machine.clone());
        let pre_build: Arc<dyn MobSessionService> = Arc::new(PreBuildMobSessionService {
            inner: probe.clone(),
            hook: Arc::new(|_| panic!("live commit must not invoke a build hook")),
            dispatch_taint: None,
            after_create_hook: None,
            runtime_adapter_override: None,
            archived_terminal_authority: None,
            member_sessions_in_memory: false,
        });
        let after_create: Arc<dyn MobSessionService> = Arc::new(AfterCreateMobSessionService {
            inner: probe.clone(),
            after_hook: Arc::new(|_, _| panic!("live commit must not invoke a create hook")),
        });
        let session_id = meerkat_core::SessionId::new();
        let provisional = meerkat_core::ProvisionalLiveHandoff::new(
            meerkat_core::LiveUserTurnCorrelation::new(
                meerkat_core::LiveChannelId::new("voice-forwarding"),
                meerkat_core::InteractionId::new(),
                meerkat_core::OpaqueProviderCorrelation::new("delegation", "user-turn")
                    .expect("provider correlation"),
            )
            .expect("turn correlation"),
            "unchanged spoken request",
            meerkat_core::LiveHandoffInputProvenance::NormalizedHandoff,
        )
        .expect("provisional input");
        let final_event = meerkat_core::RealtimeTranscriptEvent::UserTranscriptFinal {
            item_id: "user-turn".to_string(),
            previous_item_id: Some("previous-turn".to_string()),
            content_index: 2,
            text: "unchanged final transcript".to_string(),
        };
        for wrapper in [pre_build, after_create] {
            assert!(
                matches!(
                    wrapper                    .commit_live_delegation_final_transcript(
                        &machine,
                        &session_id, provisional.clone(), final_event.clone(),
                    ).await,
                    Err(SessionError::NotFound { id }) if id == session_id
                ),
                "the inner owner's result must replace the trait's Unsupported default"
            );
            assert_eq!(
                probe.live_commit.lock().expect("live commit").take(),
                Some((session_id.clone(), provisional.clone(), final_event.clone())),
            );
            wrapper
                .validate_live_bridge_member_eligibility(&session_id)
                .await
                .expect("inner eligibility");
            assert!(matches!(
                wrapper.capture_live_bridge_execution_snapshot(&session_id, "agent-a").await,
                Err(SessionError::NotFound { id }) if id == session_id
            ));
        }
        assert_eq!(
            probe.calls(),
            [
                "commit_live_delegation_final_transcript",
                "validate_live_bridge_member_eligibility",
                "capture_live_bridge_execution_snapshot",
                "commit_live_delegation_final_transcript",
                "validate_live_bridge_member_eligibility",
                "capture_live_bridge_execution_snapshot",
            ]
        );
    }

    /// meerkat 0.8.45 made `observe_member_status_view` required so no
    /// wrapper can answer it with a `read` that waits on the member's running
    /// turn. Both production decorators must forward it exactly once and
    /// return the inner service's view unchanged.
    /// meerkat 0.8.47 made `observe_live_durable_source` required: voice
    /// readiness asks it on every poll and it must stay body-free. Both
    /// production decorators forward it exactly once and return the inner
    /// observation unchanged.
    #[tokio::test]
    async fn wrappers_forward_the_live_durable_source_observation_exactly() {
        let probe = Arc::new(ForwardingProbe::default());
        let inner: Arc<dyn MobSessionService> = probe.clone();
        let pre_build: Arc<dyn MobSessionService> = Arc::new(PreBuildMobSessionService {
            inner: Arc::clone(&inner),
            hook: no_op_pre_build_hook(),
            dispatch_taint: None,
            after_create_hook: None,
            runtime_adapter_override: None,
            archived_terminal_authority: None,
            member_sessions_in_memory: false,
        });
        let after_create: Arc<dyn MobSessionService> = Arc::new(AfterCreateMobSessionService {
            inner: Arc::clone(&inner),
            after_hook: Arc::new(|_, _| Box::pin(async {})),
        });
        let session_id = meerkat_core::SessionId::new();
        for wrapper in [pre_build, after_create] {
            assert_eq!(
                wrapper
                    .observe_live_durable_source(&session_id)
                    .await
                    .expect("the inner observation is forwarded"),
                meerkat_mob::LiveDurableSourceObservation::Committed {
                    revision: Some(FORWARDED_DURABLE_REVISION),
                }
            );
        }
        assert_eq!(
            probe.calls(),
            vec!["observe_live_durable_source", "observe_live_durable_source"],
            "each wrapper forwards exactly once and adds no read of its own"
        );
    }

    /// What `ForwardingProbe` answers for a catalog view: a shape no default
    /// or wrapper-level answer produces.
    fn forwarded_catalog_view() -> meerkat_core::ToolCatalogView {
        meerkat_core::ToolCatalogView {
            mode: meerkat_core::ToolCatalogMode::Deferred,
            exact_catalog: true,
            pending_sources: vec!["forwarded-source".to_string()],
            staged_changes_pending: true,
            entries: Vec::new(),
        }
    }

    #[tokio::test]
    async fn wrappers_forward_the_tool_catalog_view_exactly() {
        let probe = Arc::new(ForwardingProbe::default());
        let inner: Arc<dyn MobSessionService> = probe.clone();
        let pre_build: Arc<dyn MobSessionService> = Arc::new(PreBuildMobSessionService {
            inner: Arc::clone(&inner),
            hook: no_op_pre_build_hook(),
            dispatch_taint: None,
            after_create_hook: None,
            runtime_adapter_override: None,
            archived_terminal_authority: None,
            member_sessions_in_memory: false,
        });
        let after_create: Arc<dyn MobSessionService> = Arc::new(AfterCreateMobSessionService {
            inner: Arc::clone(&inner),
            after_hook: Arc::new(|_, _| Box::pin(async {})),
        });
        let session_id = meerkat_core::SessionId::new();
        for wrapper in [pre_build, after_create] {
            assert_eq!(
                wrapper
                    .tool_catalog_view(&session_id)
                    .await
                    .expect("the inner view is forwarded"),
                Some(forwarded_catalog_view())
            );
        }
        assert_eq!(
            probe.calls(),
            vec!["tool_catalog_view", "tool_catalog_view"],
            "each wrapper forwards exactly once and adds no read of its own"
        );
    }

    #[tokio::test]
    async fn wrappers_preserve_retained_metadata_without_ordinary_or_transcript_reads() {
        for after_create in [false, true] {
            for outcome in ["active", "archived", "absent", "unsupported", "fault"] {
                let probe = Arc::new(ForwardingProbe::default());
                let session_id = meerkat_core::SessionId::new();
                let expected = meerkat_core::PersistedSessionMetadataView {
                    session_id: session_id.clone(),
                    session_metadata: Some(meerkat_core::SessionMetadata {
                        schema_version: meerkat_core::SESSION_METADATA_SCHEMA_VERSION,
                        model: "retained-model".to_string(),
                        model_fallback: None,
                        max_tokens: 1024,
                        structured_output_retries:
                            meerkat_core::config::default_structured_output_retries(),
                        provider: meerkat_core::Provider::Anthropic,
                        self_hosted_server_id: None,
                        provider_params: None,
                        tooling: meerkat_core::SessionTooling::default(),
                        keep_alive: false,
                        comms_name: Some("retained-peer".to_string()),
                        peer_meta: None,
                        realm_id: Some(
                            meerkat_core::RealmId::parse("retained-realm").expect("realm"),
                        ),
                        instance_id: None,
                        backend: None,
                        config_generation: None,
                        auth_binding: None,
                        mob_member_binding: Some(meerkat_core::MobMemberBinding {
                            mob_id: "retained-mob".to_string(),
                            role: "worker".to_string(),
                            member: "source-member".to_string(),
                        }),
                    }),
                    lifecycle_terminal: (outcome == "archived")
                        .then_some(meerkat_core::SessionLifecycleTerminal::Archived),
                };
                let reply = match outcome {
                    "active" | "archived" => Ok(Some(expected.clone())),
                    "absent" => Ok(None),
                    "unsupported" => Err(SessionError::Unsupported(
                        "fixture retained authority unavailable".to_string(),
                    )),
                    "fault" => Err(SessionError::Store(Box::new(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "fixture retained metadata corrupt",
                    )))),
                    _ => unreachable!(),
                };
                *probe.retained_metadata_reply.lock().expect("reply") = Some(reply);
                let wrapper: Arc<dyn MobSessionService> = if after_create {
                    Arc::new(AfterCreateMobSessionService {
                        inner: probe.clone(),
                        after_hook: Arc::new(|_, _| {
                            panic!("retained reads must not run an after-create hook")
                        }),
                    })
                } else {
                    Arc::new(PreBuildMobSessionService {
                        inner: probe.clone(),
                        hook: Arc::new(|_| panic!("retained reads must not run a pre-build hook")),
                        dispatch_taint: None,
                        after_create_hook: None,
                        runtime_adapter_override: None,
                        archived_terminal_authority: None,
                        member_sessions_in_memory: false,
                    })
                };
                let actual = wrapper.load_retained_session_metadata(&session_id).await;
                match outcome {
                    "active" | "archived" => {
                        let actual = actual.expect("retained read").expect("exact session");
                        assert_eq!(actual.session_id, expected.session_id);
                        assert_eq!(actual.lifecycle_terminal, expected.lifecycle_terminal);
                        assert_eq!(
                            serde_json::to_value(actual.session_metadata).expect("actual metadata"),
                            serde_json::to_value(expected.session_metadata)
                                .expect("expected metadata")
                        );
                    }
                    "absent" => assert!(actual.expect("absent read").is_none()),
                    "unsupported" => assert!(matches!(
                        actual,
                        Err(SessionError::Unsupported(message))
                            if message == "fixture retained authority unavailable"
                    )),
                    "fault" => match actual {
                        Err(SessionError::Store(error)) => {
                            let error = error
                                .downcast_ref::<std::io::Error>()
                                .expect("original typed read fault");
                            assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
                            assert_eq!(error.to_string(), "fixture retained metadata corrupt");
                        }
                        other => panic!("retained read fault changed: {other:?}"),
                    },
                    _ => unreachable!(),
                }
                assert_eq!(
                    *probe.retained_metadata_requests.lock().expect("requests"),
                    vec![session_id],
                    "the exact session id reaches the retained authority once"
                );
                assert_eq!(
                    probe.calls(),
                    vec!["load_retained_session_metadata"],
                    "retained reads never use ordinary metadata, live reads or transcripts"
                );
            }
        }
    }

    /// meerkat 0.8.50 made `subscribe_session_activity` required: a mob Stop
    /// or Shutdown awaits it to learn that an interrupted member's turn has
    /// ended. Both production decorators must forward it exactly once.
    #[tokio::test]
    async fn wrappers_forward_the_session_activity_subscription_exactly() {
        let probe = Arc::new(ForwardingProbe::default());
        let inner: Arc<dyn MobSessionService> = probe.clone();
        let pre_build: Arc<dyn MobSessionService> = Arc::new(PreBuildMobSessionService {
            inner: Arc::clone(&inner),
            hook: no_op_pre_build_hook(),
            dispatch_taint: None,
            after_create_hook: None,
            runtime_adapter_override: None,
            archived_terminal_authority: None,
            member_sessions_in_memory: false,
        });
        let after_create: Arc<dyn MobSessionService> = Arc::new(AfterCreateMobSessionService {
            inner: Arc::clone(&inner),
            after_hook: Arc::new(|_, _| Box::pin(async {})),
        });
        let session_id = meerkat_core::SessionId::new();
        for wrapper in [pre_build, after_create] {
            let activity = wrapper
                .subscribe_session_activity(&session_id)
                .await
                .expect("the inner subscription is forwarded");
            assert!(!activity.is_active());
        }
        assert_eq!(
            probe.calls(),
            vec!["subscribe_session_activity", "subscribe_session_activity"],
            "each wrapper forwards exactly once"
        );
    }

    #[tokio::test]
    async fn extension_wrappers_forward_retained_metadata_errors_and_exact_session() {
        let probe = Arc::new(ForwardingProbe::default());
        let inner: Arc<dyn MobSessionService> = probe.clone();
        let pre_build: Arc<dyn MobSessionService> = Arc::new(PreBuildMobSessionService {
            inner: Arc::clone(&inner),
            hook: no_op_pre_build_hook(),
            dispatch_taint: None,
            after_create_hook: None,
            runtime_adapter_override: None,
            archived_terminal_authority: None,
            member_sessions_in_memory: false,
        });
        let after_create: Arc<dyn MobSessionService> = Arc::new(AfterCreateMobSessionService {
            inner: Arc::clone(&inner),
            after_hook: Arc::new(|_, _| Box::pin(async {})),
        });
        let session_id = meerkat_core::SessionId::new();
        for wrapper in [pre_build, after_create] {
            *probe
                .retained_metadata_reply
                .lock()
                .expect("retained reply") = Some(Err(SessionError::NotFound {
                id: session_id.clone(),
            }));
            let result = wrapper.load_retained_session_metadata(&session_id).await;
            assert!(matches!(result, Err(SessionError::NotFound { id }) if id == session_id));
        }
        assert_eq!(
            *probe.retained_metadata_requests.lock().expect("requests"),
            vec![session_id.clone(), session_id],
            "both wrappers preserve the exact requested session"
        );
        assert_eq!(
            probe.calls(),
            vec![
                "load_retained_session_metadata",
                "load_retained_session_metadata"
            ],
            "each wrapper forwards exactly once"
        );
    }

    #[tokio::test]
    async fn wrappers_forward_the_member_status_view_exactly() {
        let probe = Arc::new(ForwardingProbe::default());
        let inner: Arc<dyn MobSessionService> = probe.clone();
        let pre_build: Arc<dyn MobSessionService> = Arc::new(PreBuildMobSessionService {
            inner: Arc::clone(&inner),
            hook: no_op_pre_build_hook(),
            dispatch_taint: None,
            after_create_hook: None,
            runtime_adapter_override: None,
            archived_terminal_authority: None,
            member_sessions_in_memory: false,
        });
        let after_create: Arc<dyn MobSessionService> = Arc::new(AfterCreateMobSessionService {
            inner: Arc::clone(&inner),
            after_hook: Arc::new(|_, _| Box::pin(async {})),
        });
        let session_id = meerkat_core::SessionId::new();
        for wrapper in [pre_build, after_create] {
            let view = wrapper
                .observe_member_status_view(&session_id)
                .await
                .expect("the inner view is forwarded");
            assert_eq!(
                view.last_assistant_text.as_deref(),
                Some(FORWARDED_STATUS_PREVIEW)
            );
            assert_eq!(view.total_tokens, FORWARDED_STATUS_TOKENS);
            assert!(matches!(
                view.source,
                meerkat_mob::MemberStatusViewSource::LiveWatch
            ));
        }
        assert_eq!(
            probe.calls(),
            vec!["observe_member_status_view", "observe_member_status_view"],
            "each wrapper forwards exactly once and adds no read of its own"
        );
    }

    /// Every defaulted `MobSessionService` method reachable on main must reach
    /// the inner service through BOTH production decorators. The defaults are
    /// refusals or downgrades (read cost "Unsupported", fork source `None`),
    /// so a missing forward is a silent behaviour change, not a compile error.
    #[tokio::test]
    async fn wrappers_forward_the_remaining_defaulted_methods_to_the_inner_service() {
        let probe = Arc::new(ForwardingProbe::default());
        let inner: Arc<dyn MobSessionService> = probe.clone();
        let pre_build: Arc<dyn MobSessionService> = Arc::new(PreBuildMobSessionService {
            inner: Arc::clone(&inner),
            hook: no_op_pre_build_hook(),
            dispatch_taint: None,
            after_create_hook: None,
            runtime_adapter_override: None,
            archived_terminal_authority: None,
            member_sessions_in_memory: false,
        });
        let after_create: Arc<dyn MobSessionService> = Arc::new(AfterCreateMobSessionService {
            inner: probe.clone(),
            after_hook: Arc::new(|_, _| Box::pin(async {})),
        });
        let session_id = meerkat_core::types::SessionId::new();
        let run_id = meerkat_core::RunId::new();
        let request_only = meerkat_core::TurnBoundaryDelivery::RequestOnly(vec![
            meerkat_core::lifecycle::run_primitive::TurnRequestContext::new(" first context\n")
                .expect("first request context"),
            meerkat_core::lifecycle::run_primitive::TurnRequestContext::new("second context")
                .expect("second request context"),
        ]);
        let durable = meerkat_core::TurnBoundaryDelivery::DurableAppends(
            meerkat_core::DurableTurnBoundaryAppends::try_new(
                meerkat_core::InputId::new(),
                vec![
                    meerkat_core::lifecycle::run_primitive::ConversationAppend {
                        runtime_source: None,
                        role: meerkat_core::lifecycle::run_primitive::ConversationAppendRole::User,
                        content: meerkat_core::lifecycle::run_primitive::CoreRenderable::text(
                            " durable user instruction\n",
                        ),
                        identity: None,
                    },
                    meerkat_core::lifecycle::run_primitive::ConversationAppend {
                        runtime_source: None,
                        role: meerkat_core::lifecycle::run_primitive::ConversationAppendRole::InjectedContext,
                        content: meerkat_core::lifecycle::run_primitive::CoreRenderable::text(
                            "durable injected context",
                        ),
                        identity: None,
                    },
                ],
                Some(meerkat_core::types::TranscriptMessageIdentity {
                    interaction_id: Some(meerkat_core::interaction::InteractionId::new()),
                    run_id: Some(run_id.clone()),
                    ..Default::default()
                }),
            )
            .expect("durable appends eligible for the active turn"),
        );
        let expected = meerkat_mob::SessionResumeAuthority::default();
        for wrapper in [pre_build, after_create] {
            assert!(
                Arc::clone(&wrapper)
                    .forked_participant_source_runtime()
                    .is_none(),
                "the probe answers None; the wrapper must relay it, not mint its own"
            );
            assert_eq!(
                wrapper.persisted_session_authority_read_cost(),
                meerkat_mob::runtime::PersistedSessionAuthorityReadCost::Bounded,
                "the trait default says Unsupported; only a forward can say Bounded"
            );
            assert!(
                wrapper
                    .observe_persisted_session_authority(&session_id)
                    .await
                    .expect("observe_persisted_session_authority must forward, not refuse")
                    .is_none()
            );
            wrapper
                .revalidate_session_resume_authority(&session_id, &expected)
                .await
                .expect("revalidate_session_resume_authority must forward")
                .expect("the probe accepts");
            for delivery in [&request_only, &durable] {
                let error = wrapper
                    .prepare_turn_boundary_delivery_for_active_turn(
                        &session_id,
                        &run_id,
                        delivery.clone(),
                    )
                    .await
                    .expect_err("the exact probe preparation error must forward");
                assert_eq!(
                    error,
                    meerkat_core::CoreBoundaryStageError::unavailable(
                        "probe has no boundary authority"
                    ),
                );
                let received = std::mem::take(
                    &mut *probe
                        .boundary_deliveries
                        .lock()
                        .expect("boundary deliveries"),
                );
                let [(received_session, received_run, received_delivery)] = received.as_slice()
                else {
                    panic!("one preparation must reach the inner service: {received:?}");
                };
                assert_eq!(received_session, &session_id);
                assert_eq!(received_run, &run_id);
                match (delivery, received_delivery) {
                    (
                        meerkat_core::TurnBoundaryDelivery::RequestOnly(expected),
                        meerkat_core::TurnBoundaryDelivery::RequestOnly(actual),
                    ) => assert_eq!(actual, expected, "preserve exact context bytes and order"),
                    (
                        meerkat_core::TurnBoundaryDelivery::DurableAppends(expected),
                        meerkat_core::TurnBoundaryDelivery::DurableAppends(actual),
                    ) => {
                        assert_eq!(actual.input_id(), expected.input_id());
                        assert_eq!(actual.messages(), expected.messages());
                        assert_eq!(actual.model_projection(), expected.model_projection());
                        assert_eq!(
                            serde_json::to_value(actual.peer_ingested_events())
                                .expect("serialize actual ingestion events"),
                            serde_json::to_value(expected.peer_ingested_events())
                                .expect("serialize expected ingestion events"),
                        );
                    }
                    (expected, actual) => panic!(
                        "the wrapper changed the typed boundary delivery: {expected:?} -> {actual:?}"
                    ),
                }
            }
        }
        let calls = probe.calls.lock().expect("probe calls");
        for name in [
            "forked_participant_source_runtime",
            "persisted_session_authority_read_cost",
            "observe_persisted_session_authority",
            "revalidate_session_resume_authority",
        ] {
            assert_eq!(
                calls.iter().filter(|call| **call == name).count(),
                2,
                "{name} must reach the inner service once per wrapper: {calls:?}"
            );
        }
        assert_eq!(
            calls
                .iter()
                .filter(|call| **call == "prepare_turn_boundary_delivery_for_active_turn")
                .count(),
            4,
            "both delivery variants must reach the inner service through both wrappers",
        );
    }

    struct CountingArchiveHook {
        fired: std::sync::atomic::AtomicUsize,
    }

    #[async_trait]
    impl meerkat_runtime::MachineSessionArchivePostCommitHook for CountingArchiveHook {
        async fn after_runtime_retire_commit(
            &self,
        ) -> Result<(), meerkat_runtime::RuntimeControlPlaneError> {
            self.fired.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }
    }

    /// The hook-bearing archive lane has a meerkat-mob trait default that
    /// refuses whenever a hook is present. Both production decorators must
    /// forward it so the hook reaches the inner service and fires there;
    /// before this, a destructive reset during a recovered-operation handoff
    /// failed with "session service cannot run a pre-retire archive hook".
    #[tokio::test]
    async fn wrappers_forward_the_hook_bearing_archive_lane_to_the_inner_service() {
        let probe = Arc::new(ForwardingProbe::default());
        let inner: Arc<dyn MobSessionService> = probe.clone();
        let pre_build = PreBuildMobSessionService {
            inner: Arc::clone(&inner),
            hook: no_op_pre_build_hook(),
            dispatch_taint: None,
            after_create_hook: None,
            runtime_adapter_override: None,
            archived_terminal_authority: None,
            member_sessions_in_memory: false,
        };
        let after_create = AfterCreateMobSessionService {
            inner: probe.clone(),
            after_hook: Arc::new(|_, _| Box::pin(async {})),
        };
        let session_id = meerkat_core::types::SessionId::new();
        let deadline =
            meerkat_core::time_compat::Instant::now() + std::time::Duration::from_secs(5);

        let hook = Arc::new(CountingArchiveHook {
            fired: std::sync::atomic::AtomicUsize::new(0),
        });
        MobSessionService::archive_with_mob_lifecycle_authority_under_runtime_turn_boundary_and_hook_before(
            &pre_build,
            &session_id,
            deadline,
            Some(Arc::clone(&hook) as Arc<dyn meerkat_runtime::MachineSessionArchivePostCommitHook>),
        )
        .await
        .expect("the pre-build wrapper must forward the hook-bearing archive lane");
        MobSessionService::archive_with_mob_lifecycle_authority_under_runtime_turn_boundary_and_hook_before(
            &after_create,
            &session_id,
            deadline,
            Some(Arc::clone(&hook) as Arc<dyn meerkat_runtime::MachineSessionArchivePostCommitHook>),
        )
        .await
        .expect("the after-create wrapper must forward the hook-bearing archive lane");

        assert_eq!(
            hook.fired.load(std::sync::atomic::Ordering::SeqCst),
            2,
            "the hook must fire inside the inner service once per wrapper"
        );
        let calls = probe.calls.lock().expect("probe calls");
        assert_eq!(
            calls
                .iter()
                .filter(|call| **call == "archive_and_hook_before(hook=some, fired)")
                .count(),
            2,
            "both wrappers must reach the inner service with the hook intact: {calls:?}"
        );
    }

    #[tokio::test]
    async fn pre_build_wrapper_forwards_mob_authority_and_control_extensions() {
        let probe = Arc::new(ForwardingProbe::default());
        let inner: Arc<dyn MobSessionService> = probe.clone();
        let wrapped = PreBuildMobSessionService {
            inner,
            hook: no_op_pre_build_hook(),
            dispatch_taint: None,
            after_create_hook: None,
            runtime_adapter_override: Some(Arc::new(meerkat_runtime::MeerkatMachine::ephemeral())),
            archived_terminal_authority: None,
            member_sessions_in_memory: false,
        };
        let session_id = meerkat_core::types::SessionId::new();
        let run_id = meerkat_core::lifecycle::RunId::new();

        MobSessionService::cancel_after_boundary_with_machine_authority(
            &wrapped,
            &session_id,
            &run_id,
            wrapped
                .acquire_runtime_adapter(None)
                .expect("acquire runtime adapter")
                .expect("wrapper should expose runtime adapter")
                .session_control_authority(),
        )
        .await
        .expect("machine-authorized cancel should treat a missing live session as quiesced");
        assert!(
            MobSessionService::suspend_run_with_machine_authority(
                &wrapped,
                &session_id,
                &run_id,
                wrapped
                    .acquire_runtime_adapter(None)
                    .expect("acquire runtime adapter")
                    .expect("wrapper should expose runtime adapter")
                    .session_control_authority(),
            )
            .await
            .expect("machine-authorized suspend should forward to inner service"),
            "the inner service's suspend answer comes back, not the trait default"
        );
        assert!(
            matches!(
                MobSessionService::settle_durable_tail_before_cold_start_reporting(
                    &wrapped,
                    &session_id,
                )
                .await
                .expect("the reporting cold-start settle should forward to inner service"),
                meerkat_mob::ColdStartTailSettlement::ContinuesOpenRun { .. }
            ),
            "the inner service's settle report comes back, not the trait default's Settled"
        );
        assert!(
            MobSessionService::load_persisted_session_tolerating_tail(&wrapped, &session_id)
                .await
                .expect("the tolerant committed read should forward to inner service")
                .is_some(),
            "the inner service's committed snapshot comes back, not the strict read's answer"
        );

        MobSessionService::archive_with_mob_lifecycle_authority(&wrapped, &session_id)
            .await
            .expect("archive_with_mob_lifecycle_authority should forward to inner service");
        meerkat_core::service::SessionService::reconcile_runtime_compaction_projections(
            &wrapped,
            &session_id,
            Vec::new(),
        )
        .await
        .expect("runtime compaction reconciliation should forward to inner service");
        meerkat_core::service::SessionService::acknowledge_runtime_compaction_finalization(
            &wrapped,
            &session_id,
            &meerkat_core::CommittedSessionBoundaryAuthority::HeadCanonical {
                session_id: session_id.clone(),
                committed_head_token: "probe-head-token".to_string(),
            },
        )
        .await
        .expect("runtime compaction finalization acknowledgement should forward to inner service");
        meerkat_core::service::SessionService::abort_uncommitted_compaction_projections(
            &wrapped,
            &session_id,
        )
        .await
        .expect("runtime compaction abort should forward to inner service");
        meerkat_core::service::SessionService::abort_rejected_runtime_run_projections(
            &wrapped,
            &session_id,
        )
        .await
        .expect("rejected runtime-run cleanup should forward to inner service");
        meerkat_core::service::SessionService::record_live_terminal_error(
            &wrapped,
            &session_id,
            meerkat_core::live_adapter::LiveAdapterErrorCode::ConnectionLost,
        )
        .await
        .expect("live terminal errors should forward to inner service");
        meerkat_core::service::SessionService::record_live_output_audio_degraded(
            &wrapped,
            &session_id,
            3,
        )
        .await
        .expect("live output degradation should forward to inner service");
        let staged = meerkat_core::service::SessionServiceControlExt::stage_tool_results(
            &wrapped,
            &session_id,
            meerkat_core::service::StageToolResultsRequest {
                results: Vec::new(),
            },
        )
        .await
        .expect("stage_tool_results should forward to inner service");
        let _ = meerkat_core::service::SessionServiceHistoryExt::read_transcript_revision(
            &wrapped,
            &session_id,
            meerkat_core::service::SessionTranscriptRevisionQuery {
                revision: "rev-1".to_string(),
                offset: 0,
                limit: None,
            },
        )
        .await;
        let _ = meerkat_core::service::SessionServiceHistoryExt::list_transcript_revisions(
            &wrapped,
            &session_id,
            meerkat_core::service::SessionTranscriptRevisionListQuery::default(),
        )
        .await;

        assert_eq!(staged.accepted_result_count, 7);
        let preparation_error = wrapped
            .prepare_turn_boundary_delivery_for_active_turn(
                &session_id,
                &meerkat_core::lifecycle::RunId::new(),
                meerkat_core::TurnBoundaryDelivery::RequestOnly(vec![
                    meerkat_core::lifecycle::run_primitive::TurnRequestContext::new("steer")
                        .expect("non-empty transient turn context"),
                ]),
            )
            .await
            .expect_err("probe preparation error should forward unchanged");
        assert_eq!(
            preparation_error,
            meerkat_core::CoreBoundaryStageError::unavailable("probe has no boundary authority"),
        );
        // meerkat 0.7.19 disposal-routing seam: the trait default is
        // fail-closed `true`, so a wrapper that fails to forward this
        // silently resurrects the ask-20 stranding for host-owned sessions.
        let known = wrapped
            .session_known_to_archive_authority(&session_id)
            .await
            .expect("archive-authority probe should forward");
        assert!(known, "probe answers true");
        assert_eq!(
            probe.calls(),
            vec![
                "cancel_after_boundary_with_machine_authority",
                "suspend_run_with_machine_authority",
                "settle_durable_tail_before_cold_start_reporting",
                "load_persisted_session_tolerating_tail",
                "archive_with_mob_lifecycle_authority",
                "reconcile_runtime_compaction_projections",
                "acknowledge_runtime_compaction_finalization",
                "abort_uncommitted_compaction_projections",
                "abort_rejected_runtime_run_projections",
                "record_live_terminal_error",
                "record_live_output_audio_degraded",
                "stage_tool_results",
                "read_transcript_revision",
                "list_transcript_revisions",
                "prepare_turn_boundary_delivery_for_active_turn",
                "session_known_to_archive_authority",
            ]
        );
    }

    #[tokio::test]
    async fn machine_authorized_boundary_cancel_only_normalizes_quiesced_liveness() {
        let probe = Arc::new(ForwardingProbe::default());
        let wrapped = PreBuildMobSessionService {
            inner: probe.clone(),
            hook: no_op_pre_build_hook(),
            dispatch_taint: None,
            after_create_hook: None,
            runtime_adapter_override: Some(Arc::new(meerkat_runtime::MeerkatMachine::ephemeral())),
            archived_terminal_authority: None,
            member_sessions_in_memory: false,
        };
        let session_id = meerkat_core::types::SessionId::new();
        let run_id = meerkat_core::lifecycle::RunId::new();

        for quiesced_outcome in [0, 1] {
            probe.set_cancel_outcome(quiesced_outcome);
            MobSessionService::cancel_after_boundary_with_machine_authority(
                &wrapped,
                &session_id,
                &run_id,
                wrapped
                    .acquire_runtime_adapter(None)
                    .expect("acquire runtime adapter")
                    .expect("wrapper should expose runtime adapter")
                    .session_control_authority(),
            )
            .await
            .expect("NotFound and NotRunning both prove lower-plane quiescence");
        }

        probe.set_cancel_outcome(2);
        let error = MobSessionService::cancel_after_boundary_with_machine_authority(
            &wrapped,
            &session_id,
            &run_id,
            wrapped
                .acquire_runtime_adapter(None)
                .expect("acquire runtime adapter")
                .expect("wrapper should expose runtime adapter")
                .session_control_authority(),
        )
        .await
        .expect_err("non-liveness cancellation failures must remain fatal");
        assert!(
            matches!(error, SessionError::Unsupported(ref detail) if detail == "synthetic cancel rejection"),
            "unexpected fail-closed cancellation error: {error}"
        );
    }

    /// Regression for two compounding bugs in the persistent wiring:
    ///
    /// 1. **0.6.0**: `persistent_inner` handed the
    ///    `PersistentSessionService` an `InMemoryRuntimeStore`. With the
    ///    runtime_store path active the `StoreCheckpointer` was disabled
    ///    (it's gated on `runtime_store.is_none()`), and the in-memory
    ///    store didn't survive process restart. Resume raised "missing
    ///    durable session snapshot for '<sid>'".
    ///
    /// 2. **0.6.1**: switching the session service to `runtime_store=None`
    ///    re-enabled the checkpointer (fixing #1) but broke archive/retire,
    ///    because `load_persisted_session_for_control` rejects mutations
    ///    when runtime_store is None and the session exists in the store
    ///    (the "store-only compatibility projection" error from
    ///    meerkat-session/src/persistent.rs:786).
    ///
    /// The 0.6.3 fix uses a **persistent** SqliteRuntimeStore — durable
    /// across restart AND control-op authoritative — at
    /// `<store_path>/runtime.sqlite`.
    #[tokio::test]
    async fn persistent_bootstrap_uses_sqlite_runtime_store() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let store_path = dir.path().to_path_buf();
        let Ok(definition) = meerkat_mob::MobDefinition::from_toml("[mob]\nid = \"test\"\n") else {
            panic!("failed to parse minimal mob definition");
        };
        let spec = MobBootstrapSpec::persistent(
            definition,
            meerkat_mob::MobStorage::in_memory(),
            crate::mob_composition_manifest::MobStorageProvenance::declared_ephemeral(),
            store_path.clone(),
            4,
        )
        .await
        .unwrap_or_else(|e| panic!("{e}"));
        assert!(
            spec.runtime_adapter.is_some(),
            "persistent bootstrap must provide its own runtime adapter via spec.runtime_adapter"
        );
        assert!(
            spec.durable_runtime_store.is_some(),
            "persistent bootstrap must declare the same committed runtime store for notice \
             observations and boundary reads"
        );
        assert!(
            spec.session_service
                .acquire_runtime_adapter(None)
                .expect("acquire runtime adapter")
                .is_some(),
            "session service must own a runtime_store so archive/retire don't \
             hit the store-only-projection rejection"
        );
        assert!(
            store_path.join("runtime.sqlite").exists(),
            "persistent_inner must open a SqliteRuntimeStore at <store_path>/runtime.sqlite"
        );
        assert_notice_store_observes_machine_admission(&spec).await;
    }

    async fn assert_notice_store_observes_machine_admission(spec: &MobBootstrapSpec) {
        use meerkat_runtime::service_ext::SessionServiceRuntimeExt;

        let machine = spec.runtime_adapter.as_ref().expect("runtime machine");
        let store = spec
            .durable_runtime_store
            .as_ref()
            .expect("notice observation store");
        assert!(
            machine.shares_runtime_store_authority(store),
            "notice settlement must read the store used by this runtime machine"
        );
        let session_id = meerkat_core::types::SessionId::new();
        machine
            .register_session(session_id.clone())
            .await
            .expect("register a session without an automatic runtime loop");
        let input = meerkat_runtime::Input::Prompt(meerkat_runtime::input::PromptInput::new(
            "notice authority constructor regression",
            None,
        ));
        let input_id = input.id().clone();
        let runtime_id = meerkat_runtime::LogicalRuntimeId::for_session(&session_id);
        assert!(
            store
                .load_input_state(&runtime_id, &input_id)
                .await
                .unwrap()
                .is_none()
        );
        let outcome = machine.accept_input(&session_id, input).await.unwrap();
        assert!(outcome.is_accepted());
        let stored = store
            .load_input_state(&runtime_id, &input_id)
            .await
            .expect("read actual owner admission through the notice observation store")
            .expect("the accepted input was committed to this exact store");
        assert_eq!(stored.state.input_id, input_id);
        assert_eq!(
            stored.seed.phase,
            meerkat_runtime::input_state::InputLifecycleState::Queued,
            "the notice reader sees committed generated input state, not a separate store"
        );
    }

    /// H1: a persistent-mode blob-dir open failure is a startup error —
    /// never the former silent in-memory fallback (the GKE hazard).
    #[tokio::test]
    async fn persistent_spec_fails_closed_when_blob_dir_unopenable() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let store_path = dir.path().to_path_buf();
        // A regular FILE at <store_path>/blobs makes the blob-dir open fail.
        std::fs::write(store_path.join("blobs"), b"not a directory")
            .unwrap_or_else(|e| panic!("{e}"));
        let definition = meerkat_mob::MobDefinition::from_toml("[mob]\nid = \"test\"\n")
            .unwrap_or_else(|e| panic!("{e}"));

        match MobBootstrapSpec::persistent(
            definition,
            meerkat_mob::MobStorage::in_memory(),
            crate::mob_composition_manifest::MobStorageProvenance::declared_ephemeral(),
            store_path.clone(),
            4,
        )
        .await
        {
            Err(StorageResolutionError::Blob(BlobStoreResolutionError::OpenFailed {
                path,
                ..
            })) => {
                assert_eq!(path, store_path.join("blobs"));
            }
            Err(other) => panic!("expected OpenFailed, got: {other}"),
            Ok(_) => {
                panic!("blob-dir open failure must fail closed, not fall back to in-memory blobs")
            }
        }
    }

    /// M4: a persistent-mode runtime store that cannot be read is a startup
    /// refusal, never the former silent `InMemoryRuntimeStore` twin. A
    /// directory at `<store_path>/runtime.sqlite` cannot be read, so the
    /// startup layout check refuses typed before any store opens.
    #[tokio::test]
    async fn persistent_spec_fails_closed_when_runtime_store_unopenable() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let store_path = dir.path().to_path_buf();
        // A DIRECTORY at <store_path>/runtime.sqlite cannot be opened or read.
        std::fs::create_dir_all(store_path.join(crate::storage_layout::RUNTIME_DB_FILE_NAME))
            .unwrap_or_else(|e| panic!("{e}"));
        let definition = meerkat_mob::MobDefinition::from_toml("[mob]\nid = \"test\"\n")
            .unwrap_or_else(|e| panic!("{e}"));

        match MobBootstrapSpec::persistent(
            definition,
            meerkat_mob::MobStorage::in_memory(),
            crate::mob_composition_manifest::MobStorageProvenance::declared_ephemeral(),
            store_path.clone(),
            4,
        )
        .await
        {
            Err(StorageResolutionError::Layout(
                crate::storage_layout::StorageLayoutError::NativeStorageMigrationRequired {
                    state_dir,
                    ..
                },
            )) => assert_eq!(state_dir, store_path),
            Err(other) => panic!("expected the typed startup refusal, got: {other}"),
            Ok(_) => panic!(
                "an unreadable runtime store must fail closed, not fall back to \
                 InMemoryRuntimeStore"
            ),
        }
    }

    /// 0.8.16 item 5: a caller-injected `WorkGraphStore` suppresses the local
    /// SQLite fallback and is reported as its own census slot.
    ///
    /// The load-bearing assertion is the ABSENT file, not the populated slot.
    /// `persistent_inner_with_provider_stores` is `pub(crate)` and reachable
    /// without `UnifiedRuntimeBuilder`'s conflict check, so the injected arm
    /// has to be ordered ahead of the provider arm; if the order regresses,
    /// the fallback runs and `workgraph.sqlite3` appears here.
    #[tokio::test]
    async fn injected_workgraph_store_suppresses_the_local_sqlite_file() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let store_path = dir.path().to_path_buf();
        let definition = meerkat_mob::MobDefinition::from_toml("[mob]\nid = \"test\"\n")
            .unwrap_or_else(|e| panic!("{e}"));

        let spec = MobBootstrapSpec::persistent_inner(
            definition,
            meerkat_mob::MobStorage::in_memory(),
            crate::mob_composition_manifest::MobStorageProvenance::declared_ephemeral(),
            store_path.clone(),
            4,
            None,
            false,
            None,
            Some(Arc::new(meerkat::MemoryWorkGraphStore::new())),
            None,
            CapabilityFlags::default(),
            None,
            None,
        )
        .await
        .unwrap_or_else(|e| panic!("injected workgraph store must compose: {e}"));

        // Positive control: persistent composition demonstrably ran in this
        // directory, so the absent workgraph file below means "suppressed",
        // not "nothing happened here".
        assert!(
            store_path
                .join(crate::storage_layout::RUNTIME_DB_FILE_NAME)
                .exists(),
            "persistent composition must have run in the state dir"
        );
        assert!(
            !store_path
                .join(crate::workgraph_wiring::WORKGRAPH_STORE_FILE)
                .exists(),
            "an injected workgraph store must suppress the local SQLite fallback"
        );

        let summary = spec.resolved_storage.unwrap_or_else(|| panic!("summary"));
        let workgraph_slot = summary
            .slots
            .iter()
            .find(|slot| slot.durability.domain() == "workgraph")
            .unwrap_or_else(|| panic!("workgraph slot recorded"));
        assert_eq!(workgraph_slot.backend, "custom workgraph store");
        assert_eq!(
            workgraph_slot.durability,
            crate::storage_health::StorageSlotDurability::Unverified {
                domain: "workgraph".to_string(),
                class: meerkat_core::DurabilityClass::Durable,
            }
        );
        let json = summary.status_json();
        let wire_slot = json["slots"]
            .as_array()
            .expect("slots")
            .iter()
            .find(|slot| slot["domain"] == "workgraph")
            .expect("workgraph wire slot");
        assert_eq!(wire_slot["resolution"], "unverified");
    }

    fn workgraph_census_provider() -> crate::storage_provider::ProviderMeerkatStores {
        use meerkat_core::{DurabilityDeclaration, DurabilityResolution};

        crate::storage_provider::ProviderMeerkatStores {
            realm: crate::storage_provider::default_meerkat_level_realm(),
            provider_name: "census-test-provider".to_string(),
            session_store: Arc::new(meerkat_store::MemoryStore::new()),
            runtime_store: Arc::new(meerkat_runtime::InMemoryRuntimeStore::new()),
            runtime_declaration: DurabilityDeclaration::durable(
                "runtime",
                DurabilityResolution::DeclaredEphemeral,
            ),
            workgraph_store: Arc::new(meerkat::MemoryWorkGraphStore::new()),
            workgraph_declaration: DurabilityDeclaration::durable(
                "workgraph",
                DurabilityResolution::DeclaredEphemeral,
            ),
            job_store: Arc::new(meerkat::MemoryDetachedJobStore::new()),
            job_declaration: DurabilityDeclaration::durable(
                "jobs",
                DurabilityResolution::DeclaredEphemeral,
            ),
        }
    }

    async fn workgraph_census_spec(
        persistent: bool,
        store_path: PathBuf,
        injected: Option<Arc<dyn meerkat::WorkGraphStore>>,
        provider: Option<crate::storage_provider::ProviderMeerkatStores>,
    ) -> MobBootstrapSpec {
        let definition =
            meerkat_mob::MobDefinition::from_toml("[mob]\nid = \"test\"\n").expect("definition");
        if persistent {
            MobBootstrapSpec::persistent_inner_with_provider_stores(
                definition,
                meerkat_mob::MobStorage::in_memory(),
                crate::mob_composition_manifest::MobStorageProvenance::declared_ephemeral(),
                store_path,
                4,
                None,
                false,
                None,
                injected,
                None,
                CapabilityFlags::default(),
                None,
                None,
                provider,
                crate::storage_provider::default_meerkat_level_realm(),
                None,
                None,
                None,
                None,
                PersistentAuthority::StateDir,
                None,
                None,
            )
            .await
            .expect("persistent constructor")
        } else {
            MobBootstrapSpec::ephemeral_runtime_backed_with_provider_stores(
                definition,
                meerkat_mob::MobStorage::in_memory(),
                store_path,
                4,
                None,
                None,
                injected,
                None,
                CapabilityFlags::default(),
                None,
                None,
                provider,
                None,
                None,
                None,
                None,
                None,
            )
            .expect("bootstrap spec")
        }
    }

    async fn assert_injected_workgraph_census(persistent: bool, with_provider: bool) {
        let dir = tempfile::tempdir().expect("tempdir");
        let injected: Arc<dyn meerkat::WorkGraphStore> =
            Arc::new(meerkat::MemoryWorkGraphStore::new());
        let spec = workgraph_census_spec(
            persistent,
            dir.path().to_path_buf(),
            Some(Arc::clone(&injected)),
            with_provider.then(workgraph_census_provider),
        )
        .await;
        assert!(Arc::ptr_eq(
            spec.workgraph_service.as_ref().expect("service").store(),
            &injected,
        ));
        let json = spec
            .resolved_storage
            .as_ref()
            .expect("summary")
            .status_json();
        let slots: Vec<_> = json["slots"]
            .as_array()
            .expect("slots")
            .iter()
            .filter(|slot| slot["domain"] == "workgraph")
            .collect();
        assert_eq!(slots.len(), 1);
        assert_eq!(slots[0]["resolution"], "unverified");
        assert_eq!(slots[0]["class"], "durable");
        assert_eq!(slots[0]["backend"], "custom workgraph store");
        assert_eq!(slots[0]["degraded"], false);
        assert!(
            !dir.path()
                .join(crate::workgraph_wiring::WORKGRAPH_STORE_FILE)
                .exists()
        );
    }

    #[tokio::test]
    async fn injected_workgraph_census_persistent_without_provider() {
        assert_injected_workgraph_census(true, false).await;
    }

    #[tokio::test]
    async fn injected_workgraph_census_persistent_overrides_provider() {
        assert_injected_workgraph_census(true, true).await;
    }

    #[tokio::test]
    async fn injected_workgraph_census_ephemeral_without_provider() {
        assert_injected_workgraph_census(false, false).await;
    }

    #[tokio::test]
    async fn injected_workgraph_census_ephemeral_overrides_provider() {
        assert_injected_workgraph_census(false, true).await;
    }

    #[tokio::test]
    async fn provider_workgraph_census_follows_selected_store_in_both_constructors() {
        for persistent in [false, true] {
            let dir = tempfile::tempdir().expect("tempdir");
            let provider = workgraph_census_provider();
            let selected = Arc::clone(&provider.workgraph_store);
            let declaration = provider.workgraph_declaration.clone();
            let spec =
                workgraph_census_spec(persistent, dir.path().to_path_buf(), None, Some(provider))
                    .await;
            assert!(Arc::ptr_eq(
                spec.workgraph_service.as_ref().expect("service").store(),
                &selected,
            ));
            let summary = spec.resolved_storage.as_ref().expect("summary");
            let slots: Vec<_> = summary
                .slots
                .iter()
                .filter(|slot| slot.durability.domain() == "workgraph")
                .collect();
            assert_eq!(slots.len(), 1);
            assert_eq!(slots[0].durability.declaration(), Some(&declaration));
            assert_eq!(slots[0].backend, "storage provider 'census-test-provider'");
            let json = summary.status_json();
            let wire_slot = json["slots"]
                .as_array()
                .expect("slots")
                .iter()
                .find(|slot| slot["domain"] == "workgraph")
                .expect("workgraph wire slot");
            assert_eq!(wire_slot["resolution"], "declared_ephemeral");
        }
    }

    /// H1: the happy persistent path reports disk-backed blobs and the
    /// incremental session capability (H2) on the resolved summary.
    #[tokio::test]
    async fn persistent_spec_reports_persistent_disk_and_incremental_sessions() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let store_path = dir.path().to_path_buf();
        let definition = meerkat_mob::MobDefinition::from_toml("[mob]\nid = \"test\"\n")
            .unwrap_or_else(|e| panic!("{e}"));

        let spec = MobBootstrapSpec::persistent(
            definition,
            meerkat_mob::MobStorage::in_memory(),
            crate::mob_composition_manifest::MobStorageProvenance::declared_ephemeral(),
            store_path,
            4,
        )
        .await
        .unwrap_or_else(|e| panic!("{e}"));
        let summary = spec.resolved_storage.unwrap_or_else(|| panic!("summary"));
        assert_eq!(summary.blob_durability, BlobDurability::PersistentDisk);
        assert_eq!(summary.session_store_incremental, Some(true));
        // The M4 per-slot census: durable slots persistent, the ring buffers
        // classified Scratch explicitly.
        for (domain, backend) in [
            ("runtime", "SqliteRuntimeStore"),
            ("workgraph", "SqliteWorkGraphStore"),
        ] {
            let slot = summary
                .slots
                .iter()
                .find(|slot| slot.durability.domain() == domain)
                .unwrap_or_else(|| panic!("{domain} slot recorded"));
            assert_eq!(slot.backend, backend);
            assert_eq!(
                slot.durability
                    .declaration()
                    .expect("declared slot")
                    .resolution,
                meerkat_core::DurabilityResolution::Persistent
            );
        }
        for domain in ["gating_audit", "delivery_history", "routing_resolutions"] {
            let slot = summary
                .slots
                .iter()
                .find(|slot| slot.durability.domain() == domain)
                .unwrap_or_else(|| panic!("{domain} ring buffer classified"));
            assert_eq!(
                slot.durability.class(),
                meerkat_core::DurabilityClass::Scratch
            );
        }
    }

    /// H1: persistent mode with a custom blob store reporting
    /// `!is_persistent()` is a startup error without the explicit
    /// declaration, and accepted (reported as non-persistent custom) with it.
    #[tokio::test]
    async fn persistent_spec_gates_non_persistent_custom_blob_store_on_declaration() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let store_path = dir.path().to_path_buf();
        let memory_blobs: Arc<dyn meerkat_core::BlobStore> = Arc::new(Base64BlobStoreAdapter::new(
            Arc::new(ObjectStoreBlobStore::memory()),
        ));
        let definition = meerkat_mob::MobDefinition::from_toml("[mob]\nid = \"test\"\n")
            .unwrap_or_else(|e| panic!("{e}"));

        match MobBootstrapSpec::persistent_inner(
            definition.clone(),
            meerkat_mob::MobStorage::in_memory(),
            crate::mob_composition_manifest::MobStorageProvenance::declared_ephemeral(),
            store_path.clone(),
            4,
            Some(BlobStoreInjection::Core(memory_blobs.clone())),
            false,
            None,
            None,
            None,
            CapabilityFlags::default(),
            None,
            None,
        )
        .await
        {
            Err(StorageResolutionError::Blob(
                BlobStoreResolutionError::NonPersistentUndeclared,
            )) => {}
            Err(other) => panic!("expected NonPersistentUndeclared, got: {other}"),
            Ok(_) => panic!("undeclared non-persistent custom blob store must fail composition"),
        }

        let spec = MobBootstrapSpec::persistent_inner(
            definition,
            meerkat_mob::MobStorage::in_memory(),
            crate::mob_composition_manifest::MobStorageProvenance::declared_ephemeral(),
            store_path,
            4,
            Some(BlobStoreInjection::Core(memory_blobs)),
            true,
            None,
            None,
            None,
            CapabilityFlags::default(),
            None,
            None,
        )
        .await
        .unwrap_or_else(|e| panic!("declared ephemeral blobs must compose: {e}"));
        let summary = spec.resolved_storage.unwrap_or_else(|| panic!("summary"));
        assert_eq!(
            summary.blob_durability,
            BlobDurability::Custom { persistent: false }
        );
        assert_eq!(summary.session_store_incremental, Some(true));
    }

    /// H1: `ephemeral_blobs` without a custom store is a declared in-memory
    /// choice — no `blobs/` directory materializes on disk.
    #[tokio::test]
    async fn persistent_spec_declared_ephemeral_blobs_skips_disk() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let store_path = dir.path().to_path_buf();
        let definition = meerkat_mob::MobDefinition::from_toml("[mob]\nid = \"test\"\n")
            .unwrap_or_else(|e| panic!("{e}"));

        let spec = MobBootstrapSpec::persistent_inner(
            definition,
            meerkat_mob::MobStorage::in_memory(),
            crate::mob_composition_manifest::MobStorageProvenance::declared_ephemeral(),
            store_path.clone(),
            4,
            None,
            true,
            None,
            None,
            None,
            CapabilityFlags::default(),
            None,
            None,
        )
        .await
        .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            spec.resolved_storage.map(|summary| summary.blob_durability),
            Some(BlobDurability::DeclaredEphemeral)
        );
        assert!(
            !store_path.join("blobs").exists(),
            "declared-ephemeral blobs must not touch the disk blob root"
        );
    }

    /// The memory-backed mode records its ephemeral declaration. There is no
    /// caller-owned durable session store for the H2 probe.
    #[test]
    fn ephemeral_runtime_backed_spec_reports_declared_ephemeral() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let definition = meerkat_mob::MobDefinition::from_toml("[mob]\nid = \"test\"\n")
            .unwrap_or_else(|e| panic!("{e}"));

        let spec = MobBootstrapSpec::ephemeral_runtime_backed_inner(
            definition,
            meerkat_mob::MobStorage::in_memory(),
            dir.path().to_path_buf(),
            4,
            None,
            None,
            None,
            None,
            CapabilityFlags::default(),
            None,
            None,
        )
        .expect("bootstrap spec");
        let summary = spec.resolved_storage.unwrap_or_else(|| panic!("summary"));
        assert_eq!(summary.blob_durability, BlobDurability::DeclaredEphemeral);
        assert_eq!(summary.session_store_incremental, None);
        // Every by-mode in-memory slot is a DECLARED choice in the census.
        for domain in ["sessions", "runtime", "blobs", "workgraph"] {
            let slot = summary
                .slots
                .iter()
                .find(|slot| slot.durability.domain() == domain)
                .unwrap_or_else(|| panic!("{domain} slot recorded"));
            assert_eq!(
                slot.durability
                    .declaration()
                    .expect("declared slot")
                    .resolution,
                meerkat_core::DurabilityResolution::DeclaredEphemeral,
                "{domain} must be a declared ephemeral choice"
            );
        }
    }

    /// Persistent builds run on ONE machine: the session service's runtime
    /// adapter (what the agent mob tools and every other adapter consumer
    /// use) is the exact machine the runtime hands to `MobBuilder`, not a
    /// second `MeerkatMachine::persistent` over the same runtime store.
    #[tokio::test]
    async fn persistent_spec_session_service_uses_the_spec_runtime_machine() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let definition = meerkat_mob::MobDefinition::from_toml("[mob]\nid = \"test\"\n")
            .unwrap_or_else(|e| panic!("{e}"));
        let spec = MobBootstrapSpec::persistent(
            definition,
            meerkat_mob::MobStorage::in_memory(),
            crate::mob_composition_manifest::MobStorageProvenance::declared_ephemeral(),
            dir.path().join("state"),
            4,
        )
        .await
        .unwrap_or_else(|e| panic!("{e}"));
        let runtime = spec
            .runtime_adapter
            .clone()
            .unwrap_or_else(|| panic!("persistent spec carries its runtime machine"));
        let served = spec
            .session_service
            .acquire_runtime_adapter(None)
            .expect("acquire runtime adapter")
            .unwrap_or_else(|| panic!("persistent session service exposes a runtime machine"));
        assert!(
            Arc::ptr_eq(&served, &runtime),
            "the session service must serve the runtime's own machine, not a second one"
        );
    }

    /// Ephemeral counterpart: runtime-backed ephemeral builds must use a
    /// single in-memory machine authority for session service, comms, and
    /// image-generation tooling.
    #[tokio::test]
    async fn ephemeral_runtime_backed_uses_session_service_runtime_adapter() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let store_path = dir.path().to_path_buf();
        let definition = MobDefinition::from_toml(&format!(
            "[mob]\nid = \"{}\"\n[profiles.worker]\nmodel = \"gpt-5.5\"\n[profiles.worker.tools]\ncomms = true\n",
            unique_test_mob_id(),
        )).expect("comms fixture definition");
        let mut spec = MobBootstrapSpec::ephemeral_runtime_backed_inner(
            definition,
            meerkat_mob::MobStorage::in_memory(),
            store_path,
            4,
            None,
            None,
            None,
            None,
            CapabilityFlags::default(),
            None,
            None,
        )
        .expect("bootstrap spec");
        assert!(
            spec.runtime_adapter.is_some(),
            "ephemeral_runtime_backed_inner must expose the shared runtime authority"
        );
        assert!(
            spec.session_service
                .acquire_runtime_adapter(None)
                .expect("acquire runtime adapter")
                .is_some(),
            "session service must still expose a runtime adapter so autonomous-host comms can wire"
        );
        assert_notice_store_observes_machine_admission(&spec).await;
        spec.options.default_llm_client = Some(Arc::new(meerkat_client::TestClient::for_provider(
            meerkat_core::Provider::OpenAI,
        )));
        let service = spec.session_service.clone();
        tokio::time::timeout(std::time::Duration::from_secs(30), async move {
            let runtime = Box::pin(MobRuntime::bootstrap(spec))
                .await
                .expect("bootstrap process-local runtime");
            let left = meerkat_mob::AgentIdentity::from("left");
            let right = meerkat_mob::AgentIdentity::from("right");
            for member in [&left, &right] {
                Box::pin(runtime.handle.spawn_spec(SpawnMemberSpec::new(
                    ProfileName::from("worker"),
                    member.clone(),
                )))
                .await
                .expect("create comms-enabled member");
                let session = runtime
                    .handle
                    .resolve_bridge_session_id(member)
                    .await
                    .expect("created member session");
                assert!(
                    service.comms_runtime(&session).await.is_some(),
                    "the concrete persistent service exposes live member comms immediately"
                );
            }
            runtime
                .handle
                .wire(left, right)
                .await
                .expect("reconcile an actual member edge");
            let report = runtime
                .handle
                .destroy()
                .await
                .expect("destroy process-local fixture");
            assert!(report.errors.is_empty());
            assert!(report.orphaned_remote_members.is_empty());
            assert!(!report.remote_cleanup_deadline_exceeded);
            assert!(report.metadata_scrubbed && report.events_cleared && report.namespace_cleaned);
        })
        .await
        .expect("bounded create, edge reconciliation and cleanup");
    }

    /// Pins meerkat 0.8.23's fail-closed default at meta's bootstrap
    /// boundary: a second same-process bootstrap of an already-published mob
    /// id surfaces the typed in-proc `NameOccupied` rejection as an ordinary
    /// bootstrap error - not a panic, not a hang, and not a silent
    /// displacement. Succession has exactly one sanctioned shape: the
    /// incumbent stands down through its real lifecycle (terminal shutdown
    /// releases `{mob_id}/__mob_supervisor__` at supervisor retirement), and
    /// only then does an ordinary same-id bootstrap succeed. There is no
    /// takeover flag on purpose - upstream's replacement seams demand the
    /// predecessor's own runtime handle as evidence, and a blind bootstrap
    /// has none.
    #[tokio::test(flavor = "multi_thread")]
    async fn second_same_id_bootstrap_fails_closed_until_incumbent_stands_down() {
        let mob_id = unique_test_mob_id();
        let spec_for = || {
            let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
            let factory = AgentFactory::new(dir.path()).builtins(false).comms(true);
            let builder = FactoryAgentBuilder::new(factory, Config::default());
            let session_service: Arc<dyn MobSessionService> =
                Arc::new(meerkat_session::EphemeralSessionService::new(builder, 4));
            let definition = meerkat_mob::MobDefinition::from_toml(&format!(
                "[mob]\nid = \"{mob_id}\"\n\n[profiles.worker]\nmodel = \"gpt-5.5\"\n[profiles.worker.tools]\ncomms = true\n",
            ))
            .unwrap_or_else(|e| panic!("{e}"));
            let mut spec = MobBootstrapSpec::new(
                definition,
                meerkat_mob::MobStorage::in_memory(),
                session_service,
            );
            spec.options.default_llm_client = Some(Arc::new(
                meerkat_client::TestClient::for_provider(meerkat_core::Provider::OpenAI),
            ));
            (spec, dir)
        };

        let (first_spec, _first_dir) = spec_for();
        let first = MobRuntime::bootstrap(first_spec)
            .await
            .unwrap_or_else(|e| panic!("first bootstrap must succeed: {e}"));

        let (denied_spec, _denied_dir) = spec_for();
        let denied = MobRuntime::bootstrap(denied_spec).await;
        let error = match denied {
            Err(error) => error.to_string(),
            Ok(_) => {
                panic!("second same-id bootstrap must fail closed while the incumbent is live")
            }
        };
        assert!(
            error.contains("already has a live route"),
            "the bootstrap error must surface the typed in-proc NameOccupied \
             rejection, got: {error}"
        );

        // The sanctioned succession: the incumbent stands down through its
        // real lifecycle (terminal shutdown releases the supervisor route at
        // retirement); the successor then bootstraps ordinarily with no
        // takeover of any kind.
        first
            .handle()
            .shutdown()
            .await
            .unwrap_or_else(|e| panic!("incumbent stand-down must succeed: {e}"));
        let (successor_spec, _successor_dir) = spec_for();
        let successor = MobRuntime::bootstrap(successor_spec)
            .await
            .unwrap_or_else(|e| {
                panic!("ordinary same-id bootstrap after stand-down must succeed: {e}")
            });

        let _ = successor.handle().shutdown().await;
    }

    /// Per-test mob id for tests that BOOTSTRAP a runtime: 0.8.23's
    /// fail-closed in-proc registration means concurrent bootstraps must not
    /// share a `{mob_id}/__mob_supervisor__` route.
    fn unique_test_mob_id() -> String {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        format!(
            "test-{}",
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        )
    }

    #[cfg(feature = "extension-state")]
    #[tokio::test]
    async fn document_mcp_child_supply_uses_final_policy_state() {
        let dir = tempfile::tempdir().expect("temporary store");
        let definition = MobDefinition::from_toml(&format!(
            "[mob]\nid = \"{}\"\n\n[profiles.worker]\nmodel = \"gpt-5.5\"\n[profiles.worker.tools]\ncomms = true\n",
            unique_test_mob_id()
        ))
        .expect("root definition");
        let mut spec = MobBootstrapSpec::ephemeral_runtime_backed_inner(
            definition,
            MobStorage::in_memory(),
            dir.path().to_path_buf(),
            4,
            None,
            None,
            None,
            None,
            CapabilityFlags::default(),
            None,
            None,
        )
        .expect("bootstrap spec")
        .with_child_application_tool_policy(meerkat_core::ApplicationToolPolicyBinding::Unmanaged);
        let preliminary = spec
            .agent_mob_mcp_state
            .clone()
            .expect("preliminary mob-tool state");
        let shared = meerkat_core::McpServerConfig::stdio(
            "shared-documents",
            "document-fixture-mcp",
            vec![],
            Default::default(),
        );
        let private = meerkat_core::McpServerConfig::stdio(
            "private-documents",
            "document-fixture-mcp",
            vec![],
            Default::default(),
        );
        spec.child_mcp_servers = Some(
            meerkat_mob_mcp::ChildMcpServers::new()
                .register(
                    shared.clone(),
                    meerkat_mob_mcp::ChildToolBundleAvailability::ChildAvailable,
                )
                .expect("shared descriptor")
                .register(
                    private,
                    meerkat_mob_mcp::ChildToolBundleAvailability::HostOnly,
                )
                .expect("host-only descriptor"),
        );
        spec.options.default_llm_client = Some(Arc::new(meerkat_client::TestClient::for_provider(
            meerkat_core::Provider::OpenAI,
        )));

        let runtime = MobRuntime::bootstrap(spec).await.expect("root bootstrap");
        let final_state = runtime
            .agent_mob_mcp_state
            .clone()
            .expect("final mob-tool state");
        assert!(
            !Arc::ptr_eq(&preliminary, &final_state),
            "child policy must exercise replacement of the preliminary state"
        );
        let owner = meerkat_mob::AgentIdentity::from("document-owner");
        Box::pin(
            runtime
                .handle()
                .spawn_spec(SpawnMemberSpec::host_root("worker", owner.clone())),
        )
        .await
        .expect("owner member");
        let owner_session = runtime
            .handle()
            .resolve_bridge_session_id(&owner)
            .await
            .expect("owner bridge session");
        let child_definition = MobDefinition::from_toml(&format!(
            "[mob]\nid = \"{}\"\n\n[profiles.worker]\nmodel = \"gpt-5.5\"\n[profiles.worker.tools]\ncomms = true\n",
            unique_test_mob_id()
        ))
        .expect("child definition");
        let child_id = Box::pin(final_state.mob_create_definition_with_owner_bridge_session(
            child_definition,
            owner_session.clone(),
            true,
            false,
        ))
        .await
        .expect("member-owned child creation from final state");
        let child = final_state
            .mob_handles_snapshot()
            .await
            .expect("child handles")
            .into_iter()
            .find_map(|(id, handle)| (id == child_id).then_some(handle))
            .expect("created child");
        let authority = child
            .owner_bridge_session_lifecycle_authority()
            .expect("persisted child authority");
        assert_eq!(authority.bridge_session_id, owner_session);
        assert!(authority.destroy_on_owner_archive);
        let profile = child.definition().profiles[&ProfileName::from("worker")]
            .as_inline()
            .expect("inline worker");
        assert_eq!(profile.tools.mcp_servers, vec![shared]);
        assert!(
            profile.tools.rust_bundles.is_empty(),
            "MCP supply must not introduce a native tool fallback"
        );
        child.shutdown().await.expect("child shutdown");
        runtime.handle().shutdown().await.expect("root shutdown");
    }

    #[cfg(feature = "extension-state")]
    #[tokio::test]
    async fn document_mcp_child_supply_requires_final_mob_tool_state() {
        let dir = tempfile::tempdir().expect("temporary store");
        let definition = MobDefinition::from_toml(&format!(
            "[mob]\nid = \"{}\"\n\n[profiles.worker]\nmodel = \"gpt-5.5\"\n",
            unique_test_mob_id()
        ))
        .expect("root definition");
        let mut spec = MobBootstrapSpec::ephemeral_runtime_backed_inner(
            definition,
            MobStorage::in_memory(),
            dir.path().to_path_buf(),
            4,
            None,
            None,
            None,
            None,
            CapabilityFlags::default(),
            None,
            None,
        )
        .expect("bootstrap spec");
        spec.agent_mob_mcp_state = None;
        spec.agent_mob_tools_install = None;
        spec.child_mcp_servers = Some(meerkat_mob_mcp::ChildMcpServers::new());

        assert!(matches!(
            MobRuntime::bootstrap(spec).await,
            Err(MobRuntimeError::InvalidConfig(message))
                if message == "document MCP child delivery requires an agent mob-tool state"
        ));
    }

    /// A member whose profile carries a marked document MCP descriptor that
    /// no host document binding resolved is refused at its build, in either
    /// spelling of the marker and with or without the extension-state
    /// feature: the descriptor never launches as an unbound service.
    #[tokio::test]
    async fn extension_unresolved_document_marker_refuses_member_builds() {
        for marker in ["META_MCP_DOCUMENT_BINDING", "MOBKIT_MCP_DOCUMENT_BINDING"] {
            let dir = tempfile::tempdir().expect("temporary store");
            let mut definition = meerkat_mob::MobDefinition::from_toml(&format!(
                "[mob]\nid = \"{}\"\n\n[profiles.worker]\nmodel = \"gpt-5.5\"\n\
                 runtime_mode = \"turn_driven\"\n[profiles.worker.tools]\ncomms = true\n",
                unique_test_mob_id()
            ))
            .expect("definition");
            let Some(meerkat_mob::ProfileBinding::Inline(profile)) = definition
                .profiles
                .get_mut(&meerkat_mob::ProfileName::from("worker"))
            else {
                panic!("the definition declares an inline worker profile");
            };
            profile
                .tools
                .mcp_servers
                .push(meerkat_core::McpServerConfig::stdio(
                    "documents",
                    "document-fixture-mcp",
                    vec![],
                    std::collections::HashMap::from([(
                        marker.to_string(),
                        "documents".to_string(),
                    )]),
                ));
            let mut spec = MobBootstrapSpec::ephemeral_runtime_backed_inner(
                definition,
                meerkat_mob::MobStorage::in_memory(),
                dir.path().to_path_buf(),
                4,
                None,
                None,
                None,
                None,
                CapabilityFlags::default(),
                None,
                None,
            )
            .expect("bootstrap spec");
            spec.options.default_llm_client = Some(Arc::new(
                meerkat_client::TestClient::for_provider(meerkat_core::Provider::OpenAI),
            ));
            let runtime = MobRuntime::bootstrap(spec).await.expect("bootstrap");
            let refused = Box::pin(runtime.handle().spawn_spec(SpawnMemberSpec::host_root(
                "worker",
                meerkat_mob::AgentIdentity::from("reader"),
            )))
            .await
            .map(|_| ())
            .expect_err("an unresolved marked descriptor refuses the member build");
            let detail = format!("{refused} {refused:?}");
            assert!(detail.contains("document MCP"), "{marker}: {detail}");
            runtime.handle().shutdown().await.expect("shutdown");
        }
    }

    /// The same refusal on a spec over a session service composed outside the
    /// stock constructors, as the stock gateways build theirs: its base layer
    /// refuses, with or without the extension-state feature.
    #[tokio::test]
    async fn extension_unresolved_document_marker_refuses_builds_on_a_supplied_session_service() {
        for marker in ["META_MCP_DOCUMENT_BINDING", "MOBKIT_MCP_DOCUMENT_BINDING"] {
            let dir = tempfile::tempdir().expect("temporary store");
            let mut definition = meerkat_mob::MobDefinition::from_toml(&format!(
                "[mob]\nid = \"{}\"\n\n[profiles.worker]\nmodel = \"gpt-5.5\"\n\
                 runtime_mode = \"turn_driven\"\n[profiles.worker.tools]\ncomms = true\n",
                unique_test_mob_id()
            ))
            .expect("definition");
            let Some(meerkat_mob::ProfileBinding::Inline(profile)) = definition
                .profiles
                .get_mut(&meerkat_mob::ProfileName::from("worker"))
            else {
                panic!("the definition declares an inline worker profile");
            };
            profile
                .tools
                .mcp_servers
                .push(meerkat_core::McpServerConfig::stdio(
                    "documents",
                    "document-fixture-mcp",
                    vec![],
                    std::collections::HashMap::from([(
                        marker.to_string(),
                        "documents".to_string(),
                    )]),
                ));
            let factory = AgentFactory::new(dir.path()).builtins(false).comms(true);
            let builder = FactoryAgentBuilder::new(factory, Config::default());
            let session_service: Arc<dyn MobSessionService> =
                Arc::new(meerkat_session::EphemeralSessionService::new(builder, 4));
            let mut spec = MobBootstrapSpec::new(
                definition,
                meerkat_mob::MobStorage::in_memory(),
                session_service,
            );
            spec.options.default_llm_client = Some(Arc::new(
                meerkat_client::TestClient::for_provider(meerkat_core::Provider::OpenAI),
            ));
            let runtime = MobRuntime::bootstrap(spec).await.expect("bootstrap");
            let refused = Box::pin(runtime.handle().spawn_spec(SpawnMemberSpec::host_root(
                "worker",
                meerkat_mob::AgentIdentity::from("reader"),
            )))
            .await
            .map(|_| ())
            .expect_err("an unresolved marked descriptor refuses the member build");
            let detail = format!("{refused} {refused:?}");
            assert!(detail.contains("document MCP"), "{marker}: {detail}");
            runtime.handle().shutdown().await.expect("shutdown");
        }
    }

    #[tokio::test]
    async fn agent_mob_tools_expose_definition_profiles_as_realm_profiles() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let store_path = dir.path().to_path_buf();
        let Ok(definition) = meerkat_mob::MobDefinition::from_toml(&format!(
            "[mob]\nid = \"{}\"\n\n[profiles.investigation-worker]\nmodel = \"gpt-5.5\"\n[profiles.investigation-worker.tools]\ncomms = true\nmob = true\n\n[profiles.person-worker]\nmodel = \"gpt-5.5\"\n[profiles.person-worker.tools]\ncomms = true\n",
            unique_test_mob_id()
        )) else {
            panic!("failed to parse mob definition with worker profiles");
        };

        let spec = MobBootstrapSpec::ephemeral_runtime_backed_inner(
            definition,
            meerkat_mob::MobStorage::in_memory(),
            store_path,
            4,
            None,
            None,
            None,
            None,
            CapabilityFlags::default(),
            None,
            None,
        )
        .expect("bootstrap spec");
        let state = spec
            .agent_mob_mcp_state
            .expect("agent mob MCP state should be installed");

        let profiles = state
            .realm_profile_list()
            .await
            .expect("definition profiles should list through agent mob tools");
        let names = profiles
            .iter()
            .map(|profile| profile.name.as_str())
            .collect::<Vec<_>>();
        assert!(
            names.contains(&"investigation-worker"),
            "definition profiles must be visible to mob_profile_list so agents can create mobs that reference them"
        );
        assert!(names.contains(&"person-worker"));

        let worker = state
            .realm_profile_get("investigation-worker")
            .await
            .expect("definition profile lookup should succeed")
            .expect("definition profile should exist");
        assert_eq!(worker.profile.model, "gpt-5.5");
        assert_eq!(
            worker.revision, 0,
            "definition-backed profiles are immutable runtime seeds, not persisted realm revisions"
        );
    }

    #[tokio::test]
    async fn agent_created_mobs_can_spawn_definition_seeded_realm_profiles() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let store_path = dir.path().to_path_buf();
        let Ok(parent_definition) = meerkat_mob::MobDefinition::from_toml(
            "[mob]\nid = \"parent\"\n\n[profiles.investigation-worker]\nmodel = \"gpt-5.5\"\n[profiles.investigation-worker.tools]\ncomms = true\nmob = true\n",
        ) else {
            panic!("failed to parse parent mob definition");
        };

        let mut spec = MobBootstrapSpec::ephemeral_runtime_backed_inner(
            parent_definition,
            meerkat_mob::MobStorage::in_memory(),
            store_path,
            4,
            None,
            None,
            None,
            None,
            CapabilityFlags::default(),
            None,
            None,
        )
        .expect("bootstrap spec");
        // `TestClient::default()` DOES synthesize accounting under 0.8.22, but
        // under `Provider::Other` - and every profile in these tests is
        // `gpt-5.5`, whose canonical owner is `Provider::OpenAI`, so the spawn
        // turn would fail closed with
        // `normalized_provider_accounting_identity_mismatch`. See the rule in
        // `tests/support/llm_usage.rs`.
        spec.options.default_llm_client = Some(Arc::new(meerkat_client::TestClient::for_provider(
            meerkat_core::Provider::OpenAI,
        )));
        let runtime = MobRuntime::bootstrap(spec)
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        let state = runtime
            .agent_mob_mcp_state
            .clone()
            .expect("agent mob MCP state should be installed");
        let Ok(child_definition) = meerkat_mob::MobDefinition::from_toml(
            "[mob]\nid = \"child\"\n\n[profiles.investigation-worker]\nrealm_profile = \"investigation-worker\"\n",
        ) else {
            panic!("failed to parse child mob definition");
        };

        let mob_id = Box::pin(state.mob_create_definition(child_definition))
            .await
            .expect("child mob should be created");
        Box::pin(state.mob_spawn_spec(
            &mob_id,
            SpawnMemberSpec::new(
                ProfileName::from("investigation-worker"),
                // meerkat 0.7: MemberCommsName is fail-closed; raw mob
                // member ids must be identifier-safe (no ":").
                meerkat_mob::AgentIdentity::from("investigation-worker-one"),
            ),
        ))
        .await
        .expect("created mob should resolve definition-seeded realm profile at spawn time");
    }

    /// Regression: public ephemeral builds without image generation retain the
    /// exact runtime authority carrying their live LLM reconfiguration host.
    #[test]
    fn ephemeral_bootstrap_without_image_generation_retains_runtime_host() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let store_path = dir.path().to_path_buf();
        let Ok(definition) = meerkat_mob::MobDefinition::from_toml(&format!(
            "[mob]\nid = \"{}\"\n\n[profiles.worker]\nmodel = \"gpt-5.5\"\nruntime_mode = \"autonomous_host\"\n[profiles.worker.tools]\ncomms = true\n",
            unique_test_mob_id()
        )) else {
            panic!("failed to parse definition");
        };
        let spec = MobBootstrapSpec::ephemeral_inner(
            definition,
            meerkat_mob::MobStorage::in_memory(),
            store_path,
            4,
            None,
            None,
            CapabilityFlags::default(),
            None,
            None,
            None,
        )
        .expect("bootstrap spec");
        let spec_adapter = spec
            .runtime_adapter
            .as_ref()
            .unwrap_or_else(|| panic!("public ephemeral builds must retain their runtime adapter"));
        let service_adapter = spec
            .session_service
            .acquire_runtime_adapter(None)
            .expect("acquire runtime adapter")
            .unwrap_or_else(|| panic!("session service must expose the retained runtime adapter"));
        assert!(
            Arc::ptr_eq(spec_adapter, &service_adapter),
            "the spec and session service must share one exact runtime authority"
        );
        assert!(
            spec_adapter.has_session_llm_reconfigure_host(),
            "the retained runtime authority must carry the live LLM reconfiguration host"
        );
    }

    /// Ordinary model/provider declarations are not operator resume authority.
    /// Provider params retain the existing declaration-driven behavior.
    #[test]
    fn auto_mark_does_not_synthesize_model_provider_authority() {
        let Ok(mut definition) = meerkat_mob::MobDefinition::from_toml(
            "[mob]\nid = \"auto-mark\"\n\n[profiles.worker]\nmodel = \"claude-opus-4-8\"\nprovider = \"anthropic\"\n[profiles.worker.provider_params]\nthinking_budget_tokens = 1024\n",
        ) else {
            panic!("failed to parse definition");
        };
        auto_mark_declared_resume_overrides(&mut definition);
        let profile = definition
            .profiles
            .get(&ProfileName::from("worker"))
            .and_then(|binding| binding.as_inline())
            .unwrap_or_else(|| panic!("worker profile must be inline"));
        for field in [
            meerkat_mob::ResumeOverrideField::Model,
            meerkat_mob::ResumeOverrideField::Provider,
        ] {
            assert!(
                !profile.resume_overrides.contains(&field),
                "ordinary field {field:?} must keep durable-wins semantics"
            );
        }
        assert!(
            profile
                .resume_overrides
                .contains(&meerkat_mob::ResumeOverrideField::ProviderParams),
            "declared provider params retain their existing override behavior"
        );
    }

    /// A profile without explicit resume authority must not gain a synthetic
    /// provider or mask merely because its model is catalog-known.
    #[test]
    fn auto_mark_leaves_an_ordinary_catalog_model_durable_wins() {
        let Ok(mut definition) = meerkat_mob::MobDefinition::from_toml(
            "[mob]\nid = \"auto-mark\"\n\n[profiles.worker]\nmodel = \"gpt-5.5\"\n",
        ) else {
            panic!("failed to parse definition");
        };
        auto_mark_declared_resume_overrides(&mut definition);
        let profile = definition
            .profiles
            .get(&ProfileName::from("worker"))
            .and_then(|binding| binding.as_inline())
            .unwrap_or_else(|| panic!("worker profile must be inline"));
        assert_eq!(
            profile.provider, None,
            "ordinary profile normalization must not mint a provider declaration"
        );
        assert!(
            !profile
                .resume_overrides
                .contains(&meerkat_mob::ResumeOverrideField::Model)
                && !profile
                    .resume_overrides
                    .contains(&meerkat_mob::ResumeOverrideField::Provider),
            "ordinary model/provider declarations must not override durable routing"
        );
        assert!(
            !profile
                .resume_overrides
                .contains(&meerkat_mob::ResumeOverrideField::ProviderParams),
            "undeclared provider_params must keep durable-wins"
        );
    }

    /// The production incident shape must be impossible: an explicit
    /// `resume_overrides = ["model", "provider"]` with NO provider key used
    /// to apply the profile model while the durable provider survived
    /// (profile provider was None → nothing to apply), minting invalid
    /// pairs like (claude-fable-5, openai). The auto-mark now writes the
    /// catalog-derived provider onto the profile so the pair applies
    /// atomically.
    #[test]
    fn auto_mark_completes_the_pair_for_explicit_model_provider_mask() {
        let Ok(mut definition) = meerkat_mob::MobDefinition::from_toml(
            "[mob]\nid = \"auto-mark\"\n\n[profiles.worker]\nmodel = \"claude-opus-4-8\"\nresume_overrides = [\"model\", \"provider\"]\n",
        ) else {
            panic!("failed to parse definition");
        };
        auto_mark_declared_resume_overrides(&mut definition);
        let profile = definition
            .profiles
            .get(&ProfileName::from("worker"))
            .and_then(|binding| binding.as_inline())
            .unwrap_or_else(|| panic!("worker profile must be inline"));
        assert_eq!(
            profile.provider,
            Some(Provider::Anthropic),
            "an explicit model+provider mask with no provider key must gain the catalog \
             provider, or the mask applies the model against the durable provider"
        );
        assert_eq!(
            profile
                .resume_overrides
                .iter()
                .filter(|field| **field == meerkat_mob::ResumeOverrideField::Provider)
                .count(),
            1,
            "the explicit mask entry must not be duplicated"
        );
    }

    /// Without an explicit model/provider override, neither catalog nor
    /// definition model entries synthesize one. Provider params remain
    /// independently declaration-driven.
    #[test]
    fn auto_mark_unknown_model_uses_config_entry_or_stays_durable_wins() {
        let Ok(mut definition) = meerkat_mob::MobDefinition::from_toml(
            "[mob]\nid = \"auto-mark\"\n\n[models.house-llm]\nprovider = \"openai\"\n\n[profiles.custom]\nmodel = \"house-llm\"\n\n[profiles.orphan]\nmodel = \"nobody-knows-this-model\"\n[profiles.orphan.provider_params]\nthinking_budget_tokens = 64\n",
        ) else {
            panic!("failed to parse definition");
        };
        auto_mark_declared_resume_overrides(&mut definition);
        let custom = definition
            .profiles
            .get(&ProfileName::from("custom"))
            .and_then(|binding| binding.as_inline())
            .unwrap_or_else(|| panic!("custom profile must be inline"));
        assert_eq!(
            custom.provider, None,
            "a model registry entry must not mint operator resume authority"
        );
        assert!(
            !custom
                .resume_overrides
                .contains(&meerkat_mob::ResumeOverrideField::Model)
                && !custom
                    .resume_overrides
                    .contains(&meerkat_mob::ResumeOverrideField::Provider),
            "ordinary config-entry models keep durable-wins semantics"
        );
        let orphan = definition
            .profiles
            .get(&ProfileName::from("orphan"))
            .and_then(|binding| binding.as_inline())
            .unwrap_or_else(|| panic!("orphan profile must be inline"));
        assert_eq!(
            orphan.provider, None,
            "no coherent provider source: the profile must not gain one"
        );
        assert!(
            !orphan
                .resume_overrides
                .contains(&meerkat_mob::ResumeOverrideField::Model)
                && !orphan
                    .resume_overrides
                    .contains(&meerkat_mob::ResumeOverrideField::Provider),
            "without a coherent pair neither field is masked — durable truth wins whole"
        );
        assert!(
            orphan
                .resume_overrides
                .contains(&meerkat_mob::ResumeOverrideField::ProviderParams),
            "provider_params declaration is independent of the LLM-identity pair"
        );
    }

    /// Declaring both fields is still not the same as explicitly placing them
    /// in `resume_overrides`.
    #[test]
    fn auto_mark_leaves_an_unmasked_declared_pair_durable_wins() {
        let Ok(mut definition) = meerkat_mob::MobDefinition::from_toml(
            "[mob]\nid = \"auto-mark\"\n\n[profiles.worker]\nmodel = \"claude-opus-4-8\"\nprovider = \"openai\"\n",
        ) else {
            panic!("failed to parse definition");
        };
        auto_mark_declared_resume_overrides(&mut definition);
        let profile = definition
            .profiles
            .get(&ProfileName::from("worker"))
            .and_then(|binding| binding.as_inline())
            .unwrap_or_else(|| panic!("worker profile must be inline"));
        assert_eq!(
            profile.provider,
            Some(Provider::OpenAI),
            "a declared provider is honored as written, never catalog-corrected"
        );
        assert!(
            !profile
                .resume_overrides
                .contains(&meerkat_mob::ResumeOverrideField::Model)
                && !profile
                    .resume_overrides
                    .contains(&meerkat_mob::ResumeOverrideField::Provider),
            "declared fields alone must not become operator resume authority"
        );
    }

    /// A self-hosted binding also stays unmasked without explicit operator
    /// resume authority.
    #[test]
    fn auto_mark_leaves_an_ordinary_self_hosted_binding_durable_wins() {
        let Ok(mut definition) = meerkat_mob::MobDefinition::from_toml(
            "[mob]\nid = \"auto-mark\"\n\n[profiles.worker]\nmodel = \"house-llm\"\nself_hosted_server_id = \"srv-1\"\n",
        ) else {
            panic!("failed to parse definition");
        };
        auto_mark_declared_resume_overrides(&mut definition);
        let profile = definition
            .profiles
            .get(&ProfileName::from("worker"))
            .and_then(|binding| binding.as_inline())
            .unwrap_or_else(|| panic!("worker profile must be inline"));
        assert_eq!(
            profile.provider, None,
            "ordinary normalization must not mint a provider declaration"
        );
        assert!(
            !profile
                .resume_overrides
                .contains(&meerkat_mob::ResumeOverrideField::Model)
                && !profile
                    .resume_overrides
                    .contains(&meerkat_mob::ResumeOverrideField::Provider),
            "ordinary self-hosted identity must remain durable-wins"
        );
    }

    /// An explicit `resume_overrides = ["provider"]` with no provider key is
    /// the mirror of the production shape: a provider-only mask over a None profile
    /// provider applies NOTHING (resume falls back to durable), while the
    /// declared model stays unmasked. Auto-mark completes it to the full
    /// pair — provider derived from the catalog, model added to the mask —
    /// so a provider-only application is structurally impossible.
    #[test]
    fn auto_mark_completes_explicit_provider_only_mask_to_the_pair() {
        let Ok(mut definition) = meerkat_mob::MobDefinition::from_toml(
            "[mob]\nid = \"auto-mark\"\n\n[profiles.worker]\nmodel = \"gpt-5.5\"\nresume_overrides = [\"provider\"]\n",
        ) else {
            panic!("failed to parse definition");
        };
        auto_mark_declared_resume_overrides(&mut definition);
        let profile = definition
            .profiles
            .get(&ProfileName::from("worker"))
            .and_then(|binding| binding.as_inline())
            .unwrap_or_else(|| panic!("worker profile must be inline"));
        assert_eq!(
            profile.provider,
            Some(Provider::OpenAI),
            "the masked provider must exist on the profile, derived from the declared model"
        );
        assert!(
            profile
                .resume_overrides
                .contains(&meerkat_mob::ResumeOverrideField::Model),
            "the model joins the explicit provider mask: the pair is never split"
        );
        assert_eq!(
            profile
                .resume_overrides
                .iter()
                .filter(|field| **field == meerkat_mob::ResumeOverrideField::Provider)
                .count(),
            1,
            "the explicit provider entry must not be duplicated"
        );
    }

    /// An explicit `resume_overrides` list is preserved, declared fields are
    /// added without duplicates, and the pass is idempotent across boots.
    #[test]
    fn auto_mark_mixed_preserves_explicit_list_without_duplicates() {
        let Ok(mut definition) = meerkat_mob::MobDefinition::from_toml(
            "[mob]\nid = \"auto-mark\"\n\n[profiles.worker]\nmodel = \"gpt-5.5\"\nprovider = \"openai\"\nresume_overrides = [\"model\"]\n",
        ) else {
            panic!("failed to parse definition");
        };
        auto_mark_declared_resume_overrides(&mut definition);
        auto_mark_declared_resume_overrides(&mut definition);
        let profile = definition
            .profiles
            .get(&ProfileName::from("worker"))
            .and_then(|binding| binding.as_inline())
            .unwrap_or_else(|| panic!("worker profile must be inline"));
        let model_entries = profile
            .resume_overrides
            .iter()
            .filter(|field| **field == meerkat_mob::ResumeOverrideField::Model)
            .count();
        assert_eq!(
            model_entries, 1,
            "an explicitly listed field must not be duplicated"
        );
        assert!(
            profile
                .resume_overrides
                .contains(&meerkat_mob::ResumeOverrideField::Provider),
            "declared provider must be added alongside the explicit list"
        );
        assert!(
            !profile
                .resume_overrides
                .contains(&meerkat_mob::ResumeOverrideField::ProviderParams),
            "undeclared provider_params must stay durable-wins"
        );
    }

    /// The single bootstrap ingress must preserve the distinction between an
    /// ordinary profile and an explicit operator resume override.
    #[tokio::test]
    async fn bootstrap_does_not_synthesize_model_provider_resume_overrides() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let store_path = dir.path().to_path_buf();
        let Ok(definition) = meerkat_mob::MobDefinition::from_toml(
            "[mob]\nid = \"auto-mark\"\n\n[profiles.worker]\nmodel = \"claude-opus-4-8\"\nprovider = \"anthropic\"\n",
        ) else {
            panic!("failed to parse definition");
        };
        let spec = MobBootstrapSpec::ephemeral_inner(
            definition,
            meerkat_mob::MobStorage::in_memory(),
            store_path,
            4,
            None,
            None,
            CapabilityFlags::default(),
            None,
            None,
            None,
        )
        .expect("bootstrap spec");
        let runtime = MobRuntime::bootstrap(spec)
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        let profile = runtime
            .handle
            .definition()
            .profiles
            .get(&ProfileName::from("worker"))
            .and_then(|binding| binding.as_inline())
            .unwrap_or_else(|| panic!("worker profile must be inline"));
        assert!(
            !profile
                .resume_overrides
                .contains(&meerkat_mob::ResumeOverrideField::Model)
                && !profile
                    .resume_overrides
                    .contains(&meerkat_mob::ResumeOverrideField::Provider),
            "bootstrap must not turn ordinary profile fields into operator authority"
        );
        runtime
            .handle
            .shutdown()
            .await
            .unwrap_or_else(|e| panic!("failed to quiesce runtime: {e}"));
    }

    /// Regression: public ephemeral image-generation builds must expose the same
    /// runtime adapter through the spec and the session service. The generated
    /// image tool consults runtime session/image-operation state by session id,
    /// so a fresh, tool-only MeerkatMachine cannot be used here.
    #[test]
    fn ephemeral_bootstrap_with_image_generation_shares_runtime_adapter() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let store_path = dir.path().to_path_buf();
        let Ok(definition) = meerkat_mob::MobDefinition::from_toml(
            r#"
[mob]
id = "test"

[profiles.commander]
model = "gpt-5.5"

[profiles.commander.tools]
builtins = true
image_generation = true
"#,
        ) else {
            panic!("failed to parse image-generation definition");
        };
        let spec = MobBootstrapSpec::ephemeral(
            definition,
            meerkat_mob::MobStorage::in_memory(),
            store_path,
            4,
            None,
        )
        .expect("bootstrap spec");
        let spec_adapter = spec
            .runtime_adapter
            .as_ref()
            .expect("image-generation ephemeral builds must expose a runtime adapter");
        assert!(
            !spec_adapter.has_runtime_persistence(),
            "image-only ephemeral mode must not inject a store-backed machine into its service"
        );
        let service_adapter = spec
            .session_service
            .acquire_runtime_adapter(None)
            .expect("acquire runtime adapter")
            .expect("session service must expose the same runtime adapter");
        assert!(
            spec_adapter.shares_runtime_execution_owner_with(&service_adapter),
            "image-generation tool state and session state must share one runtime authority"
        );
    }

    /// Runtime-owned handling/routing semantics must be stripped before a
    /// runtime-applied turn reaches the direct session-service path.
    #[test]
    fn normalize_runtime_turn_request_strips_runtime_owned_semantics() {
        let req = meerkat_core::service::StartTurnRequest {
            prompt: meerkat_core::ContentInput::Text("checkpoint".to_string()),
            injected_context: Vec::new(),
            system_prompt: Some("system".to_string()),
            event_tx: None,
            runtime: meerkat_core::service::StartTurnRuntimeSemantics {
                work_authorization: None,
                input_identity: None,
                handling_mode: meerkat_core::types::HandlingMode::Steer,
                turn_tool_overlay: None,
                typed_turn_appends: Vec::new(),
                // Render metadata now lives only on the typed turn-metadata
                // carrier (meerkat 0.7).
                turn_metadata: Some(
                    meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                        render_metadata: Some(meerkat_core::types::RenderMetadata {
                            class: meerkat_core::types::RenderClass::OpsProgress,
                            salience: meerkat_core::types::RenderSalience::Urgent,
                        }),
                        ..Default::default()
                    },
                ),
            },
        };

        let expected_prompt = req.prompt.clone();
        let expected_system_prompt = req.system_prompt.clone();

        let normalized = normalize_runtime_turn_request(req);

        assert_eq!(
            normalized.runtime.handling_mode,
            meerkat_core::types::HandlingMode::Queue,
            "runtime-applied turns must downgrade Steer before reaching direct session services"
        );
        assert!(
            normalized
                .runtime
                .turn_metadata
                .as_ref()
                .is_none_or(|metadata| metadata.render_metadata.is_none()),
            "runtime-owned render metadata must not be forwarded through the direct agent path"
        );
        assert_eq!(normalized.prompt, expected_prompt);
        assert_eq!(normalized.system_prompt, expected_system_prompt);
    }

    /// SessionCreatedContext must carry model, labels, and optional system_prompt.
    #[test]
    fn session_created_context_fields() {
        let ctx = SessionCreatedContext {
            model: "claude-sonnet-4-5".to_string(),
            labels: std::collections::BTreeMap::from([(
                "agent_type".to_string(),
                "lead".to_string(),
            )]),
            system_prompt: Some("You are a lead agent.".to_string()),
        };
        assert_eq!(ctx.model, "claude-sonnet-4-5");
        assert_eq!(ctx.labels["agent_type"], "lead");
        assert_eq!(ctx.system_prompt.as_deref(), Some("You are a lead agent."));
    }

    /// SessionHook default implementations are no-ops — calling them must not panic.
    #[tokio::test]
    async fn session_hook_default_impls_are_noop() {
        struct EmptyHook;
        #[async_trait]
        impl SessionHook for EmptyHook {}

        let hook = EmptyHook;
        let mut req = CreateSessionRequest {
            model: "test".to_string(),
            prompt: meerkat_core::ContentInput::Text("test".to_string()),
            system_prompt: meerkat_core::config::SystemPromptOverride::Inherit,
            max_tokens: None,
            event_tx: None,
            initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
            build: None,
            labels: None,
            deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::default(),
            injected_context: Vec::new(),
        };
        // before_create must succeed with default impl.
        hook.before_create(&mut req).await.unwrap();
        // after_create must not panic.
        let ctx = SessionCreatedContext {
            model: "test".to_string(),
            labels: Default::default(),
            system_prompt: None,
        };
        hook.after_create(&meerkat_core::types::SessionId::new(), &ctx)
            .await;
    }

    /// before_create returning Err must abort (the caller decides how).
    #[tokio::test]
    async fn session_hook_before_create_can_abort() {
        struct AbortHook;
        #[async_trait]
        impl SessionHook for AbortHook {
            async fn before_create(
                &self,
                _req: &mut CreateSessionRequest,
            ) -> Result<(), SessionError> {
                Err(SessionError::Unsupported("hook abort".into()))
            }
        }

        let hook = AbortHook;
        let mut req = CreateSessionRequest {
            model: "test".to_string(),
            prompt: meerkat_core::ContentInput::Text("test".to_string()),
            system_prompt: meerkat_core::config::SystemPromptOverride::Inherit,
            max_tokens: None,
            event_tx: None,
            initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
            build: None,
            labels: None,
            deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::default(),
            injected_context: Vec::new(),
        };
        let result = hook.before_create(&mut req).await;
        assert!(result.is_err());
    }

    /// before_create mutations must be visible in the request.
    #[tokio::test]
    async fn session_hook_before_create_mutates_request() {
        struct MutatingHook;
        #[async_trait]
        impl SessionHook for MutatingHook {
            async fn before_create(
                &self,
                req: &mut CreateSessionRequest,
            ) -> Result<(), SessionError> {
                req.model = "hook-overridden".to_string();
                req.system_prompt =
                    meerkat_core::config::SystemPromptOverride::Set("injected by hook".to_string());
                Ok(())
            }
        }

        let hook = MutatingHook;
        let mut req = CreateSessionRequest {
            model: "original".to_string(),
            prompt: meerkat_core::ContentInput::Text("test".to_string()),
            system_prompt: meerkat_core::config::SystemPromptOverride::Inherit,
            max_tokens: None,
            event_tx: None,
            initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
            build: None,
            labels: None,
            deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::default(),
            injected_context: Vec::new(),
        };
        hook.before_create(&mut req).await.unwrap();
        assert_eq!(req.model, "hook-overridden");
        assert_eq!(req.system_prompt.as_set_prompt(), Some("injected by hook"));
    }

    #[test]
    fn recoverable_lifecycle_cleanup_accepts_ambiguous_member_cleanup() {
        let error = "previous member cleanup ambiguous for member rt:deep-investigator:singleton:0";

        assert!(is_previous_member_cleanup_ambiguous_error(error));
        assert!(is_recoverable_lifecycle_cleanup_error(error));
    }

    #[test]
    fn topology_restore_failed_peer_ids_extracts_tolerated_peers() {
        let identity = meerkat_mob::AgentIdentity::from("rt:review:singleton:0");
        let receipt = meerkat_mob::MemberRespawnReceipt::new(
            identity.clone(),
            meerkat_mob::AgentRuntimeId::new(identity, meerkat_mob::ids::Generation::INITIAL),
            meerkat_mob::FenceToken::new(1),
            meerkat_mob::FenceToken::new(2),
        );
        let err = meerkat_mob::MobRespawnError::TopologyRestoreFailed {
            receipt,
            failed_peer_ids: vec![meerkat_mob::RespawnTopologyPeerId::from(
                "initiative:broken",
            )],
        };

        assert_eq!(
            topology_restore_failed_peer_ids(&err),
            Some(vec!["initiative:broken".to_string()])
        );
        assert_eq!(
            topology_restore_failed_peer_ids(&meerkat_mob::MobRespawnError::NoRuntimeControl {
                identity: meerkat_mob::AgentIdentity::from("rt:review:singleton:0"),
            }),
            None
        );
    }

    #[test]
    fn topology_restore_warning_json_surfaces_isolated_peers() {
        let failed_peer_ids = vec!["initiative:broken".to_string(), "helper:cold".to_string()];

        assert_eq!(
            topology_restore_warning_json(&failed_peer_ids),
            serde_json::json!({
                "kind": "topology_restore_degraded",
                "failed_peer_ids": ["initiative:broken", "helper:cold"],
            })
        );
    }

    #[test]
    fn recoverable_lifecycle_cleanup_preserves_archive_cancel_race() {
        let error = "internal error: disposal completed but ArchiveSession failed: \
            session error: agent error: Internal error: runtime cancel-before-retire failed \
            for 019e3c52-0f1b-73d3-a5c7-4b21c2bbf131: Runtime not ready: running";

        assert!(is_recoverable_lifecycle_cleanup_error(error));
    }

    #[test]
    fn recoverable_lifecycle_cleanup_rejects_unrelated_errors() {
        assert!(!is_recoverable_lifecycle_cleanup_error(
            "actor task dropped"
        ));
        assert!(!is_recoverable_lifecycle_cleanup_error(
            "model provider returned rate limit"
        ));
    }

    /// Regression (meerkat 0.7.1): idle members sit in machine state
    /// `Stopped`, whose DSL authority rejects the archive step's final
    /// `Retire` input. Disposal completed, so retire/respawn must treat the
    /// failed bookkeeping transition as success instead of surfacing -32000.
    #[test]
    fn recoverable_lifecycle_cleanup_accepts_stopped_guard_archive_retire() {
        let error = "internal error: disposal completed but ArchiveSession failed: \
            session error: agent error: Internal error: machine archive retire failed \
            after registration: Internal error: DSL authority (Retire): guard rejected \
            transition from Stopped for input::Retire";

        assert!(is_recoverable_lifecycle_cleanup_error(error));
    }

    /// Regression (meerkat 0.7.1): retire now performs a final fenced
    /// continuity save. Identity-first reset/delete advance or remove the
    /// meta-owned continuity record before retiring the old generation, so
    /// that save fails fail-closed with "record not found" / "stale fencing
    /// token" — both must be recoverable cleanup, not reset/delete failures.
    #[test]
    fn recoverable_lifecycle_cleanup_accepts_stale_continuity_save_on_retire() {
        let record_gone = "internal error: disposal completed but ArchiveSession failed: \
            session error: agent error: Internal error: continuity save: \
            continuity record not found for identity:luka";
        let stale_fence = "internal error: disposal completed but ArchiveSession failed: \
            session error: agent error: Internal error: continuity save: \
            stale fencing token for identity:luka: presented 1, current 6";

        assert!(is_recoverable_lifecycle_cleanup_error(record_gone));
        assert!(is_recoverable_lifecycle_cleanup_error(stale_fence));
    }

    /// Regression (meerkat 0.7.1): an idle member's session machine sits in
    /// `Stopped`; the archive protocol commits the durable document first,
    /// then fails its runtime `Retire` realization on the `Stopped` guard.
    /// The session-service wrapper tolerates exactly this signature so
    /// member retire/respawn disposal completes (meerkat-mob's own archive
    /// helper treats `Stopped` as already-retired).
    #[test]
    fn stopped_session_archive_retire_rejection_matches_only_stopped_guard() {
        assert!(is_stopped_session_archive_retire_rejection(
            "session error: agent error: Internal error: machine archive retire failed \
             after registration: Internal error: DSL authority (Retire): guard rejected \
             transition from Stopped for input::Retire"
        ));
        // The pre-registration variant carries the same meaning.
        assert!(is_stopped_session_archive_retire_rejection(
            "agent error: Internal error: machine archive retire failed: Internal error: \
             DSL authority (Retire): guard rejected transition from Stopped for input::Retire"
        ));
        // Other guard rejections and other retire failures stay fail-closed.
        assert!(!is_stopped_session_archive_retire_rejection(
            "machine archive retire failed after registration: Internal error: \
             DSL authority (Retire): guard rejected transition from Running for input::Retire"
        ));
        assert!(!is_stopped_session_archive_retire_rejection(
            "guard rejected transition from Stopped for input::Retire"
        ));
        assert!(!is_stopped_session_archive_retire_rejection(
            "machine archive retire failed: store unavailable"
        ));
    }

    /// The new arms must stay scoped to completed disposals: the same inner
    /// failures without the "disposal completed" prefix (e.g. a continuity
    /// save failing mid-delivery) are real errors.
    #[test]
    fn recoverable_lifecycle_cleanup_requires_completed_disposal() {
        assert!(!is_recoverable_lifecycle_cleanup_error(
            "continuity save: continuity record not found for identity:luka"
        ));
        assert!(!is_recoverable_lifecycle_cleanup_error(
            "DSL authority (Retire): guard rejected transition from Stopped for input::Retire"
        ));
        assert!(!is_recoverable_lifecycle_cleanup_error(
            "disposal aborted at ArchiveSession: continuity save: stale fencing token"
        ));
    }

    /// The mob-member lifecycle gate stays fail-closed on an archive NotFound
    /// after a completed disposal: a spawned member whose archive authority
    /// lost its record is a real orphan.
    #[test]
    fn mob_member_lifecycle_gate_still_rejects_archive_not_found() {
        let error = "internal error: disposal completed but ArchiveSession failed: \
            session error: agent error: Internal error: mob archive authority returned \
            NotFound for registered runtime session 019ee136-33bc-7bc3-80f9-2aac38736291";

        assert!(!is_recoverable_lifecycle_cleanup_error(error));
    }

    mod console_spawn_projection {
        use super::*;
        use crate::console_spawn::new_console_spawn_sink_slot;
        use crate::unified_runtime::ConsoleEventStore;
        use meerkat_core::AgentToolDispatcher;

        /// Inner dispatcher standing in for the meerkat-mob-mcp tool surface:
        /// returns a canned payload for any call.
        struct CannedDispatcher {
            payload: Value,
            is_error: bool,
        }

        #[async_trait::async_trait]
        impl meerkat_core::AgentToolDispatcher for CannedDispatcher {
            fn tools(&self) -> Arc<[Arc<meerkat_core::types::ToolDef>]> {
                Vec::<Arc<meerkat_core::types::ToolDef>>::new().into()
            }

            async fn dispatch(
                &self,
                call: meerkat_core::types::ToolCallView<'_>,
            ) -> Result<meerkat_core::ToolDispatchOutcome, meerkat_core::ToolError> {
                Ok(meerkat_core::ToolDispatchOutcome::sync_result(
                    meerkat_core::types::ToolResult::new(
                        call.id.to_string(),
                        self.payload.to_string(),
                        self.is_error,
                    ),
                ))
            }

            fn capabilities(&self) -> meerkat_core::agent::DispatcherCapabilities {
                meerkat_core::agent::DispatcherCapabilities::default()
            }
        }

        fn console_wrapper(
            payload: Value,
            is_error: bool,
            store: Option<&ConsoleEventStore>,
        ) -> AutoWireParentMobToolDispatcher {
            let console_spawn_sink = new_console_spawn_sink_slot();
            if let Some(store) = store {
                *console_spawn_sink
                    .write()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) =
                    Some(ConsoleSpawnSink::new(store.clone()));
            }
            AutoWireParentMobToolDispatcher {
                inner: Arc::new(CannedDispatcher { payload, is_error }),
                implicit_delegate_retirement_overrides:
                    ImplicitDelegateRetirementOverrides::default(),
                console_spawn_sink,
                identity_runtime: Arc::new(std::sync::RwLock::new(None)),
                protected_mob_id: "test-mob".to_string(),
                spawner_comms_name: Some("ops/orchestrator/ops-lead".to_string()),
            }
        }

        async fn dispatch_tool(
            dispatcher: &AutoWireParentMobToolDispatcher,
            name: &str,
            args: Value,
        ) -> meerkat_core::ToolDispatchOutcome {
            let raw = serde_json::value::RawValue::from_string(args.to_string()).expect("raw args");
            dispatcher
                .dispatch(meerkat_core::types::ToolCallView {
                    id: "call-1",
                    name,
                    args: &raw,
                })
                .await
                .expect("dispatch succeeds")
        }

        async fn kickoff_events(
            store: &ConsoleEventStore,
        ) -> Vec<crate::console_contracts::ConsoleIdentityEventEnvelope> {
            store
                .replay_all(None)
                .await
                .expect("replay")
                .into_iter()
                .filter(|event| event.event_type == "user_input")
                .collect()
        }

        #[tokio::test]
        async fn mob_spawn_member_projects_kickoff_into_console() {
            let store = ConsoleEventStore::new();
            let dispatcher = console_wrapper(
                serde_json::json!({
                    "agent_identity": "worker-3",
                    "member_ref": "opaque-ref"
                }),
                false,
                Some(&store),
            );

            let outcome = dispatch_tool(
                &dispatcher,
                "mob_spawn_member",
                serde_json::json!({
                    "mob_id": "ops",
                    "profile": "person-worker",
                    "member_id": "worker-3",
                    "initial_message": "Find the person",
                    "labels": { "group": "workers" }
                }),
            )
            .await;
            assert!(!outcome.result.is_error);

            let kickoffs = kickoff_events(&store).await;
            assert_eq!(kickoffs.len(), 1, "spawn must project one kickoff");
            let kickoff = &kickoffs[0];
            assert_eq!(kickoff.identity, "worker-3");
            assert!(kickoff.event_id.starts_with("spawn-kickoff:ops:worker-3:"));
            assert_eq!(kickoff.data["content"][0]["text"], "Find the person");
            assert_eq!(kickoff.data["via_tool"], "mob_spawn_member");
            assert_eq!(kickoff.data["parent_identity"], "ops-lead");

            let labels = store
                .identity_labels("worker-3")
                .await
                .expect("spawn registers console identity labels");
            assert_eq!(labels.get("group").map(String::as_str), Some("workers"));
            assert_eq!(
                labels.get("spawned_by").map(String::as_str),
                Some("ops-lead")
            );
        }

        #[tokio::test]
        async fn repeated_spawn_dispatch_keeps_one_kickoff() {
            let store = ConsoleEventStore::new();
            let dispatcher = console_wrapper(
                serde_json::json!({ "agent_identity": "worker-3" }),
                false,
                Some(&store),
            );
            let args = serde_json::json!({
                "mob_id": "ops",
                "profile": "person-worker",
                "member_id": "worker-3",
                "initial_message": "Find the person"
            });

            dispatch_tool(&dispatcher, "mob_spawn_member", args.clone()).await;
            dispatch_tool(&dispatcher, "mob_spawn_member", args).await;

            assert_eq!(
                kickoff_events(&store).await.len(),
                1,
                "retry/double-spawn must not duplicate the kickoff frame"
            );
        }

        #[tokio::test]
        async fn delegate_projects_task_kickoff_for_generated_helper() {
            let store = ConsoleEventStore::new();
            let dispatcher = console_wrapper(
                serde_json::json!({
                    "agent_identity": "helper-3f2a",
                    "member_ref": "opaque",
                    "mob_id": "implicit-1",
                    "wired": true
                }),
                false,
                Some(&store),
            );

            dispatch_tool(
                &dispatcher,
                "delegate",
                serde_json::json!({ "task": "Review the diff" }),
            )
            .await;

            let kickoffs = kickoff_events(&store).await;
            assert_eq!(kickoffs.len(), 1);
            assert_eq!(kickoffs[0].identity, "helper-3f2a");
            assert_eq!(kickoffs[0].data["content"][0]["text"], "Review the diff");
            assert_eq!(kickoffs[0].data["via_tool"], "delegate");
        }

        #[tokio::test]
        async fn spawn_without_console_sink_leaves_outcome_unchanged() {
            let dispatcher = console_wrapper(
                serde_json::json!({ "agent_identity": "worker-3" }),
                false,
                None,
            );

            let outcome = dispatch_tool(
                &dispatcher,
                "mob_spawn_member",
                serde_json::json!({
                    "mob_id": "ops",
                    "profile": "person-worker",
                    "member_id": "worker-3",
                    "initial_message": "Find the person"
                }),
            )
            .await;

            assert!(!outcome.result.is_error);
            assert!(
                outcome
                    .result
                    .text_content()
                    .contains("\"agent_identity\":\"worker-3\""),
                "no console store → spawn outcome passes through untouched"
            );
        }

        #[tokio::test]
        async fn failed_spawn_projects_nothing() {
            let store = ConsoleEventStore::new();
            let dispatcher = console_wrapper(
                serde_json::json!({ "error": "spawn rejected" }),
                true,
                Some(&store),
            );

            dispatch_tool(
                &dispatcher,
                "mob_spawn_member",
                serde_json::json!({
                    "mob_id": "ops",
                    "profile": "person-worker",
                    "member_id": "worker-3",
                    "initial_message": "Find the person"
                }),
            )
            .await;

            assert!(
                kickoff_events(&store).await.is_empty(),
                "failed spawns must not seed console chats"
            );
            assert!(store.identity_labels("worker-3").await.is_none());
        }
    }

    pub(super) fn acquisition_observation_probe(
        reject: bool,
        reads: Arc<std::sync::atomic::AtomicUsize>,
    ) -> Arc<dyn MobSessionService> {
        Arc::new(ForwardingProbe {
            acquisition_outcome: std::sync::atomic::AtomicU8::new(if reject { 2 } else { 1 }),
            execution_snapshot_reads: reads,
            ..ForwardingProbe::default()
        })
    }

    pub(super) fn failed_snapshot_probe(
        live: Option<bool>,
        persisted: bool,
    ) -> Arc<dyn MobSessionService> {
        Arc::new(ForwardingProbe {
            failed_snapshot: Mutex::new(Some(FailedSnapshot { live, persisted })),
            // No runtime owner: the snapshot alone decides, as for a member
            // whose machine holds no run.
            acquisition_outcome: std::sync::atomic::AtomicU8::new(1),
            ..ForwardingProbe::default()
        })
    }

    pub(super) fn archive_authority_probe(answer: Option<bool>) -> Arc<dyn MobSessionService> {
        Arc::new(ForwardingProbe {
            archive_authority_answer: Mutex::new(answer.map_or(
                ArchiveAuthorityAnswer::Fails,
                ArchiveAuthorityAnswer::Answers,
            )),
            ..ForwardingProbe::default()
        })
    }

    type AcquisitionFixture = (
        Arc<meerkat_session::PersistentSessionService<FactoryAgentBuilder>>,
        Arc<meerkat_runtime::MeerkatMachine>,
        Arc<dyn meerkat_runtime::RuntimeStore>,
        Arc<dyn meerkat_core::BlobStore>,
        Arc<dyn meerkat::DetachedJobStore>,
    );

    fn acquisition_fixture(path: &Path) -> AcquisitionFixture {
        let store: Arc<dyn meerkat_runtime::RuntimeStore> =
            Arc::new(meerkat_runtime::InMemoryRuntimeStore::new());
        let blobs: Arc<dyn meerkat_core::BlobStore> = Arc::new(Base64BlobStoreAdapter::new(
            Arc::new(ObjectStoreBlobStore::memory()),
        ));
        let machine = Arc::new(
            meerkat_runtime::MeerkatMachine::persistent(store.clone(), blobs.clone())
                .expect("construct selected machine"),
        );
        let jobs: Arc<dyn meerkat::DetachedJobStore> =
            Arc::new(meerkat::MemoryDetachedJobStore::new());
        let mut builder = FactoryAgentBuilder::new(
            AgentFactory::new(path).builtins(false).comms(true),
            Config::default(),
        );
        builder.default_detached_job_store = Some(Arc::clone(&jobs));
        let service = Arc::new(meerkat_session::PersistentSessionService::new(
            builder,
            4,
            Arc::new(meerkat_store::MemoryStore::new()),
            store.clone(),
            blobs.clone(),
        ));
        (service, machine, store, blobs, jobs)
    }

    /// The pre-start getter returns the same selected service the runtime
    /// will use (the same store), and nothing without one.
    #[test]
    fn the_bootstrap_spec_exposes_its_selected_workgraph_service() {
        let dir = tempfile::tempdir().expect("fixture directory");
        let (service, _selected, _store, _blobs, _jobs) = acquisition_fixture(dir.path());
        let spec =
            MobBootstrapSpec::new(acquisition_definition(), MobStorage::in_memory(), service);
        assert!(spec.workgraph_service().is_none());
        let store: Arc<dyn meerkat::WorkGraphStore> =
            Arc::new(meerkat::MemoryWorkGraphStore::default());
        let selected = meerkat::WorkGraphService::new(Arc::clone(&store));
        let spec = spec.with_workgraph_service(Some(selected.clone()));
        let read = spec.workgraph_service().expect("the selected service");
        assert!(Arc::ptr_eq(read.store(), selected.store()));
        assert!(Arc::ptr_eq(read.store(), &store));
        assert!(
            spec.workgraph_service()
                .is_some_and(|again| Arc::ptr_eq(again.store(), &store)),
            "reading never replaces the selection"
        );
    }

    fn acquisition_definition() -> MobDefinition {
        MobDefinition::from_toml(&format!("[mob]\nid = \"{}\"\n", unique_test_mob_id()))
            .expect("acquisition fixture definition")
    }

    #[tokio::test]
    async fn explicit_owner_is_adopted_before_mcp_and_reused_by_wrappers() {
        let dir = tempfile::tempdir().expect("fixture directory");
        let (service, selected, store, blobs, jobs) = acquisition_fixture(dir.path());
        let slot = Arc::new(std::sync::RwLock::new(None));
        let mut spec = MobBootstrapSpec::new(
            acquisition_definition(),
            MobStorage::in_memory(),
            service.clone(),
        );
        // Exercise the public explicit field as well as the fallible facade.
        // MCP construction must not acquire a default owner first.
        spec.runtime_adapter = Some(selected.clone());
        let spec = spec
            .with_runtime_delivery(MobRuntimeDelivery::new(store.clone(), jobs))
            .with_agent_mob_tools(slot.clone())
            .expect("install using selected owner");
        assert!(slot.read().expect("tool slot").is_some());
        let acquired = service
            .acquire_canonical_runtime_adapter(None)
            .expect("native owner");
        assert!(acquired.shares_runtime_execution_owner_with(&selected));
        let served = spec
            .agent_mob_mcp_state
            .as_ref()
            .expect("MCP state")
            .session_service()
            .acquire_runtime_adapter(None)
            .expect("MCP acquisition")
            .expect("MCP owner");
        assert!(served.shares_runtime_execution_owner_with(&selected));

        // A distinct outer Arc around a machine clone is the SAME native owner.
        let same_owner = Arc::new(selected.as_ref().clone());
        assert!(!Arc::ptr_eq(&selected, &same_owner));
        let spec = spec
            .with_session_runtime_adapter(same_owner.clone())
            .expect("same native owner is accepted");
        let wrapped = spec.session_service;
        assert!(
            wrapped
                .acquire_runtime_adapter(Some(same_owner))
                .expect("clone acquisition")
                .expect("clone owner")
                .shares_runtime_execution_owner_with(&selected)
        );

        let other_owner = Arc::new(
            meerkat_runtime::MeerkatMachine::persistent(store, blobs)
                .expect("different machine over same physical store"),
        );
        assert!(matches!(
            wrapped.acquire_runtime_adapter(Some(other_owner)),
            Err(
                meerkat_runtime::RuntimeDriverError::ControllerReadinessUnavailable {
                    reason: meerkat_runtime::traits::ControllerReadinessFailure::AuthorityChanged,
                }
            )
        ));
        assert!(
            wrapped
                .acquire_runtime_adapter(None)
                .expect("owner remains available")
                .expect("retained owner")
                .shares_runtime_execution_owner_with(&selected)
        );
    }

    #[tokio::test]
    async fn participant_custody_follows_final_storage_and_policy_reinstallation() {
        for (replace_storage, remove_store, child_policy) in [
            (false, false, false),
            (true, false, false),
            (true, true, false),
            (false, false, true),
        ] {
            let dir = tempfile::tempdir().expect("fixture directory");
            let (service, selected, store, _, jobs) = acquisition_fixture(dir.path());
            let slot = Arc::new(std::sync::RwLock::new(None));
            let mut spec =
                MobBootstrapSpec::new(acquisition_definition(), MobStorage::in_memory(), service)
                    .with_session_runtime_adapter(selected)
                    .expect("adopt owner")
                    .with_runtime_delivery(MobRuntimeDelivery::new(store, jobs))
                    .with_agent_mob_tools(Arc::clone(&slot))
                    .expect("initial tool state");
            let initial_state = spec.agent_mob_mcp_state.clone().expect("initial state");
            let initial_store = spec
                .storage
                .forked_participant_store()
                .cloned()
                .expect("initial root custody");
            assert!(Arc::ptr_eq(
                &initial_store,
                &initial_state.forked_participant_store_for_tests(),
            ));
            let initial_generation = initial_state
                .continuation_binding_generation()
                .expect("initial continuation binding");
            let console_slot = spec.console_spawn_sink_slot.clone().expect("console slot");
            let identity_slot = spec.identity_runtime_slot.clone().expect("identity slot");
            if replace_storage {
                spec.storage = MobStorage::in_memory();
                if remove_store {
                    spec.storage = spec.storage.with_forked_participant_store(None);
                }
            }
            if child_policy {
                spec = spec.with_tool_consequence_policy_registry(Arc::new(
                    meerkat_core::ToolConsequencePolicyRegistry::new(
                        Vec::new(),
                        Default::default(),
                        None,
                    )
                    .expect("empty policy registry"),
                ));
            }
            let selected_store = spec.storage.forked_participant_store().cloned();
            spec.apply_agent_mob_child_policy()
                .expect("install final storage and child policy");
            let state = spec.agent_mob_mcp_state.as_ref().expect("final state");
            let changed = replace_storage || child_policy;
            assert_eq!(!Arc::ptr_eq(&initial_state, state), changed);
            assert_eq!(
                state.continuation_binding_generation() != Some(initial_generation),
                changed,
                "only a changed composition replaces the continuation binding"
            );
            assert!(Arc::ptr_eq(
                &console_slot,
                spec.console_spawn_sink_slot.as_ref().expect("console slot"),
            ));
            assert!(Arc::ptr_eq(
                &identity_slot,
                spec.identity_runtime_slot.as_ref().expect("identity slot"),
            ));
            let served_store = state.forked_participant_store_for_tests();
            if let Some(selected_store) = selected_store {
                assert!(Arc::ptr_eq(&selected_store, &served_store));
                assert!(Arc::ptr_eq(
                    &selected_store,
                    spec.storage
                        .forked_participant_store()
                        .expect("root custody"),
                ));
            } else {
                assert!(spec.storage.forked_participant_store().is_none());
                assert!(
                    !Arc::ptr_eq(&initial_store, &served_store),
                    "removing root custody must not retain its old store in the MCP state"
                );
            }
            assert_eq!(state.detached_delivery_blocked_because(), None);
            assert!(slot.read().expect("tool slot").is_some());
        }
    }

    #[tokio::test]
    async fn rejected_acquisition_does_not_publish_mob_tools_and_can_retry() {
        let dir = tempfile::tempdir().expect("fixture directory");
        let (service, selected, store, _, jobs) = acquisition_fixture(dir.path());
        let (_, wrong_store_owner, _, _, _) = acquisition_fixture(dir.path());
        let slot = Arc::new(std::sync::RwLock::new(None));
        let mut rejected = MobBootstrapSpec::new(
            acquisition_definition(),
            MobStorage::in_memory(),
            service.clone(),
        );
        rejected.runtime_adapter = Some(wrong_store_owner);
        assert!(matches!(
            rejected.with_agent_mob_tools(slot.clone()),
            Err(MobRuntimeDeliveryError::Runtime(
                meerkat_runtime::RuntimeDriverError::ControllerReadinessUnavailable {
                    reason: meerkat_runtime::traits::ControllerReadinessFailure::UnsupportedScope,
                }
            ))
        ));
        assert!(
            slot.read().expect("tool slot").is_none(),
            "a rejected acquisition must not publish a host tool factory"
        );
        let retried = MobBootstrapSpec::new(
            acquisition_definition(),
            MobStorage::in_memory(),
            service.clone(),
        )
        .with_session_runtime_adapter(selected.clone())
        .expect("first successful explicit acquisition")
        .with_runtime_delivery(MobRuntimeDelivery::new(store, jobs))
        .with_agent_mob_tools(slot.clone())
        .expect("retry initialization");
        assert!(slot.read().expect("tool slot").is_some());
        assert!(
            retried
                .runtime_adapter
                .expect("selected owner")
                .shares_runtime_execution_owner_with(&selected)
        );
        assert!(
            service
                .acquire_canonical_runtime_adapter(None)
                .expect("native owner after retry")
                .shares_runtime_execution_owner_with(&selected),
            "the rejected initialization must not install a default owner"
        );
    }

    #[tokio::test]
    async fn prepare_rejects_changed_owner_before_reinstalling_child_mob_tools() {
        let dir = tempfile::tempdir().expect("fixture directory");
        let (service, selected, store, blobs, jobs) = acquisition_fixture(dir.path());
        let slot = Arc::new(std::sync::RwLock::new(None));
        let mut spec =
            MobBootstrapSpec::new(acquisition_definition(), MobStorage::in_memory(), service)
                .with_session_runtime_adapter(selected)
                .expect("adopt owner")
                .with_runtime_delivery(MobRuntimeDelivery::new(store.clone(), jobs))
                .with_agent_mob_tools(slot.clone())
                .expect("initial tool state")
                .with_tool_consequence_policy_registry(Arc::new(
                    meerkat_core::ToolConsequencePolicyRegistry::new(
                        Vec::new(),
                        Default::default(),
                        None,
                    )
                    .expect("empty policy registry"),
                ));
        let installed = slot
            .read()
            .expect("tool slot")
            .clone()
            .expect("installed factory");
        spec.runtime_adapter = Some(Arc::new(
            meerkat_runtime::MeerkatMachine::persistent(store, blobs).expect("different owner"),
        ));
        // Hold the composition release past the refused prepare, so the slot
        // still shows what prepare left there (a refused spec otherwise
        // empties its tool slots when it drops).
        let _held = spec.composition_release.clone();
        assert!(matches!(
            MobRuntime::prepare(spec).await,
            Err(MobRuntimeError::Runtime(
                meerkat_runtime::RuntimeDriverError::ControllerReadinessUnavailable {
                    reason: meerkat_runtime::traits::ControllerReadinessFailure::AuthorityChanged,
                }
            ))
        ));
        assert!(
            Arc::ptr_eq(
                &installed,
                slot.read()
                    .expect("tool slot")
                    .as_ref()
                    .expect("original factory")
            ),
            "prepare must reject before its final-policy MCP reinstall publishes a replacement"
        );
    }

    #[test]
    fn ephemeral_service_rejects_store_backed_owner_without_poisoning_retry() {
        let dir = tempfile::tempdir().expect("fixture directory");
        let (_, persistent, _, _, _) = acquisition_fixture(dir.path());
        let service = Arc::new(meerkat_session::EphemeralSessionService::new(
            FactoryAgentBuilder::new(AgentFactory::new(dir.path()), Config::default()),
            4,
        ));
        let make_spec = || {
            MobBootstrapSpec::new(
                acquisition_definition(),
                MobStorage::in_memory(),
                service.clone(),
            )
        };
        assert!(matches!(
            make_spec().with_session_runtime_adapter(persistent),
            Err(
                meerkat_runtime::RuntimeDriverError::ControllerReadinessUnavailable {
                    reason: meerkat_runtime::traits::ControllerReadinessFailure::UnsupportedScope,
                }
            )
        ));
        let ephemeral = Arc::new(meerkat_runtime::MeerkatMachine::ephemeral());
        let retried = make_spec()
            .with_session_runtime_adapter(ephemeral.clone())
            .expect("supported retry");
        assert!(
            retried
                .runtime_adapter
                .expect("ephemeral owner")
                .shares_runtime_execution_owner_with(&ephemeral)
        );
    }

    #[test]
    fn wrapper_acquisition_preserves_none_and_typed_errors_even_with_override() {
        for outcome in [1, 2, 3] {
            let probe = Arc::new(ForwardingProbe::default());
            probe
                .acquisition_outcome
                .store(outcome, std::sync::atomic::Ordering::Relaxed);
            let default = Arc::new(meerkat_runtime::MeerkatMachine::ephemeral());
            let explicit = Arc::new(meerkat_runtime::MeerkatMachine::ephemeral());
            let wrapped: Arc<dyn MobSessionService> = Arc::new(AfterCreateMobSessionService {
                inner: Arc::new(PreBuildMobSessionService {
                    inner: probe.clone(),
                    hook: no_op_pre_build_hook(),
                    dispatch_taint: None,
                    after_create_hook: None,
                    runtime_adapter_override: Some(default.clone()),
                    archived_terminal_authority: None,
                    member_sessions_in_memory: false,
                }),
                after_hook: Arc::new(|_, _| Box::pin(async {})),
            });
            for request in [None, Some(explicit.clone())] {
                let result = wrapped.acquire_runtime_adapter(request.clone());
                match outcome {
                    1 => assert!(matches!(result, Ok(None))),
                    2 => assert!(matches!(result,
                        Err(meerkat_runtime::RuntimeDriverError::ControllerReadinessUnavailable {
                            reason: meerkat_runtime::traits::ControllerReadinessFailure::AuthorityChanged,
                        }))),
                    3 => assert!(matches!(result,
                        Err(meerkat_runtime::RuntimeDriverError::RecoveryCorruption { reason })
                            if reason == "acquisition probe")),
                    _ => unreachable!(),
                }
                let forwarded = probe
                    .acquisition_inputs
                    .lock()
                    .expect("acquisition inputs")
                    .last()
                    .cloned()
                    .flatten()
                    .expect("forwarded explicit or configured input");
                assert!(Arc::ptr_eq(
                    &forwarded,
                    request.as_ref().unwrap_or(&default)
                ));
            }
        }
        let probe = Arc::new(ForwardingProbe::default());
        probe
            .acquisition_outcome
            .store(1, std::sync::atomic::Ordering::Relaxed);
        let spec = MobBootstrapSpec::new(acquisition_definition(), MobStorage::in_memory(), probe)
            .with_session_runtime_adapter(Arc::new(meerkat_runtime::MeerkatMachine::ephemeral()))
            .expect("ownerless custom service is a legitimate result");
        assert!(spec.runtime_adapter.is_none());
        assert!(
            spec.session_service
                .acquire_runtime_adapter(None)
                .expect("ownerless acquisition")
                .is_none()
        );
    }

    #[tokio::test]
    async fn direct_deferred_creates_register_acquired_owner_before_user_hook() {
        use meerkat_runtime::SessionServiceRuntimeExt as _;
        for runtime_backed in [false, true] {
            let dir = tempfile::tempdir().expect("fixture directory");
            let owner_slot = Arc::new(Mutex::new(None::<Arc<meerkat_runtime::MeerkatMachine>>));
            let hook_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let hook: AfterCreateHook = {
                let owner_slot = owner_slot.clone();
                let hook_calls = hook_calls.clone();
                Arc::new(move |session_id, _| {
                    let owner = owner_slot
                        .lock()
                        .expect("owner slot")
                        .clone()
                        .expect("selected owner");
                    let hook_calls = hook_calls.clone();
                    Box::pin(async move {
                        owner
                            .session_model_routing_status(&session_id)
                            .await
                            .expect("direct create registers before the user after-create hook");
                        hook_calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    })
                })
            };
            let spec = if runtime_backed {
                MobBootstrapSpec::ephemeral_runtime_backed_inner(
                    acquisition_definition(),
                    MobStorage::in_memory(),
                    dir.path().to_path_buf(),
                    4,
                    None,
                    None,
                    None,
                    None,
                    CapabilityFlags::default(),
                    Some(hook),
                    None,
                )
                .expect("process-local runtime-backed spec")
            } else {
                MobBootstrapSpec::ephemeral_inner(
                    acquisition_definition(),
                    MobStorage::in_memory(),
                    dir.path().to_path_buf(),
                    4,
                    None,
                    None,
                    CapabilityFlags {
                        image_generation: true,
                        ..CapabilityFlags::default()
                    },
                    Some(hook),
                    None,
                    None,
                )
                .expect("image-only ephemeral spec")
            };
            let owner = spec.runtime_adapter.clone().expect("acquired owner");
            *owner_slot.lock().expect("owner slot") = Some(owner.clone());
            assert!(
                matches!(
                    owner
                        .session_model_routing_status(&meerkat_core::SessionId::new())
                        .await,
                    Err(meerkat_runtime::RuntimeDriverError::NotFound { .. })
                ),
                "the runtime status probe must reject an unregistered session"
            );
            let request = CreateSessionRequest {
                model: "gpt-5.5".to_string(),
                prompt: meerkat_core::ContentInput::Text(
                    "deferred registration control".to_string(),
                ),
                system_prompt: meerkat_core::config::SystemPromptOverride::Inherit,
                max_tokens: None,
                event_tx: None,
                initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
                build: Some(meerkat_core::service::SessionBuildOptions {
                    llm_client_override: Some(meerkat::encode_llm_client_override_for_service(
                        Arc::new(meerkat_client::TestClient::for_provider(
                            meerkat_core::Provider::OpenAI,
                        )),
                    )),
                    ..Default::default()
                }),
                labels: None,
                deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
                injected_context: Vec::new(),
            };
            tokio::time::timeout(std::time::Duration::from_secs(30), async {
                let created = Box::pin(spec.session_service.create_session(request))
                    .await
                    .expect("direct deferred create");
                owner
                    .session_model_routing_status(&created.session_id)
                    .await
                    .expect("created session registered on selected owner");
                assert_eq!(hook_calls.load(std::sync::atomic::Ordering::SeqCst), 1);
                spec.session_service
                    .discard_live_session(&created.session_id)
                    .await
                    .expect("discard direct fixture actor");
                owner
                    .unregister_session(&created.session_id)
                    .await
                    .expect("unregister fixture driver");
            })
            .await
            .expect("bounded direct create and cleanup");
        }
    }
}
