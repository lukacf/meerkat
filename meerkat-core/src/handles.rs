//! Cross-crate DSL handle traits.
//!
//! Downstream crates (`meerkat-mcp`, `meerkat-comms`, `meerkat-session`) drive
//! DSL transitions through these trait objects without importing
//! `meerkat-runtime`. Concrete impls live in `meerkat-runtime`, where the DSL
//! authority lives.
//!
//! The mob side (`meerkat-mob`) already depends on `meerkat-runtime` and owns
//! its MobMachine DSL authority in-crate, so it drives DSL transitions via
//! direct `dsl_authority.apply(...)` calls — no cross-crate trait required.
//!
//! Trait methods are named per-DSL input, not per-authority input.
//! DSL-owned discriminants (turn phase, drain mode, surface phase, surface
//! pending/staged op, auth lease phase) flow as typed enums defined here in
//! `meerkat-core` — each maps 1-to-1 with the typed DSL state that
//! [`meerkat-runtime::meerkat_machine`] owns. Free-form `String` values are
//! reserved for opaque identifiers (surface ids, binding keys, error
//! messages).
//!
//! Return type is `Result<(), DslTransitionError>`. The DSL decides legality;
//! phase/field reads happen elsewhere (direct DSL state accessors, not via
//! these traits).

#[cfg(all(meerkat_internal_generated_authority_bridge, not(test)))]
use std::any::Any;
use std::collections::BTreeSet;
use std::sync::Arc;

use crate::LoopState;
use crate::auth::RefreshFailureObservation;
use crate::comms::InputSource;
use crate::interaction::{
    PeerIngressAdmission, PeerIngressDequeueAuthority, PeerIngressDequeueFacts,
    PeerIngressEnvelopeFacts, PeerIngressPlainEventFacts, PeerIngressReceiveAuthority,
    PeerIngressReceiveFacts,
};
use crate::lifecycle::run_primitive::ModelId;
use crate::lifecycle::{InputId, RunId};
use crate::ops::{AsyncOpRef, OperationId};
use crate::peer_correlation::{
    InboundPeerRequestState, InteractionStreamAbandonReason, InteractionStreamState,
    OutboundPeerRequestState, PeerCorrelationId,
};
use crate::retry::LlmRetrySchedule;
use crate::tool_scope::{
    ExternalToolSurfaceBaseState, ExternalToolSurfaceDeltaOperation, ExternalToolSurfaceDeltaPhase,
    ExternalToolSurfaceFailureCause, ExternalToolSurfaceGlobalPhase, ExternalToolSurfacePendingOp,
    ExternalToolSurfaceStagedOp,
};
use crate::turn_execution_authority::{
    ContentShape, TurnExecutionEffect, TurnExecutionInput, TurnFailureReason, TurnFailureSource,
    TurnPhase, TurnPrimitiveKind, TurnTerminalCauseKind, TurnTerminalOutcome,
};
use crate::types::{HandlingMode, SessionId};

// ---------------------------------------------------------------------------
// Typed cross-crate enums for DSL-owned discriminants.
//
// Each maps 1-to-1 with the typed DSL state that meerkat-runtime's
// MeerkatMachine / AuthMachine own. The runtime handle impls do a single
// exhaustive `match` from DSL-typed to handle-typed — no string parsing,
// no `_ => default` arms, no parallel adapters.
// ---------------------------------------------------------------------------

/// Mode for a comms drain task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrainMode {
    /// Legacy timed drain with idle timeout.
    Timed,
    /// Live session ingress while a runtime-backed session is attached.
    AttachedSession,
    /// Long-lived host drain (no idle timeout, respawnable on failure).
    PersistentHost,
}

/// Reason a drain task exited.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrainExitReason {
    IdleTimeout,
    Dismissed,
    Failed,
    Aborted,
    SessionShutdown,
}

/// Session model-routing baseline handle.
///
/// Runtime-backed surfaces create sessions before the factory has resolved the
/// final LLM identity. The factory uses this handle after resolution so the DSL
/// owns the canonical baseline model and capability surface before tools or
/// visibility projections observe those facts.
pub trait ModelRoutingHandle: Send + Sync {
    /// Set the session's canonical model-routing baseline.
    fn set_baseline(
        &self,
        baseline_model: ModelId,
        realtime_capable: bool,
    ) -> Result<(), DslTransitionError>;

    /// Hydrate the session's canonical LLM capability surface.
    ///
    /// `profile` is a typed catalog observation. The generated machine decides
    /// whether the paired capability-base filter is legal for that surface.
    fn hydrate_llm_capability_surface(
        &self,
        identity: &crate::SessionLlmIdentity,
        profile: Option<&crate::model_profile::ModelProfile>,
        capability_base_filter: &crate::ToolFilter,
    ) -> Result<(), DslTransitionError>;

    /// Stage a machine-authorized sticky model fallback.
    ///
    /// The generated machine revalidates the accepted recovery attempt and
    /// previous session identity, then atomically updates current identity,
    /// capability truth, and the model-routing baseline. Runtime-backed agent
    /// loops use this as the canonical commit inside their compensated
    /// client/auth/machine transaction. `activation` is a one-shot, opaque
    /// capability minted inside core only after generated recovery acceptance
    /// and exact effective-registry validation. Public handle holders cannot
    /// fabricate one from a raw or foreign registry witness.
    fn stage_sticky_model_fallback(
        &self,
        activation: crate::StickyModelFallbackActivationProof,
        visibility_plan: &StickyModelFallbackVisibilityPlan,
    ) -> Result<Box<dyn StickyModelFallbackMachineCommit>, DslTransitionError>;
}

/// One-shot generated-authority commit staged for an exact sticky fallback.
///
/// Staging previews the generated transition against an exact authority
/// snapshot without publishing it. Consuming this token commits only if that
/// snapshot is still current, so a durable coordinator can place its session
/// compare-and-swap between generated preauthorization and synchronous machine
/// publication without allowing a caller to replay the transition.
pub trait StickyModelFallbackMachineCommit: Send + Sync {
    fn commit(self: Box<Self>) -> Result<(), DslTransitionError>;
}

/// Fully-derived visibility witness carried by a sticky model fallback.
///
/// This is the same contract used by live LLM reconfiguration: generated
/// authority validates the previous state, target capability filter, visible
/// `view_image` delta, and monotonic revision before it commits either the LLM
/// identity or the canonical visibility state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StickyModelFallbackVisibilityPlan {
    pub previous_state: crate::SessionToolVisibilityState,
    pub next_state: crate::SessionToolVisibilityState,
    pub view_image_tool_available: bool,
    pub previous_view_image_visible: bool,
    pub next_view_image_visible: bool,
    pub committed_visible_set_changed: bool,
    pub revision_bumped: bool,
}

/// Opaque durable-session control delta for one sticky fallback.
///
/// Only the core agent loop can construct this value. The runtime may validate
/// and apply it to an exact persisted [`crate::Session`] snapshot, but cannot
/// fabricate a different identity or visibility transition. Applying the delta
/// changes only canonical control metadata; transcript messages are untouched.
#[derive(Debug, Clone)]
pub struct StickyModelFallbackControlDelta {
    previous_identity: crate::SessionLlmIdentity,
    target_identity: crate::SessionLlmIdentity,
    persisted_visibility_parent: crate::SessionToolVisibilityState,
    target_visibility_state: crate::SessionToolVisibilityState,
}

impl StickyModelFallbackControlDelta {
    pub(crate) fn new(
        previous_identity: crate::SessionLlmIdentity,
        target_identity: crate::SessionLlmIdentity,
        visibility_plan: &StickyModelFallbackVisibilityPlan,
        persisted_visibility_parent: crate::SessionToolVisibilityState,
    ) -> Self {
        Self {
            previous_identity,
            target_identity,
            persisted_visibility_parent,
            target_visibility_state: visibility_plan.next_state.clone(),
        }
    }

    pub fn previous_identity(&self) -> &crate::SessionLlmIdentity {
        &self.previous_identity
    }

    pub fn target_identity(&self) -> &crate::SessionLlmIdentity {
        &self.target_identity
    }

    pub fn previous_visibility_state(&self) -> &crate::SessionToolVisibilityState {
        &self.persisted_visibility_parent
    }

    pub fn target_visibility_state(&self) -> &crate::SessionToolVisibilityState {
        &self.target_visibility_state
    }

    /// Validate the exact persisted control parent and apply its target.
    pub fn validate_and_apply(
        &self,
        session: &mut crate::Session,
    ) -> Result<(), StickyModelFallbackControlDeltaError> {
        let mut metadata = session
            .try_session_metadata()
            .map_err(|error| {
                StickyModelFallbackControlDeltaError::InvalidSessionMetadata(error.to_string())
            })?
            .ok_or(StickyModelFallbackControlDeltaError::MissingSessionMetadata)?;
        let current_identity = metadata.llm_identity();
        if current_identity != self.previous_identity {
            return Err(
                StickyModelFallbackControlDeltaError::IdentityParentMismatch {
                    expected: Box::new(self.previous_identity.clone()),
                    actual: Box::new(current_identity),
                },
            );
        }
        let current_visibility = session
            .try_tool_visibility_state()
            .map_err(|error| {
                StickyModelFallbackControlDeltaError::InvalidVisibilityMetadata(error.to_string())
            })?
            .ok_or(StickyModelFallbackControlDeltaError::MissingVisibilityMetadata)?;
        if current_visibility != self.persisted_visibility_parent {
            return Err(StickyModelFallbackControlDeltaError::VisibilityParentMismatch);
        }

        metadata.apply_llm_identity(&self.target_identity);
        session.set_session_metadata(metadata).map_err(|error| {
            StickyModelFallbackControlDeltaError::InvalidSessionMetadata(error.to_string())
        })?;
        session
            .set_tool_visibility_state(
                crate::AuthorizedSessionToolVisibilityState::from_generated_authority(
                    self.target_visibility_state.clone(),
                ),
            )
            .map_err(|error| {
                StickyModelFallbackControlDeltaError::InvalidVisibilityMetadata(error.to_string())
            })?;
        Ok(())
    }
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum StickyModelFallbackControlDeltaError {
    #[error("persisted session has no canonical LLM identity metadata")]
    MissingSessionMetadata,
    #[error("persisted session has no canonical tool visibility metadata")]
    MissingVisibilityMetadata,
    #[error("persisted session LLM identity parent does not match the staged fallback")]
    IdentityParentMismatch {
        expected: Box<crate::SessionLlmIdentity>,
        actual: Box<crate::SessionLlmIdentity>,
    },
    #[error("persisted session tool visibility parent does not match the staged fallback")]
    VisibilityParentMismatch,
    #[error("persisted session LLM identity metadata is invalid: {0}")]
    InvalidSessionMetadata(String),
    #[error("persisted session tool visibility metadata is invalid: {0}")]
    InvalidVisibilityMetadata(String),
}

/// Resultful, cancellation-safe durable sticky-fallback handoff.
pub trait StickyModelFallbackCommitCoordinator: Send + Sync {
    fn begin(
        &self,
        machine_commit: Box<dyn StickyModelFallbackMachineCommit>,
        control_delta: StickyModelFallbackControlDelta,
    ) -> Result<Arc<dyn StickyModelFallbackCommitOperation>, StickyModelFallbackCommitError>;
}

/// Join handle for one supervised sticky-fallback transaction.
///
/// Dropping a `wait` future does not cancel the transaction. Callers may wait
/// again through the retained operation and observe the same terminal result.
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
pub trait StickyModelFallbackCommitOperation: Send + Sync {
    async fn wait(&self) -> Result<(), StickyModelFallbackCommitError>;
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum StickyModelFallbackCommitError {
    #[error("durable sticky fallback is unavailable without a RuntimeStore")]
    StoreUnavailable,
    #[error("durable sticky fallback session snapshot is missing for {session_id}")]
    SnapshotMissing { session_id: crate::SessionId },
    #[error("durable sticky fallback session snapshot is invalid: {0}")]
    SnapshotInvalid(String),
    #[error("durable sticky fallback snapshot belongs to {actual}, expected {expected}")]
    SessionMismatch {
        expected: crate::SessionId,
        actual: crate::SessionId,
    },
    #[error(transparent)]
    InvalidControlDelta(StickyModelFallbackControlDeltaError),
    #[error("durable sticky fallback store operation failed before commit: {0}")]
    Store(String),
    #[error("durable sticky fallback compare-and-swap observed a competing snapshot")]
    SnapshotConflict,
    #[error("durable sticky fallback compare-and-swap outcome is unknown: {0}")]
    SnapshotOutcomeUnknown(String),
    #[error("generated authority rejected the staged sticky fallback: {0}")]
    MachineRejected(DslTransitionError),
    #[error(
        "generated authority rejected the staged sticky fallback and durable compensation failed: {0}"
    )]
    CompensationFailed(String),
    #[error("durable sticky fallback supervisor ended without a retained result")]
    SupervisorLost,
}

