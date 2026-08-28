//! Source-owner service for forked-participant capabilities.
//!
//! The service is the only writer of a capability record, and it never decides
//! lifecycle legality itself: every mutation loads the record, drives the
//! canonical `ForkedParticipantLifecycleMachine` with a typed input, interprets
//! the machine's typed effects, and compare-and-swaps the result. There is no
//! handwritten transition table here.
//!
//! Three properties are worth calling out.
//!
//! - Crash safety comes from ordering. The record (with a planned child
//!   `SessionId`) is durable BEFORE the fork is taken, so a create that dies
//!   between the child save and the activation record retries against the exact
//!   same planned child.
//! - Concurrency converges instead of leaking storage errors. A lost
//!   compare-and-swap re-loads and re-drives the SAME machine input under a
//!   bounded retry, so a duplicate attach/release/revoke converges on the
//!   machine's own replay verdict and a genuinely different attachment gets the
//!   machine's typed Busy denial.
//! - Inheritance is verified, not asserted. The branch inherits the source's
//!   effective tool/auth/realm policy; the service never supplies a
//!   replacement, and a retry proves the durable child's inherited execution
//!   metadata against the source's own evidence before recording activation.

use super::types::{
    FORKED_PARTICIPANT_CLEANUP_CLAIM_TTL, ForkedParticipantAttachmentId,
    ForkedParticipantCapabilityId, ForkedParticipantCleanupAttemptId,
    ForkedParticipantCleanupClaim, ForkedParticipantCleanupClaimOutcome,
    ForkedParticipantCleanupDebt, ForkedParticipantCleanupId, ForkedParticipantCleanupLease,
    ForkedParticipantCleanupPublish, ForkedParticipantCleanupReport,
    ForkedParticipantExpirySweepReport, ForkedParticipantFingerprintError,
    ForkedParticipantForkProtection, ForkedParticipantGrant, ForkedParticipantOwnerRoute,
    ForkedParticipantPendingAttachment, ForkedParticipantPendingAttachmentReport,
    ForkedParticipantPendingTerminal, ForkedParticipantProvenance, ForkedParticipantRef,
    ForkedParticipantReleaseOutcome, ForkedParticipantRequest, ForkedParticipantRequestId,
    ForkedParticipantReservation, ForkedParticipantRevocationId,
    ForkedParticipantRevocationOutcome, ForkedParticipantSweepEntry, MAX_FORKED_PARTICIPANT_TTL,
    MAX_FORKED_PARTICIPANT_USES,
};
use crate::ids::AgentIdentity;
use crate::machines::forked_participant_lifecycle as fp;
use crate::store::{
    ForkedParticipantRecord, ForkedParticipantSidecar, ForkedParticipantStore, MobStoreError,
};
use async_trait::async_trait;
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use meerkat_core::SessionId;
use meerkat_core::connection::RealmId;
use meerkat_core::service::SessionError;
use std::sync::Arc;
use thiserror::Error;

/// Bounded number of compare-and-swap attempts per logical operation.
const FORKED_PARTICIPANT_CAS_ATTEMPTS: u32 = 5;

/// One planned durable fork request handed to the source runtime.
///
/// It carries the source member identity and the owning realm so the source
/// runtime can verify that the session it is about to fork really belongs to
/// that participant and realm BEFORE forking. There is no tool-policy field:
/// the branch inherits the source's effective policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedForkRequest {
    /// Source member the session must belong to.
    pub source_identity: AgentIdentity,
    /// Realm the source session must belong to.
    pub owner_realm: RealmId,
    /// Session to fork.
    pub source_session_id: SessionId,
    /// Child identity reserved before the fork.
    pub planned_child_session_id: SessionId,
    /// Complete-boundary prefix length; `None` selects the head.
    pub prefix_message_count: Option<usize>,
}

/// Provenance returned by a successful planned fork.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedForkOutcome {
    /// Durable child identity actually created. Must equal the planned id.
    pub child_session_id: SessionId,
    /// Number of source messages selected into the child.
    pub prefix_message_count: usize,
    /// Content digest of the selected prefix.
    pub prefix_digest: String,
}

/// Sanitized execution evidence about a session.
///
/// Used to prove inheritance without copying anything mutable: it names the
/// owning participant and realm and the effective tool/auth policy references.
/// It carries no credential material and no transcript body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionExecutionEvidence {
    /// Member identity recorded on the session, when it has one.
    pub agent_identity: Option<AgentIdentity>,
    /// Realm recorded on the session.
    pub realm_id: Option<RealmId>,
    /// Effective tool access policy recorded on the session.
    pub tool_access_policy: Option<meerkat_core::ops::ToolAccessPolicy>,
    /// Realm-scoped auth binding REFERENCE (never secret material).
    pub auth_binding: Option<meerkat_core::AuthBindingRef>,
}

/// Sanitized durable evidence about an already-created planned child.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedChildEvidence {
    /// Content digest of the child's transcript.
    pub prefix_digest: String,
    /// Number of messages in the child's transcript.
    pub prefix_message_count: usize,
    /// The child's own execution evidence.
    pub execution: SessionExecutionEvidence,
}

