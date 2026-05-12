//! Live-channel orchestration.
//!
//! Populated by W2-A. Hosts the surface-agnostic helper free functions
//! that build live projection snapshots, extract realtime system
//! prompts, and decide when a live channel needs a forced close vs
//! in-place refresh.
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

/// R10: extract the root system prompt from a projected `seed_messages`
/// vector for use in `LiveProjectionSnapshot.system_prompt`.
///
/// Mirror of `extract_system_prompt_from_seed_messages` in
/// `meerkat-rpc::handlers::live`; duplicated here so the runtime-side
/// `propagate_config_to_live_channels` builder can populate the snapshot
/// without depending on handler-private helpers (matches the existing
/// duplication pattern between the two snapshot builders). See R10 in
/// `LIVE_ADAPTER_REVIEW2_TODO.md` for the full rationale.
///
/// **Why both `System` AND `SystemNotice` are valid lead messages.**
/// `realtime_projection_messages` only rewrites `seed_messages[0]` when
/// `realtime_projection_root_system_message` returns `Some` (resolved
/// system prompt or build instructions present); when that returns
/// `None`, the canonical session transcript's original first message is
/// left in place — and that can legitimately be a `Message::SystemNotice`
/// (e.g. an idle pre-prompt session whose only lead is a runtime-injected
/// typed MCP-pending notice). Without honoring `SystemNotice`
/// here, a `propagate_config_to_live_channels` refresh whose snapshot
/// leads with one would silently emit empty instructions on
/// `session.update` and wipe the realtime provider's session-level
/// instructions. We use `SystemNoticeMessage::model_projection_text()` so
/// the provider sees the internal projection without making prefix prose a
/// transcript contract again.
#[must_use]
pub fn extract_system_prompt_from_seed_messages_runtime(
    seed_messages: &[Message],
) -> Option<String> {
    match seed_messages.first()? {
        Message::System(system) => Some(system.content.clone()),
        Message::SystemNotice(notice) => Some(notice.model_projection_text()),
        _ => None,
    }
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
        // R10: extract the root system prompt from the first
        // `Message::System` entry in `seed_messages`.
        system_prompt: extract_system_prompt_from_seed_messages_runtime(&open_config.seed_messages),
        model_id: open_config.llm_identity.model.clone(),
        provider_id: open_config.llm_identity.provider.as_str().to_string(),
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
/// `bound_identity = None` means the channel was opened without identity
/// recording (degraded factory-less path). Treat as "no swap" so the
/// legacy Refresh fallthrough path still applies.
///
/// Audio-rate change is intentionally NOT checked here. The OpenAI
/// Refresh guard rejects it, but R11's typed runtime path is scoped to
/// model + provider until `audio_config` is plumbed into the projection
/// snapshot. Audio mismatches still surface as the existing async
/// `LiveAdapterErrorCode::ConfigRejected` error from the adapter.
#[must_use]
pub fn live_channel_requires_close_for_identity_change(
    bound_identity: Option<&SessionLlmIdentity>,
    new_identity: &SessionLlmIdentity,
) -> bool {
    match bound_identity {
        Some(prev) => prev.model != new_identity.model || prev.provider != new_identity.provider,
        None => false,
    }
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
/// differed from `prior_global_model`. That heuristic conflated two
/// distinct cases — (a) a session that explicitly chose its initial
/// model via `CreateSessionRequest.model` while the global differed, and
/// (b) a session that was later reconfigured via `llm_reconfigure`. Both
/// cases produced `current != prior_global`, but only (b) carries a
/// "sticky override" intent. Without a typed override marker on
/// `SessionMetadata` we cannot disambiguate; broadcasting the new global
/// is the correct default for `config/patch agent.model` because that
/// patch is itself a global policy change. Sessions that need a sticky
/// override should issue a session-scoped reconfigure after the patch.
///
/// `prior_global_model` is retained on the public signature so existing
/// callers (the RPC config/patch handler) don't have to be re-plumbed,
/// but it is no longer consulted: the rule now depends only on the
/// session's current model and the new global model.
#[must_use]
pub fn should_apply_global_model_hot_swap(
    current_session_model: &str,
    _prior_global_model: Option<&str>,
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

/// Build the projection-root system message for a realtime session. The
/// content is the union of the resolved `system_prompt` (or the first
/// existing `System`/`SystemNotice` lead) and any session-build
/// `additional_instructions`.
#[must_use]
pub fn realtime_projection_root_system_message(session: &Session) -> Option<Message> {
    let build_state = session.build_state().unwrap_or_default();
    let mut content = build_state
        .system_prompt
        .or_else(|| {
            session
                .messages()
                .first()
                .and_then(|message| match message {
                    Message::System(system) => Some(system.content.clone()),
                    Message::SystemNotice(notice) => Some(notice.model_projection_text()),
                    _ => None,
                })
        })
        .unwrap_or_default();

    if let Some(additional_instructions) = build_state.additional_instructions
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
        None
    } else {
        Some(Message::System(SystemMessage::new(content)))
    }
}

/// Project a session's transcript for realtime delivery: prepend or
/// rewrite the lead message with [`realtime_projection_root_system_message`]
/// when one is available.
#[must_use]
pub fn realtime_projection_messages(session: &Session) -> Vec<Message> {
    let mut projected = session.messages().to_vec();
    if let Some(root_system) = realtime_projection_root_system_message(session) {
        match projected.first() {
            Some(Message::System(_) | Message::SystemNotice(_)) => projected[0] = root_system,
            _ => projected.insert(0, root_system),
        }
    }
    projected
}

/// Project a session's runtime system context into the realtime
/// open-config shape (applied + pending appends concatenated).
#[must_use]
pub fn realtime_projection_runtime_system_context(
    session: &Session,
) -> Vec<PendingSystemContextAppend> {
    let state = session.system_context_state().unwrap_or_default();
    state.applied.into_iter().chain(state.pending).collect()
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
/// loop's stable owner key derivation for the builtin source. Used by
/// RPC tests; kept un-gated for the same reason as
/// [`exported_tool_visibility_state`].
#[must_use]
pub fn builtin_tool_visibility_witness() -> meerkat_core::ToolVisibilityWitness {
    let provenance = meerkat_core::ToolProvenance {
        kind: meerkat_core::ToolSourceKind::Builtin,
        source_id: "builtin".into(),
    };
    meerkat_core::ToolVisibilityWitness {
        stable_owner_key: Some(
            meerkat_core::tool_catalog::stable_owner_key_from_provenance(&provenance),
        ),
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
    use meerkat_core::types::{ContentInput, SessionId};
    use meerkat_core::{DeferredPromptPolicy, SurfaceSessionRecoveryOverrides};
    use meerkat_live::LiveAdapterHost;
    use meerkat_llm_core::realtime_session::RealtimeSessionOpenConfig;
    use meerkat_runtime::{
        MeerkatMachine, SessionLlmReconfigureHost, SessionLlmReconfigureRequest,
    };
    use meerkat_session::PersistentSessionService;

    use super::{
        build_live_projection_snapshot_for_runtime,
        live_channel_requires_close_for_identity_change, precheck_identity,
        realtime_projection_messages, realtime_projection_runtime_system_context,
        should_apply_global_model_hot_swap,
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
        /// Surface-supplied LLM reconfigure host used for the idle
        /// hot-swap branch of `propagate_config_to_live_channels`.
        pub llm_reconfigure_host: &'a dyn SessionLlmReconfigureHost,
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
            build.realm_id = build
                .realm_id
                .or_else(|| self.realm_id.map(ToString::to_string));
            build.instance_id = build
                .instance_id
                .or_else(|| self.instance_id.map(ToString::to_string));
            build.backend = build
                .backend
                .or_else(|| self.backend.map(ToString::to_string));
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
                render_metadata: None,
                system_prompt: build_config.system_prompt.clone(),
                max_tokens: build_config.max_tokens,
                event_tx: None,
                skill_references: None,
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
                .map(|metadata| metadata.keep_alive)
                .unwrap_or(false);
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
                realtime_projection_messages(&session),
            )
            .with_runtime_system_context(realtime_projection_runtime_system_context(&session)))
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

        /// Pre-flight checks for `live/open` before any infra is minted.
        pub async fn precheck_live_open(
            &self,
            session_id: &SessionId,
        ) -> Result<(), LiveOpenPrecheckError> {
            let map_lookup_err = |err: SessionError| LiveOpenPrecheckError::SessionLookup {
                session_id: session_id.clone(),
                source: err,
            };
            if let Some(info) = self.staged_sessions.info(session_id).await {
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

        /// Fan out `Refresh` (or `Close` if the new resolved model is no
        /// longer realtime-capable, or if the model/provider was
        /// swapped) to every active live channel. Best-effort:
        /// per-channel errors are swallowed via tracing.
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
        ///
        /// `prior_global_model` is retained on the signature so the
        /// `config/patch` handler does not have to be re-plumbed, but
        /// it is no longer consulted by the hot-swap rule.
        pub async fn propagate_config_to_live_channels(&self, prior_global_model: Option<&str>) {
            let Some(host) = self.host.as_ref() else {
                return;
            };
            let channels = host.active_channels().await;
            let mut unique_sessions: Vec<SessionId> = Vec::new();
            for channel_id in &channels {
                if let Ok(session_id) = host.channel_session(channel_id).await
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
                                tracing::debug!(
                                    target: "meerkat::session_runtime::live_orchestration",
                                    ?session_id,
                                    ?err,
                                    "live identity lookup failed during config \
                                     propagation; skipping global hot-swap"
                                );
                                continue;
                            }
                        };
                    // G5 revisited: skip only when the swap would be a
                    // no-op (`current_model == new_global_model`). The
                    // pure helper encodes the rule so it can be
                    // unit-tested in isolation; see its doc-comment for
                    // the s72 regression rationale.
                    if !should_apply_global_model_hot_swap(
                        &current_model,
                        prior_global_model,
                        &new_global_model,
                    ) {
                        tracing::debug!(
                            target: "meerkat::session_runtime::live_orchestration",
                            ?session_id,
                            current_model = %current_model,
                            prior_global_model = ?prior_global_model,
                            new_global_model = %new_global_model,
                            "skipping global hot-swap on config propagation \
                             (override or already-synced)"
                        );
                        continue;
                    }
                    let request = SessionLlmReconfigureRequest {
                        model: Some(new_global_model.clone()),
                        provider: None,
                        provider_params: None,
                        clear_provider_params: false,
                        auth_binding: None,
                        clear_auth_binding: false,
                    };
                    if let Err(err) = crate::session_runtime::llm_reconfigure::hot_swap_llm_client_on_idle_session(
                        self.llm_reconfigure_host,
                        session_id,
                        &request,
                    )
                    .await
                    {
                        tracing::debug!(
                            target: "meerkat::session_runtime::live_orchestration",
                            ?session_id,
                            ?err,
                            "hot-swap on config propagation failed; per-channel \
                             precheck will fall back to current session identity"
                        );
                    }
                }
            }
            for channel_id in channels {
                let session_id = match host.channel_session(&channel_id).await {
                    Ok(id) => id,
                    Err(err) => {
                        tracing::debug!(
                            target: "meerkat::session_runtime::live_orchestration",
                            ?channel_id,
                            ?err,
                            "live channel session lookup failed during config propagation"
                        );
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
                    let _ = host
                        .signal_terminal_error(
                            &channel_id,
                            meerkat_core::live_adapter::LiveAdapterErrorCode::ConfigRejected {
                                reason,
                            },
                        )
                        .await;
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
                        continue;
                    }
                };
                let bound_identity = match host.channel_llm_identity(&channel_id).await {
                    Ok(identity) => identity,
                    Err(err) => {
                        tracing::debug!(
                            target: "meerkat::session_runtime::live_orchestration",
                            ?channel_id,
                            ?session_id,
                            ?err,
                            "channel identity lookup failed; skipping live channel"
                        );
                        continue;
                    }
                };
                if let Some(prev) = bound_identity.as_ref()
                    && live_channel_requires_close_for_identity_change(
                        Some(prev),
                        &open_config.llm_identity,
                    )
                {
                    tracing::info!(
                        target: "meerkat::session_runtime::live_orchestration",
                        %channel_id,
                        %session_id,
                        old_model_id = %prev.model,
                        new_model_id = %open_config.llm_identity.model,
                        old_provider_id = ?prev.provider,
                        new_provider_id = ?open_config.llm_identity.provider,
                        reason = "model_swap",
                        "closing live channel: config patch swapped \
                         model/provider; SDK must reopen against new identity"
                    );
                    let reason =
                        meerkat_core::live_adapter::LiveConfigRejectionReason::ChannelIdentitySwap {
                            from_model: prev.model.clone(),
                            from_provider: prev.provider,
                            to_model: open_config.llm_identity.model.clone(),
                            to_provider: open_config.llm_identity.provider,
                        };
                    if let Err(err) = host
                        .signal_terminal_error(
                            &channel_id,
                            meerkat_core::live_adapter::LiveAdapterErrorCode::ConfigRejected {
                                reason,
                            },
                        )
                        .await
                    {
                        tracing::warn!(
                            target: "meerkat::session_runtime::live_orchestration",
                            ?channel_id,
                            ?session_id,
                            ?err,
                            "failed to signal terminal error on live channel after model swap"
                        );
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
                        continue;
                    }
                }
                if let Err(err) = host
                    .send_command(
                        &channel_id,
                        meerkat_core::live_adapter::LiveAdapterCommand::Refresh { snapshot },
                    )
                    .await
                {
                    tracing::warn!(
                        target: "meerkat::session_runtime::live_orchestration",
                        ?channel_id,
                        ?session_id,
                        ?err,
                        "failed to dispatch Refresh command to live channel"
                    );
                }
            }
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