impl StickyModelFallbackCommitError {
    /// Whether the coordinator could not prove a single authoritative parent
    /// or compensation result. These outcomes require canonical executor
    /// teardown instead of ordinary failed-batch retry.
    pub fn requires_teardown(&self) -> bool {
        matches!(
            self,
            Self::SnapshotConflict
                | Self::SnapshotOutcomeUnknown(_)
                | Self::CompensationFailed(_)
                | Self::SupervisorLost
        )
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod sticky_model_fallback_control_delta_tests {
    use super::*;
    use crate::{
        Message, Provider, SESSION_METADATA_SCHEMA_VERSION, Session, SessionMetadata,
        SessionTooling, ToolFilter, UserMessage,
    };

    fn identity(model: &str) -> crate::SessionLlmIdentity {
        crate::SessionLlmIdentity {
            model: model.to_string(),
            provider: Provider::OpenAI,
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: None,
        }
    }

    fn session_with_control_state(
        identity: &crate::SessionLlmIdentity,
        visibility: &crate::SessionToolVisibilityState,
    ) -> Session {
        let mut session = Session::new();
        session
            .set_session_metadata(SessionMetadata {
                schema_version: SESSION_METADATA_SCHEMA_VERSION,
                model: identity.model.clone(),
                max_tokens: 4096,
                structured_output_retries: 2,
                provider: identity.provider,
                self_hosted_server_id: identity.self_hosted_server_id.clone(),
                provider_params: identity.provider_params.clone(),
                tooling: SessionTooling::default(),
                keep_alive: true,
                comms_name: None,
                peer_meta: None,
                realm_id: None,
                instance_id: None,
                backend: None,
                config_generation: Some(7),
                auth_binding: identity.auth_binding.clone(),
                mob_member_binding: None,
            })
            .unwrap();
        session
            .set_tool_visibility_state(
                crate::AuthorizedSessionToolVisibilityState::from_generated_authority(
                    visibility.clone(),
                ),
            )
            .unwrap();
        session.push(Message::User(UserMessage::text(
            "uncommitted turn must not appear",
        )));
        session
    }

    #[test]
    fn control_delta_changes_only_identity_and_typed_visibility() {
        let previous = identity("primary");
        let target = identity("backup");
        let previous_visibility = crate::SessionToolVisibilityState::default();
        let mut target_visibility = previous_visibility.clone();
        target_visibility.capability_base_filter = ToolFilter::Deny(
            [crate::VIEW_IMAGE_TOOL_NAME.to_string()]
                .into_iter()
                .collect(),
        );
        target_visibility.active_revision = 1;
        target_visibility.staged_revision = 1;
        let plan = StickyModelFallbackVisibilityPlan {
            previous_state: previous_visibility.clone(),
            next_state: target_visibility.clone(),
            view_image_tool_available: true,
            previous_view_image_visible: true,
            next_view_image_visible: false,
            committed_visible_set_changed: true,
            revision_bumped: true,
        };
        let delta = StickyModelFallbackControlDelta::new(
            previous,
            target.clone(),
            &plan,
            previous_visibility,
        );
        let mut session = session_with_control_state(
            delta.previous_identity(),
            delta.previous_visibility_state(),
        );
        let messages_before = session.messages().to_vec();
        let total_tokens_before = session.total_tokens();
        let unrelated_generation = session
            .session_metadata()
            .and_then(|metadata| metadata.config_generation);

        delta.validate_and_apply(&mut session).unwrap();

        assert_eq!(session.messages(), messages_before);
        assert_eq!(session.total_tokens(), total_tokens_before);
        let metadata = session.session_metadata().unwrap();
        assert_eq!(metadata.llm_identity(), target);
        assert_eq!(metadata.config_generation, unrelated_generation);
        assert_eq!(
            session.tool_visibility_state().unwrap(),
            Some(target_visibility)
        );
    }

    #[test]
    fn control_delta_rejects_a_non_parent_without_mutation() {
        let previous = identity("primary");
        let target = identity("backup");
        let visibility = crate::SessionToolVisibilityState::default();
        let plan = StickyModelFallbackVisibilityPlan {
            previous_state: visibility.clone(),
            next_state: visibility.clone(),
            view_image_tool_available: false,
            previous_view_image_visible: false,
            next_view_image_visible: false,
            committed_visible_set_changed: false,
            revision_bumped: false,
        };
        let delta =
            StickyModelFallbackControlDelta::new(previous, target, &plan, visibility.clone());
        let mut session = session_with_control_state(&identity("different"), &visibility);
        let bytes_before = serde_json::to_vec(&session).unwrap();

        assert!(matches!(
            delta.validate_and_apply(&mut session),
            Err(StickyModelFallbackControlDeltaError::IdentityParentMismatch { .. })
        ));
        assert_eq!(serde_json::to_vec(&session).unwrap(), bytes_before);
    }

    #[test]
    fn control_delta_accepts_exact_persisted_pre_boundary_visibility_parent() {
        let previous = identity("primary");
        let target = identity("backup");
        let mut persisted_visibility = crate::SessionToolVisibilityState {
            staged_filter: ToolFilter::Deny(["shell".to_string()].into_iter().collect()),
            staged_revision: 1,
            ..Default::default()
        };
        persisted_visibility.staged_requested_deferred_names =
            [crate::ToolName::from("deferred")].into_iter().collect();
        let promoted_visibility = persisted_visibility.projected_boundary_applied();
        let mut target_visibility = promoted_visibility.clone();
        target_visibility.capability_base_filter = ToolFilter::Deny(
            [crate::VIEW_IMAGE_TOOL_NAME.to_string()]
                .into_iter()
                .collect(),
        );
        target_visibility.active_revision = 2;
        target_visibility.staged_revision = 2;
        let plan = StickyModelFallbackVisibilityPlan {
            previous_state: promoted_visibility,
            next_state: target_visibility.clone(),
            view_image_tool_available: true,
            previous_view_image_visible: true,
            next_view_image_visible: false,
            committed_visible_set_changed: true,
            revision_bumped: true,
        };
        let delta = StickyModelFallbackControlDelta::new(
            previous,
            target.clone(),
            &plan,
            persisted_visibility.clone(),
        );
        let mut session =
            session_with_control_state(delta.previous_identity(), &persisted_visibility);

        delta.validate_and_apply(&mut session).unwrap();

        assert_eq!(session.session_metadata().unwrap().llm_identity(), target);
        assert_eq!(
            session.tool_visibility_state().unwrap(),
            Some(target_visibility)
        );
    }
}

impl DrainExitReason {
    /// Stable discriminant for wire logging (drain exit reason is not yet a
    /// typed DSL field; the handle passes the discriminant through).
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::IdleTimeout => "IdleTimeout",
            Self::Dismissed => "Dismissed",
            Self::Failed => "Failed",
            Self::Aborted => "Aborted",
            Self::SessionShutdown => "SessionShutdown",
        }
    }
}

/// Auth lease lifecycle phase, projected from the per-binding AuthMachine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthLeasePhase {
    Valid,
    Expiring,
    Expired,
    Refreshing,
    ReauthRequired,
    Released,
}

/// Typed credential-use intent fed by the resolver shell to the AuthMachine's
/// credential-use admission classifier.
///
/// Identifies WHICH credential gate is asking — never a policy decision. The
/// per-binding AuthMachine owns the `(lifecycle_phase, credential_present,
/// intent)` -> [`CredentialUseDisposition`] verdict; the resolver shell extracts
/// only this typed intent and mirrors the emitted verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialUseIntent {
    /// Resolver "use the credential now" read.
    UseCredential,
    /// Resolver post-publish lifecycle-authority gate.
    HoldAuthority,
    /// Resolver OAuth-refresh begin gate.
    BeginRefresh,
}

/// Machine-owned credential-use disposition the resolver shell mirrors.
///
/// Decided by the per-binding AuthMachine's credential-use admission classifier
/// from its own `(lifecycle_phase, credential_present)` plus the shell-supplied
/// [`CredentialUseIntent`]. The resolver shell maps each variant to its existing
/// behavior/error and never decides the disposition itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialUseDisposition {
    /// Credential may be used / lifecycle authority is held.
    Authorized,
    /// Caller must refresh first.
    RefreshRequired,
    /// A refresh is required to proceed but the binding's config does not permit
    /// silent refresh (`allow_refresh == false`); the caller surfaces a
    /// refresh-required error instead of beginning a refresh. Only emitted by the
    /// OAuth-login cached-vs-refresh disposition.
    RefreshDisallowed,
    /// Interactive user reauthorization is required.
    ReauthRequired,
    /// No usable lease is present.
    LeaseAbsent,
    /// A refresh is already in flight; the begin-refresh caller no-ops.
    AlreadyRefreshing,
}

/// Pure provider-runtime observations fed to the AuthMachine's OAuth-login
/// cached-vs-refresh disposition classifier.
///
/// The provider runtime shell holds these three facts and never composes them
/// into a disposition itself: the per-binding AuthMachine owns the
/// `(lifecycle_phase, self.credential_present, credential_present, force_refresh,
/// refresh_allowed)` -> [`CredentialUseDisposition`] policy and the shell mirrors
/// the emitted verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OAuthLoginCredentialFacts {
    /// Whether a persisted credential secret is present
    /// (`persisted.primary_secret.is_some()`).
    pub credential_present: bool,
    /// Whether the caller forced a refresh (`env.force_refresh`).
    pub force_refresh: bool,
    /// Whether the binding config permits silent refresh
    /// (`refresh_allowed(binding)` / `allow_refresh`).
    pub refresh_allowed: bool,
}

/// Typed classification of why a DSL transition was rejected.
///
/// Emitted by the generated kernel's `apply` / `apply_signal` methods and
/// bridged into [`DslTransitionError::kind`]. Callers that fire
/// idempotently (realtime dispatchers, monotonic watermark advances,
/// etc.) inspect this to distinguish "input was out of scope for this
/// phase" (a real error) from "input was recognised but the guard dropped
/// it" (a successful no-op).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DslRejectionKind {
    /// No transition is declared for this `(phase, trigger)` pair — the
    /// shell fired an input that is semantically out of scope for the
    /// current phase. This is a programming mistake on the shell side.
    NoMatchingTransition,
    /// A transition is declared for this `(phase, trigger)` pair but
    /// every candidate transition's guard evaluated false. Callers
    /// firing idempotently treat this as a no-op; callers firing
    /// unconditionally treat it as a user-visible error.
    GuardRejected,
    /// Generated authority rejected recovered state before any transition
    /// was attempted. This is not an idempotent transition guard no-op.
    RecoveredStateInvariantRejected,
}

/// Error surfaced when a DSL transition is rejected.
///
/// Wraps the generated kernel's typed rejection. Trait impls populate
/// `context` from the trait method name so callers can tell which handle
/// rejected; `kind` lets callers distinguish guard rejection from
/// out-of-scope input without substring-matching the rendered message.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("DSL transition rejected in {context}: {reason}")]
pub struct DslTransitionError {
    /// Name of the trait method / DSL variant whose transition was rejected.
    pub context: &'static str,
    /// Typed classification of the rejection — see [`DslRejectionKind`].
    pub kind: DslRejectionKind,
    /// Underlying rejection reason (typically the generated
    /// `NoMatchingTransition`/`GuardRejected` formatted).
    pub reason: String,
}

impl DslTransitionError {
    /// Construct an error with `kind = NoMatchingTransition`.
    pub fn no_matching(context: &'static str, reason: impl Into<String>) -> Self {
        Self {
            context,
            kind: DslRejectionKind::NoMatchingTransition,
            reason: reason.into(),
        }
    }

    /// Construct an error with `kind = GuardRejected`.
    pub fn guard_rejected(context: &'static str, reason: impl Into<String>) -> Self {
        Self {
            context,
            kind: DslRejectionKind::GuardRejected,
            reason: reason.into(),
        }
    }

    /// Construct an error with `kind = RecoveredStateInvariantRejected`.
    pub fn recovered_state_invariant_rejected(
        context: &'static str,
        reason: impl Into<String>,
    ) -> Self {
        Self {
            context,
            kind: DslRejectionKind::RecoveredStateInvariantRejected,
            reason: reason.into(),
        }
    }

    /// True iff this rejection came from a guard evaluating false.
    pub fn is_guard_rejected(&self) -> bool {
        self.kind == DslRejectionKind::GuardRejected
    }
}

// ---------------------------------------------------------------------------
// Cross-crate peer prompt/context projection seam
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PeerResponseProgressProjectionPhase {
    Accepted,
    InProgress,
    PartialResult,
}