/// The source runtime seam the service needs.
///
/// Deliberately narrow: prove a session's execution evidence, fork one planned
/// child, prove a planned child's durable evidence, and archive a fork session
/// during cleanup. Nothing here exposes mutable session state or credentials.
#[async_trait]
pub trait ForkedParticipantSourceRuntime: Send + Sync {
    /// Read sanitized execution evidence for a session.
    async fn session_execution_evidence(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<SessionExecutionEvidence>, SessionError>;

    /// Fork the source at a complete boundary under the planned child id.
    ///
    /// Implementations must verify that the source session belongs to
    /// `source_identity` and `owner_realm` before forking.
    async fn fork_planned_child(
        &self,
        request: PlannedForkRequest,
    ) -> Result<PlannedForkOutcome, SessionError>;

    /// Read sanitized durable evidence about a planned child, if it exists.
    async fn planned_child_evidence(
        &self,
        child_session_id: &SessionId,
    ) -> Result<Option<PlannedChildEvidence>, SessionError>;

    /// Archive one fork session during cleanup.
    ///
    /// An already-absent session converges as success: cleanup is idempotent.
    async fn archive_fork_session(&self, child_session_id: &SessionId) -> Result<(), SessionError>;

    /// Stamp one fork session with the exact mob member identity it is being
    /// seated as, before ordinary resume provisioning.
    ///
    /// A capability takes its branch before any target member exists, so the
    /// child commits with no member binding. The runtime that owns the fork
    /// body is the only authority that may add one, which is why the seam
    /// lives on this trait rather than on the target side.
    ///
    /// It is deliberately narrow: it may add ONLY the member identity, never
    /// tool, auth, realm, or transcript state. Implementations must be
    /// idempotent on the exact binding and must refuse a different one.
    ///
    /// The default refuses: a runtime that cannot seat a branch as an ordinary
    /// member must say so rather than silently resume an unbound session.
    async fn bind_fork_session_to_member(
        &self,
        child_session_id: &SessionId,
        mob_id: &str,
        role: &str,
        member: &str,
    ) -> Result<(), SessionError> {
        let _ = (child_session_id, mob_id, role, member);
        Err(SessionError::Unsupported(
            "this source runtime cannot seat a fork session as a mob member".to_string(),
        ))
    }
}

/// Typed failures of the source-owner capability service.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ForkedParticipantError {
    /// The durable store refused the operation.
    #[error("forked participant store failure")]
    Store(#[source] MobStoreError),

    /// The source session runtime refused the operation.
    ///
    /// The typed [`SessionError`] is retained as the source rather than
    /// flattened to a string, so callers can still match on it.
    #[error("forked participant source session failure: {0}")]
    Session(#[source] SessionError),

    /// OS entropy for the bearer identity was unavailable.
    #[error("forked participant capability entropy failure")]
    Entropy(#[source] meerkat_core::secret_entropy::SecretEntropyError),

    /// The canonical request fingerprint could not be computed.
    #[error("forked participant request fingerprint failure")]
    Fingerprint(#[source] ForkedParticipantFingerprintError),

    /// The request is not admissible before it reaches the machine.
    #[error("invalid forked participant request: {detail}")]
    InvalidRequest {
        /// What was wrong with the request.
        detail: String,
    },

    /// The operation named a route this owner does not serve.
    #[error("forked participant route is not owned by this service: {detail}")]
    ForeignRoute {
        /// What mismatched.
        detail: String,
    },

    /// The source session does not belong to the claimed participant/realm.
    #[error("forked participant source ownership rejected: {detail}")]
    SourceOwnershipRejected {
        /// What mismatched.
        detail: String,
    },

    /// The machine refused a reservation.
    #[error("forked participant reservation rejected: {reason:?}")]
    ReservationRejected {
        /// Typed machine reason.
        reason: fp::ForkedParticipantReservationRejection,
    },

    /// The machine refused an activation record.
    #[error("forked participant activation rejected: {reason:?}")]
    ActivationRejected {
        /// Typed machine reason.
        reason: fp::ForkedParticipantActivationRejection,
    },

    /// The machine denied an attach.
    #[error("forked participant attach denied: {reason:?}")]
    AttachDenied {
        /// Typed machine reason.
        reason: fp::ForkedParticipantAttachDenial,
    },

    /// The machine rejected a release.
    #[error("forked participant release rejected: {reason:?}")]
    ReleaseRejected {
        /// Typed machine reason.
        reason: fp::ForkedParticipantReleaseRejection,
    },

    /// The machine denied a revocation.
    #[error("forked participant revocation denied: {reason:?}")]
    RevocationDenied {
        /// Typed machine reason.
        reason: fp::ForkedParticipantRevocationDenial,
    },

    /// The presented capability reference is not valid for this owner.
    #[error("forked participant capability rejected: {detail}")]
    CapabilityRejected {
        /// Why the presented reference was refused.
        detail: String,
    },

    /// The durable planned child contradicts the request that planned it.
    #[error("planned fork child conflicts with durable evidence: {detail}")]
    PlannedChildConflict {
        /// What contradicted.
        detail: String,
    },

    /// A bounded compare-and-swap retry budget was exhausted.
    #[error("forked participant record is under concurrent update: {detail}")]
    ConcurrentUpdate {
        /// Store-reported conflict detail.
        detail: String,
    },

    /// The lifecycle machine refused to transition at all.
    #[error("forked participant lifecycle machine refused: {detail}")]
    MachineRefused {
        /// Machine refusal detail.
        detail: String,
    },
}

impl From<MobStoreError> for ForkedParticipantError {
    fn from(error: MobStoreError) -> Self {
        Self::Store(error)
    }
}

impl From<SessionError> for ForkedParticipantError {
    fn from(error: SessionError) -> Self {
        Self::Session(error)
    }
}

impl From<meerkat_core::secret_entropy::SecretEntropyError> for ForkedParticipantError {
    fn from(error: meerkat_core::secret_entropy::SecretEntropyError) -> Self {
        Self::Entropy(error)
    }
}

impl From<ForkedParticipantFingerprintError> for ForkedParticipantError {
    fn from(error: ForkedParticipantFingerprintError) -> Self {
        Self::Fingerprint(error)
    }
}

fn machine_refused(detail: impl std::fmt::Debug) -> ForkedParticipantError {
    ForkedParticipantError::MachineRefused {
        detail: format!("{detail:?}"),
    }
}

/// How a transition loads the record it is about to advance.
#[derive(Clone, Copy)]
enum RecordLookup<'a> {
    /// Holder-driven: the FULL immutable reference must match.
    Exact(&'a ForkedParticipantRef),
    /// Owner-driven maintenance path.
    CapabilityId(&'a ForkedParticipantCapabilityId),
}

/// One driven transition: a typed outcome plus the record to commit, if any.
struct TransitionPlan<T> {
    outcome: T,
    next: Option<ForkedParticipantRecord>,
}

/// Source-owner service for forked-participant capabilities.
///
/// The service is constructed with the exact route it owns and refuses any
/// request or capability whose immutable route names a different owner.
pub struct ForkedParticipantService {
    owner_route: ForkedParticipantOwnerRoute,
    store: Arc<dyn ForkedParticipantStore>,
    runtime: Arc<dyn ForkedParticipantSourceRuntime>,
}

impl std::fmt::Debug for ForkedParticipantService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ForkedParticipantService")
            .field("owner_route", &self.owner_route)
            .field("store", &"<dyn ForkedParticipantStore>")
            .field("runtime", &"<dyn ForkedParticipantSourceRuntime>")
            .finish()
    }
}

impl ForkedParticipantService {
    /// Compose the service for one owned route.
    pub fn new(
        owner_route: ForkedParticipantOwnerRoute,
        store: Arc<dyn ForkedParticipantStore>,
        runtime: Arc<dyn ForkedParticipantSourceRuntime>,
    ) -> Result<Self, ForkedParticipantError> {
        Ok(Self {
            owner_route,
            store,
            runtime,
        })
    }

    /// The exact route this service owns.
    pub fn owner_route(&self) -> &ForkedParticipantOwnerRoute {
        &self.owner_route
    }

    fn require_owned_route(
        &self,
        route: &ForkedParticipantOwnerRoute,
    ) -> Result<(), ForkedParticipantError> {
        if route != &self.owner_route {
            return Err(ForkedParticipantError::ForeignRoute {
                detail: "the presented route is not the route this owner serves".to_string(),
            });
        }
        Ok(())
    }

    /// Verify that the source session really belongs to the claimed member and
    /// the owned realm before anything is reserved or forked.
    async fn require_source_ownership(
        &self,
        source_session_id: &SessionId,
        source_identity: &AgentIdentity,
    ) -> Result<SessionExecutionEvidence, ForkedParticipantError> {
        let evidence = self
            .runtime
            .session_execution_evidence(source_session_id)
            .await
            .map_err(ForkedParticipantError::Session)?
            .ok_or_else(|| ForkedParticipantError::SourceOwnershipRejected {
                detail: "source session has no durable execution evidence".to_string(),
            })?;
        match evidence.agent_identity.as_ref() {
            Some(identity) if identity == source_identity => {}
            Some(_) => {
                return Err(ForkedParticipantError::SourceOwnershipRejected {
                    detail: "source session belongs to a different member identity".to_string(),
                });
            }
            None => {
                return Err(ForkedParticipantError::SourceOwnershipRejected {
                    detail: "source session carries no member identity".to_string(),
                });
            }
        }
        match evidence.realm_id.as_ref() {
            Some(realm) if realm == self.owner_route.realm_id() => {}
            Some(_) => {
                return Err(ForkedParticipantError::SourceOwnershipRejected {
                    detail: "source session belongs to a different realm".to_string(),
                });
            }
            None => {
                return Err(ForkedParticipantError::SourceOwnershipRejected {
                    detail: "source session carries no realm".to_string(),
                });
            }
        }
        Ok(evidence)
    }

    /// Validate a request and compute its absolute expiry with checked math.
    fn validate(
        request: &ForkedParticipantRequest,
        now: DateTime<Utc>,
    ) -> Result<(u64, DateTime<Utc>), ForkedParticipantError> {
        if request.source_identity.as_str().trim().is_empty() {
            return Err(ForkedParticipantError::InvalidRequest {
                detail: "source identity must not be empty".to_string(),
            });
        }
        let max_uses = request.reuse.max_uses();
        if max_uses == 0 || max_uses > MAX_FORKED_PARTICIPANT_USES {
            return Err(ForkedParticipantError::InvalidRequest {
                detail: format!("reuse budget must be 1..={MAX_FORKED_PARTICIPANT_USES}"),
            });
        }
        if request.ttl.is_zero() {
            return Err(ForkedParticipantError::InvalidRequest {
                detail: "time-to-live must be positive".to_string(),
            });
        }
        if request.ttl > MAX_FORKED_PARTICIPANT_TTL {
            return Err(ForkedParticipantError::InvalidRequest {
                detail: format!(
                    "time-to-live must not exceed {} seconds",
                    MAX_FORKED_PARTICIPANT_TTL.as_secs()
                ),
            });
        }
        let ttl = ChronoDuration::from_std(request.ttl).map_err(|error| {
            ForkedParticipantError::InvalidRequest {
                detail: format!("time-to-live is not representable: {error}"),
            }
        })?;
        let expires_at =
            now.checked_add_signed(ttl)
                .ok_or_else(|| ForkedParticipantError::InvalidRequest {
                    detail: "expiry instant overflows the representable range".to_string(),
                })?;
        Ok((u64::from(max_uses), expires_at))
    }

    async fn load(
        &self,
        lookup: RecordLookup<'_>,
    ) -> Result<ForkedParticipantRecord, ForkedParticipantError> {
        match lookup {
            RecordLookup::Exact(capability) => {
                let record = self.store.load_exact(capability).await?;
                if &record.sidecar.owner_route != capability.owner_route() {
                    return Err(ForkedParticipantError::CapabilityRejected {
                        detail: "capability route does not match the owning record".to_string(),
                    });
                }
                if &record.sidecar.source_identity != capability.source_identity() {
                    return Err(ForkedParticipantError::CapabilityRejected {
                        detail: "capability source identity does not match the owning record"
                            .to_string(),
                    });
                }
                Ok(record)
            }
            RecordLookup::CapabilityId(capability_id) => self
                .store
                .load_by_capability_id(capability_id)
                .await?
                .ok_or_else(|| ForkedParticipantError::CapabilityRejected {
                    detail: format!("capability {} not found", capability_id.correlation_hint()),
                }),
        }
    }

    /// Load, drive the machine, and commit under a bounded compare-and-swap
    /// retry.
    ///
    /// A lost race re-loads and re-drives the SAME input, so an exact duplicate
    /// converges on the machine's own replay verdict instead of surfacing a
    /// storage conflict, and a genuinely conflicting command still gets its
    /// typed machine denial from the reloaded state.
    async fn transition<T>(
        &self,
        lookup: RecordLookup<'_>,
        mut drive: impl FnMut(
            &ForkedParticipantRecord,
        ) -> Result<TransitionPlan<T>, ForkedParticipantError>,
    ) -> Result<T, ForkedParticipantError> {
        let mut last_conflict = String::new();
        for _ in 0..FORKED_PARTICIPANT_CAS_ATTEMPTS {
            let record = self.load(lookup).await?;
            let plan = drive(&record)?;
            let Some(next) = plan.next else {
                return Ok(plan.outcome);
            };
            match self.store.commit(&next).await {
                Ok(_) => return Ok(plan.outcome),
                Err(MobStoreError::CasConflict(detail)) => {
                    last_conflict = detail;
                }
                Err(other) => return Err(other.into()),
            }
        }
        Err(ForkedParticipantError::ConcurrentUpdate {
            detail: last_conflict,
        })
    }

    /// Reserve one capability identity and planned child for a request.
    pub async fn reserve(
        &self,
        request: &ForkedParticipantRequest,
        now: DateTime<Utc>,
    ) -> Result<ForkedParticipantReservation, ForkedParticipantError> {
        self.require_owned_route(&request.owner_route)?;
        let (max_uses, expires_at) = Self::validate(request, now)?;
        self.require_source_ownership(&request.source_session_id, &request.source_identity)
            .await?;
        let fingerprint = request.fingerprint()?;

        if let Some(existing) = self.store.load_by_request_id(&request.request_id).await? {
            return self
                .replay_reservation(existing, &fingerprint, max_uses)
                .await;
        }

        let mut authority = fp::ForkedParticipantLifecycleMachineAuthority::new();
        let transition = fp::ForkedParticipantLifecycleMachineMutator::apply(
            &mut authority,
            fp::ForkedParticipantLifecycleInput::Reserve {
                request_fingerprint: fingerprint.clone(),
                max_uses,
            },
        )
        .map_err(machine_refused)?;
        Self::expect_reserved(transition.effects())?;

        let capability_id = ForkedParticipantCapabilityId::mint()?;
        let record = ForkedParticipantRecord {
            capability_id,
            request_id: request.request_id.clone(),
            request_fingerprint: fingerprint.clone(),
            planned_child_session_id: SessionId::new(),
            sidecar: ForkedParticipantSidecar {
                source_identity: request.source_identity.clone(),
                source_session_id: request.source_session_id.clone(),
                owner_route: request.owner_route.clone(),
                scope: request.scope,
                reuse: request.reuse,
                expires_at,
                requested_prefix_message_count: request.prefix_message_count,
                capability_ref: None,
            },
            machine_state: authority.state().clone(),
            cleanup_debt: None,
            cleanup_claim: None,
            revision: 0,
            created_at: now,
            updated_at: now,
        };

        match self.store.insert_reserved(&record).await {
            Ok(stored) => Ok(ForkedParticipantReservation {
                capability_id: stored.capability_id.clone(),
                request_id: stored.request_id.clone(),
                request_fingerprint: stored.request_fingerprint.clone(),
                planned_child_session_id: stored.planned_child_session_id,
                replayed: false,
            }),
            Err(MobStoreError::CasConflict(_)) => {
                // A concurrent reserve for the same request won the race. The
                // loser converges onto the winner, never minting a second
                // capability.
                let existing = self
                    .store
                    .load_by_request_id(&request.request_id)
                    .await?
                    .ok_or_else(|| ForkedParticipantError::InvalidRequest {
                        detail: "reservation conflict without a durable winner".to_string(),
                    })?;
                self.replay_reservation(existing, &fingerprint, max_uses)
                    .await
            }
            Err(error) => Err(error.into()),
        }
    }

    async fn replay_reservation(
        &self,
        existing: ForkedParticipantRecord,
        fingerprint: &str,
        max_uses: u64,
    ) -> Result<ForkedParticipantReservation, ForkedParticipantError> {
        let capability_id = existing.capability_id.clone();
        let fingerprint = fingerprint.to_owned();
        self.transition(RecordLookup::CapabilityId(&capability_id), move |record| {
            let mut authority = fp::ForkedParticipantLifecycleMachineAuthority::recover_from_state(
                record.machine_state.clone(),
            )
            .map_err(machine_refused)?;
            let transition = fp::ForkedParticipantLifecycleMachineMutator::apply(
                &mut authority,
                fp::ForkedParticipantLifecycleInput::Reserve {
                    request_fingerprint: fingerprint.clone(),
                    max_uses,
                },
            )
            .map_err(machine_refused)?;
            Self::expect_reserved(transition.effects())?;

            let outcome = ForkedParticipantReservation {
                capability_id: record.capability_id.clone(),
                request_id: record.request_id.clone(),
                request_fingerprint: record.request_fingerprint.clone(),
                planned_child_session_id: record.planned_child_session_id.clone(),
                replayed: true,
            };
            if authority.state() == &record.machine_state {
                return Ok(TransitionPlan {
                    outcome,
                    next: None,
                });
            }
            let mut next = record.clone();
            next.machine_state = authority.state().clone();
            Ok(TransitionPlan {
                outcome,
                next: Some(next),
            })
        })
        .await
    }

    fn expect_reserved(
        effects: &[fp::ForkedParticipantLifecycleEffect],
    ) -> Result<(), ForkedParticipantError> {
        for effect in effects {
            match effect {
                fp::ForkedParticipantLifecycleEffect::CapabilityReserved { .. }
                | fp::ForkedParticipantLifecycleEffect::ReservationReplayed { .. } => {}
                fp::ForkedParticipantLifecycleEffect::ReservationRejected { reason } => {
                    return Err(ForkedParticipantError::ReservationRejected { reason: *reason });
                }
                other => return Err(machine_refused(other)),
            }
        }
        Ok(())
    }

    /// Create (or converge on) the durable fork for a reserved request.
    pub async fn create(
        &self,
        request: &ForkedParticipantRequest,
        now: DateTime<Utc>,
    ) -> Result<ForkedParticipantRef, ForkedParticipantError> {
        self.require_owned_route(&request.owner_route)?;
        let source_evidence = self
            .require_source_ownership(&request.source_session_id, &request.source_identity)
            .await?;
        let fingerprint = request.fingerprint()?;

        // An already-activated record converges through the machine's exact
        // activation replay; re-reserving it would be a typed
        // `AlreadyProvisioned` rejection.
        if let Some(existing) = self.store.load_by_request_id(&request.request_id).await?
            && existing.request_fingerprint == fingerprint
            && let Some(capability) = existing.sidecar.capability_ref.clone()
        {
            return self
                .record_activation(&existing.capability_id, capability)
                .await;
        }

        let reservation = self.reserve(request, now).await?;
        let record = self
            .store
            .load_by_request_id(&request.request_id)
            .await?
            .ok_or_else(|| ForkedParticipantError::InvalidRequest {
                detail: "reserved capability disappeared before activation".to_string(),
            })?;
        if let Some(capability) = record.sidecar.capability_ref.clone() {
            return self
                .record_activation(&record.capability_id, capability)
                .await;
        }

        let planned_child = reservation.planned_child_session_id.clone();
        let outcome = match self
            .runtime
            .planned_child_evidence(&planned_child)
            .await
            .map_err(ForkedParticipantError::Session)?
        {
            Some(evidence) => {
                // Crash after the child was saved but before activation was
                // recorded. Prove the durable row against the SOURCE's own
                // evidence before trusting it.
                Self::verify_planned_child(&record, &source_evidence, &evidence)?;
                PlannedForkOutcome {
                    child_session_id: planned_child.clone(),
                    prefix_message_count: evidence.prefix_message_count,
                    prefix_digest: evidence.prefix_digest,
                }
            }
            None => {
                match self
                    .runtime
                    .fork_planned_child(PlannedForkRequest {
                        source_identity: request.source_identity.clone(),
                        owner_realm: self.owner_route.realm_id().clone(),
                        source_session_id: request.source_session_id.clone(),
                        planned_child_session_id: planned_child.clone(),
                        prefix_message_count: request.prefix_message_count,
                    })
                    .await
                {
                    Ok(outcome) => outcome,
                    Err(error) => {
                        self.record_activation_failure(&record.capability_id)
                            .await?;
                        return Err(ForkedParticipantError::Session(error));
                    }
                }
            }
        };

        if outcome.child_session_id != planned_child {
            self.record_activation_failure(&record.capability_id)
                .await?;
            return Err(ForkedParticipantError::PlannedChildConflict {
                detail: "source runtime created a child other than the planned identity"
                    .to_string(),
            });
        }

        let capability = ForkedParticipantRef::new_source_owned(
            record.capability_id.clone(),
            record.sidecar.source_identity.clone(),
            planned_child,
            record.sidecar.owner_route.clone(),
            ForkedParticipantProvenance {
                source_session_id: record.sidecar.source_session_id.clone(),
                prefix_message_count: outcome.prefix_message_count,
                prefix_digest: outcome.prefix_digest,
            },
            record.sidecar.scope,
            record.sidecar.expires_at,
            record.sidecar.reuse,
            ForkedParticipantRevocationId::for_request(&record.request_id),
            ForkedParticipantCleanupId::for_request(&record.request_id),
        );
        self.record_activation(&record.capability_id, capability)
            .await
    }

    /// Prove that a durable planned child really is this request's child.
    ///
    /// Inheritance is verified against the SOURCE's evidence, not against a
    /// caller-supplied override: the child must carry the source's effective
    /// tool policy, auth binding reference, and realm.
    fn verify_planned_child(
        record: &ForkedParticipantRecord,
        source: &SessionExecutionEvidence,
        child: &PlannedChildEvidence,
    ) -> Result<(), ForkedParticipantError> {
        if let Some(expected) = record.sidecar.requested_prefix_message_count
            && child.prefix_message_count != expected
        {
            return Err(ForkedParticipantError::PlannedChildConflict {
                detail: format!(
                    "planned child holds {} messages, request selected {expected}",
                    child.prefix_message_count
                ),
            });
        }
        if child.execution.tool_access_policy != source.tool_access_policy {
            return Err(ForkedParticipantError::PlannedChildConflict {
                detail: "planned child does not inherit the source tool access policy".to_string(),
            });
        }
        if child.execution.auth_binding != source.auth_binding {
            return Err(ForkedParticipantError::PlannedChildConflict {
                detail: "planned child does not inherit the source auth binding".to_string(),
            });
        }
        if child.execution.realm_id != source.realm_id {
            return Err(ForkedParticipantError::PlannedChildConflict {
                detail: "planned child does not inherit the source realm".to_string(),
            });
        }
        Ok(())
    }

    async fn record_activation(
        &self,
        capability_id: &ForkedParticipantCapabilityId,
        capability: ForkedParticipantRef,
    ) -> Result<ForkedParticipantRef, ForkedParticipantError> {
        let capability_for_plan = capability.clone();
        self.transition(RecordLookup::CapabilityId(capability_id), move |record| {
            let mut authority = fp::ForkedParticipantLifecycleMachineAuthority::recover_from_state(
                record.machine_state.clone(),
            )
            .map_err(machine_refused)?;
            let transition = fp::ForkedParticipantLifecycleMachineMutator::apply(
                &mut authority,
                fp::ForkedParticipantLifecycleInput::RecordForkActivation {
                    request_fingerprint: record.request_fingerprint.clone(),
                    fork_activation_id: capability_for_plan.fork_session_id().to_string(),
                },
            )
            .map_err(machine_refused)?;
            for effect in transition.effects() {
                match effect {
                    fp::ForkedParticipantLifecycleEffect::ForkActivated { .. }
                    | fp::ForkedParticipantLifecycleEffect::ForkActivationReplayed { .. } => {}
                    fp::ForkedParticipantLifecycleEffect::ActivationRejected { reason } => {
                        return Err(ForkedParticipantError::ActivationRejected { reason: *reason });
                    }
                    other => return Err(machine_refused(other)),
                }
            }

            if authority.state() == &record.machine_state
                && let Some(existing) = record.sidecar.capability_ref.clone()
            {
                return Ok(TransitionPlan {
                    outcome: existing,
                    next: None,
                });
            }
            let mut next = record.clone();
            next.machine_state = authority.state().clone();
            next.sidecar.capability_ref = Some(capability_for_plan.clone());
            Ok(TransitionPlan {
                outcome: capability_for_plan.clone(),
                next: Some(next),
            })
        })
        .await
    }

    async fn record_activation_failure(
        &self,
        capability_id: &ForkedParticipantCapabilityId,
    ) -> Result<(), ForkedParticipantError> {
        self.transition(RecordLookup::CapabilityId(capability_id), |record| {
            let mut authority = fp::ForkedParticipantLifecycleMachineAuthority::recover_from_state(
                record.machine_state.clone(),
            )
            .map_err(machine_refused)?;
            let transition = fp::ForkedParticipantLifecycleMachineMutator::apply(
                &mut authority,
                fp::ForkedParticipantLifecycleInput::RecordForkActivationFailure {
                    request_fingerprint: record.request_fingerprint.clone(),
                },
            )
            .map_err(machine_refused)?;
            for effect in transition.effects() {
                match effect {
                    fp::ForkedParticipantLifecycleEffect::ForkActivationFailed { .. }
                    | fp::ForkedParticipantLifecycleEffect::ForkActivationFailureReplayed {
                        ..
                    } => {}
                    fp::ForkedParticipantLifecycleEffect::ActivationRejected { reason } => {
                        return Err(ForkedParticipantError::ActivationRejected { reason: *reason });
                    }
                    other => return Err(machine_refused(other)),
                }
            }
            if authority.state() == &record.machine_state {
                return Ok(TransitionPlan {
                    outcome: (),
                    next: None,
                });
            }
            let mut next = record.clone();
            next.machine_state = authority.state().clone();
            Ok(TransitionPlan {
                outcome: (),
                next: Some(next),
            })
        })
        .await
    }

    /// Attach one participant to a temporary mob.
    ///
    /// `caller_authorized` is the caller-identity observation supplied by the
    /// authenticated source/temp-supervisor bridge command; `now` is the
    /// sampled wall clock. The machine reads neither for itself.
    pub async fn attach(
        &self,
        capability: &ForkedParticipantRef,
        attachment_id: &ForkedParticipantAttachmentId,
        caller_authorized: bool,
        now: DateTime<Utc>,
    ) -> Result<ForkedParticipantGrant, ForkedParticipantError> {
        self.require_owned_route(capability.owner_route())?;
        let attachment = attachment_id.clone();
        let fork_session_id = capability.fork_session_id().clone();
        self.transition(RecordLookup::Exact(capability), move |record| {
            let expired = now >= record.sidecar.expires_at;
            let mut authority = fp::ForkedParticipantLifecycleMachineAuthority::recover_from_state(
                record.machine_state.clone(),
            )
            .map_err(machine_refused)?;
            let transition = fp::ForkedParticipantLifecycleMachineMutator::apply(
                &mut authority,
                fp::ForkedParticipantLifecycleInput::Attach {
                    attachment_id: attachment.as_str().to_string(),
                    authentication_valid: caller_authorized,
                    expired,
                },
            )
            .map_err(machine_refused)?;

            let mut grant = None;
            let mut denial = None;
            for effect in transition.effects() {
                match effect {
                    fp::ForkedParticipantLifecycleEffect::AttachmentGranted {
                        use_index,
                        remaining_uses,
                        ..
                    } => {
                        grant = Some(ForkedParticipantGrant {
                            attachment_id: attachment.clone(),
                            use_index: *use_index,
                            remaining_uses: *remaining_uses,
                            replayed: false,
                            scope: record.sidecar.scope,
                            fork_session_id: fork_session_id.clone(),
                        });
                    }
                    fp::ForkedParticipantLifecycleEffect::AttachmentGrantReplayed {
                        use_index,
                        ..
                    } => {
                        let max_uses = u64::from(record.sidecar.reuse.max_uses());
                        grant = Some(ForkedParticipantGrant {
                            attachment_id: attachment.clone(),
                            use_index: *use_index,
                            remaining_uses: max_uses.saturating_sub(*use_index),
                            replayed: true,
                            scope: record.sidecar.scope,
                            fork_session_id: fork_session_id.clone(),
                        });
                    }
                    fp::ForkedParticipantLifecycleEffect::AttachDenied { reason, .. } => {
                        denial = Some(*reason);
                    }
                    fp::ForkedParticipantLifecycleEffect::CapabilityExpired { .. } => {}
                    other => return Err(machine_refused(other)),
                }
            }

            // Expiry observed at attach time terminalizes the record even
            // though the attach itself is denied, so that change must persist.
            let next = if authority.state() == &record.machine_state {
                None
            } else {
                let mut next = record.clone();
                next.machine_state = authority.state().clone();
                Some(next)
            };

            // A denial can still change durable state: an expiry observed at
            // attach time terminalizes the record even though the attach is
            // refused. The verdict therefore travels as the plan's outcome so
            // the state change commits before the typed denial is surfaced.
            let outcome = match (grant, denial) {
                (Some(grant), _) => Ok(grant),
                (None, Some(reason)) => Err(ForkedParticipantError::AttachDenied { reason }),
                (None, None) => {
                    return Err(ForkedParticipantError::MachineRefused {
                        detail: "attach produced no typed verdict".to_string(),
                    });
                }
            };
            Ok(TransitionPlan { outcome, next })
        })
        .await?
    }

    /// Release the exact active attachment.
    pub async fn release(
        &self,
        capability: &ForkedParticipantRef,
        attachment_id: &ForkedParticipantAttachmentId,
    ) -> Result<ForkedParticipantReleaseOutcome, ForkedParticipantError> {
        self.require_owned_route(capability.owner_route())?;
        let attachment = attachment_id.clone();
        self.transition(RecordLookup::Exact(capability), move |record| {
            let mut authority = fp::ForkedParticipantLifecycleMachineAuthority::recover_from_state(
                record.machine_state.clone(),
            )
            .map_err(machine_refused)?;
            let transition = fp::ForkedParticipantLifecycleMachineMutator::apply(
                &mut authority,
                fp::ForkedParticipantLifecycleInput::Release {
                    attachment_id: attachment.as_str().to_string(),
                },
            )
            .map_err(machine_refused)?;

            let mut outcome = None;
            let mut rejection = None;
            for effect in transition.effects() {
                match effect {
                    fp::ForkedParticipantLifecycleEffect::AttachmentReleased { .. } => {
                        outcome.get_or_insert(ForkedParticipantReleaseOutcome::Reusable);
                    }
                    fp::ForkedParticipantLifecycleEffect::CapabilityExhausted { .. } => {
                        outcome = Some(ForkedParticipantReleaseOutcome::Exhausted);
                    }
                    fp::ForkedParticipantLifecycleEffect::CapabilityRevoked { .. } => {
                        outcome = Some(ForkedParticipantReleaseOutcome::Revoked);
                    }
                    fp::ForkedParticipantLifecycleEffect::CapabilityExpired { .. } => {
                        outcome = Some(ForkedParticipantReleaseOutcome::Expired);
                    }
                    fp::ForkedParticipantLifecycleEffect::ReleaseReplayed { .. } => {
                        outcome = Some(ForkedParticipantReleaseOutcome::Replayed);
                    }
                    fp::ForkedParticipantLifecycleEffect::ReleaseRejected { reason, .. } => {
                        rejection = Some(*reason);
                    }
                    other => return Err(machine_refused(other)),
                }
            }

            let next = if authority.state() == &record.machine_state {
                None
            } else {
                let mut next = record.clone();
                next.machine_state = authority.state().clone();
                Some(next)
            };

            match (outcome, rejection) {
                (Some(outcome), _) => Ok(TransitionPlan { outcome, next }),
                (None, Some(reason)) => Err(ForkedParticipantError::ReleaseRejected { reason }),
                (None, None) => Err(ForkedParticipantError::MachineRefused {
                    detail: "release produced no typed verdict".to_string(),
                }),
            }
        })
        .await
    }

    /// Revoke a capability by bearer identity, under a caller-identity
    /// observation supplied by the authenticated command surface.
    pub async fn revoke(
        &self,
        capability_id: &ForkedParticipantCapabilityId,
        caller_authorized: bool,
    ) -> Result<ForkedParticipantRevocationOutcome, ForkedParticipantError> {
        let owner_route = self.owner_route.clone();
        self.transition(RecordLookup::CapabilityId(capability_id), move |record| {
            if record.sidecar.owner_route != owner_route {
                return Err(ForkedParticipantError::ForeignRoute {
                    detail: "the record's route is not the route this owner serves".to_string(),
                });
            }
            let mut authority = fp::ForkedParticipantLifecycleMachineAuthority::recover_from_state(
                record.machine_state.clone(),
            )
            .map_err(machine_refused)?;
            let transition = fp::ForkedParticipantLifecycleMachineMutator::apply(
                &mut authority,
                fp::ForkedParticipantLifecycleInput::Revoke {
                    authentication_valid: caller_authorized,
                },
            )
            .map_err(machine_refused)?;

            let mut outcome = None;
            let mut denial = None;
            for effect in transition.effects() {
                match effect {
                    fp::ForkedParticipantLifecycleEffect::CapabilityRevoked { cleanup_pending } => {
                        outcome = Some(ForkedParticipantRevocationOutcome::Revoked {
                            cleanup_pending: *cleanup_pending,
                        });
                    }
                    fp::ForkedParticipantLifecycleEffect::RevocationPendingRecorded => {
                        outcome = Some(ForkedParticipantRevocationOutcome::PendingAttachedRelease);
                    }
                    fp::ForkedParticipantLifecycleEffect::RevocationConverged => {
                        outcome = Some(ForkedParticipantRevocationOutcome::Converged);
                    }
                    fp::ForkedParticipantLifecycleEffect::RevocationDenied { reason } => {
                        denial = Some(*reason);
                    }
                    other => return Err(machine_refused(other)),
                }
            }

            let next = if authority.state() == &record.machine_state {
                None
            } else {
                let mut next = record.clone();
                next.machine_state = authority.state().clone();
                Some(next)
            };

            match (outcome, denial) {
                (Some(outcome), _) => Ok(TransitionPlan { outcome, next }),
                (None, Some(reason)) => Err(ForkedParticipantError::RevocationDenied { reason }),
                (None, None) => Err(ForkedParticipantError::MachineRefused {
                    detail: "revoke produced no typed verdict".to_string(),
                }),
            }
        })
        .await
    }

    /// Feed one expiry observation per record.
    ///
    /// One record's failure never aborts the sweep: it is reported and the
    /// sweep continues.
    pub async fn sweep_expiry(
        &self,
        now: DateTime<Utc>,
    ) -> Result<ForkedParticipantExpirySweepReport, ForkedParticipantError> {
        let mut report = ForkedParticipantExpirySweepReport::default();
        for record in self.store.list_all().await? {
            if record.sidecar.owner_route != self.owner_route {
                continue;
            }
            let entry = ForkedParticipantSweepEntry {
                capability_id: record.capability_id.clone(),
                fork_session_id: record.fork_session_id().cloned(),
            };
            let mut observed = None;
            let result = self
                .transition(
                    RecordLookup::CapabilityId(&record.capability_id),
                    |record| {
                        let expired = now >= record.sidecar.expires_at;
                        let mut authority =
                            fp::ForkedParticipantLifecycleMachineAuthority::recover_from_state(
                                record.machine_state.clone(),
                            )
                            .map_err(machine_refused)?;
                        let transition = fp::ForkedParticipantLifecycleMachineMutator::apply(
                            &mut authority,
                            fp::ForkedParticipantLifecycleInput::ObserveExpiry { expired },
                        )
                        .map_err(machine_refused)?;
                        for effect in transition.effects() {
                            match effect {
                                fp::ForkedParticipantLifecycleEffect::CapabilityExpired {
                                    ..
                                } => observed = Some(ExpiryVerdict::Expired),
                                fp::ForkedParticipantLifecycleEffect::ExpiryPendingRecorded => {
                                    observed = Some(ExpiryVerdict::PendingAttached);
                                }
                                fp::ForkedParticipantLifecycleEffect::ExpiryObservationIgnored {
                                    ..
                                } => {}
                                other => return Err(machine_refused(other)),
                            }
                        }
                        if authority.state() == &record.machine_state {
                            return Ok(TransitionPlan {
                                outcome: (),
                                next: None,
                            });
                        }
                        let mut next = record.clone();
                        next.machine_state = authority.state().clone();
                        Ok(TransitionPlan {
                            outcome: (),
                            next: Some(next),
                        })
                    },
                )
                .await;

            match result {
                Ok(()) => match observed {
                    Some(ExpiryVerdict::Expired) => report.expired.push(entry),
                    Some(ExpiryVerdict::PendingAttached) => {
                        report.expiry_pending_attached.push(entry);
                    }
                    None => {}
                },
                Err(error) => report.failed.push((entry, error.to_string())),
            }
        }
        Ok(report)
    }

    /// Enumerate every capability this owner serves whose recorded terminal is
    /// parked behind a still-active attachment.
    ///
    /// This is the seam that makes coordinator loss survivable. A capability
    /// that expired or was revoked while attached stays parked until the exact
    /// attachment is released; if the coordinator that took the attachment
    /// never comes back, only the owner can see the parked terminal, and only
    /// the attachment HOLDER (the host that materialized the residency) can
    /// prove the attachment releasable. So the owner publishes typed candidates
    /// — full immutable reference plus the exact active attachment id — and the
    /// holder correlates them against its own durable rows.
    ///
    /// The phase is read from the canonical machine state; nothing here parses
    /// text or guesses from a report string. A record whose parked phase cannot
    /// be turned into a typed entry is reported as unreadable rather than
    /// dropped, and never aborts the enumeration.
    pub async fn list_pending_attached(
        &self,
    ) -> Result<ForkedParticipantPendingAttachmentReport, ForkedParticipantError> {
        let mut report = ForkedParticipantPendingAttachmentReport::default();
        for record in self.store.list_all().await? {
            let terminal = match record.machine_state.lifecycle_phase {
                fp::ForkedParticipantLifecycleState::ExpiryPendingAttached => {
                    ForkedParticipantPendingTerminal::Expiry
                }
                fp::ForkedParticipantLifecycleState::RevocationPendingAttached => {
                    ForkedParticipantPendingTerminal::Revocation
                }
                _ => continue,
            };
            if record.sidecar.owner_route != self.owner_route {
                continue;
            }
            let Some(capability) = record.sidecar.capability_ref.clone() else {
                report.unreadable.push((
                    record.capability_id.clone(),
                    "parked terminal on a record with no activated capability reference"
                        .to_string(),
                ));
                continue;
            };
            let Some(raw_attachment_id) = record.machine_state.active_attachment_id.as_ref() else {
                report.unreadable.push((
                    record.capability_id.clone(),
                    "parked-attached terminal with no active attachment id".to_string(),
                ));
                continue;
            };
            match ForkedParticipantAttachmentId::new(raw_attachment_id) {
                Ok(attachment_id) => report.pending.push(ForkedParticipantPendingAttachment {
                    capability,
                    attachment_id,
                    terminal,
                }),
                Err(error) => report.unreadable.push((
                    record.capability_id.clone(),
                    format!("active attachment id is not a valid identity: {error}"),
                )),
            }
        }
        Ok(report)
    }

    /// The capability record that owns `fork_session_id`, if any.
    ///
    /// This is the CONTAINMENT lookup. A fork child's session id is visible —
    /// it rides in provenance, in host rows, and in replies that name the
    /// residency — so possession of the id must never be enough to reach the
    /// branch. Any holder-facing surface that can address a session by id asks
    /// this first and refuses to serve a protected session without the
    /// authenticated capability.
    ///
    /// Protection begins at RESERVATION, not activation: the planned child id
    /// is durable before the fork is taken, so the crash window between "child
    /// is saved" and "activation is recorded" is covered rather than being a
    /// blind spot. In that window `capability` is `None` — there is no
    /// reference yet, so nothing can authenticate against it and every request
    /// for that session must be refused.
    pub async fn protected_fork_session(
        &self,
        fork_session_id: &SessionId,
    ) -> Result<Option<ForkedParticipantForkProtection>, ForkedParticipantError> {
        let Some(record) = self.store.load_by_fork_session_id(fork_session_id).await? else {
            return Ok(None);
        };
        Ok(Some(ForkedParticipantForkProtection {
            capability_hint: record.capability_id.correlation_hint(),
            owner_route: record.sidecar.owner_route.clone(),
            capability: record.sidecar.capability_ref.clone(),
        }))
    }

    /// Archive terminal detached records that carry machine-owned cleanup debt.
    ///
    /// Concurrency is mechanical: a sweeper takes an exclusive, crash-
    /// recoverable claim on the record before archiving, so two sweepers cannot
    /// both archive one fork and neither can record false debt for the other's
    /// work. Terminal cleanup completion stays machine-owned. One failing
    /// record never aborts the sweep.
    pub async fn sweep_cleanup(
        &self,
        now: DateTime<Utc>,
    ) -> Result<ForkedParticipantCleanupReport, ForkedParticipantError> {
        let mut report = ForkedParticipantCleanupReport::default();
        for record in self.store.list_all().await? {
            if record.sidecar.owner_route != self.owner_route {
                continue;
            }
            if record.machine_state.cleanup_state != fp::ForkedParticipantCleanupState::Pending {
                continue;
            }
            let entry = ForkedParticipantSweepEntry {
                capability_id: record.capability_id.clone(),
                fork_session_id: record.fork_session_id().cloned(),
            };

            let lease = match self.claim_cleanup(&record.capability_id, now).await {
                Ok(ForkedParticipantCleanupClaimOutcome::Claimed(lease)) => lease,
                Ok(ForkedParticipantCleanupClaimOutcome::ClaimedElsewhere) => {
                    report.claimed_elsewhere.push(entry);
                    continue;
                }
                // The record stopped carrying cleanup debt between the listing
                // and the claim: another attempt already finished it.
                Ok(ForkedParticipantCleanupClaimOutcome::NotPending) => continue,
                Err(error) => {
                    report.failed.push((entry, error.to_string()));
                    continue;
                }
            };

            let archive = match entry.fork_session_id.as_ref() {
                Some(fork_session_id) => self.archive_converging(fork_session_id).await,
                // Cleanup debt without an activated fork: nothing durable was
                // created, so the debt discharges directly.
                None => Ok(()),
            };

            // Only the CURRENT claimant may publish. If the claim was taken
            // over while this attempt archived, the late outcome is dropped:
            // session archive is idempotent, so a duplicate archive after a
            // TTL takeover is harmless, but record state has exactly one
            // publisher.
            match archive {
                Ok(()) => match self.publish_cleanup_success(&lease).await {
                    Ok(ForkedParticipantCleanupPublish::Published(())) => {
                        report.completed.push(entry);
                    }
                    Ok(ForkedParticipantCleanupPublish::ClaimLost) => {
                        report.claimed_elsewhere.push(entry);
                    }
                    Err(error) => report.failed.push((entry, error.to_string())),
                },
                Err(error) => {
                    let detail = error.to_string();
                    match self.publish_cleanup_failure(&lease, detail, now).await {
                        Ok(ForkedParticipantCleanupPublish::Published(debt)) => {
                            report.retained.push((entry, debt));
                        }
                        Ok(ForkedParticipantCleanupPublish::ClaimLost) => {
                            report.claimed_elsewhere.push(entry);
                        }
                        Err(error) => report.failed.push((entry, error.to_string())),
                    }
                }
            }
        }
        Ok(report)
    }

    /// Archive one fork session, converging on an already-absent session.
    async fn archive_converging(
        &self,
        fork_session_id: &SessionId,
    ) -> Result<(), ForkedParticipantError> {
        match self.runtime.archive_fork_session(fork_session_id).await {
            Ok(()) => Ok(()),
            // Idempotent by contract: a session that is already gone is the
            // state cleanup wanted.
            Err(SessionError::NotFound { .. }) => Ok(()),
            Err(error) => Err(ForkedParticipantError::Session(error)),
        }
    }

    /// Take the mechanical exclusive cleanup claim on one record.
    ///
    /// The claim is admitted only when, at the moment of the compare-and-swap,
    /// the record still carries machine-owned cleanup debt AND either no claim
    /// exists or the existing claim has gone stale. A completed or otherwise
    /// non-pending record is never claimable, so a sweeper cannot take a claim
    /// on work another attempt already finished.
    ///
    /// The returned lease names a freshly minted attempt id — per record
    /// attempt, never per service — so two concurrent sweeps issued by the same
    /// service fence each other exactly like two separate processes.
    pub async fn claim_cleanup(
        &self,
        capability_id: &ForkedParticipantCapabilityId,
        now: DateTime<Utc>,
    ) -> Result<ForkedParticipantCleanupClaimOutcome, ForkedParticipantError> {
        let attempt_id = ForkedParticipantCleanupAttemptId::mint()?;
        let stale_after =
            ChronoDuration::from_std(FORKED_PARTICIPANT_CLEANUP_CLAIM_TTL).map_err(|error| {
                ForkedParticipantError::InvalidRequest {
                    detail: format!("cleanup claim ttl is not representable: {error}"),
                }
            })?;
        self.transition(RecordLookup::CapabilityId(capability_id), move |record| {
            // Re-checked on every compare-and-swap attempt against the freshly
            // loaded record, so a record that stopped being Pending while we
            // raced is refused rather than re-claimed.
            if record.machine_state.cleanup_state != fp::ForkedParticipantCleanupState::Pending {
                return Ok(TransitionPlan {
                    outcome: ForkedParticipantCleanupClaimOutcome::NotPending,
                    next: None,
                });
            }
            if let Some(claim) = record.cleanup_claim.as_ref()
                && now.signed_duration_since(claim.claimed_at) < stale_after
            {
                return Ok(TransitionPlan {
                    outcome: ForkedParticipantCleanupClaimOutcome::ClaimedElsewhere,
                    next: None,
                });
            }
            let claim_revision = record.revision.checked_add(1).ok_or_else(|| {
                ForkedParticipantError::InvalidRequest {
                    detail: "cleanup claim revision counter is exhausted".to_string(),
                }
            })?;
            let mut next = record.clone();
            next.cleanup_claim = Some(ForkedParticipantCleanupClaim {
                attempt_id: attempt_id.clone(),
                claimed_at: now,
            });
            Ok(TransitionPlan {
                outcome: ForkedParticipantCleanupClaimOutcome::Claimed(
                    ForkedParticipantCleanupLease::new_owned(
                        record.capability_id.clone(),
                        attempt_id.clone(),
                        now,
                        claim_revision,
                    ),
                ),
                next: Some(next),
            })
        })
        .await
    }

    /// Publish a failed archive as typed cleanup debt, under a lease.
    ///
    /// A lease whose claim was taken over publishes nothing: a superseded
    /// attempt must never record debt against work another attempt owns.
    pub async fn publish_cleanup_failure(
        &self,
        lease: &ForkedParticipantCleanupLease,
        detail: String,
        now: DateTime<Utc>,
    ) -> Result<ForkedParticipantCleanupPublish<ForkedParticipantCleanupDebt>, ForkedParticipantError>
    {
        let attempt_id = lease.attempt_id().clone();
        self.transition(
            RecordLookup::CapabilityId(lease.capability_id()),
            move |record| {
                if !claim_is_held_by(record, &attempt_id) {
                    return Ok(TransitionPlan {
                        outcome: ForkedParticipantCleanupPublish::ClaimLost,
                        next: None,
                    });
                }
                let Some(fork_session_id) = record.fork_session_id().cloned() else {
                    return Err(ForkedParticipantError::MachineRefused {
                        detail: "cleanup failure recorded for a record without a fork".to_string(),
                    });
                };
                let attempts = record
                    .cleanup_debt
                    .as_ref()
                    .map_or(0, |debt| debt.attempts)
                    .saturating_add(1);
                let debt = ForkedParticipantCleanupDebt {
                    fork_session_id,
                    attempts,
                    last_error: detail.clone(),
                    observed_at: now,
                };
                let mut next = record.clone();
                next.cleanup_debt = Some(debt.clone());
                next.cleanup_claim = None;
                Ok(TransitionPlan {
                    outcome: ForkedParticipantCleanupPublish::Published(debt),
                    next: Some(next),
                })
            },
        )
        .await
    }

    /// Publish a successful archive by driving machine-owned cleanup
    /// completion, under a lease.
    ///
    /// Terminality stays machine-owned: the lease only decides WHO may ask, the
    /// machine still decides whether completion is legal. A lease whose claim
    /// was taken over publishes nothing.
    pub async fn publish_cleanup_success(
        &self,
        lease: &ForkedParticipantCleanupLease,
    ) -> Result<ForkedParticipantCleanupPublish<()>, ForkedParticipantError> {
        let attempt_id = lease.attempt_id().clone();
        self.transition(
            RecordLookup::CapabilityId(lease.capability_id()),
            move |record| {
                if !claim_is_held_by(record, &attempt_id) {
                    return Ok(TransitionPlan {
                        outcome: ForkedParticipantCleanupPublish::ClaimLost,
                        next: None,
                    });
                }
                let mut authority =
                    fp::ForkedParticipantLifecycleMachineAuthority::recover_from_state(
                        record.machine_state.clone(),
                    )
                    .map_err(machine_refused)?;
                let transition = fp::ForkedParticipantLifecycleMachineMutator::apply(
                    &mut authority,
                    fp::ForkedParticipantLifecycleInput::CompleteCleanup {},
                )
                .map_err(machine_refused)?;
                for effect in transition.effects() {
                    match effect {
                        fp::ForkedParticipantLifecycleEffect::CleanupCompleted
                        | fp::ForkedParticipantLifecycleEffect::CleanupCompletionReplayed => {}
                        fp::ForkedParticipantLifecycleEffect::CleanupCompletionRejected {
                            reason,
                        } => {
                            return Err(ForkedParticipantError::MachineRefused {
                                detail: format!("cleanup completion rejected: {reason:?}"),
                            });
                        }
                        other => return Err(machine_refused(other)),
                    }
                }
                let mut next = record.clone();
                next.machine_state = authority.state().clone();
                next.cleanup_debt = None;
                next.cleanup_claim = None;
                Ok(TransitionPlan {
                    outcome: ForkedParticipantCleanupPublish::Published(()),
                    next: Some(next),
                })
            },
        )
        .await
    }

    /// Read one record by bearer identity (owner-side maintenance path).
    pub async fn load_record(
        &self,
        capability_id: &ForkedParticipantCapabilityId,
    ) -> Result<Option<ForkedParticipantRecord>, ForkedParticipantError> {
        Ok(self.store.load_by_capability_id(capability_id).await?)
    }
}

/// Whether the record's live claim belongs to this attempt.
fn claim_is_held_by(
    record: &ForkedParticipantRecord,
    attempt_id: &ForkedParticipantCleanupAttemptId,
) -> bool {
    record
        .cleanup_claim
        .as_ref()
        .is_some_and(|claim| &claim.attempt_id == attempt_id)
}

#[derive(Clone, Copy)]
enum ExpiryVerdict {
    Expired,
    PendingAttached,
}
