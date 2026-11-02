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
//! settle ([`super::MobHandle::settle_identity_preparation`]) commits its
//! receipt and its row (present again), and seats it Dormant from its row's
//! spec; a spec that owes a kickoff is embodied and runs it once. A
//! roster-owned identity is settled by its host's next declaration, which
//! names no preparation.
//!
//! A `constructed` preparation is a held construction the host activates. Its
//! host asks for one ([`super::MobHandle::construct_identity`]): MobMachine
//! holds the attempt (`BeginPreparationConstruction`; a store-owned
//! identity's row records the attempt), and Core holds the identity's member
//! constructed and unpublished, starting no run, under the entry's custody.
//! The host reads the hold's witnesses ([`HeldPreparation`]), then settles
//! naming the attempt: decided before the attempt's deadline and binding the
//! identity's current spec, the settle accepts the activation of exactly that
//! construction (`SettleHostPreparation`), and its receipt journals the
//! acceptance. The mob owns each attempt, MobMachine's held-construction
//! record and the hold the mob retains: a dropped observer or a lost reply
//! ends nothing. Before an activation is accepted, an abort
//! ([`super::MobHandle::abort_identity_construction`]), the attempt's
//! deadline, a refused construction, a crash or a removal ends it, and the
//! identity stays pending; once accepted, nothing replaces it until its
//! publication ends: published (the membership commit that seats the member
//! ends the attempt, and a store-owned identity's row is present again), or
//! failed, or interrupted by a crash (the settle's replay answers it), in
//! which cases the identity stays pending and a new attempt may follow.
//!
//! A roster-owned identity's attempts count within one process: no
//! construction survives a restart, and its next attempt has new witnesses.

use std::collections::BTreeMap;

use crate::MobError;
use crate::identity::{
    CreatedIdentitySpec, IdentityChangeConsent, IdentityChangeReceipt, IdentityChangeRequester,
    IdentityPreparationMode,
};
use crate::ids::AgentIdentity;
use crate::machines::mob_machine as mob_dsl;

use super::DeclaredEntryOutcome;
use super::identity_change::{
    ChangeAdmissionFacts, ChangeAttribution, IdentityChangeCommand, IdentityChangeConsentInput,
    IdentityChangeGrant, IdentityChangeOutcome, IdentityChangeRefusal,
    IdentityChangeRequesterInput, IdentityOperation, now_ms, refused,
};

/// How a held construction ended
/// ([`PreparationSettlement::PublicationEnded`],
/// [`super::MobHandle::abort_identity_construction`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ConstructionEnd {
    /// Aborted by its host, or by the identity's removal, before an
    /// activation was accepted.
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

impl ConstructionEnd {
    /// The end's stable token (`aborted`, `expired`, `refused`,
    /// `publication_failed`, `publication_interrupted`).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Aborted => "aborted",
            Self::Expired => "expired",
            Self::Refused => "refused",
            Self::PublicationFailed => "publication_failed",
            Self::PublicationInterrupted => "publication_interrupted",
        }
    }

    /// MobMachine's kind of the end, for an end MobMachine records (an
    /// interrupted publication is the settle's replay's answer, after a
    /// restart that holds no attempt).
    pub(super) fn machine_kind(self) -> Option<mob_dsl::ConstructionEndKind> {
        match self {
            Self::Aborted => Some(mob_dsl::ConstructionEndKind::Aborted),
            Self::Expired => Some(mob_dsl::ConstructionEndKind::Expired),
            Self::Refused => Some(mob_dsl::ConstructionEndKind::Refused),
            Self::PublicationFailed => Some(mob_dsl::ConstructionEndKind::PublicationFailed),
            Self::PublicationInterrupted => None,
        }
    }
}