impl PeerResponseProgressProjectionPhase {
    fn label(self) -> &'static str {
        match self {
            Self::Accepted => "accepted",
            Self::InProgress => "in_progress",
            Self::PartialResult => "partial_result",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PeerResponseTerminalProjectionStatus {
    Completed,
    Failed,
    Cancelled,
}

impl PeerResponseTerminalProjectionStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PeerResponseTerminalFactError {
    #[error("transport identity cannot be empty")]
    EmptyTransportIdentity,
    #[error("route identity cannot be empty")]
    EmptyRouteIdentity,
    #[error("route identity must be a canonical peer UUID")]
    InvalidRouteIdentity,
    #[error("display identity is required")]
    MissingDisplayIdentity,
    #[error("display identity cannot be empty")]
    EmptyDisplayIdentity,
    #[error("display identity cannot contain control characters")]
    InvalidDisplayIdentity,
    #[error("correlation id cannot be empty")]
    EmptyCorrelationId,
    #[error("correlation id must be a UUID: {input}")]
    InvalidCorrelationId { input: String },
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct PeerResponseTerminalTransportIdentity(String);

impl PeerResponseTerminalTransportIdentity {
    pub fn parse(raw: impl Into<String>) -> Result<Self, PeerResponseTerminalFactError> {
        let raw = raw.into();
        if raw.trim().is_empty() {
            return Err(PeerResponseTerminalFactError::EmptyTransportIdentity);
        }
        Ok(Self(raw))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for PeerResponseTerminalTransportIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct PeerResponseTerminalRouteIdentity(crate::comms::PeerId);

impl PeerResponseTerminalRouteIdentity {
    pub fn parse(raw: impl Into<String>) -> Result<Self, PeerResponseTerminalFactError> {
        let raw = raw.into();
        if raw.trim().is_empty() {
            return Err(PeerResponseTerminalFactError::EmptyRouteIdentity);
        }
        if raw.chars().any(char::is_control) {
            return Err(PeerResponseTerminalFactError::InvalidRouteIdentity);
        }
        let peer_id = crate::comms::PeerId::parse(raw.trim())
            .map_err(|_| PeerResponseTerminalFactError::InvalidRouteIdentity)?;
        Ok(Self(peer_id))
    }

    /// The canonical typed routing identity.
    pub fn peer_id(&self) -> crate::comms::PeerId {
        self.0
    }

    pub fn as_str(&self) -> String {
        self.0.as_str()
    }
}

impl std::fmt::Display for PeerResponseTerminalRouteIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct PeerResponseTerminalDisplayIdentity(String);

impl PeerResponseTerminalDisplayIdentity {
    pub fn parse(raw: impl Into<String>) -> Result<Self, PeerResponseTerminalFactError> {
        let raw = raw.into();
        if raw.trim().is_empty() {
            return Err(PeerResponseTerminalFactError::EmptyDisplayIdentity);
        }
        if raw.chars().any(char::is_control) {
            return Err(PeerResponseTerminalFactError::InvalidDisplayIdentity);
        }
        Ok(Self(raw))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for PeerResponseTerminalDisplayIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct PeerResponseTerminalCorrelationId(PeerCorrelationId);

impl PeerResponseTerminalCorrelationId {
    pub fn parse(raw: impl AsRef<str>) -> Result<Self, PeerResponseTerminalFactError> {
        let raw = raw.as_ref();
        if raw.trim().is_empty() {
            return Err(PeerResponseTerminalFactError::EmptyCorrelationId);
        }
        uuid::Uuid::parse_str(raw)
            .map(|uuid| Self(PeerCorrelationId::from_uuid(uuid)))
            .map_err(|_| PeerResponseTerminalFactError::InvalidCorrelationId {
                input: raw.to_string(),
            })
    }

    pub const fn from_peer_correlation_id(correlation_id: PeerCorrelationId) -> Self {
        Self(correlation_id)
    }

    pub const fn as_peer_correlation_id(self) -> PeerCorrelationId {
        self.0
    }
}

impl std::fmt::Display for PeerResponseTerminalCorrelationId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct PeerResponseTerminalRenderPayload(Option<serde_json::Value>);

impl PeerResponseTerminalRenderPayload {
    pub fn new(payload: Option<serde_json::Value>) -> Self {
        Self(payload)
    }

    pub fn as_ref(&self) -> Option<&serde_json::Value> {
        self.0.as_ref()
    }
}

impl From<Option<serde_json::Value>> for PeerResponseTerminalRenderPayload {
    fn from(payload: Option<serde_json::Value>) -> Self {
        Self::new(payload)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PeerResponseTerminalSource {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transport_identity: Option<PeerResponseTerminalTransportIdentity>,
    pub route_identity: PeerResponseTerminalRouteIdentity,
    pub display_identity: PeerResponseTerminalDisplayIdentity,
}

impl PeerResponseTerminalSource {
    pub fn new(
        transport_identity: Option<PeerResponseTerminalTransportIdentity>,
        route_identity: PeerResponseTerminalRouteIdentity,
        display_identity: PeerResponseTerminalDisplayIdentity,
    ) -> Self {
        Self {
            transport_identity,
            route_identity,
            display_identity,
        }
    }

    pub fn parse(
        transport_identity: Option<impl Into<String>>,
        route_identity: impl Into<String>,
        display_identity: impl Into<String>,
    ) -> Result<Self, PeerResponseTerminalFactError> {
        Ok(Self::new(
            transport_identity
                .map(PeerResponseTerminalTransportIdentity::parse)
                .transpose()?,
            PeerResponseTerminalRouteIdentity::parse(route_identity)?,
            PeerResponseTerminalDisplayIdentity::parse(display_identity)?,
        ))
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PeerResponseTerminalFact {
    pub source: PeerResponseTerminalSource,
    pub correlation_id: PeerResponseTerminalCorrelationId,
    pub status: PeerResponseTerminalProjectionStatus,
    pub render_payload: PeerResponseTerminalRenderPayload,
}

impl PeerResponseTerminalFact {
    pub fn new(
        source: PeerResponseTerminalSource,
        correlation_id: PeerResponseTerminalCorrelationId,
        status: PeerResponseTerminalProjectionStatus,
        render_payload: PeerResponseTerminalRenderPayload,
    ) -> Self {
        Self {
            source,
            correlation_id,
            status,
            render_payload,
        }
    }

    pub fn prompt_text(&self) -> String {
        format!(
            "Peer terminal response from {}. Request ID: {}. Status: {}. Result: {}.",
            self.source.display_identity,
            self.correlation_id,
            self.status.label(),
            format_peer_projection_payload(self.render_payload.as_ref())
        )
    }

    pub fn context_key(&self) -> String {
        peer_response_terminal_context_key(&self.source.route_identity, self.correlation_id)
    }

    /// Typed render payload accessor for surfaces that summarize the terminal
    /// fact directly instead of re-parsing the flattened prompt text.
    pub fn render_payload_value(&self) -> Option<&serde_json::Value> {
        self.render_payload.as_ref()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum PeerConversationProjection {
    Message {
        peer_id: String,
    },
    Request {
        peer_id: crate::comms::PeerId,
        display_name: Option<String>,
        request_id: String,
        intent: String,
        payload: Option<serde_json::Value>,
    },
    ResponseProgress {
        peer_id: String,
        request_id: String,
        phase: PeerResponseProgressProjectionPhase,
        payload: Option<serde_json::Value>,
    },
    ResponseTerminal {
        fact: PeerResponseTerminalFact,
    },
}

impl PeerConversationProjection {
    pub fn response_terminal(fact: PeerResponseTerminalFact) -> Self {
        Self::ResponseTerminal { fact }
    }

    pub fn block_prefix_text(&self) -> Option<String> {
        match self {
            Self::Message { peer_id } => Some(format!("Peer message from {peer_id}")),
            Self::Request { .. }
            | Self::ResponseProgress { .. }
            | Self::ResponseTerminal { .. } => None,
        }
    }

    pub fn prompt_text(&self) -> String {
        match self {
            Self::Message { .. } => String::new(),
            Self::Request {
                peer_id,
                display_name,
                request_id,
                intent,
                payload,
            } => {
                let display_suffix = display_name
                    .as_deref()
                    .map(str::trim)
                    .filter(|name| !name.is_empty())
                    .map(|name| format!(" (display_name: {name})"))
                    .unwrap_or_default();
                let response_call = crate::interaction::SendResponseCallProjection::new(
                    *peer_id,
                    display_name.as_deref(),
                    request_id.clone(),
                );
                format!(
                    "Peer request from peer_id {peer_id}{display_suffix}. Intent: {intent}. Request ID: {request_id}. Params: {}. This is not a normal user request and not a prompt for direct user-facing output. {} Do not use send_message for this reply.",
                    format_peer_projection_payload(payload.as_ref()),
                    response_call.instruction_text()
                )
            }
            Self::ResponseProgress {
                peer_id,
                request_id,
                phase,
                payload,
            } => format!(
                "Peer response progress from {peer_id}. Request ID: {request_id}. Phase: {}. Payload: {}.",
                phase.label(),
                format_peer_projection_payload(payload.as_ref())
            ),
            Self::ResponseTerminal { fact } => fact.prompt_text(),
        }
    }

    pub fn context_key(&self) -> Option<String> {
        match self {
            Self::ResponseTerminal { fact } => Some(fact.context_key()),
            Self::Message { .. } | Self::Request { .. } | Self::ResponseProgress { .. } => None,
        }
    }
}

pub fn peer_response_terminal_context_key(
    route_identity: &PeerResponseTerminalRouteIdentity,
    correlation_id: PeerResponseTerminalCorrelationId,
) -> String {
    format!("peer_response_terminal:{route_identity}:{correlation_id}")
}

fn format_peer_projection_payload(payload: Option<&serde_json::Value>) -> String {
    serde_json::to_string_pretty(payload.unwrap_or(&serde_json::Value::Null))
        .unwrap_or_else(|_| "null".to_string())
}

// ---------------------------------------------------------------------------
// TurnStateHandle
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnStateSnapshot {
    pub active_run_id: Option<RunId>,
    /// Exact run whose terminal outcome/cause is projected below. Unlike
    /// `active_run_id`, this remains populated after the terminal transition
    /// clears the active binding.
    pub terminal_run_id: Option<RunId>,
    /// Observable loop-state projection supplied by the turn-state owner.
    ///
    /// Consumers should not reclassify [`TurnPhase`] locally. Runtime-backed
    /// handles derive this from the same DSL snapshot as `turn_phase`; test
    /// handles do the same from their in-core test state.
    pub loop_state: LoopState,
    pub turn_phase: TurnPhase,
    /// Machine-owned turn-terminality verdict.
    ///
    /// Consumers must not reclassify [`TurnPhase`] locally. This bool is the
    /// `TurnTerminalityClassified.terminal` verdict emitted by the canonical
    /// MeerkatMachine `ClassifyTurnTerminality` input over the same DSL snapshot
    /// as `turn_phase`; the turn-state owner mirrors it, failing closed.
    pub turn_terminal: bool,
    /// Typed primitive kind recorded by the DSL (dogma #5, #19 — no stringly
    /// discriminants). `None` means no primitive is currently in flight.
    pub primitive_kind: Option<TurnPrimitiveKind>,
    pub admitted_content_shape: Option<ContentShape>,
    pub vision_enabled: bool,
    pub image_tool_results_enabled: bool,
    pub tool_calls_pending: u64,
    pub pending_op_refs: BTreeSet<AsyncOpRef>,
    pub barrier_operation_ids: BTreeSet<OperationId>,
    pub has_barrier_ops: bool,
    pub barrier_satisfied: bool,
    pub boundary_count: u64,
    pub cancel_after_boundary: bool,
    /// Typed terminal outcome recorded by the DSL (dogma #5, #19 — no stringly
    /// discriminants). `None` means the turn has not reached a terminal phase.
    pub terminal_outcome: Option<TurnTerminalOutcome>,
    /// Typed terminal cause recorded by the DSL. `None` means no failure cause
    /// has been selected for the current turn.
    pub terminal_cause_kind: Option<TurnTerminalCauseKind>,
    pub extraction_attempts: u64,
    pub max_extraction_retries: u64,
    /// Machine-owned total answer to "is this turn inside the
    /// structured-output extraction sub-flow" (dogma K9). Set by
    /// `EnterExtraction`, cleared on every turn-terminal transition and on
    /// run start. Consumers must read this — never derive in-extraction from
    /// loop-local scratch like `extraction_state.primary_output`.
    pub extraction_active: bool,
    pub llm_retry_attempt: u32,
    pub llm_retry_max_retries: u32,
    pub llm_retry_selected_delay_ms: u64,
}

/// Turn-execution DSL handle.
pub trait TurnStateHandle: Send + Sync {
    /// Apply one typed turn-execution input and return the generated
    /// turn-authority effects emitted by that transition.
    fn apply_turn_input(
        &self,
        input: TurnExecutionInput,
    ) -> Result<Vec<TurnExecutionEffect>, DslTransitionError>;

    fn start_conversation_run(
        &self,
        run_id: RunId,
        primitive_kind: TurnPrimitiveKind,
        admitted_content_shape: ContentShape,
        vision_enabled: bool,
        image_tool_results_enabled: bool,
        max_extraction_retries: u64,
    ) -> Result<(), DslTransitionError>;

    fn start_immediate_append(&self, run_id: RunId) -> Result<(), DslTransitionError>;

    fn start_immediate_context(&self, run_id: RunId) -> Result<(), DslTransitionError>;

    fn primitive_applied(&self, run_id: RunId) -> Result<(), DslTransitionError>;

    fn llm_returned_tool_calls(
        &self,
        run_id: RunId,
        tool_count: u64,
    ) -> Result<(), DslTransitionError>;

    fn llm_returned_terminal(&self, run_id: RunId) -> Result<(), DslTransitionError>;

    fn register_pending_ops(
        &self,
        run_id: RunId,
        op_refs: BTreeSet<AsyncOpRef>,
        barrier_operation_ids: BTreeSet<OperationId>,
    ) -> Result<(), DslTransitionError>;

    fn tool_calls_resolved(&self, run_id: RunId) -> Result<(), DslTransitionError>;

    fn ops_barrier_satisfied(
        &self,
        run_id: RunId,
        operation_ids: BTreeSet<OperationId>,
    ) -> Result<(), DslTransitionError>;

    fn boundary_continue(&self, run_id: RunId) -> Result<(), DslTransitionError>;

    fn boundary_complete(&self, run_id: RunId) -> Result<(), DslTransitionError>;

    fn enter_extraction(&self, run_id: RunId, max_retries: u32) -> Result<(), DslTransitionError>;

    fn extraction_start(&self, run_id: RunId) -> Result<(), DslTransitionError>;

    fn extraction_validation_passed(&self, run_id: RunId) -> Result<(), DslTransitionError>;

    fn extraction_validation_failed(
        &self,
        run_id: RunId,
        error: String,
    ) -> Result<(), DslTransitionError>;

    fn extraction_failed(&self, run_id: RunId, error: String) -> Result<(), DslTransitionError>;

    fn recoverable_failure(
        &self,
        run_id: RunId,
        retry: LlmRetrySchedule,
    ) -> Result<(), DslTransitionError>;

    fn fatal_failure(
        &self,
        run_id: RunId,
        failure: TurnFailureSource,
    ) -> Result<(), DslTransitionError>;

    fn retry_requested(&self, run_id: RunId, retry_attempt: u32) -> Result<(), DslTransitionError>;

    fn cancel_now(&self, run_id: RunId) -> Result<(), DslTransitionError>;

    fn request_cancel_after_boundary(&self, run_id: RunId) -> Result<(), DslTransitionError>;

    fn cancellation_observed(&self, run_id: RunId) -> Result<(), DslTransitionError>;

    fn acknowledge_terminal(&self, run_id: RunId) -> Result<(), DslTransitionError>;

    fn turn_limit_reached(
        &self,
        run_id: RunId,
        turn_count: u64,
        max_turns: u64,
    ) -> Result<(), DslTransitionError>;

    fn budget_exhausted(&self, run_id: RunId) -> Result<(), DslTransitionError>;

    fn time_budget_exceeded(&self, run_id: RunId) -> Result<(), DslTransitionError>;

    fn force_cancel_no_run(&self) -> Result<(), DslTransitionError>;

    fn run_completed(&self, run_id: RunId) -> Result<(), DslTransitionError>;

    fn run_failed(
        &self,
        run_id: RunId,
        reason: TurnFailureReason,
    ) -> Result<(), DslTransitionError>;

    fn run_cancelled(&self, run_id: RunId) -> Result<(), DslTransitionError>;

    fn snapshot(&self) -> TurnStateSnapshot;
}

// ---------------------------------------------------------------------------
// CommsDrainHandle
// ---------------------------------------------------------------------------

/// Comms drain lifecycle DSL handle.
///
/// Covers the `drain_phase`/`drain_mode` DSL substate: ensure/spawn/stop the
/// comms drain task and report typed exit reasons. The machine classifies the
/// resulting stopped vs respawnable state.
pub trait CommsDrainHandle: Send + Sync {
    /// Fire the `EnsureDrainRunning` signal — lazy spawn path.
    fn ensure_drain_running(&self) -> Result<(), DslTransitionError>;

    /// Fire the `SpawnDrain { mode }` input — explicit spawn with typed mode.
    fn spawn_drain(&self, mode: DrainMode) -> Result<(), DslTransitionError>;

    /// Fire the `StopDrain` input.
    fn stop_drain(&self) -> Result<(), DslTransitionError>;

    /// Fire the `NotifyDrainExited { reason }` input with a typed reason.
    fn notify_drain_exited(&self, reason: DrainExitReason) -> Result<(), DslTransitionError>;
}

// ---------------------------------------------------------------------------
// ExternalToolSurfaceHandle
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SurfaceSnapshot {
    pub surface_id: String,
    /// Typed base lifecycle state (dogma #5, #17 — no stringly discriminants
    /// across the cross-crate handle boundary).
    pub base_state: Option<ExternalToolSurfaceBaseState>,
    pub pending_op: ExternalToolSurfacePendingOp,
    pub staged_op: ExternalToolSurfaceStagedOp,
    pub staged_intent_sequence: Option<u64>,
    pub pending_task_sequence: Option<u64>,
    pub pending_lineage_sequence: Option<u64>,
    pub inflight_calls: u64,
    /// Typed last-emitted delta operation (dogma #5, #17).
    pub last_delta_operation: Option<ExternalToolSurfaceDeltaOperation>,
    /// Typed last-emitted delta phase (dogma #5, #17).
    pub last_delta_phase: Option<ExternalToolSurfaceDeltaPhase>,
    pub removal_draining_since_ms: Option<u64>,
    pub removal_timeout_at_ms: Option<u64>,
    pub removal_applied_at_turn: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SurfaceDiagnosticSnapshot {
    pub surface_phase: ExternalToolSurfaceGlobalPhase,
    pub known_surfaces: BTreeSet<String>,
    pub visible_surfaces: BTreeSet<String>,
    pub snapshot_epoch: u64,
    pub snapshot_aligned_epoch: u64,
    pub has_pending_or_staged: bool,
    pub entries: Vec<SurfaceSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExternalToolSurfaceInput {
    SetRemovalTimeout {
        timeout_ms: u64,
    },
    StageAdd {
        surface_id: String,
        now_ms: u64,
    },
    StageRemove {
        surface_id: String,
        now_ms: u64,
    },
    StageReload {
        surface_id: String,
        now_ms: u64,
    },
    ApplyBoundary {
        surface_id: String,
        now_ms: u64,
        staged_intent_sequence: u64,
        applied_at_turn: u64,
    },
    MarkPendingSucceeded {
        surface_id: String,
        pending_task_sequence: u64,
        staged_intent_sequence: u64,
    },
    MarkPendingFailed {
        surface_id: String,
        pending_task_sequence: u64,
        staged_intent_sequence: u64,
        cause: ExternalToolSurfaceFailureCause,
    },
    CallStarted {
        surface_id: String,
    },
    CallFinished {
        surface_id: String,
    },
    FinalizeRemovalClean {
        surface_id: String,
    },
    FinalizeRemovalForced {
        surface_id: String,
    },
    SnapshotAligned {
        epoch: u64,
    },
    Shutdown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExternalToolSurfaceEffect {
    ScheduleSurfaceCompletion {
        surface_id: String,
        operation: ExternalToolSurfaceDeltaOperation,
        pending_task_sequence: u64,
        staged_intent_sequence: u64,
        applied_at_turn: u64,
    },
    RefreshVisibleSurfaceSet {
        snapshot_epoch: u64,
    },
    EmitExternalToolDelta {
        surface_id: String,
        operation: ExternalToolSurfaceDeltaOperation,
        phase: ExternalToolSurfaceDeltaPhase,
        cause: Option<ExternalToolSurfaceFailureCause>,
    },
    CloseSurfaceConnection {
        surface_id: String,
    },
    RejectSurfaceCall {
        surface_id: String,
        cause: ExternalToolSurfaceFailureCause,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalToolSurfaceTransition {
    pub phase: ExternalToolSurfaceGlobalPhase,
    pub effects: Vec<ExternalToolSurfaceEffect>,
}

/// External tool surface lifecycle DSL handle.
pub trait ExternalToolSurfaceHandle: Send + Sync {
    fn apply_surface_input(
        &self,
        input: ExternalToolSurfaceInput,
    ) -> Result<ExternalToolSurfaceTransition, DslTransitionError>;

    fn register(&self, surface_id: String) -> Result<(), DslTransitionError>;

    fn stage_add(&self, surface_id: String, now_ms: u64) -> Result<(), DslTransitionError>;

    fn stage_remove(&self, surface_id: String, now_ms: u64) -> Result<(), DslTransitionError>;

    fn stage_reload(&self, surface_id: String, now_ms: u64) -> Result<(), DslTransitionError>;

    fn apply_boundary(
        &self,
        surface_id: String,
        now_ms: u64,
        staged_intent_sequence: u64,
        applied_at_turn: u64,
    ) -> Result<(), DslTransitionError>;

    fn mark_pending_succeeded(
        &self,
        surface_id: String,
        pending_task_sequence: u64,
        staged_intent_sequence: u64,
    ) -> Result<(), DslTransitionError>;

    fn mark_pending_failed(
        &self,
        surface_id: String,
        pending_task_sequence: u64,
        staged_intent_sequence: u64,
        cause: ExternalToolSurfaceFailureCause,
    ) -> Result<(), DslTransitionError>;

    fn call_started(&self, surface_id: String) -> Result<(), DslTransitionError>;

    fn call_finished(&self, surface_id: String) -> Result<(), DslTransitionError>;

    fn finalize_removal_clean(&self, surface_id: String) -> Result<(), DslTransitionError>;

    fn finalize_removal_forced(&self, surface_id: String) -> Result<(), DslTransitionError>;

    fn snapshot_aligned(&self, epoch: u64) -> Result<(), DslTransitionError>;

    fn shutdown_surface(&self) -> Result<(), DslTransitionError>;

    fn surface_snapshot(&self, surface_id: &str) -> Option<SurfaceSnapshot>;

    fn diagnostic_snapshot(&self) -> SurfaceDiagnosticSnapshot;

    fn visible_surfaces(&self) -> BTreeSet<String>;

    fn removing_surfaces(&self) -> BTreeSet<String>;

    fn pending_surfaces(&self) -> BTreeSet<String>;

    fn has_pending_or_staged(&self) -> bool;

    fn snapshot_epoch(&self) -> u64;

    fn snapshot_aligned_epoch(&self) -> u64;
}

// ---------------------------------------------------------------------------
// PeerCommsHandle
// ---------------------------------------------------------------------------

/// Peer comms ingress classification DSL handle.
///
/// Covers the peer-envelope classification and receive/dequeue authority
/// signals on the MeerkatMachine DSL. Runtime-backed comms ingress hands
/// parsed transport facts and queue observations to this handle and receives
/// the complete typed classification/admission/phase facts back. A rejection
/// is authoritative and callers fail closed. Classified comms ingress without
/// this session DSL handle fails closed rather than deriving machine facts in
/// the transport shell.
pub trait PeerCommsHandle: Send + Sync {
    /// Fire the `ClassifyExternalEnvelope` signal and return machine-owned
    /// admission facts for the parsed envelope.
    fn classify_external_envelope(
        &self,
        facts: PeerIngressEnvelopeFacts,
    ) -> Result<PeerIngressAdmission, DslTransitionError>;

    /// Fire the `ClassifyPlainEvent` signal and return machine-owned
    /// admission facts for the parsed plain event.
    fn classify_plain_event(
        &self,
        facts: PeerIngressPlainEventFacts,
    ) -> Result<PeerIngressAdmission, DslTransitionError>;

    /// Fire `ResolvePeerIngressReceive` and return the machine-owned
    /// admission outcome plus authority phase for a classified peer envelope.
    fn resolve_peer_ingress_receive(
        &self,
        facts: PeerIngressReceiveFacts,
    ) -> Result<PeerIngressReceiveAuthority, DslTransitionError>;

    /// Fire `ResolvePeerIngressDequeue` and return the machine-owned
    /// authority phase for a classified queue dequeue observation.
    fn resolve_peer_ingress_dequeue(
        &self,
        facts: PeerIngressDequeueFacts,
    ) -> Result<PeerIngressDequeueAuthority, DslTransitionError>;

    /// Fire the `SetPeerIngressContext { keep_alive }` input.
    fn set_peer_ingress_context(&self, keep_alive: bool) -> Result<(), DslTransitionError>;

    /// Route a local-runtime endpoint observation through generated machine
    /// authority and install the accepted generated authority package on the
    /// target.
    fn install_generated_peer_comms_on_target(
        &self,
        _expected_owner: &crate::comms::GeneratedPeerCommsOwnerToken,
        _target: &(dyn PeerCommsInstallTarget + '_),
    ) -> Result<(), String> {
        Err("peer-comms handle does not expose generated install target authority".to_string())
    }
}

#[derive(Clone)]
pub struct GeneratedPeerCommsInstallFactory {
    handle: std::sync::Arc<dyn PeerCommsHandle>,
    owner_token: crate::comms::GeneratedPeerCommsOwnerToken,
}

impl std::fmt::Debug for GeneratedPeerCommsInstallFactory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GeneratedPeerCommsInstallFactory")
            .field("handle", &"<dyn PeerCommsHandle>")
            .field("owner_token", &self.owner_token)
            .finish()
    }
}

impl GeneratedPeerCommsInstallFactory {
    #[cfg(all(meerkat_internal_generated_authority_bridge, not(test)))]
    #[doc(hidden)]
    pub fn __from_runtime_generated_authority(
        token: &'static (dyn Any + Send + Sync),
        handle: std::sync::Arc<dyn PeerCommsHandle>,
        owner_token: std::sync::Arc<dyn Any + Send + Sync>,
    ) -> Result<Self, String> {
        validate_peer_comms_install_bridge_token(token)?;
        Ok(Self {
            handle,
            owner_token: crate::comms::GeneratedPeerCommsOwnerToken::from_generated_owner_token(
                owner_token,
            ),
        })
    }

    pub fn peer_comms_handle(&self) -> &std::sync::Arc<dyn PeerCommsHandle> {
        &self.handle
    }

    pub fn install_on_target(
        &self,
        target: &(dyn PeerCommsInstallTarget + '_),
    ) -> Result<(), String> {
        self.handle
            .install_generated_peer_comms_on_target(&self.owner_token, target)
    }
}

#[derive(Clone)]
pub struct GeneratedPeerCommsInstall {
    handle: std::sync::Arc<dyn PeerCommsHandle>,
    owner_token: crate::comms::GeneratedPeerCommsOwnerToken,
    target_peer_id: crate::comms::PeerId,
}

impl std::fmt::Debug for GeneratedPeerCommsInstall {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GeneratedPeerCommsInstall")
            .field("handle", &"<dyn PeerCommsHandle>")
            .field("owner_token", &self.owner_token)
            .field("target_peer_id", &self.target_peer_id)
            .finish()
    }
}

impl GeneratedPeerCommsInstall {
    #[cfg(all(meerkat_internal_generated_authority_bridge, not(test)))]
    #[doc(hidden)]
    pub fn __from_runtime_generated_authority(
        token: &'static (dyn Any + Send + Sync),
        handle: std::sync::Arc<dyn PeerCommsHandle>,
        owner_token: std::sync::Arc<dyn Any + Send + Sync>,
        target_peer_id: crate::comms::PeerId,
    ) -> Result<Self, String> {
        validate_peer_comms_install_bridge_token(token)?;
        Ok(Self {
            handle,
            owner_token: crate::comms::GeneratedPeerCommsOwnerToken::from_generated_owner_token(
                owner_token,
            ),
            target_peer_id,
        })
    }

    pub fn peer_comms_handle(&self) -> &std::sync::Arc<dyn PeerCommsHandle> {
        &self.handle
    }

    pub fn owner_token(&self) -> crate::comms::GeneratedPeerCommsOwnerToken {
        self.owner_token.clone()
    }

    pub fn target_peer_id(&self) -> crate::comms::PeerId {
        self.target_peer_id
    }
}

#[cfg(all(meerkat_internal_generated_authority_bridge, not(test)))]
#[allow(improper_ctypes_definitions, unsafe_code)]
unsafe extern "Rust" {
    #[link_name = concat!(
        "__meerkat_runtime_generated_authority_bridge_token_is_valid_v1_comms_trust_reconcile_",
        env!("MEERKAT_GENERATED_AUTHORITY_BRIDGE_SYMBOL_SUFFIX")
    )]
    fn runtime_peer_comms_install_generated_authority_bridge_token_is_valid(
        token: &(dyn Any + Send + Sync),
    ) -> bool;
}

#[cfg(all(meerkat_internal_generated_authority_bridge, not(test)))]
fn validate_peer_comms_install_bridge_token(token: &(dyn Any + Send + Sync)) -> Result<(), String> {
    #[allow(unsafe_code)]
    let valid =
        unsafe { runtime_peer_comms_install_generated_authority_bridge_token_is_valid(token) };
    if valid {
        Ok(())
    } else {
        Err("generated peer-comms install requires the matching generated runtime protocol bridge token".into())
    }
}

/// Target that can bind a peer-comms handle to its generated trust owner.
///
/// Generic [`PeerCommsHandle`] implementations classify ingress only. This
/// path accepts an opaque generated install package rather than a raw owner
/// token, so handwritten code cannot bind machine trust facts by copying a
/// token into a fake handle.
pub trait PeerCommsInstallTarget: crate::agent::CommsRuntime {
    fn generated_peer_comms_target_endpoint(
        &self,
    ) -> Result<crate::comms::TrustedPeerDescriptor, String> {
        let peer_id = self
            .peer_id()
            .ok_or_else(|| "runtime peer_id unavailable".to_string())?;
        let name = self
            .comms_name()
            .ok_or_else(|| "runtime comms_name unavailable".to_string())?;
        let address = self
            .advertised_address()
            .ok_or_else(|| "runtime advertised_address unavailable".to_string())?;
        let pubkey = self
            .public_key_bytes()
            .ok_or_else(|| "runtime public_key_bytes unavailable".to_string())?;
        crate::comms::TrustedPeerDescriptor::unsigned_with_pubkey(
            name,
            peer_id.to_string(),
            pubkey,
            address,
        )
        .map_err(|error| format!("runtime peer-comms install target endpoint invalid: {error}"))
    }

    fn install_generated_peer_comms_handle(
        &self,
        install: GeneratedPeerCommsInstall,
    ) -> Result<(), String>;
}

// ---------------------------------------------------------------------------
// SessionAdmissionHandle
// ---------------------------------------------------------------------------

/// Session turn admission DSL handle.
///
/// Covers the admission-adjacent inputs on the MeerkatMachine DSL: ingest an
/// input into the session, accept it (with or without wake), and prepare a
/// run. Commit terminalization is owned by the runtime loop's
/// `commit_runtime_loop_run` durable receipt path; failed run return is owned
/// by the runtime turn-state path after a typed terminal cause is recorded.
/// These inputs manage the input-lifecycle
/// substate maps (`input_phases`, `input_run_associations`, etc.) and the
/// top-level `current_run_id` / `pre_run_phase` fields.
pub trait SessionAdmissionHandle: Send + Sync {
    /// Fire the `Ingest { runtime_id, work_id, origin }` input.
    ///
    /// `runtime_id` is the stringified logical runtime id; `work_id` the
    /// stringified work identifier (typically the same domain as `InputId`).
    /// `origin` is the typed transport source that admitted the input
    /// (dogma #5, #17 — no stringly discriminants across the handle boundary).
    fn ingest(
        &self,
        runtime_id: &str,
        work_id: &str,
        origin: InputSource,
    ) -> Result<(), DslTransitionError>;

    /// Fire the `AcceptWithCompletion { input_id, request_immediate_processing,
    /// interrupt_yielding, wake_if_idle, run_id }` input.
    ///
    /// `wake_if_idle` carries the policy-level "this input must wake the
    /// runtime loop once the session reaches idle" intent (e.g.
    /// `peer_response_terminal` staged while running): the DSL's
    /// Running+Queued transition splits on it and emits a
    /// `PostAdmissionSignal::WakeLoop` so the pending wake lands on the
    /// next idle reach. Idle/Attached queued arms already wake
    /// unconditionally, so the flag is ignored in those guards.
    fn accept_with_completion(
        &self,
        input_id: &InputId,
        request_immediate_processing: bool,
        interrupt_yielding: bool,
        wake_if_idle: bool,
    ) -> Result<(), DslTransitionError>;

    /// Fire the `AcceptWithoutWake { input_id }` input.
    fn accept_without_wake(&self, input_id: &InputId) -> Result<(), DslTransitionError>;

    /// Fire the `Prepare { session_id, run_id }` input — bound for the session this handle was prepared for.
    fn prepare(&self, run_id: &RunId) -> Result<(), DslTransitionError>;
}

// ---------------------------------------------------------------------------
// AuthLeaseHandle (Phase 1.5-rev)
// ---------------------------------------------------------------------------

/// Typed key for one auth lease machine.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LeaseKey {
    pub realm: crate::connection::RealmId,
    pub binding: crate::connection::BindingId,
    pub profile: Option<crate::connection::ProfileId>,
}

impl LeaseKey {
    pub fn new(
        realm: crate::connection::RealmId,
        binding: crate::connection::BindingId,
        profile: Option<crate::connection::ProfileId>,
    ) -> Self {
        Self {
            realm,
            binding,
            profile,
        }
    }

    pub fn from_auth_binding(auth_binding: &crate::connection::AuthBindingRef) -> Self {
        Self {
            realm: auth_binding.realm.clone(),
            binding: auth_binding.binding.clone(),
            profile: auth_binding.profile.clone(),
        }
    }
}

impl std::fmt::Display for LeaseKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.profile {
            Some(profile) => write!(f, "{}:{}:{}", self.realm, self.binding, profile),
            None => write!(f, "{}:{}", self.realm, self.binding),
        }
    }
}

/// Observable snapshot of an auth lease's DSL state for a given [`LeaseKey`].
///
/// Returned by [`AuthLeaseHandle::snapshot`]. If the binding is not tracked
/// at all, `phase` is `None` and `expires_at` is `None`. `generation`
/// advances when credential material is published. Non-publishing lifecycle
/// transitions, such as marking a lease expiring or a transient refresh failure,
/// do not advance this credential marker generation. This lets consumers
/// distinguish a stale projection from a freshly reacquired lease even when the
/// expiry timestamp is unchanged, without invalidating retryable stored
/// credentials after state-only transitions. OAuth login-flow membership
/// transitions also do not advance this credential marker generation.
/// `credential_present` distinguishes credential lifecycle authority from
/// OAuth login-flow membership that may keep an AuthMachine instance alive
/// after credential rollback.
/// `credential_published_at_millis` advances only when credential material is
/// acquired/refreshed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthLeaseSnapshot {
    pub phase: Option<AuthLeasePhase>,
    pub expires_at: Option<u64>,
    pub credential_present: bool,
    pub generation: u64,
    pub credential_published_at_millis: Option<u64>,
}

/// Opaque token for restoring a previously captured auth lease snapshot.
///
/// This is intentionally not constructible from public snapshot fields. A
/// rollback caller must first capture it from an [`AuthLeaseHandle`], then hand
/// that exact token back to the same authority boundary if a later durable
/// write fails.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthLeaseRestoreSnapshot {
    lease_key: LeaseKey,
    snapshot: AuthLeaseSnapshot,
    captured_by: std::any::TypeId,
    captured_by_instance: usize,
}

impl AuthLeaseRestoreSnapshot {
    fn capture(
        lease_key: LeaseKey,
        snapshot: AuthLeaseSnapshot,
        captured_by: std::any::TypeId,
        captured_by_instance: usize,
    ) -> Self {
        Self {
            lease_key,
            snapshot,
            captured_by,
            captured_by_instance,
        }
    }

    pub fn lease_key(&self) -> &LeaseKey {
        &self.lease_key
    }

    pub fn snapshot(&self) -> &AuthLeaseSnapshot {
        &self.snapshot
    }

    #[doc(hidden)]
    pub fn captured_by_type_id(&self) -> std::any::TypeId {
        self.captured_by
    }

    #[doc(hidden)]
    pub fn captured_by_instance_id(&self) -> usize {
        self.captured_by_instance
    }
}

/// Result of an accepted auth lease lifecycle transition.
///
/// `generation` is the projection version assigned while the transition is
/// accepted, so consumers can bind derived material to the exact lease state
/// that published it without taking a later snapshot.
/// `credential_published_at_millis` is the durable credential publication
/// timestamp attached to acquired/refreshed credential material.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthLeaseTransition {
    lease_key: LeaseKey,
    phase: AuthLeasePhase,
    expires_at: u64,
    generation: u64,
    credential_published_at_millis: Option<u64>,
}

impl AuthLeaseTransition {
    pub fn lease_key(&self) -> &LeaseKey {
        &self.lease_key
    }

    pub fn phase(&self) -> AuthLeasePhase {
        self.phase
    }

    pub fn expires_at(&self) -> u64 {
        self.expires_at
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn credential_published_at_millis(&self) -> Option<u64> {
        self.credential_published_at_millis
    }

    #[cfg_attr(
        any(not(meerkat_internal_generated_authority_bridge), test),
        allow(dead_code)
    )]
    fn from_generated_auth_lease_publication_parts(
        lease_key: LeaseKey,
        phase: AuthLeasePhase,
        expires_at: u64,
        generation: u64,
        credential_published_at_millis: Option<u64>,
    ) -> Self {
        Self {
            lease_key,
            phase,
            expires_at,
            generation,
            credential_published_at_millis,
        }
    }
}

/// Auth lease lifecycle handle certified by the generated AuthMachine
/// publication authority.
///
/// The wrapped trait object remains the mechanical dispatch surface, but
/// production resolver/factory seams accept this type so callers cannot install
/// an arbitrary handwritten reducer as lifecycle authority.
#[derive(Clone)]
pub struct GeneratedAuthLeaseHandle {
    inner: Arc<dyn AuthLeaseHandle>,
}

impl GeneratedAuthLeaseHandle {
    pub fn as_handle(&self) -> &dyn AuthLeaseHandle {
        self.inner.as_ref()
    }

    pub fn clone_handle(&self) -> Arc<dyn AuthLeaseHandle> {
        Arc::clone(&self.inner)
    }

    #[cfg_attr(
        any(not(meerkat_internal_generated_authority_bridge), test),
        allow(dead_code)
    )]
    fn from_generated_authority(inner: Arc<dyn AuthLeaseHandle>) -> Self {
        Self { inner }
    }
}

impl std::fmt::Debug for GeneratedAuthLeaseHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GeneratedAuthLeaseHandle")
            .finish_non_exhaustive()
    }
}

impl std::ops::Deref for GeneratedAuthLeaseHandle {
    type Target = dyn AuthLeaseHandle;

    fn deref(&self) -> &Self::Target {
        self.inner.as_ref()
    }
}

impl AsRef<dyn AuthLeaseHandle> for GeneratedAuthLeaseHandle {
    fn as_ref(&self) -> &dyn AuthLeaseHandle {
        self.inner.as_ref()
    }
}

#[cfg(all(meerkat_internal_generated_authority_bridge, not(test)))]
#[allow(improper_ctypes_definitions, unsafe_code)]
unsafe extern "Rust" {
    #[link_name = concat!(
        "__meerkat_runtime_generated_authority_bridge_token_is_valid_v1_auth_lease_lifecycle_publication_",
        env!("MEERKAT_GENERATED_AUTHORITY_BRIDGE_SYMBOL_SUFFIX")
    )]
    fn runtime_auth_lease_lifecycle_publication_generated_authority_bridge_token_is_valid(
        token: &(dyn std::any::Any + Send + Sync),
    ) -> bool;
}

#[cfg(all(meerkat_internal_generated_authority_bridge, not(test)))]
#[doc(hidden)]
#[allow(improper_ctypes_definitions, unsafe_code)]
#[unsafe(export_name = concat!(
    "__meerkat_core_runtime_generated_auth_lease_transition_build_v1_",
    env!("MEERKAT_GENERATED_AUTHORITY_BRIDGE_SYMBOL_SUFFIX")
))]
pub(crate) extern "Rust" fn runtime_generated_auth_lease_transition_build(
    token: &'static (dyn std::any::Any + Send + Sync),
    lease_key: LeaseKey,
    phase: AuthLeasePhase,
    expires_at: u64,
    generation: u64,
    credential_published_at_millis: Option<u64>,
) -> Result<AuthLeaseTransition, String> {
    validate_runtime_generated_authority_bridge_token(token)?;
    Ok(
        AuthLeaseTransition::from_generated_auth_lease_publication_parts(
            lease_key,
            phase,
            expires_at,
            generation,
            credential_published_at_millis,
        ),
    )
}

