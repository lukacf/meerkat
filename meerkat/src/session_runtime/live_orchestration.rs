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
//! The load-bearing open, recovery, staged-materialization, configuration, and
//! propagation methods are owned by `LiveOrchestrator<'a>`. RPC supplies its
//! surface-specific adapters and policy inputs instead of reimplementing the
//! lifecycle orchestration.
//!
//! These free functions only depend on `meerkat-llm-core`, `meerkat-core`,
//! and the model catalog — they do NOT import `meerkat-live`, so they
//! compile unconditionally regardless of the `live` feature. The
//! `live` feature gates the [`crate::session_runtime::live_orchestration::LiveOrchestrator`]
//! methods that consume `LiveAdapterHost`.

use meerkat_core::error::AgentError;
use meerkat_core::service::SessionError;
use meerkat_core::types::{Message, SessionId, SystemMessage};
use meerkat_core::{
    PendingSystemContextAppend, Session, SessionLlmIdentity, SessionToolVisibilityState,
};
use meerkat_llm_core::realtime_session::RealtimeSessionOpenConfig;
use std::num::NonZeroUsize;

use crate::session_runtime::errors::LiveOpenPrecheckError;

/// Apply the B19 (realtime-capability) gate to a resolved LLM identity.
/// Shared between the staged-session and live-session branches of
/// `precheck_live_open` so both paths enforce identical catalog capability
/// contracts. B18 (provider has a wired live adapter) is owned by the concrete
/// realtime factory at the live-open surface because the factory mints the
/// adapter.
pub fn precheck_identity(identity: &SessionLlmIdentity) -> Result<(), LiveOpenPrecheckError> {
    let realtime_capable = meerkat_models::capabilities_for(identity.provider, &identity.model)
        .map(|caps| caps.realtime)
        .unwrap_or(false);
    apply_precheck_gates(identity.provider, &identity.model, realtime_capable)
}

/// Pure B19 helper: model realtime capability is catalog-owned. Provider
/// adapter support is deliberately not checked here; the
/// [`RealtimeSessionFactory`](meerkat_llm_core::realtime_session::RealtimeSessionFactory)
/// support predicate owns that fact.
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
        user_content_identities: open_config.user_content_identities.clone(),
        user_content_tombstones: open_config.user_content_tombstones.clone(),
        canonical_user_image_decoded_bytes: open_config.canonical_user_image_decoded_bytes,
        transcript_rewrite_generation: open_config.transcript_rewrite_generation,
    }
}

/// Pure helper deciding whether a newly resolved live identity represents
/// a channel-bound identity swap relative to the identity the channel was
/// opened with.
///
/// Returns `true` when the channel must be closed (so the SDK can reopen
/// against the new identity); `false` when an in-place `Refresh` is safe.
///
/// Callers must obtain `bound_identity` from generated live-open admission
/// authority. Missing generated identity is handled as a fail-closed channel
/// close before this comparison is reached.
///
/// Provider params are intentionally NOT checked here. Provider parameters
/// are projected through in-place refresh semantics and do not necessarily
/// require a new provider connection. The durable auth binding is checked
/// because live adapters resolve credentials at open/attach time; a changed
/// binding means the already-open provider session may be authenticated with
/// stale credentials and must be closed + reopened.
///
/// Audio-rate change is intentionally NOT checked here. The OpenAI
/// Refresh guard rejects it, but R11's typed runtime path is scoped to
/// channel-bound LLM identity until `audio_config` is plumbed into the
/// projection snapshot. Audio mismatches still surface as the existing async
/// `LiveAdapterErrorCode::ConfigRejected` error from the adapter.
#[must_use]
pub fn live_channel_requires_close_for_identity_change(
    bound_identity: &SessionLlmIdentity,
    new_identity: &SessionLlmIdentity,
) -> bool {
    bound_identity.model != new_identity.model
        || bound_identity.provider != new_identity.provider
        || bound_identity.auth_binding != new_identity.auth_binding
}

#[cfg(all(
    feature = "session-store",
    feature = "live",
    not(target_arch = "wasm32")
))]
fn live_channel_identity_swap_reason(
    bound_identity: &SessionLlmIdentity,
    new_identity: &SessionLlmIdentity,
) -> meerkat_core::live_adapter::LiveConfigRejectionReason {
    meerkat_core::live_adapter::LiveConfigRejectionReason::ChannelIdentitySwap {
        from_model: bound_identity.model.clone(),
        from_provider: bound_identity.provider,
        to_model: new_identity.model.clone(),
        to_provider: new_identity.provider,
        auth_binding_changed: bound_identity.auth_binding != new_identity.auth_binding,
    }
}

