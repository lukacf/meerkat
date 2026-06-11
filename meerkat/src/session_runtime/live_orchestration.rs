//! Live-channel orchestration.
//!
//! Populated by W2-A. Hosts the surface-agnostic helper free functions
//! that build live projection snapshots and decide when a live channel
//! needs a forced close vs in-place refresh. Live prompt truth is owned
//! by the typed `RealtimeSessionOpenConfig.system_prompt` field — it is
//! never re-derived from seed history.
//!
//! Gated by the `live` feature on the `meerkat` facade so surfaces
//! that don't ship a live channel (CLI today, MCP-server, embedded
//! examples) don't pull in the `meerkat-live` dependency.
//!
//! The load-bearing methods (`precheck_live_open`,
//! `recover_live_session_for_realtime_open`,
//! `materialize_staged_session_for_realtime_open`,
//! `realtime_session_open_config`, `live_open_config_for_session`,
//! `propagate_config_to_live_channels`) currently live in
//! `meerkat-rpc::SessionRuntime` because they consume a long list of
//! still-RPC-private helpers (`replay_promoted_system_context_on_service`,
//! `current_materialized_llm_identity`, `archive_runtime_cleanup`,
//! `runtime_state_ops`). Once the corresponding accessors land in
//! W3-A/W3-B they will be promoted onto `LiveOrchestrator<'a>`.
//!
//! These free functions only depend on `meerkat-llm-core`, `meerkat-core`,
//! and the model catalog — they do NOT import `meerkat-live`, so they
//! compile unconditionally regardless of the `live` feature. The
//! `live` feature is reserved for the future
//! [`crate::session_runtime::live_orchestration::LiveOrchestrator`]
//! struct that will own the methods consuming `LiveAdapterHost`.

use meerkat_core::error::AgentError;
use meerkat_core::service::SessionError;
use meerkat_core::types::{Message, SessionId, SystemMessage};
use meerkat_core::{
    PendingSystemContextAppend, Session, SessionLlmIdentity, SessionToolVisibilityState,
};
use meerkat_llm_core::realtime_session::RealtimeSessionOpenConfig;

use crate::session_runtime::errors::LiveOpenPrecheckError;

/// Apply the B19 (realtime-capability) and B18 (provider-supported) gates
/// to a resolved LLM identity. Shared between the staged-session and live-
/// session branches of `precheck_live_open` so both paths enforce
/// identical contracts.
pub fn precheck_identity(identity: &SessionLlmIdentity) -> Result<(), LiveOpenPrecheckError> {
    let realtime_capable = meerkat_core::model_profile::capabilities::capabilities_for(
        identity.provider,
        &identity.model,
    )
    .map(|caps| caps.realtime)
    .unwrap_or(false);
    apply_precheck_gates(identity.provider, &identity.model, realtime_capable)
}

/// Pure gate-ordering helper: B19 (realtime capability) fires before
/// B18 (provider has live adapter). Split out from `precheck_identity`
/// so unit tests can pin the B18 branch — the catalog cannot naturally
/// produce a realtime-capable non-OpenAI row, so e2e never reaches
/// `ProviderHasNoLiveAdapter` and a synthetic `realtime_capable: true`
/// is the only way to assert the non-OpenAI rejection contract. Also
/// documents the gate ordering as intentional: a non-realtime non-OpenAI
/// session reports `ModelNotRealtime` (the more specific failure), not
/// `ProviderHasNoLiveAdapter`.
pub fn apply_precheck_gates(
    provider: meerkat_core::Provider,
    model: &str,
    realtime_capable: bool,
) -> Result<(), LiveOpenPrecheckError> {
    if !realtime_capable {
        return Err(LiveOpenPrecheckError::ModelNotRealtime {
            model: model.to_string(),
            provider: provider.as_str(),
        });
    }
    if !matches!(provider, meerkat_core::Provider::OpenAI) {
        return Err(LiveOpenPrecheckError::ProviderHasNoLiveAdapter {
            provider: provider.as_str(),
        });
    }
    Ok(())
}

/// P1#5: build a [`LiveProjectionSnapshot`] from the resolved
/// [`RealtimeSessionOpenConfig`].
///
/// Mirror of `build_live_projection_snapshot` in
/// `meerkat-rpc::handlers::live`; we duplicate here so
/// `propagate_config_to_live_channels` can run from the runtime layer
/// without depending on handler-private helpers.
///
/// R8: this builder stamps `snapshot_version: 0` as a placeholder. The
/// caller (`propagate_config_to_live_channels`) overwrites it with
/// `host.next_snapshot_version(channel_id)` before dispatch so adapters
/// gating on `snapshot_version` for stale-refresh detection see strictly
/// increasing generations. Do not treat the field returned here as the
/// final stamp.
#[must_use]
pub fn build_live_projection_snapshot_for_runtime(
    session_id: &SessionId,
    open_config: &RealtimeSessionOpenConfig,
) -> meerkat_core::live_adapter::LiveProjectionSnapshot {
    meerkat_core::live_adapter::LiveProjectionSnapshot {
        session_id: session_id.clone(),
        snapshot_version: 0,
        seed_messages: open_config.seed_messages.clone(),
        visible_tools: open_config.visible_tools.clone(),
        // R10: the typed `RealtimeSessionOpenConfig.system_prompt` field is
        // the single owner of live prompt truth (populated by
        // `realtime_session_open_config` from the resolved root system
        // message). Never re-derive it by inspecting `seed_messages[0]`.
        system_prompt: open_config.system_prompt.clone(),
        model_id: open_config.llm_identity.model.clone(),
        provider_id: open_config.llm_identity.provider,
        audio_config: None,
        // R3: forward typed runtime system-context so refresh snapshots
        // carry the same authoritative system instructions the open path
        // emitted (peer terminal, ops_lifecycle, etc.).
        runtime_system_context: open_config.runtime_system_context.clone(),
    }
}