#[cfg(all(meerkat_internal_generated_authority_bridge, not(test)))]
#[doc(hidden)]
#[allow(improper_ctypes_definitions, unsafe_code)]
#[unsafe(export_name = concat!(
    "__meerkat_core_runtime_generated_auth_lease_handle_build_v1_",
    env!("MEERKAT_GENERATED_AUTHORITY_BRIDGE_SYMBOL_SUFFIX")
))]
pub(crate) extern "Rust" fn runtime_generated_auth_lease_handle_build(
    token: &'static (dyn std::any::Any + Send + Sync),
    handle: Arc<dyn AuthLeaseHandle>,
) -> Result<GeneratedAuthLeaseHandle, String> {
    validate_runtime_generated_authority_bridge_token(token)?;
    Ok(GeneratedAuthLeaseHandle::from_generated_authority(handle))
}

#[cfg(all(meerkat_internal_generated_authority_bridge, not(test)))]
fn validate_runtime_generated_authority_bridge_token(
    token: &(dyn std::any::Any + Send + Sync),
) -> Result<(), String> {
    #[allow(unsafe_code)]
    let valid = unsafe {
        runtime_auth_lease_lifecycle_publication_generated_authority_bridge_token_is_valid(token)
    };
    if valid {
        Ok(())
    } else {
        Err(
            "generated auth lease transition requires the generated AuthMachine protocol bridge token"
                .into(),
        )
    }
}