/// A host's request for a held construction of an identity awaiting its
/// `constructed` preparation ([`super::MobHandle::construct_identity`]).
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct IdentityConstructionRequest {
    pub operation: IdentityOperation,
    pub identity: AgentIdentity,
    /// The attempt's deadline: an attempt whose activation was not accepted
    /// before it ends, expired.
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
    pub(super) spec_digest: Option<String>,
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

    /// The digest of the spec the construction was built from: a
    /// store-owned identity's row's, which its activation binds. `None` for
    /// a roster-owned identity.
    pub fn spec_digest(&self) -> Option<&str> {
        self.spec_digest.as_deref()
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
    /// The spec digest the host's issuance checked: an activation of a
    /// construction built from another spec is refused
    /// ([`IdentityChangeRefusal::PreparationMismatch`]).
    pub expected_spec_digest: Option<String>,
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
            expected_spec_digest: None,
        }
    }

    /// Activate exactly the held construction of `attempt`.
    #[must_use]
    pub fn with_attempt(mut self, attempt: u64) -> Self {
        self.attempt = Some(attempt);
        self
    }

    /// Bind the activation to the spec digest the host's issuance checked.
    #[must_use]
    pub fn with_expected_spec_digest(mut self, digest: impl Into<String>) -> Self {
        self.expected_spec_digest = Some(digest.into());
        self
    }
}

/// What a settle did to its identity.
#[derive(Debug)]
#[non_exhaustive]
pub enum PreparationSettlement {
    /// No construction: the identity's seat, as the declared apply answers
    /// it (seated Dormant, embodied for an owed kickoff, or hosted on its
    /// first use; a seat that failed leaves it owed its seat, which the
    /// mob's next start seats).
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

// ---------------------------------------------------------------------------
// The commands the actor serves
// ---------------------------------------------------------------------------

/// What a settle's commit step takes.
pub(super) struct PreparationSettleCommit {
    pub(super) identity: AgentIdentity,
    pub(super) attempt: Option<u64>,
    pub(super) expected_spec_digest: Option<String>,
    pub(super) operation: IdentityOperation,
    pub(super) facts: ChangeAdmissionFacts,
    pub(super) grant: Option<IdentityChangeGrant>,
    pub(super) decided: bool,
    pub(super) requester: IdentityChangeRequester,
    pub(super) consent: IdentityChangeConsent,
}

/// What a settle's commit step answers.
pub(super) enum PreparationSettleCommitted {
    /// Committed without a construction: the entry is owed its seat, which
    /// the handle carries out from the row's spec.
    Seat {
        receipt: IdentityChangeReceipt,
        spec: Box<CreatedIdentitySpec>,
        held_creation: bool,
    },
    /// The activation of `attempt` was accepted: `outcome` reports how its
    /// publication ended.
    #[cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]
    Accepted {
        receipt: IdentityChangeReceipt,
        attempt: u64,
        outcome: crate::tokio::sync::watch::Receiver<Option<ConstructionOutcome>>,
    },
    /// The operation committed before with the same request: what its
    /// settlement came to, when it accepted an activation.
    Replayed {
        receipt: IdentityChangeReceipt,
        settlement: Option<PreparationSettlement>,
    },
}

/// What a held construction's start answers.
#[cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]
pub(super) enum ConstructionBegun {
    /// MobMachine holds `attempt`: the mob's construction task spawns the
    /// held member from `spec`.
    Held {
        attempt: u64,
        spec: Box<super::SpawnMemberSpec>,
        spec_digest: Option<String>,
    },
    /// The held construction of the same identity and operation: its
    /// observer (a lost reply asked again).
    Existing(Box<HeldPreparation>),
}

/// How one held construction attempt ended, as the mob's registry reports it
/// to a settle or an abort that waits on it.
#[cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]
#[derive(Debug, Clone)]
pub(super) enum ConstructionOutcome {
    /// The accepted activation published the member.
    Published,
    /// The attempt ended without a publication; `cleanup` is the error of a
    /// hold's cleanup its abort could not confirm.
    Ended {
        end: ConstructionEnd,
        cleanup: Option<String>,
    },
}

/// Await the end of the attempt `outcome` reports. A registry gone before it
/// reported (the mob stopped) answers the publication interrupted.
#[cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]
pub(super) async fn construction_outcome(
    mut outcome: crate::tokio::sync::watch::Receiver<Option<ConstructionOutcome>>,
) -> ConstructionOutcome {
    loop {
        if let Some(reported) = outcome.borrow_and_update().clone() {
            return reported;
        }
        if outcome.changed().await.is_err() {
            return outcome
                .borrow()
                .clone()
                .unwrap_or(ConstructionOutcome::Ended {
                    end: ConstructionEnd::PublicationInterrupted,
                    cleanup: None,
                });
        }
    }
}

/// One held construction the mob owns, in its registry
/// ([`HeldConstructions`]): MobMachine's held-construction record decides
/// the attempt, and this holds what carrying out its decisions needs: the
/// hold's decision channel and the presence that keeps the hold, the
/// attempt's deadline, the spec digest its activation binds, its observer,
/// and the report of its end.
#[cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]
pub(super) struct HeldConstruction {
    pub(super) attempt: u64,
    pub(super) operation_id: String,
    pub(super) deadline: super::held_spawn::HeldSpawnDeadline,
    pub(super) spec_digest: Option<String>,
    pub(super) decision:
        Option<crate::tokio::sync::oneshot::Sender<super::held_spawn::HeldSpawnDecision>>,
    /// The presence that keeps the hold while the registry keeps it.
    pub(super) _presence: super::held_spawn::HeldSetupCaller,
    pub(super) observer: HeldPreparation,
    pub(super) outcome: crate::tokio::sync::watch::Sender<Option<ConstructionOutcome>>,
    /// Whether the activation of the attempt was sent to its hold: its
    /// publication's end is the attempt's end.
    pub(super) activation_sent: bool,
}

/// The mob's registry of held constructions (the actor's): the hold of each
/// identity's open attempt, the end of each identity's last ended attempt
/// in this process, and the last attempt each roster-owned identity held in
/// this process.
#[derive(Default)]
pub(super) struct HeldConstructions {
    #[cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]
    held: BTreeMap<AgentIdentity, HeldConstruction>,
    ended: BTreeMap<AgentIdentity, (u64, ConstructionEnd)>,
    #[cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]
    roster_attempts: BTreeMap<AgentIdentity, u64>,
}

impl HeldConstructions {
    /// The next attempt of a roster-owned identity in this process.
    #[cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]
    pub(super) fn next_roster_attempt(&mut self, identity: &AgentIdentity) -> u64 {
        let ended = self.ended.get(identity).map_or(0, |(attempt, _)| *attempt);
        let last = self.roster_attempts.get(identity).copied().unwrap_or(0);
        let next = last.max(ended).saturating_add(1);
        self.roster_attempts.insert(identity.clone(), next);
        next
    }