/// R11: pure helper deciding whether a `config/patch`-resolved live
/// identity represents a model or provider swap relative to the identity
/// the channel was opened with.
///
/// Returns `true` when the channel must be closed (so the SDK can reopen
/// against the new identity); `false` when an in-place `Refresh` is safe.
///
/// Callers must obtain `bound_identity` from generated live-open admission
/// authority. Missing generated identity is handled as a fail-closed channel
/// close before this comparison is reached.
///
/// Audio-rate change is intentionally NOT checked here. The OpenAI
/// Refresh guard rejects it, but R11's typed runtime path is scoped to
/// model + provider until `audio_config` is plumbed into the projection
/// snapshot. Audio mismatches still surface as the existing async
/// `LiveAdapterErrorCode::ConfigRejected` error from the adapter.
#[must_use]
pub fn live_channel_requires_close_for_identity_change(
    bound_identity: &SessionLlmIdentity,
    new_identity: &SessionLlmIdentity,
) -> bool {
    bound_identity.model != new_identity.model || bound_identity.provider != new_identity.provider
}

/// Decide whether `propagate_config_to_live_channels` should hot-swap a
/// given session's live LLM identity to the new global model.
///
/// The rule is:
///
/// - If the session's current model already equals the new global, the
///   hot-swap would be a no-op — skip it (the per-channel Refresh
///   fan-out below still runs).
/// - Otherwise propagate the new global to the session. A `config/patch`
///   that mutates `agent.model` is a global policy change and the live
///   path must reflect it, including for sessions that pinned a model at
///   `session/create` time (s72: a session created with an explicit
///   `model: "gpt-realtime-2"` against a non-realtime global must still
///   re-resolve to the new non-realtime global so the next `live/open`
///   precheck rejects via B19).
///
/// G5 (P1) revisited: the original G5 rule attempted to preserve
/// "per-session overrides" by skipping when `current_session_model`
/// differed from the prior global model. That heuristic conflated two
/// distinct cases — (a) a session that explicitly chose its initial
/// model via `CreateSessionRequest.model` while the global differed, and
/// (b) a session that was later reconfigured via `llm_reconfigure`. Both
/// cases produced `current != prior_global`, but only (b) carries a
/// "sticky override" intent. Without a typed override marker on
/// `SessionMetadata` we cannot disambiguate; broadcasting the new global
/// is the correct default for `config/patch agent.model` because that
/// patch is itself a global policy change. Sessions that need a sticky
/// override should issue a session-scoped reconfigure after the patch.
/// The rule therefore depends only on the session's current model and
/// the new global model.
#[must_use]
pub fn should_apply_global_model_hot_swap(
    current_session_model: &str,
    new_global_model: &str,
) -> bool {
    current_session_model != new_global_model
}

/// R3-2-4 (P1+P2): pure rule deciding whether a `config/set` or
/// `config/patch` commit should fan out
/// `propagate_config_to_live_channels` to active live channels.
///
/// **Field set consulted by the propagate body** (verified against
/// [`super::orchestrator::LiveOrchestrator::propagate_config_to_live_channels`]
/// at the time of writing):
///
/// - `agent.model` — read as `new_global_model` and threaded into the
///   per-session hot-swap rule via [`should_apply_global_model_hot_swap`].
///   This is the ONLY `Config` field the propagate path currently
///   consults. Per-session live identity is re-resolved via
///   `live_session_llm_identity` (session-bound state, not config),
///   and the per-channel `Refresh` snapshot is rebuilt from the
///   session, not the config.
///
/// If the propagate body grows to consult additional fields (e.g.
/// `agent.provider`, realtime audio defaults, tool catalog scopes),
/// extend this helper AND the regression tests in
/// `meerkat/tests/session_runtime_live_orchestration.rs`. Keeping the
/// predicate field set in lock-step with the propagate body is the
/// whole point of the helper: an under-fired propagate (P2) leaves
/// live channels stale; an over-fired propagate (P1) retargets or
/// closes channels for unrelated config edits.
///
/// Returns `true` iff a propagate-affecting field actually changed.
/// `false` short-circuits the orchestrator fan-out — the per-session
/// hot-swap loop and per-channel Refresh dispatch are skipped, which
/// is correct: there is nothing to propagate.
#[must_use]
pub fn should_fire_live_propagation(
    prior: &meerkat_core::config::Config,
    new: &meerkat_core::config::Config,
) -> bool {
    prior.agent.model != new.agent.model
}

/// Why a per-session global hot-swap was skipped during config propagation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LiveHotSwapSkipReason {
    /// The session's current model already matches the new global model, or a
    /// session-scoped override is in effect — the swap would be a no-op.
    NoOpOrOverride,
    /// The session's live LLM identity could not be looked up.
    IdentityLookupFailed(String),
}

/// Why a per-channel refresh was dropped (not delivered) during config
/// propagation. Channels that were intentionally closed for a rejection are
/// recorded separately as `closed`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LiveChannelRefreshFailure {
    /// Building the refreshed open_config failed.
    OpenConfigBuildFailed(String),
    /// Stamping the snapshot version failed.
    SnapshotVersionFailed(String),
    /// Enqueueing the Refresh command failed.
    EnqueueFailed(String),
    /// The refresh queue acceptance was rejected by generated authority.
    QueueAcceptanceRejected(String),
}

/// Typed failure of a config-rejection live-channel close.
///
/// Returned by `close_live_channel_for_config_rejection` so the propagation
/// report carries the fault instead of laundering it into tracing while the
/// channel is reported as cleanly closed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LiveChannelCloseFailure {
    /// Signaling the terminal error on the channel failed.
    SignalFailed(String),
    /// The generated close authority rejected the terminal cleanup.
    CloseAuthorityRejected(String),
    /// The close authority omitted the host commit handoff.
    CommitHandoffMissing,
    /// The host close commit failed after the generated terminal cleanup.
    HostCommitFailed(String),
}