/// Window (in seconds) before `expires_at` at which a `valid` lease is
/// eligible to transition into `expiring` at the next CallingLlm
/// boundary. Owned here — on the handle trait module — rather than in
/// shell code, per dogma §9 ("policy composes at the facade/factory
/// seam, not in random helpers") and §20 ("every important behavior
/// reduces to one clear owner").
///
/// The actual state transition is gated by the AuthMachine DSL's
/// `MarkAuthExpiring` input (which enforces the `valid → expiring`
/// legality); this constant only controls *when* the runner fires
/// that input, not whether the transition is legal.
pub const AUTH_LEASE_TTL_REFRESH_WINDOW_SECS: u64 = 60;

/// Auth lease lifecycle DSL handle.
pub trait AuthLeaseHandle: Send + Sync + std::any::Any {
    /// Fire `AcquireAuthLease { lease_key, expires_at }` — unconditional.
    ///
    /// Moves the binding into `auth_valid_leases` and records its expiry.
    /// Returns the generation assigned by the accepted transition.
    fn acquire_lease(
        &self,
        lease_key: &LeaseKey,
        expires_at: u64,
    ) -> Result<AuthLeaseTransition, DslTransitionError>;

    /// Fire `MarkAuthExpiring { lease_key }` — only legal from `valid`.
    fn mark_expiring(&self, lease_key: &LeaseKey) -> Result<(), DslTransitionError>;