    /// How `identity`'s last ended attempt ended, in this process.
    pub(super) fn ended(&self, identity: &AgentIdentity) -> Option<(u64, ConstructionEnd)> {
        self.ended.get(identity).copied()
    }

    /// Record how `identity`'s `attempt` ended.
    #[cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]
    pub(super) fn record_end(
        &mut self,
        identity: &AgentIdentity,
        attempt: u64,
        end: ConstructionEnd,
    ) {
        self.ended.insert(identity.clone(), (attempt, end));
    }

    /// The hold of `identity`'s open attempt.
    #[cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]
    pub(super) fn held(&self, identity: &AgentIdentity) -> Option<&HeldConstruction> {
        self.held.get(identity)
    }

    /// The hold of `identity`'s open attempt, to carry out a decision on.
    #[cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]
    pub(super) fn held_mut(&mut self, identity: &AgentIdentity) -> Option<&mut HeldConstruction> {
        self.held.get_mut(identity)
    }

    /// Keep the hold of `identity`'s attempt. A hold of an older attempt it
    /// replaces ends with its dropped decision channel.
    #[cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]
    pub(super) fn keep(&mut self, identity: AgentIdentity, construction: HeldConstruction) {
        self.held.insert(identity, construction);
    }

    /// Let go of the hold of `identity`'s `attempt`, if the registry keeps
    /// it.
    #[cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]
    pub(super) fn release(
        &mut self,
        identity: &AgentIdentity,
        attempt: u64,
    ) -> Option<HeldConstruction> {
        if self
            .held
            .get(identity)
            .is_some_and(|held| held.attempt == attempt)
        {
            self.held.remove(identity)
        } else {
            None
        }
    }
}

/// The refusal MobMachine's verdict of a held construction's start answers,
/// if any.
#[cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]
pub(super) fn construction_refusal(
    verdict: mob_dsl::PreparationConstructionVerdict,
    identity: &AgentIdentity,
    open_attempt: Option<u64>,
) -> Option<IdentityChangeRefusal> {
    use mob_dsl::PreparationConstructionVerdict as Verdict;
    match verdict {
        Verdict::Held => None,
        Verdict::NotPending => Some(IdentityChangeRefusal::PreparationNotPending {
            identity: identity.clone(),
        }),
        Verdict::NotConstructed => Some(IdentityChangeRefusal::NotConstructed {
            identity: identity.clone(),
        }),
        Verdict::ConstructionOpen => Some(IdentityChangeRefusal::ConstructionOpen {
            identity: identity.clone(),
            attempt: open_attempt.unwrap_or_default(),
        }),
        Verdict::NonAuthoritative => Some(IdentityChangeRefusal::CandidateLaunch),
        Verdict::MobNotRunning => Some(IdentityChangeRefusal::MobNotRunning),
    }
}

/// The refusal MobMachine's verdict of a settle answers, if any; `open` is
/// the identity's held or activating attempt.
pub(super) fn settle_refusal(
    verdict: mob_dsl::PreparationSettleVerdict,
    identity: &AgentIdentity,
    attempt: Option<u64>,
    open: Option<u64>,
) -> Option<IdentityChangeRefusal> {
    use mob_dsl::PreparationSettleVerdict as Verdict;
    let identity = identity.clone();
    match verdict {
        Verdict::Seat | Verdict::ActivationAccepted => None,
        Verdict::NotPending => Some(IdentityChangeRefusal::PreparationNotPending { identity }),
        Verdict::ConstructionRequired => {
            Some(IdentityChangeRefusal::ConstructionRequired { identity })
        }
        Verdict::ConstructionOpen => Some(IdentityChangeRefusal::ConstructionOpen {
            identity,
            attempt: open.unwrap_or_default(),
        }),
        Verdict::StaleAttempt => Some(IdentityChangeRefusal::StaleAttempt {
            identity,
            current: open,
        }),
        // The named attempt's deadline passed: its expiry ends it, and no
        // attempt is held any more.
        Verdict::Expired => Some(IdentityChangeRefusal::StaleAttempt {
            identity,
            current: None,
        }),
        Verdict::Mismatch => Some(IdentityChangeRefusal::PreparationMismatch { identity }),
        Verdict::ActivationPending => Some(IdentityChangeRefusal::ActivationPending {
            identity,
            attempt: attempt.or(open).unwrap_or_default(),
        }),
        Verdict::NonAuthoritative => Some(IdentityChangeRefusal::CandidateLaunch),
        Verdict::MobNotRunning => Some(IdentityChangeRefusal::MobNotRunning),
    }
}

/// How a held construction's spawn ended, from its failure: an attempt
/// whose activation was sent (`activation_sent`) ended its publication;
/// otherwise its hold expired, was aborted, or its construction was
/// refused. The cleanup error an abort could not confirm is kept.
#[cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]
pub(super) fn construction_end_of(
    error: &MobError,
    activation_sent: bool,
) -> (ConstructionEnd, Option<String>) {
    match error {
        MobError::HeldSpawnExpired { cleanup, .. } if !activation_sent => (
            ConstructionEnd::Expired,
            cleanup.as_ref().map(ToString::to_string),
        ),
        MobError::HeldSpawnAborted { cleanup, .. } if !activation_sent => (
            ConstructionEnd::Aborted,
            cleanup.as_ref().map(ToString::to_string),
        ),
        _ if activation_sent => (ConstructionEnd::PublicationFailed, None),
        _ => (ConstructionEnd::Refused, None),
    }
}

impl super::MobHandle {
    /// Ask for a held construction of an identity awaiting its `constructed`
    /// preparation (see the module docs): MobMachine holds a new attempt
    /// (a store-owned identity's row records it), and the mob holds the
    /// identity's member constructed and unpublished under the entry's
    /// custody, starting no run, until the attempt's activation, its abort,
    /// its deadline or the identity's removal. Answers the attempt's
    /// observer once the member is held; asked again for the same identity
    /// and operation while the attempt is held, answers the same observer.
    /// The mob owns the construction: dropping this call's future or its
    /// observer ends nothing.
    ///
    /// Refused typed: an identity awaiting no preparation
    /// ([`IdentityChangeRefusal::PreparationNotPending`]), one whose
    /// preparation is not `constructed`
    /// ([`IdentityChangeRefusal::NotConstructed`]; a roster-owned
    /// identity's request names its spec), an attempt still open
    /// ([`IdentityChangeRefusal::ConstructionOpen`]), and a construction that
    /// could not be held ([`IdentityChangeRefusal::ConstructionRefused`]; the
    /// identity stays pending).
    #[cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]
    pub async fn construct_identity(
        &self,
        request: IdentityConstructionRequest,
    ) -> Result<HeldPreparation, MobError> {
        if self.identity_member.is_none() {
            return Err(refused(IdentityChangeRefusal::StorageUnsupported));
        }
        if request.operation.operation_id.trim().is_empty() {
            return Err(refused(IdentityChangeRefusal::InvalidRequest {
                problem: "a construction needs an operation id".to_string(),
            }));
        }
        let identity = request.identity.clone();
        let operation_id = request.operation.operation_id.clone();
        let deadline = request.deadline;
        let begun = self
            .send_actor_command(|reply_tx| {
                super::MobCommand::IdentityChange(Box::new(
                    IdentityChangeCommand::BeginConstruction {
                        request: Box::new(request),
                        reply_tx,
                    },
                ))
            })
            .await??;
        let (attempt, spec, spec_digest) = match begun {
            ConstructionBegun::Existing(observer) => return Ok(*observer),
            ConstructionBegun::Held {
                attempt,
                spec,
                spec_digest,
            } => (attempt, spec, spec_digest),
        };
        // The mob's own construction from here: a caller that goes away
        // ends nothing.
        let owner = self.non_owning().with_command_authority(
            crate::control_policy::CommandAuthority::principal(
                crate::control_policy::MobControlPrincipal::Owner,
            ),
        );
        let (answer_tx, answer_rx) = crate::tokio::sync::oneshot::channel();
        crate::tokio::spawn(async move {
            owner
                .hold_construction(
                    HeldConstructionStart {
                        identity,
                        operation_id,
                        attempt,
                        deadline,
                        spec_digest,
                    },
                    *spec,
                    answer_tx,
                )
                .await;
        });
        answer_rx
            .await
            .map_err(|_| MobError::ActorReplyChannelClosed)?
    }