/// Aggregated typed outcome of [`propagate_config_to_live_channels`].
///
/// Replaces the prior pure-logging fan-out: each per-session hot-swap and
/// per-channel refresh outcome is recorded so the caller (the config/patch
/// handler) receives a structured report rather than relying on tracing to
/// observe a propagation that silently failed.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
#[must_use]
pub struct LiveConfigPropagationReport {
    /// Sessions hot-swapped to the new global model.
    pub swapped: Vec<SessionId>,
    /// Sessions whose hot-swap was skipped, with the typed reason.
    pub skipped: Vec<(SessionId, LiveHotSwapSkipReason)>,
    /// Sessions whose hot-swap reconfigure failed.
    pub swap_failed: Vec<(SessionId, String)>,
    /// Live channels refreshed in place.
    pub refreshed: Vec<SessionId>,
    /// Live channels closed (identity swap / non-realtime / missing identity).
    pub closed: Vec<SessionId>,
    /// Live channels whose refresh was dropped, with the typed failure.
    pub refresh_failed: Vec<(SessionId, LiveChannelRefreshFailure)>,
    /// Live channels whose config-rejection close itself failed, with the
    /// typed failure. These channels are NOT in `closed`: the close did not
    /// complete, and reporting them as closed would launder the fault.
    pub close_failed: Vec<(SessionId, LiveChannelCloseFailure)>,
}

impl LiveConfigPropagationReport {
    /// `true` when every channel was either refreshed, intentionally closed, or
    /// deliberately skipped — i.e. no refresh was silently dropped and no
    /// hot-swap failed unexpectedly.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.swap_failed.is_empty()
            && self.refresh_failed.is_empty()
            && self.close_failed.is_empty()
    }
}

/// Build the projection-root system message for a realtime session. The
/// content is the union of the resolved `system_prompt` (or the first
/// existing `System`/`SystemNotice` lead) and any session-build
/// `additional_instructions`.
pub fn realtime_projection_root_system_message(
    session: &Session,
) -> Result<Option<Message>, SessionError> {
    let build_state = session.build_state().ok_or_else(|| {
        SessionError::Agent(AgentError::InternalError(format!(
            "session {} is missing session build state",
            session.id()
        )))
    })?;
    let mut content = match build_state.system_prompt.clone() {
        meerkat_core::SystemPromptOverride::Set(prompt) => prompt,
        // An explicit Disable means no system prompt; do not fall back to the
        // transcript lead.
        meerkat_core::SystemPromptOverride::Disable => String::new(),
        meerkat_core::SystemPromptOverride::Inherit => session
            .messages()
            .first()
            .and_then(|message| match message {
                Message::System(system) => Some(system.content.clone()),
                Message::SystemNotice(notice) => Some(notice.model_projection_text()),
                _ => None,
            })
            .unwrap_or_default(),
    };

    if let Some(additional_instructions) = &build_state.additional_instructions
        && !additional_instructions.is_empty()
    {
        if !content.trim().is_empty() {
            content.push_str("\n\n");
        }
        content.push_str("[Session Build Instructions]");
        for instruction in additional_instructions {
            let instruction = instruction.trim();
            if instruction.is_empty() {
                continue;
            }
            content.push_str("\n- ");
            content.push_str(instruction);
        }
    }

    if content.trim().is_empty() {
        Ok(None)
    } else {
        Ok(Some(Message::System(SystemMessage::new(content))))
    }
}

/// Project a session's transcript for realtime delivery: prepend or
/// rewrite the lead message with [`realtime_projection_root_system_message`]
/// when one is available.
pub fn realtime_projection_messages(session: &Session) -> Result<Vec<Message>, SessionError> {
    let mut projected = session.messages().to_vec();
    if let Some(root_system) = realtime_projection_root_system_message(session)? {
        match projected.first() {
            Some(Message::System(_) | Message::SystemNotice(_)) => projected[0] = root_system,
            _ => projected.insert(0, root_system),
        }
    }
    Ok(projected)
}

/// Project a session's runtime system context into the realtime
/// open-config shape (applied + pending appends concatenated).
pub fn realtime_projection_runtime_system_context(
    session: &Session,
) -> Result<Vec<PendingSystemContextAppend>, SessionError> {
    let state = session
        .try_system_context_state()
        .map_err(|err| {
            SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                "generated system-context authority rejected realtime projection restore: {err}"
            )))
        })?
        .unwrap_or_default();
    Ok(state.realtime_projection_appends())
}

/// Read the typed visibility state directly from the session without
/// going through the realtime projection. Used by RPC tests to verify
/// projection equivalence; kept un-gated so `meerkat-rpc` test builds
/// can call into it even when the upstream `meerkat` crate is not
/// itself built in test mode.
#[allow(clippy::expect_used)]
pub fn exported_tool_visibility_state(session: &Session) -> SessionToolVisibilityState {
    session
        .tool_visibility_state()
        .expect("exported visibility state should decode")
        .unwrap_or_default()
}

/// Synthesize a builtin tool visibility witness that matches the agent
/// loop's provenance identity for the builtin source. Used by
/// RPC tests; kept un-gated for the same reason as
/// [`exported_tool_visibility_state`].
#[must_use]
pub fn builtin_tool_visibility_witness() -> meerkat_core::ToolVisibilityWitness {
    let provenance = meerkat_core::ToolProvenance {
        kind: meerkat_core::ToolSourceKind::Builtin,
        source_id: "builtin".into(),
    };
    meerkat_core::ToolVisibilityWitness {
        last_seen_provenance: Some(provenance),
    }
}

/// Phase 4 R1: surface-agnostic [`LiveOrchestrator`] that owns the
/// load-bearing live-channel methods previously stranded on
/// `meerkat-rpc::SessionRuntime`.
///
/// `LiveOrchestrator<'a>` is a borrowing struct: surfaces hand it the
/// concrete references they own and the orchestrator drives the
/// recovery / staged-promotion / refresh / hot-swap flows. The RPC
/// shell wraps this in a thin shim that translates the typed
/// [`LiveOpenPrecheckError`] / [`SessionError`] onto `RpcError`.
#[cfg(all(
    feature = "session-store",
    feature = "live",
    not(target_arch = "wasm32")
))]
pub use orchestrator::LiveOrchestrator;