    /// Fire `ObserveCredentialFreshness { lease_key, now, refresh_window }`.
    ///
    /// AuthMachine owns whether the credential remains valid, becomes
    /// expiring, or becomes expired from its observed expiry facts.
    fn observe_credential_freshness(
        &self,
        lease_key: &LeaseKey,
        now: u64,
        refresh_window_secs: u64,
    ) -> Result<(), DslTransitionError>;

    /// Fire `BeginAuthRefresh { lease_key }` — legal from `valid` or
    /// `expiring` or `expired`.
    ///
    /// Provides the DSL-level refresh dedup: once the binding is in
    /// `auth_refreshing_leases`, no concurrent `BeginAuthRefresh` is
    /// permitted until `CompleteAuthRefresh` or `AuthRefreshFailed` moves
    /// it back out.
    fn begin_refresh(&self, lease_key: &LeaseKey) -> Result<(), DslTransitionError>;

    /// Fire `CompleteAuthRefresh { lease_key, new_expires_at, now }` — only
    /// legal from `refreshing`. Returns the generation assigned by the accepted
    /// transition.
    fn complete_refresh(
        &self,
        lease_key: &LeaseKey,
        new_expires_at: u64,
        now: u64,
    ) -> Result<AuthLeaseTransition, DslTransitionError>;

    /// Fire the typed refresh-failure observation input — only legal from
    /// `refreshing`. AuthMachine decides the permanent/transient lifecycle
    /// result from the observation.
    fn refresh_failed(
        &self,
        lease_key: &LeaseKey,
        observation: RefreshFailureObservation,
    ) -> Result<(), DslTransitionError>;

    /// Fire `MarkReauthRequired { lease_key }` — any known state → reauth.
    fn mark_reauth_required(&self, lease_key: &LeaseKey) -> Result<(), DslTransitionError>;

    /// Fire `ReleaseAuthLease { lease_key }` — removes the binding from all
    /// sets and the expiry map.
    fn release_lease(&self, lease_key: &LeaseKey) -> Result<(), DslTransitionError>;

    /// Clear credential lifecycle authority without treating persisted token
    /// bytes as a new lease source.
    ///
    /// Handles that co-locate short-lived OAuth flow membership with credential
    /// lifecycle state should preserve those flow memberships when clearing
    /// only the credential side after a failed login commit.
    fn release_credential_lifecycle(&self, lease_key: &LeaseKey) -> Result<(), DslTransitionError> {
        self.release_lease(lease_key)
    }

    /// Capture the current lifecycle snapshot for possible rollback.
    ///
    /// The returned token is an opaque handoff for `restore_auth_lifecycle_snapshot`;
    /// callers may inspect the read-only snapshot but cannot fabricate a restore
    /// request from token-store or projection metadata.
    fn capture_auth_lifecycle_restore_snapshot(
        &self,
        lease_key: &LeaseKey,
    ) -> AuthLeaseRestoreSnapshot {
        AuthLeaseRestoreSnapshot::capture(
            lease_key.clone(),
            self.snapshot(lease_key),
            self.type_id(),
            self.auth_lifecycle_restore_instance_id(),
        )
    }

    #[doc(hidden)]
    fn auth_lifecycle_restore_instance_id(&self) -> usize {
        std::ptr::from_ref(self).cast::<()>() as usize
    }

    /// Restore a captured lifecycle snapshot after a later durable write failed.
    ///
    /// Production handles must implement this through generated machine
    /// authority. The default fails closed so a handwritten handle cannot become
    /// a lifecycle reducer by replaying public snapshot fields.
    fn restore_auth_lifecycle_snapshot(
        &self,
        snapshot: &AuthLeaseRestoreSnapshot,
    ) -> Result<Option<AuthLeaseTransition>, DslTransitionError> {
        let _ = snapshot;
        Err(DslTransitionError::no_matching(
            "AuthLeaseHandle::restore_auth_lifecycle_snapshot",
            "restoring auth lifecycle snapshots requires generated AuthMachine authority",
        ))
    }

    /// Restore a durable credential lifecycle publication through generated
    /// AuthMachine authority.
    ///
    /// The default fails closed so token stores cannot become handwritten
    /// lifecycle reducers. Production handles must route this through the
    /// generated `RestoreAuthoritySnapshot` input before exposing a restored
    /// lease transition.
    fn restore_published_credential_lifecycle(
        &self,
        lease_key: &LeaseKey,
        publication: &crate::generated::auth_lease_durable_lifecycle_marker::AuthLeaseDurableRestorePublication,
    ) -> Result<AuthLeaseTransition, DslTransitionError> {
        let _ = (lease_key, publication);
        Err(DslTransitionError::no_matching(
            "AuthLeaseHandle::restore_published_credential_lifecycle",
            "restoring durable auth lifecycle publications requires generated AuthMachine authority",
        ))
    }

    /// Classify the credential-use disposition for a binding under `intent`.
    ///
    /// Drives the per-binding AuthMachine's `ResolveCredentialUseAdmission`
    /// read-only classifier over the live machine and mirrors the emitted
    /// `CredentialUseAdmissionResolved` disposition. The AuthMachine owns the
    /// complete `(lifecycle_phase, credential_present, intent)` -> disposition
    /// POLICY; the caller (the auth-core resolver) extracts only the typed
    /// `intent` and mirrors the verdict.
    ///
    /// Production handles must implement this through generated AuthMachine
    /// authority. The default fails closed so a handwritten handle cannot become
    /// a credential-use reducer.
    fn resolve_credential_use_admission(
        &self,
        lease_key: &LeaseKey,
        intent: CredentialUseIntent,
    ) -> Result<CredentialUseDisposition, DslTransitionError> {
        let _ = (lease_key, intent);
        Err(DslTransitionError::no_matching(
            "AuthLeaseHandle::resolve_credential_use_admission",
            "classifying credential-use admission requires generated AuthMachine authority",
        ))
    }

    /// Classify the OAuth-login cached-vs-refresh disposition for a binding.
    ///
    /// Drives the per-binding AuthMachine's
    /// `ResolveOAuthLoginCredentialDisposition` read-only classifier over the
    /// live machine and mirrors the emitted `CredentialUseAdmissionResolved`
    /// disposition. The AuthMachine owns the complete `(lifecycle_phase,
    /// self.credential_present, credential_present, force_refresh,
    /// refresh_allowed)` -> disposition POLICY; the provider runtime shell
    /// extracts only the pure [`OAuthLoginCredentialFacts`] observations and
    /// mirrors the verdict (`Authorized` -> use cached, `RefreshRequired` ->
    /// begin refresh, `RefreshDisallowed` -> refresh-required error, etc.).
    ///
    /// Production handles must implement this through generated AuthMachine
    /// authority. The default fails closed so a handwritten handle cannot become
    /// a cached-vs-refresh reducer.
    fn resolve_oauth_login_credential_disposition(
        &self,
        lease_key: &LeaseKey,
        facts: OAuthLoginCredentialFacts,
    ) -> Result<CredentialUseDisposition, DslTransitionError> {
        let _ = (lease_key, facts);
        Err(DslTransitionError::no_matching(
            "AuthLeaseHandle::resolve_oauth_login_credential_disposition",
            "classifying OAuth-login credential disposition requires generated AuthMachine authority",
        ))
    }

    /// Observe the current DSL-level state of a binding.
    fn snapshot(&self, lease_key: &LeaseKey) -> AuthLeaseSnapshot;
}

// ---------------------------------------------------------------------------
// McpServerLifecycleHandle (Phase 5G / T5g)
// ---------------------------------------------------------------------------

/// MCP client handshake lifecycle DSL handle (session-scoped).
///
/// Routes each per-server MCP handshake event into the MeerkatMachine DSL's
/// `mcp_server_states` substate. Distinct from the external-tool surface
/// lifecycle (which tracks staged/pending *tool surface* intents): this handle
/// tracks per-server *connection* lifecycle (PendingConnect → Connected |
/// Failed | Disconnected), keyed by the configured MCP server name.
///
/// Read side (`pending_server_ids`) is the authoritative source for the
/// `[MCP_PENDING]` system-notice toggle — any server in `PendingConnect` means
/// the notice is emitted; otherwise the notice is suppressed.
///
/// Concrete impls live in `meerkat-runtime`; standalone callers (tests,
/// fixtures) pass `None` for the handle and the router's shell-level behavior
/// remains identical (DSL record-keeping is skipped, which is fine because
/// there is no session DSL to mirror into).
pub trait McpServerLifecycleHandle: Send + Sync {
    /// Fire `McpServerConnectPending { server_id }` — server staged for
    /// background connect.
    fn apply_connect_pending(&self, server_id: &str) -> Result<(), DslTransitionError>;

    /// Fire `McpServerConnected { server_id }` — handshake succeeded.
    fn apply_connected(&self, server_id: &str) -> Result<(), DslTransitionError>;

    /// Fire `McpServerFailed { server_id, error }` — handshake failed.
    fn apply_failed(&self, server_id: &str, error: &str) -> Result<(), DslTransitionError>;

    /// Fire `McpServerDisconnected { server_id }` — connection closed.
    fn apply_disconnected(&self, server_id: &str) -> Result<(), DslTransitionError>;

    /// Fire `McpServerReload { server_id }` — reload requested; server returns
    /// to `PendingConnect` while the shell tears down and redials.
    fn apply_reload(&self, server_id: &str) -> Result<(), DslTransitionError>;

    /// Observe the set of server ids currently in `PendingConnect`.
    ///
    /// Used by the agent loop to drive the `[MCP_PENDING]` system-notice
    /// lifecycle: non-empty → emit notice; empty → strip notice.
    fn pending_server_ids(&self) -> BTreeSet<String>;
}

// ---------------------------------------------------------------------------
// PeerInteractionHandle (W1-A / issue #264)
// ---------------------------------------------------------------------------

/// Terminal disposition companion for [`PeerInteractionHandle::response_terminal`].
///
/// Carried as a typed wire value so the DSL can route `Completed` / `Failed`
/// terminal transitions without the shell re-interpreting `ResponseStatus`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PeerTerminalDisposition {
    /// Terminal response with `Completed` status.
    Completed,
    /// Terminal response with `Failed` status.
    Failed,
}