#[cfg(all(
    feature = "session-store",
    feature = "live",
    not(target_arch = "wasm32")
))]
fn live_channel_identity_swap_context(
    bound_identity: &SessionLlmIdentity,
    new_identity: &SessionLlmIdentity,
) -> &'static str {
    if bound_identity.model == new_identity.model
        && bound_identity.provider == new_identity.provider
        && bound_identity.auth_binding != new_identity.auth_binding
    {
        "auth_binding_swap"
    } else {
        "model_swap"
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

/// Build the projection-root system message for a realtime session from the
/// exact assembled bytes persisted by the canonical factory prompt owner.
///
/// Runtime-appended system context is deliberately excluded here. It travels
/// through the typed `RealtimeSessionOpenConfig.runtime_system_context` field,
/// so open/refresh cannot duplicate that context into the assembled base.
pub fn realtime_projection_root_system_message(
    session: &Session,
) -> Result<Option<Message>, SessionError> {
    let build_state = session.build_state().ok_or_else(|| {
        SessionError::Agent(AgentError::InternalError(format!(
            "session {} is missing session build state",
            session.id()
        )))
    })?;
    let content = build_state.assembled_system_prompt.ok_or_else(|| {
        SessionError::Agent(AgentError::InternalError(format!(
            "session {} is missing its canonical assembled system prompt",
            session.id()
        )))
    })?;

    if content.is_empty() {
        Ok(None)
    } else {
        // Projection must not mint ephemeral metadata: seed-window sizing is
        // a pure function of canonical session state. Reuse the durable lead
        // timestamp when there is one, otherwise the stable session creation
        // timestamp. `SystemMessage::new` would stamp wall-clock time and make
        // an exact-boundary window nondeterministic across identical opens.
        let created_at = match session.messages().first() {
            Some(Message::System(system)) => system.created_at,
            Some(Message::SystemNotice(notice)) => notice.created_at,
            _ => session.created_at().into(),
        };
        Ok(Some(Message::System(SystemMessage {
            content,
            mutation_kind: meerkat_core::SystemPromptMutationKind::Unspecified,
            created_at,
        })))
    }
}

/// Caller-selected bound for the canonical transcript seed used by a live
/// provider open. The count is over the serialized projected messages, after
/// root-prompt resolution and image hydration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LiveSeedWindow {
    max_chars: NonZeroUsize,
}

impl LiveSeedWindow {
    pub fn new(max_chars: usize) -> Result<Self, LiveSeedProjectionError> {
        NonZeroUsize::new(max_chars)
            .map(|max_chars| Self { max_chars })
            .ok_or(LiveSeedProjectionError::ZeroWindow)
    }

    #[must_use]
    pub fn max_chars(self) -> usize {
        self.max_chars.get()
    }
}

/// Completeness of a provider replay seed relative to canonical history.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveSeedProjectionStatus {
    Complete,
    Windowed {
        dropped_messages: usize,
        included_compaction_summary: bool,
    },
}

impl LiveSeedProjectionStatus {
    #[must_use]
    pub fn has_known_gaps(self) -> bool {
        matches!(self, Self::Windowed { .. })
    }
}

/// Selected realtime seed plus the typed completeness fact consumed by the
/// live/open continuity projection.
#[derive(Debug, Clone)]
pub struct LiveSeedMessageProjection {
    pub messages: Vec<Message>,
    pub status: LiveSeedProjectionStatus,
}

/// Failures selecting a bounded live seed before any channel/provider state is
/// minted.
#[derive(Debug, thiserror::Error)]
pub enum LiveSeedProjectionError {
    #[error(transparent)]
    Session(#[from] SessionError),
    #[error("live seed window must be greater than zero")]
    ZeroWindow,
    #[error(
        "live seed root requires {required_chars} serialized characters, exceeding the requested {max_chars}-character window"
    )]
    RootExceedsWindow {
        required_chars: usize,
        max_chars: usize,
    },
    #[error("failed to serialize live seed projection: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("live seed projection size overflowed usize")]
    SizeOverflow,
}

/// Provider-neutral open projection: the mechanical provider config plus the
/// canonical seed-completeness fact used by public continuity reporting.
#[derive(Debug, Clone)]
pub struct RealtimeSessionOpenProjection {
    pub open_config: RealtimeSessionOpenConfig,
    pub seed_status: LiveSeedProjectionStatus,
}