#[cfg(all(
    feature = "session-store",
    feature = "live",
    not(target_arch = "wasm32")
))]
mod orchestrator {
    use std::sync::Arc;

    use meerkat_core::service::{
        CreateSessionRequest, InitialTurnPolicy, SessionError, SessionService,
    };
    use meerkat_core::types::{ContentInput, Message, SessionId};
    use meerkat_core::{DeferredPromptPolicy, SessionLlmIdentity, SurfaceSessionRecoveryOverrides};
    use meerkat_live::LiveAdapterHost;
    use meerkat_llm_core::realtime_session::RealtimeSessionOpenConfig;
    use meerkat_runtime::{MeerkatMachine, SessionLlmReconfigureRequest, SessionServiceRuntimeExt};
    use meerkat_session::PersistentSessionService;

    use super::{
        LiveChannelCloseFailure, LiveChannelRefreshFailure, LiveConfigPropagationReport,
        LiveHotSwapSkipReason, build_live_projection_snapshot_for_runtime,
        live_channel_requires_close_for_identity_change, precheck_identity,
        realtime_projection_messages, realtime_projection_root_system_message,
        realtime_projection_runtime_system_context, should_apply_global_model_hot_swap,
    };

    use crate::service_factory::FactoryAgentBuilder;
    use crate::session_runtime::admission::{
        StagedCapacityAdmissions, take_staged_capacity_admission,
    };
    use crate::session_runtime::errors::LiveOpenPrecheckError;
    use crate::session_runtime::recovery::{RecoveryContext, RecoveryRuntimeBindingMode};
    use crate::session_runtime::runtime_state::ArchiveRuntimeCleanup;
    use crate::session_runtime::staged_promotion::PendingPromotionCleanup;
    use crate::{StagedLifecycleError, StagedSessionRegistry};
    use meerkat_core::error::AgentError;