/// Peer request / response lifecycle DSL handle (W1-A).
///
/// Routes the full peer-interaction lifecycle — outbound `Sent`,
/// progress / terminal response arrival, timeouts, and inbound
/// `Received` / `Replied` — into the MeerkatMachine DSL's
/// `pending_peer_requests` / `inbound_peer_requests` substate maps.
///
/// Terminal transitions emit a DSL-owned cleanup effect that the shell
/// observes to drop any subscriber / stream channel associated with the
/// correlation id. The channels themselves live in shell-owned maps (they
/// hold `mpsc::Sender` values that cannot live in DSL state); those maps
/// are strict projections of DSL state, with the invariant "channel live
/// iff `corr_id ∈ pending ∧ state ≠ terminal`" enforced by the effect.
pub trait PeerInteractionHandle: Send + Sync {
    /// Fire `PeerRequestSent { corr_id }`.
    ///
    /// Guard: `corr_id` is not already in `pending_peer_requests`.
    fn request_sent(&self, corr_id: PeerCorrelationId) -> Result<(), DslTransitionError>;

    /// Fire `PeerResponseProgressArrived { corr_id }`.
    ///
    /// Guard: `corr_id` is in `pending_peer_requests`. Progress after
    /// progress is admitted as a self-loop (the DSL overwrites the state
    /// slot). Rejects on unknown corr_id.
    fn response_progress(&self, corr_id: PeerCorrelationId) -> Result<(), DslTransitionError>;

    /// Fire `PeerResponseTerminalArrived { corr_id, disposition }`.
    ///
    /// Guard: `corr_id` is in `pending_peer_requests`. Terminal transitions
    /// remove the map entry and emit the `PeerInteractionCleanup` effect,
    /// so any second terminal on the same corr_id is rejected at the
    /// `pending_exists` guard by construction.
    fn response_terminal(
        &self,
        corr_id: PeerCorrelationId,
        disposition: PeerTerminalDisposition,
    ) -> Result<(), DslTransitionError>;

    /// Fire `PeerResponseRejected { corr_id }`.
    ///
    /// Guard: `corr_id` is in `pending_peer_requests`. This is used when
    /// peer ingress produced a response observation that cannot be admitted
    /// as progress or terminal because the generated terminality feedback is
    /// missing or inconsistent. The DSL owns the terminal cleanup; callers do
    /// not substitute a response disposition.
    fn response_rejected(&self, corr_id: PeerCorrelationId) -> Result<(), DslTransitionError>;

    /// Fire `PeerRequestTimedOut { corr_id }`.
    ///
    /// Guard: `corr_id` is in `pending_peer_requests`. Like `response_terminal`,
    /// the map entry is removed on success and the `PeerInteractionCleanup`
    /// effect is emitted; subsequent fires fail the guard.
    fn request_timed_out(&self, corr_id: PeerCorrelationId) -> Result<(), DslTransitionError>;

    /// Fire `PeerRequestSendFailed { corr_id }` (#291).
    ///
    /// Distinct from `request_timed_out`: the outbound request never reached the
    /// peer (transport/send failure), versus a genuine elapsed-deadline timeout.
    /// Emits `OutboundPeerRequestState::Failed` and removes the pending entry,
    /// mirroring the `PeerResponseRejected` failure disposition.
    fn request_send_failed(&self, corr_id: PeerCorrelationId) -> Result<(), DslTransitionError>;

    /// Fire `PeerRequestReceived { corr_id, handling_mode }` (inbound).
    ///
    /// Guard: `corr_id` is not already in `inbound_peer_requests`.
    fn request_received(
        &self,
        corr_id: PeerCorrelationId,
        handling_mode: HandlingMode,
    ) -> Result<(), DslTransitionError>;

    /// Ask the generated machine authority to classify a typed outbound reply
    /// status before shell transport send/cleanup code consumes terminality.
    fn classify_response_reply(
        &self,
        status: crate::ResponseStatus,
    ) -> Result<crate::TerminalityClass, DslTransitionError>;

    /// Fire `PeerResponseReplied { corr_id }` (inbound reply sent).
    ///
    /// Guard: `corr_id` is in `inbound_peer_requests` with state `Received`.
    fn response_replied(&self, corr_id: PeerCorrelationId) -> Result<(), DslTransitionError>;

    /// Observe the DSL-owned state of an outbound peer request.
    ///
    /// Returns `None` if the correlation id is not in `pending_peer_requests`.
    fn outbound_state(&self, corr_id: PeerCorrelationId) -> Option<OutboundPeerRequestState>;

    /// Observe the DSL-owned state of an inbound peer request.
    fn inbound_state(&self, corr_id: PeerCorrelationId) -> Option<InboundPeerRequestState>;

    /// Observe the DSL-owned handling-mode default for an inbound peer request.
    fn inbound_handling_mode(&self, corr_id: PeerCorrelationId) -> Option<HandlingMode>;

    /// Install a projection-cleanup observer for the peer-interaction
    /// lifecycle. The runtime handle invokes the observer whenever a DSL
    /// transition emits `PeerInteractionCleanup`, closing the loop
    /// "terminal transition → effect → shell projection cleanup".
    ///
    /// Implementations with no observer simply drop any emitted cleanup
    /// notifications on the floor. Standalone / WASM paths leave this
    /// unset.
    fn install_cleanup_observer(&self, observer: Arc<dyn PeerInteractionCleanupObserver>);
}

/// Observer invoked by [`PeerInteractionHandle`] when a DSL
/// `PeerInteractionCleanup` effect is emitted.
///
/// Shell-owned projection consumers (the comms runtime's subscriber /
/// stream registries) implement this to drop channel entries keyed on the
/// terminated correlation id. The observer is invoked under the same
/// authority lock as the transition that emitted the effect, so the
/// "terminal transition → effect → cleanup" chain is causal, not lexically
/// adjacent.
pub trait PeerInteractionCleanupObserver: Send + Sync {
    /// Called once per emitted `PeerInteractionCleanup { corr_id }` effect.
    ///
    /// Idempotent: a well-formed DSL run emits exactly one cleanup per
    /// correlation id because terminal transitions remove the map entry
    /// (subsequent attempts are rejected at the `pending_exists` guard),
    /// but observers should tolerate a redundant call defensively.
    fn on_peer_interaction_cleanup(&self, corr_id: PeerCorrelationId);
}

/// Session-context advancement DSL handle (W2-E / issue #264).
///
/// Shell callers fire `context_advanced(updated_at_ms)` at every site that
/// mutates canonical session truth (prompt append, external content
/// injection, tool-result append, external assistant output,
/// runtime-system-context append, any `summary_tx.send_replace`). The
/// transition is monotonic: the DSL guard drops ticks whose `updated_at_ms`
/// isn't strictly greater than the last recorded watermark, so callers can
/// fire unconditionally post-mutation.
///
/// Every successful transition emits `SessionContextAdvanced` which is
/// dispatched to the installed [`SessionContextAdvancedObserver`] — the
/// realtime projection consumer uses the observer to drive a typed
/// `ProjectionFreshness` state instead of polling a watch channel.
pub trait SessionContextHandle: Send + Sync {
    /// Fire `AdvanceSessionContext { updated_at_ms }`.
    ///
    /// Guard: `updated_at_ms` is strictly greater than the last recorded
    /// watermark. Returns `Ok(false)` when the guard rejects the tick as
    /// non-advancing (duplicate or out-of-order); returns `Ok(true)` when
    /// the transition lands and the effect is emitted. Transition errors
    /// (lock poisoning, unexpected DSL state) surface as `Err`.
    fn context_advanced(&self, updated_at_ms: u64) -> Result<bool, DslTransitionError>;

    /// The monotonic watermark in milliseconds of the last successful
    /// `AdvanceSessionContext` transition recorded on this handle.
    ///
    /// Returns `0` before any advance has been recorded. The realtime
    /// projection consumer reads this once at install time to seed its
    /// `ProjectionFreshness` baseline, so the consumer and the DSL agree
    /// on the initial frontier by construction (no two-read race).
    fn current_watermark_ms(&self) -> u64;

    /// Install a typed observer for `SessionContextAdvanced` effect
    /// emission. Implementations without an installed observer drop the
    /// effect on the floor (standalone / WASM paths).
    fn install_observer(&self, observer: Arc<dyn SessionContextAdvancedObserver>);

    /// Atomically install a typed observer and return the current watermark
    /// as a single critical section. Implementations MUST hold the same
    /// authority lock that `context_advanced` uses for both the watermark
    /// read and the observer installation, so no `SessionContextAdvanced`
    /// effect can slip between "sampled baseline" and "observer visible".
    ///
    /// Callers use the returned `u64` as their `ProjectionFreshness`
    /// baseline; any subsequent `context_advanced` tick is guaranteed to
    /// either (a) have already been included in the returned watermark, or
    /// (b) be visible to the observer. The `current_watermark_ms` +
    /// `install_observer` pair is NOT a substitute: a transition can land
    /// between those two non-atomic steps and be lost to both the baseline
    /// and the observer.
    fn install_observer_with_baseline(
        &self,
        observer: Arc<dyn SessionContextAdvancedObserver>,
    ) -> u64;
}

/// Observer invoked by [`SessionContextHandle`] when a DSL
/// `SessionContextAdvanced` effect is emitted (W2-E / issue #264).
///
/// The realtime projection consumer implements this to advance its typed
/// `ProjectionFreshness` state. Runtime handles sample the installed
/// observer under the same authority lock as the transition that emitted
/// the effect, then dispatch the callback immediately after releasing the
/// lock so re-entrant observer implementations can safely route back
/// through the same DSL authority.
pub trait SessionContextAdvancedObserver: Send + Sync {
    /// Called once per emitted `SessionContextAdvanced { updated_at_ms }`
    /// effect. `updated_at_ms` is the monotonic millisecond watermark of
    /// the canonical session-context mutation that produced this tick.
    fn on_session_context_advanced(&self, updated_at_ms: u64);
}

// ---------------------------------------------------------------------------
// SessionClaimHandle (dogma #2 — canonical session-identity owner)
// ---------------------------------------------------------------------------

/// Error surfaced by [`SessionClaimHandle::try_acquire`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SessionClaimError {
    /// Another live claim already exists for this session id.
    #[error("session identity already claimed: {0}")]
    SessionIdentityInUse(SessionId),
}

/// RAII token returned by [`SessionClaimHandle::try_acquire`].
///
/// While alive, the underlying registry guarantees no other caller can
/// acquire a claim for the same `session_id`. Drop releases the claim back
/// through the owning handle.
pub struct SessionClaim {
    session_id: SessionId,
    handle: Arc<dyn SessionClaimHandle>,
}

impl SessionClaim {
    /// Construct a new claim — only [`SessionClaimHandle`] impls should call
    /// this, immediately after they have inserted `session_id` into their
    /// canonical registry under a single critical section.
    pub fn new(session_id: SessionId, handle: Arc<dyn SessionClaimHandle>) -> Self {
        Self { session_id, handle }
    }

    /// The session id this claim covers.
    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }
}

impl Drop for SessionClaim {
    fn drop(&mut self) {
        self.handle.release(&self.session_id);
    }
}

impl std::fmt::Debug for SessionClaim {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionClaim")
            .field("session_id", &self.session_id)
            .finish_non_exhaustive()
    }
}

/// Process-scope canonical owner of "this session id is currently active."
///
/// One canonical owner per process: `MeerkatMachine` exposes its registry
/// when a runtime is wired (so every live runtime-registered session also
/// owns its identity claim), and a default in-process registry covers bare
/// `AgentFactory` callers without a runtime. Either way, "this session id
/// is in use" lives in a typed owner — never in process-global shell
/// bookkeeping.
pub trait SessionClaimHandle: Send + Sync {
    /// Atomically reserve `session_id`. Returns a [`SessionClaim`] whose
    /// `Drop` releases the slot. Returns
    /// [`SessionClaimError::SessionIdentityInUse`] if another live claim
    /// already covers this session.
    ///
    /// Implementations MUST insert under a single critical section so two
    /// concurrent callers cannot both succeed.
    fn try_acquire(
        self: Arc<Self>,
        session_id: &SessionId,
    ) -> Result<SessionClaim, SessionClaimError>;

    /// Release a claim previously created by [`Self::try_acquire`].
    ///
    /// Called from [`SessionClaim`]'s `Drop`. Idempotent: releasing an
    /// unknown id is a no-op (the registry was already cleared, e.g. via
    /// runtime teardown).
    fn release(&self, session_id: &SessionId);
}

/// In-process default [`SessionClaimHandle`] for bare-usage paths that have
/// no `MeerkatMachine` available (standalone `AgentFactory` callers, doc
/// examples, simple SDK consumers). One process-global instance keeps the
/// "one active claim per session id" invariant intact even when no runtime
/// is wired.
pub struct DefaultSessionClaimRegistry {
    claims: std::sync::Mutex<std::collections::HashSet<SessionId>>,
}

impl DefaultSessionClaimRegistry {
    /// Construct an empty registry.
    pub fn new() -> Self {
        Self {
            claims: std::sync::Mutex::new(std::collections::HashSet::new()),
        }
    }

    /// Process-global instance — used by bare-usage facade builders.
    pub fn global() -> Arc<Self> {
        use std::sync::OnceLock;
        static GLOBAL: OnceLock<Arc<DefaultSessionClaimRegistry>> = OnceLock::new();
        Arc::clone(GLOBAL.get_or_init(|| Arc::new(DefaultSessionClaimRegistry::new())))
    }
}