    /// The mob's construction task: spawn the held member, hand its hold to
    /// the registry, answer the observer, and report the attempt's end once
    /// its spawn completes (published, or not).
    #[cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]
    async fn hold_construction(
        &self,
        start: HeldConstructionStart,
        spec: super::SpawnMemberSpec,
        answer_tx: crate::tokio::sync::oneshot::Sender<Result<HeldPreparation, MobError>>,
    ) {
        let HeldConstructionStart {
            identity,
            operation_id,
            attempt,
            deadline,
            spec_digest,
        } = start;
        let refusal = |reason: String| {
            refused(IdentityChangeRefusal::ConstructionRefused {
                identity: identity.clone(),
                reason,
            })
        };
        let held = match self.spawn_held_until(spec, deadline).await {
            Ok(super::HeldSpawnStart::Held(held)) => held,
            Ok(super::HeldSpawnStart::Unsettled(settlement)) => {
                let member = settlement.member().to_string();
                let error = match settlement.settled().await {
                    Ok(()) => MobError::HeldSpawnExpired {
                        member,
                        cleanup: None,
                    },
                    Err(error) => error,
                };
                let reason = error.to_string();
                self.end_construction(&identity, attempt, Err(error)).await;
                let _ = answer_tx.send(Err(refusal(reason)));
                return;
            }
            Err(error) => {
                let reason = error.to_string();
                self.end_construction(&identity, attempt, Err(error)).await;
                let _ = answer_tx.send(Err(refusal(reason)));
                return;
            }
        };
        let Some(parts) = held.into_parts() else {
            let reason = "the held member was decided before its owner took it".to_string();
            self.end_construction(&identity, attempt, Err(MobError::Internal(reason.clone())))
                .await;
            let _ = answer_tx.send(Err(refusal(reason)));
            return;
        };
        let session_id = self
            .machine_state_watch_rx
            .borrow()
            .pending_spawn_sessions
            .get(&mob_dsl::AgentIdentity::from_domain(&identity))
            .and_then(|session| meerkat_core::types::SessionId::parse(&session.0).ok());
        let observer = HeldPreparation {
            identity: identity.clone(),
            operation_id: operation_id.clone(),
            attempt,
            deadline,
            spec_digest: spec_digest.clone(),
            session_id,
            finished_surface: parts.finished_surface,
            controller_route: parts.controller_route,
        };
        let (outcome, _) = crate::tokio::sync::watch::channel(None);
        let construction = HeldConstruction {
            attempt,
            operation_id,
            deadline,
            spec_digest,
            decision: Some(parts.decision_tx),
            _presence: parts.caller,
            observer: observer.clone(),
            outcome,
            activation_sent: false,
        };
        let adopted = self
            .send_actor_command(|reply_tx| {
                super::MobCommand::IdentityChange(Box::new(
                    IdentityChangeCommand::AdoptConstruction {
                        identity: identity.clone(),
                        construction: Box::new(construction),
                        reply_tx,
                    },
                ))
            })
            .await
            .and_then(|adopted| adopted);
        match adopted {
            Ok(()) => {
                let _ = answer_tx.send(Ok(observer));
            }
            // The registry refused the hold: its dropped decision channel
            // aborts it, and its completion below ends the attempt.
            Err(error) => {
                let _ = answer_tx.send(Err(error));
            }
        }
        let completion = match parts.completion_rx.await {
            Ok(Ok(_receipt)) => Ok(()),
            Ok(Err(error)) => Err(error),
            Err(_) => Err(MobError::ActorReplyChannelClosed),
        };
        self.end_construction(&identity, attempt, completion).await;
    }