    /// Surface-agnostic live-channel orchestrator.
    ///
    /// Borrows the resolved infrastructure from a calling surface
    /// (RPC, REST, embedded examples) and exposes the W2-A
    /// load-bearing methods that previously lived on
    /// `meerkat-rpc::SessionRuntime`.
    pub struct LiveOrchestrator<'a> {
        /// Persistent session service.
        pub service: &'a Arc<PersistentSessionService<FactoryAgentBuilder>>,
        /// Staged session registry.
        pub staged_sessions: &'a Arc<StagedSessionRegistry>,
        /// Service-owned capacity ledger for staged sessions.
        pub staged_capacity_admissions: &'a StagedCapacityAdmissions,
        /// Runtime adapter (`MeerkatMachine`).
        pub runtime_adapter: &'a Arc<MeerkatMachine>,
        /// Optional live-adapter host (owned `Arc` clone). `None`
        /// disables refresh / close fan-out
        /// (`propagate_config_to_live_channels` becomes a no-op).
        pub host: Option<Arc<LiveAdapterHost>>,
        /// Shared config runtime (for generation stamping).
        pub config_runtime: Option<Arc<meerkat_core::ConfigRuntime>>,
        /// Default LLM client override applied to fresh sessions.
        pub default_llm_client: Option<Arc<dyn crate::LlmClient>>,
        /// Optional decorator wrapped around session LLM clients (kept
        /// here so the orchestrator can build a [`RecoveryContext`]
        /// without a separate plumbing seam).
        pub agent_llm_client_decorator: Option<meerkat_core::AgentLlmClientDecorator>,
        /// Optional external tool dispatcher (RPC's callback dispatcher,
        /// REST's external bridge, etc.).
        pub external_tools: Option<Arc<dyn meerkat_core::AgentToolDispatcher>>,
        /// Surface-supplied archive cleanup for failed recoveries.
        pub archive_runtime_cleanup: ArchiveRuntimeCleanup,
        /// Active realm id (cloned from the slot once per call).
        pub realm_id: Option<&'a meerkat_core::connection::RealmId>,
        /// Active instance id.
        pub instance_id: Option<&'a str>,
        /// Active backend label.
        pub backend: Option<&'a str>,
    }

    impl LiveOrchestrator<'_> {
        fn recovery_context(&self) -> RecoveryContext<'_> {
            RecoveryContext {
                service: self.service,
                runtime_adapter: self.runtime_adapter,
                realm_id: self.realm_id,
                instance_id: self.instance_id,
                backend: self.backend,
                default_llm_client: self.default_llm_client.clone(),
                agent_llm_client_decorator: self.agent_llm_client_decorator.clone(),
                external_tools: self.external_tools.clone(),
                config_runtime: self.config_runtime.clone(),
            }
        }

        async fn cleanup_recovered_runtime_if_new(
            &self,
            session_id: &SessionId,
            runtime_was_registered: bool,
        ) {
            if !runtime_was_registered {
                let _ = self.archive_runtime_cleanup.run(session_id).await;
            }
        }

        /// Promote a staged (deferred) session into the live service map
        /// without running a turn, so realtime-open paths can find it.
        pub async fn materialize_staged_session_for_realtime_open(
            &self,
            session_id: &SessionId,
        ) -> Result<(), SessionError> {
            let pending_session = match self.staged_sessions.begin_promotion(session_id).await {
                Ok(slot) => slot,
                Err(StagedLifecycleError::AlreadyPromoting(_)) => {
                    return Err(SessionError::Busy {
                        id: session_id.clone(),
                    });
                }
                Err(e) => {
                    return Err(SessionError::Agent(
                        meerkat_core::error::AgentError::InternalError(format!(
                            "staged session lifecycle error for {session_id}: {e}"
                        )),
                    ));
                }
            };

            let Some(slot) = pending_session else {
                return Ok(());
            };

            let staged_capacity_admission =
                take_staged_capacity_admission(self.staged_capacity_admissions, session_id);
            let mut promotion_cleanup = PendingPromotionCleanup::new(
                Arc::clone(self.staged_sessions),
                Arc::clone(self.staged_capacity_admissions),
                session_id,
                &slot,
                staged_capacity_admission,
            );

            let crate::PromotingSlot {
                build_config,
                labels,
                deferred_prompt,
                ..
            } = slot;
            let mut build_config = *build_config;

            if build_config.llm_client_override.is_none()
                && let Some(client) = self.default_llm_client.as_ref()
            {
                build_config.llm_client_override = Some(Arc::clone(client));
                promotion_cleanup.update_build_config(&build_config);
            }

            let runtime_generation = if build_config.config_generation.is_none() {
                if let Some(runtime) = self.config_runtime.as_ref() {
                    runtime.get().await.ok().map(|snapshot| snapshot.generation)
                } else {
                    None
                }
            } else {
                None
            };

            let mut build = build_config.to_session_build_options();
            build.realm_id = build.realm_id.or_else(|| self.realm_id.cloned());
            build.instance_id = build
                .instance_id
                .or_else(|| self.instance_id.map(ToString::to_string));
            build.backend = build.backend.or_else(|| {
                self.backend
                    .and_then(meerkat_core::RecoveryBackendKind::parse)
            });
            build.config_generation = build.config_generation.or(runtime_generation);

            let (prompt, deferred_prompt_policy) = match deferred_prompt {
                Some(prompt) => (prompt, DeferredPromptPolicy::Stage),
                None => (
                    ContentInput::Text(String::new()),
                    DeferredPromptPolicy::Discard,
                ),
            };

            let create_req = CreateSessionRequest {
                model: build_config.model.clone(),
                prompt,
                system_prompt: build_config.system_prompt.clone(),
                max_tokens: build_config.max_tokens,
                event_tx: None,
                initial_turn: InitialTurnPolicy::Defer,
                deferred_prompt_policy,
                build: Some(build),
                labels,
            };

            let admission = match promotion_cleanup.take_staged_capacity_admission() {
                Some(adm) => adm,
                None => self.service.reserve_create_session_admission().await?,
            };

            match self
                .service
                .create_session_with_reserved_admission(create_req, admission)
                .await
            {
                Ok(_) => {
                    promotion_cleanup.mark_materialized();
                    let _ = promotion_cleanup.finish_now().await;
                    promotion_cleanup.disarm();
                    Ok(())
                }
                Err(error) => {
                    promotion_cleanup.restore_now().await;
                    Err(error)
                }
            }
        }

        /// Recover a persisted-only session into the live service map so
        /// realtime-open paths can resolve it. Materializes a deferred
        /// staged session in place; falls back to durable-snapshot
        /// rebuild for fully archived-but-resumable sessions.
        pub async fn recover_live_session_for_realtime_open(
            &self,
            session_id: &SessionId,
        ) -> Result<(), SessionError> {
            if self.service.has_live_session(session_id).await? {
                return Ok(());
            }

            if self.staged_sessions.contains(session_id).await {
                Box::pin(self.materialize_staged_session_for_realtime_open(session_id)).await?;
                return Ok(());
            }

            let recovery_ctx = self.recovery_context();
            let session = recovery_ctx
                .load_persisted_session(session_id)
                .await?
                .ok_or_else(|| SessionError::NotFound {
                    id: session_id.clone(),
                })?;
            let keep_alive = session
                .session_metadata()
                .ok_or_else(|| {
                    SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                        "session {session_id} is missing session metadata"
                    )))
                })?
                .keep_alive;
            let recovery_overrides = SurfaceSessionRecoveryOverrides {
                keep_alive: Some(keep_alive),
                ..Default::default()
            };
            let recovered = recovery_ctx
                .recovered_create_request_with_runtime_binding_mode(
                    session_id,
                    session,
                    recovery_overrides,
                    RecoveryRuntimeBindingMode::LocalResources,
                )
                .await
                .map_err(recovery_error_to_session_error)?;
            let runtime_was_registered = recovered.runtime_was_registered;
            let admission = self.service.reserve_create_session_admission().await?;
            if let Err(error) = self
                .service
                .create_session_with_reserved_admission(recovered.request, admission)
                .await
            {
                self.cleanup_recovered_runtime_if_new(session_id, runtime_was_registered)
                    .await;
                return Err(error);
            }

            Ok(())
        }

        /// Project the owning live session into the provider-backed
        /// realtime open seam.
        pub async fn realtime_session_open_config(
            &self,
            session_id: &SessionId,
            turning_mode: meerkat_contracts::RealtimeTurningMode,
        ) -> Result<RealtimeSessionOpenConfig, SessionError> {
            Box::pin(self.recover_live_session_for_realtime_open(session_id)).await?;
            let session = match self
                .service
                .export_realtime_open_session_snapshot(session_id)
                .await
            {
                Ok(session) => session,
                Err(SessionError::NotFound { .. }) => {
                    Box::pin(self.recover_live_session_for_realtime_open(session_id)).await?;
                    self.service
                        .export_realtime_open_session_snapshot(session_id)
                        .await?
                }
                Err(error) => return Err(error),
            };
            let llm_identity = self.service.live_session_llm_identity(session_id).await?;
            let visible_tools = self.service.live_visible_tool_defs(session_id).await?;
            Ok(RealtimeSessionOpenConfig::new(
                turning_mode,
                llm_identity,
                visible_tools,
                realtime_projection_messages(&session)?,
            )
            .with_runtime_system_context(realtime_projection_runtime_system_context(&session)?)
            .with_system_prompt(
                match realtime_projection_root_system_message(&session)? {
                    Some(Message::System(system)) => Some(system.content),
                    // `realtime_projection_root_system_message` only ever yields a
                    // `Message::System` (or `None`); any other shape means there is
                    // no root system prompt to project onto the typed field.
                    _ => None,
                },
            ))
        }

        /// Build a live open config for a session that may be deferred
        /// (no turns yet).
        pub async fn live_open_config_for_session(
            &self,
            session_id: &SessionId,
            turning_mode: meerkat_contracts::RealtimeTurningMode,
        ) -> Result<RealtimeSessionOpenConfig, SessionError> {
            self.realtime_session_open_config(session_id, turning_mode)
                .await
        }

        /// Resolve the LLM identity that a new live channel will bind to
        /// before generated `live/open` admission records it.
        pub async fn live_llm_identity_for_session(
            &self,
            session_id: &SessionId,
        ) -> Result<SessionLlmIdentity, SessionError> {
            if let Some(info) = self
                .staged_sessions
                .try_info(session_id)
                .await
                .map_err(|err| SessionError::Agent(AgentError::InternalError(err.to_string())))?
            {
                return Ok(info.effective_llm_identity);
            }
            Box::pin(self.recover_live_session_for_realtime_open(session_id)).await?;
            match self.service.live_session_llm_identity(session_id).await {
                Ok(identity) => Ok(identity),
                Err(SessionError::NotFound { .. }) => {
                    Box::pin(self.recover_live_session_for_realtime_open(session_id)).await?;
                    self.service.live_session_llm_identity(session_id).await
                }
                Err(error) => Err(error),
            }
        }

        /// Pre-flight checks for `live/open` before any infra is minted.
        pub async fn precheck_live_open(
            &self,
            session_id: &SessionId,
        ) -> Result<(), LiveOpenPrecheckError> {
            let map_lookup_err = |err: SessionError| LiveOpenPrecheckError::SessionLookup {
                session_id: session_id.clone(),
                source: err,
            };
            if let Some(info) = self
                .staged_sessions
                .try_info(session_id)
                .await
                .map_err(|err| {
                    map_lookup_err(SessionError::Agent(AgentError::InternalError(
                        err.to_string(),
                    )))
                })?
            {
                return precheck_identity(&info.effective_llm_identity);
            }
            Box::pin(self.recover_live_session_for_realtime_open(session_id))
                .await
                .map_err(map_lookup_err)?;
            let identity = match self.service.live_session_llm_identity(session_id).await {
                Ok(identity) => identity,
                Err(SessionError::NotFound { .. }) => {
                    Box::pin(self.recover_live_session_for_realtime_open(session_id))
                        .await
                        .map_err(map_lookup_err)?;
                    self.service
                        .live_session_llm_identity(session_id)
                        .await
                        .map_err(map_lookup_err)?
                }
                Err(other) => return Err(map_lookup_err(other)),
            };
            precheck_identity(&identity)
        }

        /// Close a live channel after a config rejection.
        ///
        /// Every sub-step fault propagates as a typed
        /// [`LiveChannelCloseFailure`] so the caller records it in the
        /// propagation report instead of laundering it into tracing while
        /// counting the channel as cleanly closed.
        async fn close_live_channel_for_config_rejection(
            &self,
            host: &meerkat_live::LiveAdapterHost,
            session_id: &SessionId,
            channel_id: &meerkat_live::LiveChannelId,
            reason: meerkat_core::live_adapter::LiveConfigRejectionReason,
            context: &'static str,
        ) -> Result<(), LiveChannelCloseFailure> {
            let observation = host
                .signal_terminal_error_observed(
                    channel_id,
                    meerkat_core::live_adapter::LiveAdapterErrorCode::ConfigRejected { reason },
                )
                .await
                .map_err(|err| {
                    tracing::warn!(
                        target: "meerkat::session_runtime::live_orchestration",
                        ?channel_id,
                        ?session_id,
                        ?err,
                        context,
                        "failed to signal terminal error on live channel after config rejection"
                    );
                    LiveChannelCloseFailure::SignalFailed(err.to_string())
                })?;
            let authority = self
                .runtime_adapter
                .resolve_live_close_result(session_id, &observation)
                .await
                .map_err(|err| {
                    tracing::warn!(
                        target: "meerkat::session_runtime::live_orchestration",
                        ?channel_id,
                        ?session_id,
                        ?err,
                        context,
                        "live close authority rejected config-rejection terminal cleanup"
                    );
                    LiveChannelCloseFailure::CloseAuthorityRejected(err.to_string())
                })?;
            let Some(close_commit_authority) = authority.channel_close_commit_authority() else {
                tracing::warn!(
                    target: "meerkat::session_runtime::live_orchestration",
                    ?channel_id,
                    ?session_id,
                    context,
                    "live close authority omitted config-rejection host commit handoff"
                );
                return Err(LiveChannelCloseFailure::CommitHandoffMissing);
            };
            host.commit_channel_close_observation(&observation, close_commit_authority)
                .await
                .map_err(|err| {
                    tracing::warn!(
                        target: "meerkat::session_runtime::live_orchestration",
                        ?channel_id,
                        ?session_id,
                        ?err,
                        context,
                        "host close commit failed after config-rejection generated terminal cleanup"
                    );
                    LiveChannelCloseFailure::HostCommitFailed(err.to_string())
                })
        }

        /// Fan out `Refresh` (or `Close` if the new resolved model is no
        /// longer realtime-capable, or if the model/provider was
        /// swapped) to every active live channel. Per-channel faults are
        /// recorded as typed entries in the returned
        /// [`LiveConfigPropagationReport`] (`swap_failed`, `refresh_failed`,
        /// `close_failed`) — never swallowed via tracing alone.
        ///
        /// G5 (P1) revisited: a `config/patch agent.model` is a global
        /// policy change. Every session whose current live identity
        /// differs from the new global is hot-swapped to the new global
        /// (see [`should_apply_global_model_hot_swap`]). This includes
        /// sessions that pinned a model at `session/create` time —
        /// without a typed override marker on `SessionMetadata` we
        /// cannot reliably distinguish "user pinned at create" from
        /// "user reconfigured mid-session", and treating the global as
        /// authoritative is the correct default for an explicit global
        /// policy change. Sessions that need a sticky override should
        /// issue a session-scoped reconfigure after the patch.
        pub async fn propagate_config_to_live_channels(&self) -> LiveConfigPropagationReport {
            let mut report = LiveConfigPropagationReport::default();
            let Some(host) = self.host.as_ref() else {
                return report;
            };
            let channels = host.active_channels().await;
            let mut unique_sessions: Vec<SessionId> = Vec::new();
            for channel_id in &channels {
                if let Some(session_id) = self
                    .runtime_adapter
                    .live_session_for_active_channel(channel_id)
                    .await
                    && !unique_sessions.iter().any(|sid| sid == &session_id)
                {
                    unique_sessions.push(session_id);
                }
            }
            if !unique_sessions.is_empty()
                && let Some(runtime) = self.config_runtime.as_ref()
                && let Ok(snapshot) = runtime.get().await
            {
                let new_global_model = snapshot.config.agent.model.clone();
                for session_id in &unique_sessions {
                    let current_model =
                        match self.service.live_session_llm_identity(session_id).await {
                            Ok(identity) => identity.model,
                            Err(err) => {
                                report.skipped.push((
                                    session_id.clone(),
                                    LiveHotSwapSkipReason::IdentityLookupFailed(err.to_string()),
                                ));
                                continue;
                            }
                        };
                    // G5 revisited: skip only when the swap would be a
                    // no-op (`current_model == new_global_model`). The
                    // pure helper encodes the rule so it can be
                    // unit-tested in isolation; see its doc-comment for
                    // the s72 regression rationale.
                    if !should_apply_global_model_hot_swap(&current_model, &new_global_model) {
                        report
                            .skipped
                            .push((session_id.clone(), LiveHotSwapSkipReason::NoOpOrOverride));
                        continue;
                    }
                    let request = SessionLlmReconfigureRequest {
                        model: Some(new_global_model.clone()),
                        provider: None,
                        provider_params: None,
                        auth_binding: None,
                    };
                    if let Err(err) = self
                        .runtime_adapter
                        .reconfigure_session_llm_identity(session_id, request)
                        .await
                    {
                        report
                            .swap_failed
                            .push((session_id.clone(), err.to_string()));
                    } else {
                        report.swapped.push(session_id.clone());
                    }
                }
            }
            for channel_id in channels {
                let session_id = match self
                    .runtime_adapter
                    .live_session_for_active_channel(&channel_id)
                    .await
                {
                    Some(id) => id,
                    None => {
                        tracing::debug!(
                            target: "meerkat::session_runtime::live_orchestration",
                            ?channel_id,
                            "skipping live channel absent from generated active-channel authority"
                        );
                        // No SessionId is resolvable for this channel, so the
                        // failure is recorded against the channel via tracing
                        // above; there is no session key to attribute it to.
                        continue;
                    }
                };
                if let Err(precheck_err) = Box::pin(self.precheck_live_open(&session_id)).await {
                    tracing::info!(
                        target: "meerkat::session_runtime::live_orchestration",
                        ?channel_id,
                        ?session_id,
                        ?precheck_err,
                        "closing live channel: new resolution not realtime-capable"
                    );
                    let reason = meerkat_core::live_adapter::LiveConfigRejectionReason::NonRealtimeResolution {
                        detail: format!("{precheck_err:?}"),
                    };
                    match self
                        .close_live_channel_for_config_rejection(
                            host,
                            &session_id,
                            &channel_id,
                            reason,
                            "non_realtime",
                        )
                        .await
                    {
                        Ok(()) => report.closed.push(session_id.clone()),
                        Err(failure) => report.close_failed.push((session_id.clone(), failure)),
                    }
                    continue;
                }
                let open_config = match Box::pin(self.live_open_config_for_session(
                    &session_id,
                    meerkat_contracts::RealtimeTurningMode::ProviderManaged,
                ))
                .await
                {
                    Ok(config) => config,
                    Err(err) => {
                        tracing::warn!(
                            target: "meerkat::session_runtime::live_orchestration",
                            ?channel_id,
                            ?session_id,
                            ?err,
                            "failed to build refreshed open_config for live channel"
                        );
                        report.refresh_failed.push((
                            session_id.clone(),
                            LiveChannelRefreshFailure::OpenConfigBuildFailed(err.to_string()),
                        ));
                        continue;
                    }
                };
                let bound_identity = match self
                    .runtime_adapter
                    .live_channel_bound_llm_identity(&session_id, &channel_id)
                    .await
                {
                    Ok(Some(identity)) => identity,
                    Ok(None) => {
                        tracing::warn!(
                            target: "meerkat::session_runtime::live_orchestration",
                            ?channel_id,
                            ?session_id,
                            "closing live channel: generated bound LLM identity authority is absent"
                        );
                        match self
                            .close_live_channel_for_config_rejection(
                                host,
                                &session_id,
                                &channel_id,
                                meerkat_core::live_adapter::LiveConfigRejectionReason::Other {
                                    detail:
                                        "missing generated live-channel bound identity authority"
                                            .to_string(),
                                },
                                "missing_generated_identity",
                            )
                            .await
                        {
                            Ok(()) => report.closed.push(session_id.clone()),
                            Err(failure) => {
                                report.close_failed.push((session_id.clone(), failure));
                            }
                        }
                        continue;
                    }
                    Err(err) => {
                        tracing::warn!(
                            target: "meerkat::session_runtime::live_orchestration",
                            ?channel_id,
                            ?session_id,
                            ?err,
                            "closing live channel: generated bound LLM identity authority lookup failed"
                        );
                        match self
                            .close_live_channel_for_config_rejection(
                                host,
                                &session_id,
                                &channel_id,
                                meerkat_core::live_adapter::LiveConfigRejectionReason::Other {
                                    detail: format!(
                                        "generated live-channel bound identity authority lookup failed: {err}"
                                    ),
                                },
                                "generated_identity_lookup_failed",
                            )
                            .await
                        {
                            Ok(()) => report.closed.push(session_id.clone()),
                            Err(failure) => {
                                report.close_failed.push((session_id.clone(), failure));
                            }
                        }
                        continue;
                    }
                };
                if live_channel_requires_close_for_identity_change(
                    &bound_identity,
                    &open_config.llm_identity,
                ) {
                    tracing::info!(
                        target: "meerkat::session_runtime::live_orchestration",
                        %channel_id,
                        %session_id,
                        old_model_id = %bound_identity.model,
                        new_model_id = %open_config.llm_identity.model,
                        old_provider_id = ?bound_identity.provider,
                        new_provider_id = ?open_config.llm_identity.provider,
                        reason = "model_swap",
                        "closing live channel: config patch swapped \
                         model/provider; SDK must reopen against new identity"
                    );
                    let reason =
                        meerkat_core::live_adapter::LiveConfigRejectionReason::ChannelIdentitySwap {
                            from_model: bound_identity.model.clone(),
                            from_provider: bound_identity.provider,
                            to_model: open_config.llm_identity.model.clone(),
                            to_provider: open_config.llm_identity.provider,
                        };
                    match self
                        .close_live_channel_for_config_rejection(
                            host,
                            &session_id,
                            &channel_id,
                            reason,
                            "model_swap",
                        )
                        .await
                    {
                        Ok(()) => report.closed.push(session_id.clone()),
                        Err(failure) => report.close_failed.push((session_id.clone(), failure)),
                    }
                    continue;
                }
                let mut snapshot =
                    build_live_projection_snapshot_for_runtime(&session_id, &open_config);
                match host.next_snapshot_version(&channel_id).await {
                    Ok(v) => snapshot.snapshot_version = v,
                    Err(err) => {
                        tracing::debug!(
                            target: "meerkat::session_runtime::live_orchestration",
                            ?channel_id,
                            ?session_id,
                            ?err,
                            "skipping live channel: snapshot version stamp failed"
                        );
                        report.refresh_failed.push((
                            session_id.clone(),
                            LiveChannelRefreshFailure::SnapshotVersionFailed(err.to_string()),
                        ));
                        continue;
                    }
                }
                match host.enqueue_refresh(&channel_id, snapshot).await {
                    Ok(acceptance) => {
                        if let Err(err) = self
                            .runtime_adapter
                            .resolve_live_refresh_queued_result(&session_id, &acceptance)
                            .await
                        {
                            tracing::warn!(
                                target: "meerkat::session_runtime::live_orchestration",
                                ?channel_id,
                                ?session_id,
                                ?err,
                                "live refresh queue acceptance was rejected by generated authority"
                            );
                            report.refresh_failed.push((
                                session_id.clone(),
                                LiveChannelRefreshFailure::QueueAcceptanceRejected(err.to_string()),
                            ));
                        } else {
                            report.refreshed.push(session_id.clone());
                        }
                    }
                    Err(err) => {
                        tracing::warn!(
                            target: "meerkat::session_runtime::live_orchestration",
                            ?channel_id,
                            ?session_id,
                            ?err,
                            "failed to enqueue Refresh command to live channel"
                        );
                        report.refresh_failed.push((
                            session_id.clone(),
                            LiveChannelRefreshFailure::EnqueueFailed(err.to_string()),
                        ));
                    }
                }
            }
            report
        }
    }

    /// Coerce a `RecoveryError` into a `SessionError` for the
    /// recovery-error-bearing entry points
    /// (`recover_live_session_for_realtime_open`). Each variant maps to
    /// the closest typed equivalent the surface-agnostic
    /// `SessionError` carries; surfaces that need richer translation
    /// can keep using `RecoveryContext` directly.
    fn recovery_error_to_session_error(
        error: crate::session_runtime::errors::RecoveryError,
    ) -> SessionError {
        use crate::session_runtime::errors::RecoveryError;
        match error {
            RecoveryError::Recovery(error) => SessionError::Agent(
                meerkat_core::error::AgentError::InternalError(error.to_string()),
            ),
            RecoveryError::BindingPreparation { .. } => SessionError::Agent(
                meerkat_core::error::AgentError::InternalError(error.to_string()),
            ),
            RecoveryError::Session(session_error) => session_error,
        }
    }
}

