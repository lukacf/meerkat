//! Host preparation before an identity's first use, either custody (0.9.9).
//!
//! The host's rule for an identity's profile may require its preparation
//! before the identity is first seated ([`IdentityPreparationMode`]): a
//! store-owned identity's create records it pending
//! ([`crate::identity::CreatedIdentityPresence::PendingPreparation`]), and a
//! roster-owned identity's declaration names the mode
//! ([`super::DeclaredEntry::with_preparation`]), derived again at each apply.
//! Such an identity is unseated ([`super::UnseatedEntryCause::PreparationPending`])
//! and never runs: a delivery to it is refused retryable.
//!
//! A `required` preparation ends with its settle: a store-owned identity's
//! settle ([`super::MobHandle::settle_identity_preparation`]) seats it
//! Dormant from its row's spec, and a spec that owes a kickoff runs it once;
//! a roster-owned identity is settled by its host's next declaration, which
//! names no preparation.
//!
//! A `constructed` preparation is a held construction the host activates: its
//! host asks for one ([`super::MobHandle::construct_identity`]), Core holds
//! the identity's member constructed and unpublished, starting no run, and the
//! host reads its witnesses, then settles naming the attempt, which activates
//! exactly that construction. The mob owns each attempt (MobMachine's
//! held-construction record): a dropped observer or a lost reply ends
//! nothing. Before an activation is accepted, an abort, the attempt's
//! deadline, a crash or a removal ends it, and the identity stays pending;
//! once accepted (the settle's receipt journals it), nothing replaces it until
//! its publication ends: published, or failed, or interrupted by a crash, in
//! which case the identity stays pending.

use crate::MobError;
use crate::identity::IdentityPreparationMode;
use crate::ids::AgentIdentity;

use super::DeclaredEntryOutcome;
use super::identity_change::{
    IdentityChangeConsentInput, IdentityChangeOutcome, IdentityChangeRequesterInput,
    IdentityOperation,
};

/// How a held construction ended
/// ([`PreparationSettlement::PublicationEnded`],
/// [`super::MobHandle::abort_identity_construction`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ConstructionEnd {
    /// Aborted by its host before an activation was accepted.
    Aborted,
    /// Its deadline passed before an activation was accepted.
    Expired,
    /// The construction could not complete: its spawn was refused or failed
    /// before its hold.
    Refused,
    /// An accepted activation's publication failed.
    PublicationFailed,
    /// An accepted activation that a restart found without its publication.
    PublicationInterrupted,
}

/// A host's request for a held construction of an identity awaiting its
/// `constructed` preparation ([`super::MobHandle::construct_identity`]).
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct IdentityConstructionRequest {
    pub operation: IdentityOperation,
    pub identity: AgentIdentity,
    /// The attempt's deadline: an attempt not activated before it ends.
    pub deadline: super::held_spawn::HeldSpawnDeadline,
    /// A roster-owned identity's spec, as its host declares it; a
    /// store-owned identity's is its row's.
    pub roster_spec: Option<super::SpawnMemberSpec>,
}

impl IdentityConstructionRequest {
    #[must_use]
    pub fn new(
        operation: IdentityOperation,
        identity: impl Into<AgentIdentity>,
        deadline: super::held_spawn::HeldSpawnDeadline,
    ) -> Self {
        Self {
            operation,
            identity: identity.into(),
            deadline,
            roster_spec: None,
        }
    }

    /// The roster-owned identity's spec, as its host declares it.
    #[must_use]
    pub fn with_roster_spec(mut self, spec: super::SpawnMemberSpec) -> Self {
        self.roster_spec = Some(spec);
        self
    }
}

/// An observer of one held construction: its attempt and the witnesses its
/// host's issuance needs. The mob owns the construction itself; dropping
/// this ends nothing.
#[derive(Debug, Clone)]
pub struct HeldPreparation {
    pub(super) identity: AgentIdentity,
    pub(super) operation_id: String,
    pub(super) attempt: u64,
    pub(super) deadline: super::held_spawn::HeldSpawnDeadline,
    pub(super) session_id: Option<meerkat_core::types::SessionId>,
    #[cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]
    pub(super) finished_surface: meerkat_core::tool_surface::ToolSurfaceSelection,
    #[cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]
    pub(super) controller_route: super::session_service::ControllerRouteObservation,
}