impl Default for DefaultSessionClaimRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionClaimHandle for DefaultSessionClaimRegistry {
    fn try_acquire(
        self: Arc<Self>,
        session_id: &SessionId,
    ) -> Result<SessionClaim, SessionClaimError> {
        let mut claims = self
            .claims
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !claims.insert(session_id.clone()) {
            return Err(SessionClaimError::SessionIdentityInUse(session_id.clone()));
        }
        drop(claims);
        Ok(SessionClaim::new(
            session_id.clone(),
            self as Arc<dyn SessionClaimHandle>,
        ))
    }

    fn release(&self, session_id: &SessionId) {
        let mut claims = self
            .claims
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        claims.remove(session_id);
    }
}

// ---------------------------------------------------------------------------
// InteractionStreamHandle (U6 / dogma #5)
// ---------------------------------------------------------------------------

/// Interaction stream lifecycle DSL handle.
///
/// Routes the reservation/attach/completion/expire/close-early/abandon lifecycle of
/// a streamed interaction into the MeerkatMachine DSL's `interaction_streams`
/// substate map. The shell-side `interaction_stream_registry` projects
/// sender/receiver channels off this map; terminal transitions emit
/// [`InteractionStreamCleanupObserver::on_interaction_stream_cleanup`], which
/// the comms runtime uses to drop the channel projection.
///
/// Reservation TTL is shell-owned mechanics: the runtime holds the timestamp
/// and decides when to fire `expired`. Every state-meaning decision (is the
/// reservation still claimable? has the consumer attached? did a terminal
/// event win the race?) lives in the DSL.
pub trait InteractionStreamHandle: Send + Sync {
    /// Fire `InteractionStreamReserved { corr_id }`.
    ///
    /// Guard: `corr_id` is not already in `interaction_streams`. Rejected
    /// duplicates surface as [`DslTransitionError`] so the shell can refuse
    /// to register two channels under the same key.
    fn reserved(&self, corr_id: PeerCorrelationId) -> Result<(), DslTransitionError>;

    /// Fire `InteractionStreamAttached { corr_id }`.
    ///
    /// Guard: state is `Reserved`. Rejected if the reservation already
    /// expired, the consumer already attached, or the entry never existed.
    fn attached(&self, corr_id: PeerCorrelationId) -> Result<(), DslTransitionError>;

    /// Fire `InteractionStreamCompleted { corr_id }`.
    ///
    /// Guard: state is `Attached`. Terminal — emits the cleanup effect.
    fn completed(&self, corr_id: PeerCorrelationId) -> Result<(), DslTransitionError>;

    /// Fire `InteractionStreamExpired { corr_id }`.
    ///
    /// Guard: state is `Reserved`. Terminal — emits the cleanup effect.
    fn expired(&self, corr_id: PeerCorrelationId) -> Result<(), DslTransitionError>;

    /// Fire `InteractionStreamClosedEarly { corr_id }`.
    ///
    /// Guard: state is `Attached`. Terminal — emits the cleanup effect.
    fn closed_early(&self, corr_id: PeerCorrelationId) -> Result<(), DslTransitionError>;

    /// Fire `InteractionStreamAbandoned { corr_id, reason }`.
    ///
    /// Guard: state is `Reserved` or `Attached`. Terminal — emits the cleanup
    /// effect. This is the explicit failure path for send/admission/response
    /// delivery failures; callers must not substitute `expired` or a peer
    /// request timeout for these observations.
    fn abandoned(
        &self,
        corr_id: PeerCorrelationId,
        reason: InteractionStreamAbandonReason,
    ) -> Result<(), DslTransitionError>;

    /// Read the DSL-owned state for a given correlation id, if any.
    ///
    /// Returns `None` when the entry has already been removed (terminal or
    /// never reserved). Active states (`Reserved`, `Attached`) surface as
    /// `Some(..)`; terminal variants surface only via the
    /// `InteractionStreamStateChanged` effect, never on the active map.
    fn state(&self, corr_id: PeerCorrelationId) -> Option<InteractionStreamState>;

    /// Install a projection-cleanup observer for the interaction stream
    /// lifecycle. The runtime handle invokes the observer whenever a DSL
    /// transition emits `InteractionStreamCleanup`, closing the loop
    /// "terminal transition → effect → shell projection cleanup".
    fn install_cleanup_observer(&self, observer: Arc<dyn InteractionStreamCleanupObserver>);
}

/// Observer invoked by [`InteractionStreamHandle`] when a DSL
/// `InteractionStreamCleanup` effect is emitted.
///
/// Shell-owned projection consumers (the comms runtime's
/// `interaction_stream_registry`) implement this to drop channel entries
/// keyed on the terminated correlation id. Runtime handles sample the
/// observer under the same authority lock as the transition that emitted
/// the effect, then dispatch after releasing the lock.
pub trait InteractionStreamCleanupObserver: Send + Sync {
    /// Called once per emitted `InteractionStreamCleanup` effect. The optional
    /// reason is present only for the generated `Abandoned` terminal and lets
    /// the shell project that typed fault without reclassifying it.
    ///
    /// Idempotent in the well-formed case (terminal transitions remove the
    /// map entry so subsequent fires fail the guard), but observers should
    /// tolerate redundant calls defensively.
    fn on_interaction_stream_cleanup(
        &self,
        corr_id: PeerCorrelationId,
        abandon_reason: Option<InteractionStreamAbandonReason>,
    );
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::{
        DslRejectionKind, DslTransitionError, ExternalToolSurfaceEffect,
        ExternalToolSurfaceFailureCause, ExternalToolSurfaceInput, PeerConversationProjection,
        PeerResponseProgressProjectionPhase, PeerResponseTerminalCorrelationId,
        PeerResponseTerminalDisplayIdentity, PeerResponseTerminalFact,
        PeerResponseTerminalFactError, PeerResponseTerminalProjectionStatus,
        PeerResponseTerminalRenderPayload, PeerResponseTerminalRouteIdentity,
        PeerResponseTerminalSource, PeerResponseTerminalTransportIdentity,
        peer_response_terminal_context_key,
    };
    use crate::tool_scope::{ExternalToolSurfaceDeltaOperation, ExternalToolSurfaceDeltaPhase};

    #[test]
    fn recovered_state_rejection_is_not_guard_noop() {
        let err =
            DslTransitionError::recovered_state_invariant_rejected("recover", "bad invariant");
        assert_eq!(err.kind, DslRejectionKind::RecoveredStateInvariantRejected);
        assert!(!err.is_guard_rejected());
    }

    #[test]
    fn external_tool_surface_pending_failure_cause_projects_external_code() {
        let input = ExternalToolSurfaceInput::MarkPendingFailed {
            surface_id: "alpha".to_owned(),
            pending_task_sequence: 7,
            staged_intent_sequence: 11,
            cause: ExternalToolSurfaceFailureCause::PendingFailed,
        };

        let ExternalToolSurfaceInput::MarkPendingFailed { cause, .. } = input else {
            panic!("constructed MarkPendingFailed input");
        };
        assert_eq!(cause, ExternalToolSurfaceFailureCause::PendingFailed);
        assert_eq!(cause.as_str(), "pending_failed");
        assert_eq!(
            serde_json::to_value(cause).expect("serialize failure cause"),
            serde_json::json!("pending_failed")
        );

        let effect = ExternalToolSurfaceEffect::EmitExternalToolDelta {
            surface_id: "alpha".to_owned(),
            operation: ExternalToolSurfaceDeltaOperation::Add,
            phase: ExternalToolSurfaceDeltaPhase::Failed,
            cause: Some(cause),
        };
        assert!(matches!(
            effect,
            ExternalToolSurfaceEffect::EmitExternalToolDelta {
                cause: Some(ExternalToolSurfaceFailureCause::PendingFailed),
                ..
            }
        ));
    }

    #[test]
    fn peer_terminal_projection_owns_prompt_and_context_key() {
        let route_id = "550e8400-e29b-41d4-a716-446655440000";
        let route_identity =
            PeerResponseTerminalRouteIdentity::parse(route_id).expect("route identity");
        let correlation_id =
            PeerResponseTerminalCorrelationId::parse("018f6f79-7a82-7c4e-a552-a3b86f9630f1")
                .expect("correlation id");
        let projection = PeerConversationProjection::ResponseTerminal {
            fact: PeerResponseTerminalFact::new(
                PeerResponseTerminalSource::new(
                    Some(
                        PeerResponseTerminalTransportIdentity::parse("transport-runtime-1")
                            .expect("transport identity"),
                    ),
                    route_identity,
                    PeerResponseTerminalDisplayIdentity::parse("Analyst")
                        .expect("display identity"),
                ),
                correlation_id,
                PeerResponseTerminalProjectionStatus::Completed,
                PeerResponseTerminalRenderPayload::new(Some(serde_json::json!({
                    "request_intent": "checksum_token",
                    "request_subject": "alpha beta gamma",
                    "token": "birch seventeen"
                }))),
            ),
        };

        assert_eq!(
            projection.context_key().as_deref(),
            Some(
                "peer_response_terminal:550e8400-e29b-41d4-a716-446655440000:018f6f79-7a82-7c4e-a552-a3b86f9630f1"
            )
        );
        assert_eq!(
            projection.prompt_text(),
            "Peer terminal response from Analyst. Request ID: 018f6f79-7a82-7c4e-a552-a3b86f9630f1. Status: completed. Result: {\n  \"request_intent\": \"checksum_token\",\n  \"request_subject\": \"alpha beta gamma\",\n  \"token\": \"birch seventeen\"\n}."
        );
    }

    #[test]
    fn peer_terminal_fact_is_structural_projection_only() {
        let route_id = "550e8400-e29b-41d4-a716-446655440000";
        let route_identity =
            PeerResponseTerminalRouteIdentity::parse(route_id).expect("route identity");
        let correlation_id =
            PeerResponseTerminalCorrelationId::parse("018f6f79-7a82-7c4e-a552-a3b86f9630f1")
                .expect("correlation id");

        let fact = PeerResponseTerminalFact::new(
            PeerResponseTerminalSource::new(
                None,
                route_identity,
                PeerResponseTerminalDisplayIdentity::parse("Analyst").expect("display identity"),
            ),
            correlation_id,
            PeerResponseTerminalProjectionStatus::Cancelled,
            PeerResponseTerminalRenderPayload::new(None),
        );

        assert_eq!(
            fact.status,
            PeerResponseTerminalProjectionStatus::Cancelled,
            "status support is decided by generated admission authority, not fact construction"
        );
    }

    #[test]
    fn peer_progress_projection_formats_phase_from_shared_seam() {
        let projection = PeerConversationProjection::ResponseProgress {
            peer_id: "operator-rt".into(),
            request_id: "req-789".into(),
            phase: PeerResponseProgressProjectionPhase::PartialResult,
            payload: Some(serde_json::json!({ "chunk": "alpha" })),
        };

        assert_eq!(projection.context_key(), None);
        assert_eq!(
            projection.prompt_text(),
            "Peer response progress from operator-rt. Request ID: req-789. Phase: partial_result. Payload: {\n  \"chunk\": \"alpha\"\n}."
        );
    }

    #[test]
    fn peer_terminal_context_key_helper_stays_canonical() {
        let route_id = "550e8400-e29b-41d4-a716-446655440000";
        let route_identity =
            PeerResponseTerminalRouteIdentity::parse(route_id).expect("route identity");
        let correlation_id =
            PeerResponseTerminalCorrelationId::parse("018f6f79-7a82-7c4e-a552-a3b86f9630f1")
                .expect("correlation id");
        assert_eq!(
            peer_response_terminal_context_key(&route_identity, correlation_id),
            "peer_response_terminal:550e8400-e29b-41d4-a716-446655440000:018f6f79-7a82-7c4e-a552-a3b86f9630f1"
        );
    }

    #[test]
    fn peer_terminal_route_identity_rejects_display_name_alias() {
        assert!(matches!(
            PeerResponseTerminalRouteIdentity::parse("analyst-rt"),
            Err(PeerResponseTerminalFactError::InvalidRouteIdentity)
        ));
    }

    #[test]
    fn peer_terminal_fact_round_trips_through_serde() {
        // The typed fact is now persisted on `PendingSystemContextAppend` and
        // read back by the realtime consumer instead of re-parsing flattened
        // prompt text, so it must survive a durable serde round-trip.
        let fact = PeerResponseTerminalFact::new(
            PeerResponseTerminalSource::parse(
                Some("inproc://analyst"),
                "550e8400-e29b-41d4-a716-446655440000",
                "analyst-rt",
            )
            .expect("source"),
            PeerResponseTerminalCorrelationId::parse("018f6f79-7a82-7c4e-a552-a3b86f9630f1")
                .expect("correlation id"),
            PeerResponseTerminalProjectionStatus::Completed,
            PeerResponseTerminalRenderPayload::new(Some(serde_json::json!({
                "request_intent": "checksum_token",
                "token": "birch seventeen",
            }))),
        );

        let json = serde_json::to_string(&fact).expect("serialize fact");
        let decoded: PeerResponseTerminalFact =
            serde_json::from_str(&json).expect("deserialize fact");
        assert_eq!(decoded, fact);
        assert_eq!(
            decoded.context_key(),
            "peer_response_terminal:550e8400-e29b-41d4-a716-446655440000:018f6f79-7a82-7c4e-a552-a3b86f9630f1"
        );
        assert_eq!(
            decoded
                .render_payload_value()
                .and_then(|payload| payload.get("token"))
                .and_then(|token| token.as_str()),
            Some("birch seventeen")
        );
    }
}