/// Typed failure at the live-open projection boundary. Surfaces classify seed
/// policy rejection directly instead of recovering its class from strings or
/// JSON-shaped session errors.
#[derive(Debug, thiserror::Error)]
pub enum RealtimeSessionOpenProjectionError {
    #[error(transparent)]
    Session(#[from] SessionError),
    #[error(transparent)]
    Seed(#[from] LiveSeedProjectionError),
}

fn realtime_projection_messages_full_with_root_resolution(
    session: &Session,
) -> Result<(Vec<Message>, bool), SessionError> {
    let mut projected = session.messages().to_vec();
    let resolved_root = realtime_projection_root_system_message(session)?;
    let has_resolved_root = resolved_root.is_some();
    match resolved_root {
        Some(root_system) => match projected.first() {
            Some(Message::System(_) | Message::SystemNotice(_)) => projected[0] = root_system,
            _ => projected.insert(0, root_system),
        },
        None => {
            // An exactly empty canonical base owns the absence of a seed
            // system message. Any transcript lead is stale base text or the
            // runtime-context compatibility projection; runtime context is
            // already carried by the typed open-config field.
            if matches!(
                projected.first(),
                Some(Message::System(_) | Message::SystemNotice(_))
            ) {
                projected.remove(0);
            }
        }
    }
    Ok((projected, has_resolved_root))
}

fn realtime_projection_messages_full(session: &Session) -> Result<Vec<Message>, SessionError> {
    realtime_projection_messages_full_with_root_resolution(session).map(|(projected, _)| projected)
}

/// Project a session's complete transcript for realtime delivery. This keeps
/// the pre-seed-window behavior for Rust callers that do not request a bound.
pub fn realtime_projection_messages(session: &Session) -> Result<Vec<Message>, SessionError> {
    realtime_projection_messages_full(session)
}

fn serialized_message_chars(message: &Message) -> Result<usize, LiveSeedProjectionError> {
    Ok(serde_json::to_string(message)?.chars().count())
}

fn checked_message_chars(
    costs: &[usize],
    mut range: std::ops::Range<usize>,
) -> Result<usize, LiveSeedProjectionError> {
    range.try_fold(0usize, |total, index| {
        total
            .checked_add(costs[index])
            .ok_or(LiveSeedProjectionError::SizeOverflow)
    })
}

/// Select a bounded, deterministic projection. Existing typed compaction
/// summary content is the optional head; the tail is retained only at complete
/// conversational-turn boundaries, with contiguous injected context glued to
/// the user message it accompanied.
pub fn realtime_projection_messages_with_window(
    session: &Session,
    window: LiveSeedWindow,
) -> Result<LiveSeedMessageProjection, LiveSeedProjectionError> {
    let (projected, has_resolved_root) =
        realtime_projection_messages_full_with_root_resolution(session)?;
    let costs = projected
        .iter()
        .map(serialized_message_chars)
        .collect::<Result<Vec<_>, _>>()?;
    let total_chars = checked_message_chars(&costs, 0..costs.len())?;
    if total_chars <= window.max_chars() {
        return Ok(LiveSeedMessageProjection {
            messages: projected,
            status: LiveSeedProjectionStatus::Complete,
        });
    }

    let root_len = usize::from(has_resolved_root);
    let root_chars = checked_message_chars(&costs, 0..root_len)?;
    if root_chars > window.max_chars() {
        return Err(LiveSeedProjectionError::RootExceedsWindow {
            required_chars: root_chars,
            max_chars: window.max_chars(),
        });
    }

    let mut selected = vec![false; projected.len()];
    selected
        .iter_mut()
        .take(root_len)
        .for_each(|keep| *keep = true);
    let mut remaining = window.max_chars() - root_chars;

    let summary_index = (root_len..projected.len()).rev().find(|index| {
        matches!(
            &projected[*index],
            Message::User(user) if user.transcript_role.is_compaction_summary()
        )
    });
    let included_compaction_summary = summary_index.is_some_and(|index| {
        if costs[index] <= remaining {
            selected[index] = true;
            remaining -= costs[index];
            true
        } else {
            false
        }
    });
    let tail_start = summary_index.map_or(root_len, |index| index + 1);

    let mut turn_starts = Vec::new();
    for index in tail_start..projected.len() {
        if matches!(
            &projected[index],
            Message::User(user) if user.transcript_role.is_conversational()
        ) {
            let mut start = index;
            while start > tail_start
                && matches!(
                    &projected[start - 1],
                    Message::User(user) if user.transcript_role.is_injected_context()
                )
            {
                start -= 1;
            }
            turn_starts.push(start);
        }
    }

    if !turn_starts.is_empty() {
        let mut retained_suffix_start = None;
        for turn_index in (0..turn_starts.len()).rev() {
            let start = turn_starts[turn_index];
            let end = turn_starts
                .get(turn_index + 1)
                .copied()
                .unwrap_or(projected.len());
            let turn_chars = checked_message_chars(&costs, start..end)?;
            if turn_chars > remaining {
                break;
            }
            remaining -= turn_chars;
            retained_suffix_start = Some(start);
        }
        if let Some(start) = retained_suffix_start {
            selected
                .iter_mut()
                .take(projected.len())
                .skip(start)
                .for_each(|keep| *keep = true);
        }
    }

    let retained_count = selected.iter().filter(|keep| **keep).count();
    let dropped_messages = projected.len().saturating_sub(retained_count);
    let messages = projected
        .into_iter()
        .zip(selected)
        .filter_map(|(message, keep)| keep.then_some(message))
        .collect();
    Ok(LiveSeedMessageProjection {
        messages,
        status: LiveSeedProjectionStatus::Windowed {
            dropped_messages,
            included_compaction_summary,
        },
    })
}

#[cfg(all(
    feature = "session-store",
    feature = "live",
    not(target_arch = "wasm32")
))]
fn open_projection_error_to_compat_session_error(
    error: RealtimeSessionOpenProjectionError,
) -> SessionError {
    match error {
        RealtimeSessionOpenProjectionError::Session(error)
        | RealtimeSessionOpenProjectionError::Seed(LiveSeedProjectionError::Session(error)) => {
            error
        }
        RealtimeSessionOpenProjectionError::Seed(error) => {
            SessionError::Agent(AgentError::InternalError(error.to_string()))
        }
    }
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
    use meerkat_core::{
        DeferredPromptPolicy, RealtimeOpenProjectionAdmission, SessionLlmIdentity,
        SurfaceSessionRecoveryOverrides,
    };
    use meerkat_live::LiveAdapterHost;
    use meerkat_llm_core::realtime_session::RealtimeSessionOpenConfig;
    use meerkat_runtime::{MeerkatMachine, SessionLlmReconfigureRequest, SessionServiceRuntimeExt};
    use meerkat_session::PersistentSessionService;

    use super::{
        LiveChannelCloseFailure, LiveChannelRefreshFailure, LiveConfigPropagationReport,
        LiveHotSwapSkipReason, LiveSeedMessageProjection, LiveSeedProjectionStatus, LiveSeedWindow,
        RealtimeSessionOpenProjection, RealtimeSessionOpenProjectionError,
        build_live_projection_snapshot_for_runtime, live_channel_identity_swap_context,
        live_channel_identity_swap_reason, live_channel_requires_close_for_identity_change,
        open_projection_error_to_compat_session_error, precheck_identity,
        realtime_projection_messages, realtime_projection_messages_with_window,
        realtime_projection_root_system_message, realtime_projection_runtime_system_context,
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
        ) -> Result<(), SessionError> {
            if runtime_was_registered {
                return Ok(());
            }
            self.archive_runtime_cleanup.run(session_id).await
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
                deferred_injected_context,
                ..
            } = slot;
            // Realtime-open promotion stages the deferred prompt as a pending
            // continuation, and the pending-continuation turn lane rejects
            // injected context (there is no StartTurnRequest to carry it).
            // Fail closed rather than silently dropping the deferred
            // injected context; `promotion_cleanup` restores the staged slot
            // on this early return, so a later `turn/start` promotion still
            // delivers it.
            if !deferred_injected_context.is_empty() {
                return Err(SessionError::Unsupported(
                    "a deferred session created with injected_context cannot be promoted by \
                     realtime open; promote it with turn/start"
                        .to_string(),
                ));
            }
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
                injected_context: Vec::new(),
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
                    if let Err(replenish_error) = promotion_cleanup
                        .replenish_staged_capacity_admission(self.service)
                        .await
                    {
                        promotion_cleanup.restore_now().await;
                        return Err(combine_staged_materialization_replenish_errors(
                            error,
                            replenish_error,
                        ));
                    }
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
                return match self
                    .cleanup_recovered_runtime_if_new(session_id, runtime_was_registered)
                    .await
                {
                    Ok(()) => Err(error),
                    Err(cleanup_error) => Err(combine_recovery_materialization_cleanup_errors(
                        error,
                        cleanup_error,
                    )),
                };
            }

            Ok(())
        }

        /// Project the owning live session into the provider-backed realtime
        /// open seam, optionally selecting a bounded canonical seed.
        pub async fn realtime_session_open_projection(
            &self,
            session_id: &SessionId,
            turning_mode: meerkat_contracts::RealtimeTurningMode,
            seed_window: Option<LiveSeedWindow>,
        ) -> Result<RealtimeSessionOpenProjection, RealtimeSessionOpenProjectionError> {
            // Acquire process-wide custody before the persistent service can
            // hydrate blob-backed image history. The take-once slot carried on
            // the returned config transfers this same lease through provider
            // seed acknowledgement; no payload-bearing waiter is queued.
            let open_projection_lease = RealtimeOpenProjectionAdmission::global()
                .try_acquire()
                .map_err(|error| {
                    SessionError::Agent(AgentError::InternalError(error.to_string()))
                })?;
            Box::pin(self.recover_live_session_for_realtime_open(session_id)).await?;
            let (session, canonical_user_image_decoded_bytes) = match self
                .service
                .export_realtime_open_session_snapshot_with_image_usage(session_id)
                .await
            {
                Ok(snapshot) => snapshot,
                Err(SessionError::NotFound { .. }) => {
                    Box::pin(self.recover_live_session_for_realtime_open(session_id)).await?;
                    self.service
                        .export_realtime_open_session_snapshot_with_image_usage(session_id)
                        .await?
                }
                Err(error) => return Err(error.into()),
            };
            let llm_identity = self.service.live_session_llm_identity(session_id).await?;
            let visible_tools = self.service.live_visible_tool_defs(session_id).await?;
            let transcript_rewrite_generation = session
                .transcript_rewrite_generation()
                .map_err(|err| SessionError::Agent(AgentError::InternalError(err.to_string())))?;
            let seed_projection = match seed_window {
                Some(window) => realtime_projection_messages_with_window(&session, window)?,
                None => LiveSeedMessageProjection {
                    messages: realtime_projection_messages(&session)?,
                    status: LiveSeedProjectionStatus::Complete,
                },
            };
            let open_config =
                RealtimeSessionOpenConfig::new(
                    turning_mode,
                    llm_identity,
                    visible_tools,
                    seed_projection.messages,
                )
                .with_open_projection_lease(open_projection_lease)
                .with_runtime_system_context(realtime_projection_runtime_system_context(&session)?)
                .with_user_content_identities(session.realtime_user_content_identities())
                .with_user_content_tombstones(session.realtime_user_content_tombstones())
                .with_canonical_user_image_decoded_bytes(canonical_user_image_decoded_bytes)
                .with_transcript_rewrite_generation(transcript_rewrite_generation)
                .with_system_prompt(match realtime_projection_root_system_message(&session)? {
                    Some(Message::System(system)) => Some(system.content),
                    // `realtime_projection_root_system_message` only ever yields a
                    // `Message::System` (or `None`); any other shape means there is
                    // no root system prompt to project onto the typed field.
                    _ => None,
                });
            Ok(RealtimeSessionOpenProjection {
                open_config,
                seed_status: seed_projection.status,
            })
        }

        /// Compatibility wrapper retaining the pre-window full-history config.
        pub async fn realtime_session_open_config(
            &self,
            session_id: &SessionId,
            turning_mode: meerkat_contracts::RealtimeTurningMode,
        ) -> Result<RealtimeSessionOpenConfig, SessionError> {
            self.realtime_session_open_projection(session_id, turning_mode, None)
                .await
                .map(|projection| projection.open_config)
                .map_err(open_projection_error_to_compat_session_error)
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

        /// Build a live-open projection with an optional per-open seed window.
        pub async fn live_open_projection_for_session(
            &self,
            session_id: &SessionId,
            turning_mode: meerkat_contracts::RealtimeTurningMode,
            seed_window: Option<LiveSeedWindow>,
        ) -> Result<RealtimeSessionOpenProjection, RealtimeSessionOpenProjectionError> {
            self.realtime_session_open_projection(session_id, turning_mode, seed_window)
                .await
        }

        /// Build an in-place refresh projection without hydrating or replaying
        /// canonical history. Provider refresh consumes identity/tools/prompt
        /// only; open/reconnect remains the sole history hydration boundary.
        pub async fn live_refresh_config_for_session(
            &self,
            session_id: &SessionId,
            turning_mode: meerkat_contracts::RealtimeTurningMode,
        ) -> Result<RealtimeSessionOpenConfig, SessionError> {
            Box::pin(self.recover_live_session_for_realtime_open(session_id)).await?;
            let session = match self
                .service
                .export_realtime_refresh_session_snapshot(session_id)
                .await
            {
                Ok(session) => session,
                Err(SessionError::NotFound { .. }) => {
                    Box::pin(self.recover_live_session_for_realtime_open(session_id)).await?;
                    self.service
                        .export_realtime_refresh_session_snapshot(session_id)
                        .await?
                }
                Err(error) => return Err(error),
            };
            let llm_identity = self.service.live_session_llm_identity(session_id).await?;
            let visible_tools = self.service.live_visible_tool_defs(session_id).await?;
            let transcript_rewrite_generation = session
                .transcript_rewrite_generation()
                .map_err(|err| SessionError::Agent(AgentError::InternalError(err.to_string())))?;
            Ok(RealtimeSessionOpenConfig::new(
                turning_mode,
                llm_identity,
                visible_tools,
                Vec::new(),
            )
            .with_runtime_system_context(realtime_projection_runtime_system_context(&session)?)
            .with_user_content_identities(session.realtime_user_content_identities())
            .with_user_content_tombstones(session.realtime_user_content_tombstones())
            .with_transcript_rewrite_generation(transcript_rewrite_generation)
            .with_system_prompt(
                match realtime_projection_root_system_message(&session)? {
                    Some(Message::System(system)) => Some(system.content),
                    _ => None,
                },
            ))
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

        /// Close active live channels for a session after its durable LLM
        /// identity changes in a way the provider session cannot refresh in
        /// place. This is the session-scoped sibling of
        /// `propagate_config_to_live_channels`: both paths compare the
        /// generated live-open bound identity against the new session identity
        /// and close through generated live-close authority.
        pub async fn close_live_channels_for_identity_change(
            &self,
            session_id: &SessionId,
            new_identity: &SessionLlmIdentity,
        ) -> LiveConfigPropagationReport {
            let mut report = LiveConfigPropagationReport::default();
            let Some(host) = self.host.as_ref() else {
                return report;
            };
            let channels = host.active_channels().await;
            for channel_id in channels {
                let Some(channel_session_id) = self
                    .runtime_adapter
                    .live_session_for_active_channel(&channel_id)
                    .await
                else {
                    continue;
                };
                if &channel_session_id != session_id {
                    continue;
                }
                let bound_identity = match self
                    .runtime_adapter
                    .live_channel_bound_llm_identity(session_id, &channel_id)
                    .await
                {
                    Ok(Some(identity)) => identity,
                    Ok(None) => {
                        let reason = meerkat_core::live_adapter::LiveConfigRejectionReason::Other {
                            detail: "missing generated live-channel bound identity authority"
                                .to_string(),
                        };
                        match self
                            .close_live_channel_for_config_rejection(
                                host,
                                session_id,
                                &channel_id,
                                reason,
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
                        let reason = meerkat_core::live_adapter::LiveConfigRejectionReason::Other {
                            detail: format!(
                                "generated live-channel bound identity authority lookup failed: {err}"
                            ),
                        };
                        match self
                            .close_live_channel_for_config_rejection(
                                host,
                                session_id,
                                &channel_id,
                                reason,
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
                if !live_channel_requires_close_for_identity_change(&bound_identity, new_identity) {
                    report
                        .skipped
                        .push((session_id.clone(), LiveHotSwapSkipReason::NoOpOrOverride));
                    continue;
                }
                let reason = live_channel_identity_swap_reason(&bound_identity, new_identity);
                let context = live_channel_identity_swap_context(&bound_identity, new_identity);
                match self
                    .close_live_channel_for_config_rejection(
                        host,
                        session_id,
                        &channel_id,
                        reason,
                        context,
                    )
                    .await
                {
                    Ok(()) => report.closed.push(session_id.clone()),
                    Err(failure) => report.close_failed.push((session_id.clone(), failure)),
                }
            }
            report
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
                let open_config = match Box::pin(self.live_refresh_config_for_session(
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
                    let context = live_channel_identity_swap_context(
                        &bound_identity,
                        &open_config.llm_identity,
                    );
                    tracing::info!(
                        target: "meerkat::session_runtime::live_orchestration",
                        %channel_id,
                        %session_id,
                        old_model_id = %bound_identity.model,
                        new_model_id = %open_config.llm_identity.model,
                        old_provider_id = ?bound_identity.provider,
                        new_provider_id = ?open_config.llm_identity.provider,
                        old_auth_binding = ?bound_identity.auth_binding,
                        new_auth_binding = ?open_config.llm_identity.auth_binding,
                        reason = context,
                        "closing live channel: resolved live identity changed; \
                         SDK must reopen against new identity"
                    );
                    let reason = live_channel_identity_swap_reason(
                        &bound_identity,
                        &open_config.llm_identity,
                    );
                    match self
                        .close_live_channel_for_config_rejection(
                            host,
                            &session_id,
                            &channel_id,
                            reason,
                            context,
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

    fn combine_recovery_materialization_cleanup_errors(
        primary_error: SessionError,
        cleanup_error: SessionError,
    ) -> SessionError {
        SessionError::Agent(AgentError::InternalError(format!(
            "{primary_error}; additionally failed to clean up newly recovered runtime: {cleanup_error}"
        )))
    }

    fn combine_staged_materialization_replenish_errors(
        primary_error: SessionError,
        replenish_error: SessionError,
    ) -> SessionError {
        SessionError::Agent(AgentError::InternalError(format!(
            "{primary_error}; additionally failed to replenish staged capacity before materialization rollback: {replenish_error}"
        )))
    }

    #[cfg(test)]
    mod tests {
        use super::{
            combine_recovery_materialization_cleanup_errors,
            combine_staged_materialization_replenish_errors,
        };
        use meerkat_core::error::AgentError;
        use meerkat_core::service::SessionError;

        #[test]
        fn recovery_materialization_error_retains_cleanup_failure() {
            let combined = combine_recovery_materialization_cleanup_errors(
                SessionError::Agent(AgentError::InternalError(
                    "synthetic materialization failure".to_string(),
                )),
                SessionError::Agent(AgentError::InternalError(
                    "synthetic unregister failure".to_string(),
                )),
            );
            let rendered = combined.to_string();
            assert!(rendered.contains("synthetic materialization failure"));
            assert!(rendered.contains("synthetic unregister failure"));
        }

        #[test]
        fn staged_materialization_error_retains_replenish_failure() {
            let combined = combine_staged_materialization_replenish_errors(
                SessionError::Agent(AgentError::InternalError(
                    "synthetic materialization failure".to_string(),
                )),
                SessionError::Agent(AgentError::InternalError(
                    "synthetic capacity replenish failure".to_string(),
                )),
            );
            let rendered = combined.to_string();
            assert!(rendered.contains("synthetic materialization failure"));
            assert!(rendered.contains("synthetic capacity replenish failure"));
        }
    }
}

#[cfg(test)]
mod prompt_truth_tests {
    use super::{
        LiveSeedProjectionError, LiveSeedProjectionStatus, LiveSeedWindow,
        build_live_projection_snapshot_for_runtime, realtime_projection_messages,
        realtime_projection_messages_with_window, realtime_projection_root_system_message,
        realtime_projection_runtime_system_context, serialized_message_chars,
    };
    use meerkat_core::types::{
        AssistantBlock, BlockAssistantMessage, Message, SessionId, StopReason, SystemMessage,
        UserMessage,
    };
    use meerkat_core::{
        PendingSystemContextAppend, Provider, Session, SessionBuildState, SessionLlmIdentity,
        SystemPromptOverride, lifecycle::run_primitive::CoreRenderable,
        session::SystemContextSource,
    };
    use meerkat_llm_core::realtime_session::RealtimeSessionOpenConfig;
    use std::time::SystemTime;

    fn test_identity() -> SessionLlmIdentity {
        SessionLlmIdentity {
            model: "gpt-realtime-2".to_string(),
            provider: Provider::OpenAI,
            provider_params: None,
            self_hosted_server_id: None,
            auth_binding: None,
        }
    }

    fn assistant_text(content: &str) -> Message {
        Message::BlockAssistant(BlockAssistantMessage::new(
            vec![AssistantBlock::Text {
                text: content.to_string(),
                meta: None,
            }],
            StopReason::EndTurn,
        ))
    }

    fn window_test_session() -> Session {
        let mut session = Session::new();
        session
            .set_build_state(SessionBuildState {
                assembled_system_prompt: Some("resolved root".to_string()),
                ..Default::default()
            })
            .expect("test build state must serialize");
        session.push_batch(vec![
            Message::System(SystemMessage::new("resolved root")),
            Message::User(UserMessage::compaction_summary("prior history summary")),
            Message::User(UserMessage::injected_context("old injected context")),
            Message::User(UserMessage::text("old user turn")),
            assistant_text("old assistant turn"),
            Message::User(UserMessage::injected_context("new injected context")),
            Message::User(UserMessage::text("new user turn")),
            assistant_text("new assistant turn"),
        ]);
        session
    }

    fn disabled_root_test_session() -> Session {
        let mut session = Session::new();
        session
            .set_build_state(SessionBuildState {
                system_prompt: SystemPromptOverride::Disable,
                assembled_system_prompt: Some(String::new()),
                ..Default::default()
            })
            .expect("test build state must serialize");
        session.push_batch(vec![
            Message::System(SystemMessage::new("stale disabled root")),
            Message::User(UserMessage::text("current user turn")),
            assistant_text("current assistant turn"),
        ]);
        session
    }

    fn projected_system_content(session: &Session) -> String {
        match realtime_projection_root_system_message(session)
            .expect("canonical live root projection")
            .expect("non-empty canonical live root")
        {
            Message::System(system) => system.content,
            other => panic!("expected a system root, got {other:?}"),
        }
    }

    #[test]
    fn live_root_uses_exact_assembled_bytes_not_raw_set_intent() {
        let canonical =
            "request base\n\nskill inventory\n\nadditional instruction\n\ndispatcher tools";
        let mut session = Session::new();
        session
            .set_build_state(SessionBuildState {
                system_prompt: SystemPromptOverride::Set("request base".to_string()),
                additional_instructions: Some(vec!["additional instruction".to_string()]),
                assembled_system_prompt: Some(canonical.to_string()),
                ..Default::default()
            })
            .expect("test build state must serialize");
        session.push(Message::System(SystemMessage::new(format!(
            "{canonical}\n\n[Runtime System Context]\npeer context"
        ))));

        assert_eq!(projected_system_content(&session), canonical);
    }

    #[test]
    fn live_root_neither_duplicates_additional_instructions_nor_absorbs_runtime_context() {
        let canonical = "configured base\n\nadditional instruction\n\nconfig tools";
        let mut session = Session::new();
        session
            .set_build_state(SessionBuildState {
                system_prompt: SystemPromptOverride::Inherit,
                additional_instructions: Some(vec!["additional instruction".to_string()]),
                assembled_system_prompt: Some(canonical.to_string()),
                ..Default::default()
            })
            .expect("test build state must serialize");
        session.set_system_prompt(canonical.to_string());
        let runtime_context = PendingSystemContextAppend {
            content: CoreRenderable::text("peer context".to_string()),
            source: Some("comms:peer".to_string()),
            idempotency_key: Some("peer-context-1".to_string()),
            source_kind: SystemContextSource::Normal,
            peer_response_terminal: None,
            accepted_at: SystemTime::UNIX_EPOCH,
        };
        session.append_system_context_blocks(std::slice::from_ref(&runtime_context));

        let projected = projected_system_content(&session);
        assert_eq!(projected, canonical);
        assert_eq!(projected.matches("additional instruction").count(), 1);
        assert!(!projected.contains("Runtime System Context"));
        assert!(!projected.contains("peer context"));
        assert_eq!(
            realtime_projection_runtime_system_context(&session)
                .expect("typed runtime context projection"),
            vec![runtime_context]
        );
    }

    #[test]
    fn live_root_disable_keeps_canonical_appended_sections() {
        let canonical =
            "skill inventory\n\nadditional instruction\n\nconfig tools\n\ndispatcher tools";
        let mut session = Session::new();
        session
            .set_build_state(SessionBuildState {
                system_prompt: SystemPromptOverride::Disable,
                additional_instructions: Some(vec!["additional instruction".to_string()]),
                assembled_system_prompt: Some(canonical.to_string()),
                ..Default::default()
            })
            .expect("test build state must serialize");
        session.push(Message::System(SystemMessage::new(canonical)));

        assert_eq!(projected_system_content(&session), canonical);
    }

    #[test]
    fn live_root_disable_projects_runtime_context_only_through_typed_field() {
        let mut session = Session::new();
        session
            .set_build_state(SessionBuildState {
                system_prompt: SystemPromptOverride::Disable,
                assembled_system_prompt: Some(String::new()),
                ..Default::default()
            })
            .expect("test build state must serialize");
        session.set_system_prompt(String::new());
        let runtime_context = PendingSystemContextAppend {
            content: CoreRenderable::text("peer context".to_string()),
            source: Some("comms:peer".to_string()),
            idempotency_key: Some("peer-context-disabled-root".to_string()),
            source_kind: SystemContextSource::Normal,
            peer_response_terminal: None,
            accepted_at: SystemTime::UNIX_EPOCH,
        };
        session.append_system_context_blocks(std::slice::from_ref(&runtime_context));

        let projected = realtime_projection_messages(&session).expect("full projection");
        assert!(
            projected
                .iter()
                .all(|message| !matches!(message, Message::System(_) | Message::SystemNotice(_)))
        );
        assert_eq!(
            realtime_projection_runtime_system_context(&session)
                .expect("typed runtime context projection"),
            vec![runtime_context]
        );
    }

    #[test]
    fn live_root_fails_closed_without_canonical_assembled_bytes() {
        let mut session = Session::new();
        session
            .set_build_state(SessionBuildState::default())
            .expect("test build state must serialize");
        session.push(Message::System(SystemMessage::new("transcript fallback")));

        let error = realtime_projection_root_system_message(&session)
            .expect_err("live prompt projection must not reconstruct missing canonical bytes");
        assert!(
            error
                .to_string()
                .contains("missing its canonical assembled system prompt")
        );
    }

    #[test]
    fn live_seed_window_rejects_zero() {
        assert!(matches!(
            LiveSeedWindow::new(0),
            Err(LiveSeedProjectionError::ZeroWindow)
        ));
    }

    #[test]
    fn live_seed_window_preserves_full_projection_when_it_fits() {
        let session = window_test_session();
        let full = realtime_projection_messages(&session).expect("full projection");
        let full_chars = full
            .iter()
            .map(serialized_message_chars)
            .collect::<Result<Vec<_>, _>>()
            .expect("serialized costs")
            .into_iter()
            .sum();

        let projection = realtime_projection_messages_with_window(
            &session,
            LiveSeedWindow::new(full_chars).expect("positive window"),
        )
        .expect("bounded projection");

        assert_eq!(projection.messages, full);
        assert_eq!(projection.status, LiveSeedProjectionStatus::Complete);
    }

    #[test]
    fn live_seed_window_does_not_charge_a_disabled_stale_system_lead_as_root() {
        let session = disabled_root_test_session();
        let full = realtime_projection_messages(&session).expect("full projection");
        assert!(
            full.iter()
                .all(|message| !matches!(message, Message::System(_) | Message::SystemNotice(_)))
        );
        let full_budget = full
            .iter()
            .map(serialized_message_chars)
            .collect::<Result<Vec<_>, _>>()
            .expect("serialized projection costs")
            .into_iter()
            .sum::<usize>();

        let projection = realtime_projection_messages_with_window(
            &session,
            LiveSeedWindow::new(full_budget - 1).expect("positive window"),
        )
        .expect("a disabled stale lead must not cause root-too-small rejection");

        assert!(projection.messages.is_empty());
        assert_eq!(
            projection.status,
            LiveSeedProjectionStatus::Windowed {
                dropped_messages: 2,
                included_compaction_summary: false,
            }
        );
    }

    #[test]
    fn live_seed_window_is_deterministic_at_an_exact_boundary() {
        let session = window_test_session();
        let full = realtime_projection_messages(&session).expect("full projection");
        let full_chars = full
            .iter()
            .map(serialized_message_chars)
            .collect::<Result<Vec<_>, _>>()
            .expect("serialized costs")
            .into_iter()
            .sum::<usize>();
        let window = LiveSeedWindow::new(full_chars - 1).expect("positive boundary window");

        let first = realtime_projection_messages_with_window(&session, window)
            .expect("first bounded projection");
        let second = realtime_projection_messages_with_window(&session, window)
            .expect("second bounded projection");

        assert_eq!(first.messages, second.messages);
        assert_eq!(first.status, second.status);
        assert!(first.status.has_known_gaps());
    }

    #[test]
    fn live_seed_window_keeps_root_summary_and_newest_complete_turn() {
        let session = window_test_session();
        let full = realtime_projection_messages(&session).expect("full projection");
        let costs = full
            .iter()
            .map(serialized_message_chars)
            .collect::<Result<Vec<_>, _>>()
            .expect("serialized costs");
        let budget = costs[0] + costs[1] + costs[5..].iter().sum::<usize>();

        let projection = realtime_projection_messages_with_window(
            &session,
            LiveSeedWindow::new(budget).expect("positive window"),
        )
        .expect("bounded projection");

        let mut expected = full[..2].to_vec();
        expected.extend_from_slice(&full[5..]);
        assert_eq!(projection.messages, expected);
        let selected_chars = projection
            .messages
            .iter()
            .map(serialized_message_chars)
            .collect::<Result<Vec<_>, _>>()
            .expect("selected serialized costs")
            .into_iter()
            .sum::<usize>();
        assert!(selected_chars <= budget);
        assert_eq!(
            projection.status,
            LiveSeedProjectionStatus::Windowed {
                dropped_messages: 3,
                included_compaction_summary: true,
            }
        );
    }

    #[test]
    fn live_seed_window_never_keeps_a_partial_newest_turn() {
        let session = window_test_session();
        let full = realtime_projection_messages(&session).expect("full projection");
        let costs = full
            .iter()
            .map(serialized_message_chars)
            .collect::<Result<Vec<_>, _>>()
            .expect("serialized costs");
        let latest_turn_chars = costs[5..].iter().sum::<usize>();
        let budget = costs[0] + costs[1] + latest_turn_chars - 1;

        let projection = realtime_projection_messages_with_window(
            &session,
            LiveSeedWindow::new(budget).expect("positive window"),
        )
        .expect("bounded projection");

        assert_eq!(projection.messages.len(), 2);
        assert!(matches!(projection.messages[0], Message::System(_)));
        assert!(matches!(
            &projection.messages[1],
            Message::User(user) if user.transcript_role.is_compaction_summary()
        ));
        assert_eq!(
            projection.status,
            LiveSeedProjectionStatus::Windowed {
                dropped_messages: 6,
                included_compaction_summary: true,
            }
        );
    }

    #[test]
    fn live_seed_window_fails_when_resolved_root_cannot_fit() {
        let session = window_test_session();
        let full = realtime_projection_messages(&session).expect("full projection");
        let root_chars = serialized_message_chars(&full[0]).expect("root cost");

        let error = realtime_projection_messages_with_window(
            &session,
            LiveSeedWindow::new(root_chars - 1).expect("positive window"),
        )
        .expect_err("root must not be silently dropped");

        assert!(matches!(
            error,
            LiveSeedProjectionError::RootExceedsWindow {
                required_chars,
                max_chars,
            } if required_chars == root_chars && max_chars == root_chars - 1
        ));
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