impl HeldPreparation {
    /// The identity held.
    pub fn identity(&self) -> &AgentIdentity {
        &self.identity
    }

    /// The operation that asked for the construction.
    pub fn operation_id(&self) -> &str {
        &self.operation_id
    }

    /// The attempt a settle names to activate exactly this construction.
    pub fn attempt(&self) -> u64 {
        self.attempt
    }

    /// The attempt's deadline.
    pub fn deadline(&self) -> super::held_spawn::HeldSpawnDeadline {
        self.deadline
    }

    /// The held member's session, whose registration is live during the
    /// hold.
    pub fn session_id(&self) -> Option<&meerkat_core::types::SessionId> {
        self.session_id.as_ref()
    }

    /// The exact tool surface selection the held member's construction was
    /// finished with.
    #[cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]
    pub fn finished_surface(&self) -> &meerkat_core::tool_surface::ToolSurfaceSelection {
        &self.finished_surface
    }

    /// The controller route the held member's actor reported under the hold.
    #[cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]
    pub fn controller_route_report(&self) -> &super::session_service::ControllerRouteObservation {
        &self.controller_route
    }
}

/// A host's settle of an identity's preparation
/// ([`super::MobHandle::settle_identity_preparation`]): with an attempt, the
/// activation of exactly that held construction.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct IdentityPreparationSettle {
    pub operation: IdentityOperation,
    pub requester: IdentityChangeRequesterInput,
    pub identity: AgentIdentity,
    pub attempt: Option<u64>,
}

impl IdentityPreparationSettle {
    #[must_use]
    pub fn new(
        operation: IdentityOperation,
        requester: IdentityChangeRequesterInput,
        identity: impl Into<AgentIdentity>,
    ) -> Self {
        Self {
            operation,
            requester,
            identity: identity.into(),
            attempt: None,
        }
    }

    /// Activate exactly the held construction of `attempt`.
    #[must_use]
    pub fn with_attempt(mut self, attempt: u64) -> Self {
        self.attempt = Some(attempt);
        self
    }
}

/// What a settle did to its identity.
#[derive(Debug)]
#[non_exhaustive]
pub enum PreparationSettlement {
    /// No construction: the identity's seat, as the declared apply answers
    /// it (seated Dormant, embodied for an owed kickoff, or hosted on its
    /// first use).
    Seated(DeclaredEntryOutcome),
    /// The held construction of `attempt` was activated and published.
    Activated { attempt: u64 },
    /// The activation of `attempt` was accepted; its publication is in
    /// flight.
    ActivationPending { attempt: u64 },
    /// The accepted activation of `attempt` ended without its publication:
    /// the identity stays pending.
    PublicationEnded { attempt: u64, end: ConstructionEnd },
}

/// What a settle answers: the change (committed now, with its receipt, or
/// replayed), and what it did to the identity.
#[derive(Debug)]
#[non_exhaustive]
pub struct IdentityPreparationSettled {
    pub change: IdentityChangeOutcome,
    pub settlement: Option<PreparationSettlement>,
}

impl super::MobHandle {
    /// Ask for a held construction of an identity awaiting its `constructed`
    /// preparation (see the module docs).
    #[cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]
    pub async fn construct_identity(
        &self,
        request: IdentityConstructionRequest,
    ) -> Result<HeldPreparation, MobError> {
        let _ = request;
        Err(MobError::Internal(
            "held constructions are not built yet".to_string(),
        ))
    }

    /// Abort the held construction of `attempt`, before its activation was
    /// accepted.
    #[cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]
    pub async fn abort_identity_construction(
        &self,
        identity: &AgentIdentity,
        attempt: u64,
    ) -> Result<ConstructionEnd, MobError> {
        let _ = (identity, attempt);
        Err(MobError::Internal(
            "held constructions are not built yet".to_string(),
        ))
    }

    /// Settle an identity's preparation as one change (see the module docs).
    pub async fn settle_identity_preparation(
        &self,
        settle: IdentityPreparationSettle,
        consent: IdentityChangeConsentInput<'_>,
    ) -> Result<IdentityPreparationSettled, MobError> {
        let _ = (settle, consent, IdentityPreparationMode::None);
        Err(MobError::Internal(
            "preparation settles are not built yet".to_string(),
        ))
    }
}