#[cfg(test)]
mod prompt_truth_tests {
    use super::build_live_projection_snapshot_for_runtime;
    use meerkat_core::types::{Message, SessionId, SystemMessage, UserMessage};
    use meerkat_core::{Provider, SessionLlmIdentity};
    use meerkat_llm_core::realtime_session::RealtimeSessionOpenConfig;

    fn test_identity() -> SessionLlmIdentity {
        SessionLlmIdentity {
            model: "gpt-realtime-2".to_string(),
            provider: Provider::OpenAI,
            provider_params: None,
            self_hosted_server_id: None,
            auth_binding: None,
        }
    }

    /// R10: the runtime-side snapshot builder must surface the typed
    /// `RealtimeSessionOpenConfig.system_prompt` field — the single owner of
    /// live prompt truth — and never re-derive it from `seed_messages[0]`.
    /// The seed leads with a NON-system message while the typed field carries
    /// the resolved prompt, proving the source is the typed field.
    #[test]
    fn runtime_snapshot_reads_typed_system_prompt_field_not_seed_messages() {
        let resolved_prompt = "you are a helpful meerkat".to_string();
        let open_config = RealtimeSessionOpenConfig::new(
            meerkat_contracts::RealtimeTurningMode::ProviderManaged,
            test_identity(),
            Vec::new(),
            vec![Message::User(UserMessage::text("hi".to_string()))],
        )
        .with_system_prompt(Some(resolved_prompt.clone()));
        let snapshot = build_live_projection_snapshot_for_runtime(&SessionId::new(), &open_config);
        assert_eq!(
            snapshot.system_prompt,
            Some(resolved_prompt),
            "snapshot.system_prompt must read the typed open_config field, not infer from seed_messages[0]"
        );
    }

    /// R10: absence of the typed field is an honest `None` — a stray seed
    /// `Message::System` must NOT be resurrected as prompt truth.
    #[test]
    fn runtime_snapshot_system_prompt_none_when_typed_field_absent() {
        let open_config = RealtimeSessionOpenConfig::new(
            meerkat_contracts::RealtimeTurningMode::ProviderManaged,
            test_identity(),
            Vec::new(),
            vec![Message::System(SystemMessage::new(
                "stray seed system message",
            ))],
        );
        let snapshot = build_live_projection_snapshot_for_runtime(&SessionId::new(), &open_config);
        assert_eq!(
            snapshot.system_prompt, None,
            "snapshot.system_prompt must mirror the absent typed field, not infer from seed_messages[0]"
        );
    }
}