    /// Report `identity`'s `attempt` ended to the actor: published
    /// (`Ok`), or failed with its error. Never fails its caller: a mob gone
    /// keeps no attempt.
    #[cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]
    async fn end_construction(
        &self,
        identity: &AgentIdentity,
        attempt: u64,
        completion: Result<(), MobError>,
    ) {
        let ended = self
            .send_actor_command(|reply_tx| {
                super::MobCommand::IdentityChange(Box::new(
                    IdentityChangeCommand::EndConstruction {
                        identity: identity.clone(),
                        attempt,
                        completion,
                        reply_tx,
                    },
                ))
            })
            .await
            .and_then(|ended| ended);
        if let Err(error) = ended {
            tracing::warn!(
                mob_id = %self.mob_id(),
                agent_identity = %identity,
                attempt,
                error = %error,
                "a held construction's end was not recorded"
            );
        }
    }

    /// Abort the held construction of `attempt`, before its activation was
    /// accepted, and await the hold's end: its member is never published,
    /// and the identity stays pending. Answers [`ConstructionEnd::Aborted`],
    /// or the abort's typed failure when its cleanup could not be confirmed.
    /// Refused typed: an accepted activation
    /// ([`IdentityChangeRefusal::ActivationPending`]), and an attempt the
    /// identity does not hold ([`IdentityChangeRefusal::StaleAttempt`], with
    /// the open one, if any).
    #[cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]
    pub async fn abort_identity_construction(
        &self,
        identity: &AgentIdentity,
        attempt: u64,
    ) -> Result<ConstructionEnd, MobError> {
        let outcome = self
            .send_actor_command(|reply_tx| {
                super::MobCommand::IdentityChange(Box::new(
                    IdentityChangeCommand::AbortConstruction {
                        identity: identity.clone(),
                        attempt,
                        reply_tx,
                    },
                ))
            })
            .await??;
        match construction_outcome(outcome).await {
            ConstructionOutcome::Ended {
                cleanup: Some(cleanup),
                ..
            } => Err(MobError::HeldSpawnAborted {
                member: identity.to_string(),
                cleanup: Some(Box::new(MobError::Internal(cleanup))),
            }),
            ConstructionOutcome::Ended { end, .. } => Ok(end),
            ConstructionOutcome::Published => Err(MobError::Internal(format!(
                "the construction of '{identity}' was published before its abort"
            ))),
        }
    }

    /// Abort `identity`'s held construction, if one is held (not one whose
    /// activation was accepted): a removal ends it first.
    #[cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]
    pub(super) async fn abort_held_construction_of(
        &self,
        identity: &AgentIdentity,
    ) -> Result<(), MobError> {
        let held = self
            .machine_state_watch_rx
            .borrow()
            .identity_entry_constructions_held
            .get(&mob_dsl::AgentIdentity::from_domain(identity))
            .copied();
        let Some(attempt) = held else {
            return Ok(());
        };
        match self.abort_identity_construction(identity, attempt).await {
            Ok(_)
            | Err(MobError::IdentityChangeRefused {
                refusal: IdentityChangeRefusal::StaleAttempt { .. },
            }) => Ok(()),
            Err(error) => Err(error),
        }
    }

    /// Settle an identity's preparation as one change (see the module
    /// docs). Without an attempt, a store-owned identity's `required`
    /// preparation is settled: its receipt and its row (present again) are
    /// written in one store transaction, and it is seated Dormant from its
    /// row's spec (embodied when its spec owes a kickoff, which runs once).
    /// Naming an attempt, the settle accepts the activation of exactly that
    /// held construction, decided before its deadline and binding the
    /// identity's current spec (and `expected_spec_digest`, when given), and
    /// answers once its publication ended: activated, or ended with the
    /// identity still pending.
    ///
    /// A retry of a committed settle with the same request answers
    /// [`IdentityChangeOutcome::Replayed`], with what its settlement came to
    /// (an accepted activation a restart left without its publication ends
    /// [`ConstructionEnd::PublicationInterrupted`]); nothing runs again.
    /// Refused typed, with nothing written: an identity awaiting no
    /// preparation ([`IdentityChangeRefusal::PreparationNotPending`]), a
    /// `constructed` preparation settled without an attempt
    /// ([`IdentityChangeRefusal::ConstructionRequired`]), an attempt the
    /// identity does not hold ([`IdentityChangeRefusal::StaleAttempt`]), a
    /// construction that does not bind the current spec
    /// ([`IdentityChangeRefusal::PreparationMismatch`]), an activation
    /// already accepted ([`IdentityChangeRefusal::ActivationPending`]), a
    /// roster-owned identity's `required` preparation (its host's
    /// declaration settles it, [`IdentityChangeRefusal::OwnerReadOnly`]),
    /// and every refusal of [`Self::create_store_owned_identities`] that
    /// applies to a change.
    pub async fn settle_identity_preparation(
        &self,
        settle: IdentityPreparationSettle,
        consent: IdentityChangeConsentInput<'_>,
    ) -> Result<IdentityPreparationSettled, MobError> {
        if self.identity_member.is_none() {
            return Err(refused(IdentityChangeRefusal::StorageUnsupported));
        }
        let IdentityPreparationSettle {
            operation,
            requester,
            identity,
            attempt,
            expected_spec_digest,
        } = settle;
        if operation.operation_id.trim().is_empty() || operation.request_digest.trim().is_empty() {
            return Err(refused(IdentityChangeRefusal::InvalidRequest {
                problem: "a change needs an operation id and a request digest".to_string(),
            }));
        }
        let consent_facts = ChangeAttribution::facts_of(
            consent,
            &operation,
            ChangeAttribution::is_host_operator(&requester),
        );
        let ChangeAttribution {
            requester,
            grant,
            decided,
            consent,
        } = ChangeAttribution::of(requester, consent, &operation)?;
        let committed = self
            .send_actor_command(|reply_tx| {
                super::MobCommand::IdentityChange(Box::new(IdentityChangeCommand::Settle {
                    commit: Box::new(PreparationSettleCommit {
                        identity: identity.clone(),
                        attempt,
                        expected_spec_digest,
                        operation,
                        facts: consent_facts.at(now_ms()),
                        grant,
                        decided,
                        requester,
                        consent,
                    }),
                    reply_tx,
                }))
            })
            .await??;
        match committed {
            PreparationSettleCommitted::Seat {
                receipt,
                spec,
                held_creation,
            } => {
                let seat = Box::pin(self.seat_store_owned_outcome(&identity, &spec, held_creation))
                    .await?;
                Ok(IdentityPreparationSettled {
                    change: IdentityChangeOutcome::Committed { receipt },
                    settlement: Some(PreparationSettlement::Seated(seat)),
                })
            }
            #[cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]
            PreparationSettleCommitted::Accepted {
                receipt,
                attempt,
                outcome,
            } => {
                let settlement = match construction_outcome(outcome).await {
                    ConstructionOutcome::Published => PreparationSettlement::Activated { attempt },
                    ConstructionOutcome::Ended { end, .. } => {
                        PreparationSettlement::PublicationEnded { attempt, end }
                    }
                };
                Ok(IdentityPreparationSettled {
                    change: IdentityChangeOutcome::Committed { receipt },
                    settlement: Some(settlement),
                })
            }
            PreparationSettleCommitted::Replayed {
                receipt,
                settlement,
            } => {
                // A replay seats a settled identity its first seat left owed
                // its seat (a seat that failed, a crash), as an unarchive's
                // replay does.
                let owed = settlement.is_none()
                    && self
                        .machine_state_watch_rx
                        .borrow()
                        .identity_entry_unseated
                        .get(&mob_dsl::AgentIdentity::from_domain(&identity))
                        == Some(&mob_dsl::UnseatedEntryCause::SeatOwed);
                let settlement = match (settlement, owed) {
                    (Some(settlement), _) => Some(settlement),
                    (None, true) => match self.store_owned_seat_facts(&identity).await? {
                        Some((spec, held_creation)) => Some(PreparationSettlement::Seated(
                            Box::pin(self.seat_store_owned_outcome(
                                &identity,
                                &spec,
                                held_creation,
                            ))
                            .await?,
                        )),
                        None => None,
                    },
                    (None, false) => None,
                };
                Ok(IdentityPreparationSettled {
                    change: IdentityChangeOutcome::Replayed { receipt },
                    settlement,
                })
            }
        }
    }
}

/// What the mob's construction task holds of one attempt it starts.
#[cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]
struct HeldConstructionStart {
    identity: AgentIdentity,
    operation_id: String,
    attempt: u64,
    deadline: super::held_spawn::HeldSpawnDeadline,
    spec_digest: Option<String>,
}

/// Whether `preparation` asks the host's preparation before the first seat.
pub(super) fn preparation_required(preparation: IdentityPreparationMode) -> bool {
    preparation != IdentityPreparationMode::None
}

/// Settle the row of a store-owned identity whose held construction was
/// published (its membership commit is journaled before its row is
/// settled): a row still pending its preparation is present again, checked
/// against the row read here. Answers whether a row was settled.
pub(super) async fn settle_published_construction_row(
    store: &dyn crate::store::MobIdentityMemberStore,
    mob_id: &crate::ids::MobId,
    identity: &AgentIdentity,
) -> Result<bool, MobError> {
    let Some(crate::identity::CreatedIdentityObservation::Created(row)) =
        store.observe_created_identity(mob_id, identity).await?
    else {
        return Ok(false);
    };
    if !matches!(
        row.presence,
        crate::identity::CreatedIdentityPresence::PendingPreparation { .. }
    ) {
        return Ok(false);
    }
    let expected = crate::store::CreatedIdentityRowExpectation::Current {
        authority_digest: row.authority_digest.clone(),
    };
    let record = row
        .with_presence(crate::identity::CreatedIdentityPresence::Present)
        .map_err(|error| MobError::Internal(format!("a store-owned identity's row: {error}")))?;
    Ok(store
        .settle_created_identity_row(&crate::store::CreatedIdentityRowWrite { record, expected })
        .await?)
}
