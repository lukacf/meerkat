//! Adoption into store custody (0.9.11): one identity change moves an
//! existing member into store custody, keeping its member's runtime,
//! generation, session and history ([`super::MobHandle::adopt_identity`]).
//!
//! MobMachine decides it (`AdoptIdentityIntoStore`) over the name's facts,
//! after the change's admission (`AdmitIdentityChange`), on the step's one
//! prepared authority: a roster-owned identity (a Declared entry, seated on
//! a local member bound to a session, or owed its seat), or a worker member
//! (seated, with no identity entry, local and bound to a session) at the
//! generation its host observed. A member step in flight (a spawn, a wake, a
//! rotation, a retirement, a removal, an archive or a held construction)
//! and an open declared apply leave the adoption to its retry; any other
//! shape is refused [`super::IdentityChangeRefusal::NotAdoptable`] with its
//! reason.
//!
//! The change's one store transaction writes the identity's row (revision
//! 1, presence `present`, its spec, provenance `adopted` with the member's
//! former spawner, and the owned edges the step declares from its member's
//! current edges to other identities, each recorded `adopted`), the
//! member's binding row with the Created origin on its unchanged runtime,
//! generation and session, and the receipt (kind `adopt`, which names the
//! edges that became owned and those that stayed plain wiring: an owned
//! edge joins identities only). The binding row's fold clears the member's
//! spawner, live and on replay: the member is a host root, its former
//! spawner's retirement never reaches it, and its former spawner's
//! agent-lane commands answer `identity_member`, as for every identity.
//!
//! No start, reconcile, declared apply or upgrade adopts into store custody:
//! an adoption is a change its host asks for, under its consent.

use crate::MobError;
use crate::identity::{
    CreatedIdentitySpec, IdentityChangeConsent, IdentityChangeReceipt, IdentityChangeRequester,
};
use crate::ids::{AgentIdentity, Generation};
use crate::machines::mob_machine as mob_dsl;

use super::identity_change::{
    ChangeAdmissionFacts, ChangeAttribution, IdentityChangeCommand, IdentityChangeConsentInput,
    IdentityChangeGrant, IdentityChangeOutcome, IdentityChangeRefusal,
    IdentityChangeRequesterInput, IdentityOperation, now_ms, refused,
};

/// Where an adoption takes its identity from ([`IdentityAdoption::source`]).
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum AdoptionSource {
    /// A roster-owned identity, with the spec its host's roster declares for
    /// it now, which becomes its row's spec: the host reads its roster, and
    /// the spec names the identity's own role.
    Roster { spec: Box<CreatedIdentitySpec> },
    /// A worker member, at the generation its host observed. Its row's spec
    /// is its member's spawn facts (its role, labels and runtime mode), with
    /// the display name and the additional instructions the adoption gives
    /// it.
    Worker {
        generation: Generation,
        display_name: Option<String>,
        additional_instructions: Vec<String>,
    },
}

impl AdoptionSource {
    /// A roster-owned identity's adoption, with its roster's spec.
    #[must_use]
    pub fn roster(spec: CreatedIdentitySpec) -> Self {
        Self::Roster {
            spec: Box::new(spec),
        }
    }

    /// A worker member's adoption, at the generation its host observed.
    #[must_use]
    pub fn worker(generation: Generation) -> Self {
        Self::Worker {
            generation,
            display_name: None,
            additional_instructions: Vec::new(),
        }
    }

    /// The display name a worker's adoption gives it (ignored for a roster
    /// source, whose spec names its own).
    #[must_use]
    pub fn with_display_name(mut self, name: impl Into<String>) -> Self {
        if let Self::Worker { display_name, .. } = &mut self {
            *display_name = Some(name.into());
        }
        self
    }

    /// The additional instructions a worker's adoption gives it (ignored
    /// for a roster source, whose spec names its own).
    #[must_use]
    pub fn with_additional_instructions(mut self, instructions: Vec<String>) -> Self {
        if let Self::Worker {
            additional_instructions,
            ..
        } = &mut self
        {
            *additional_instructions = instructions;
        }
        self
    }

    /// The source as MobMachine names it.
    pub(super) fn machine_source(&self) -> mob_dsl::IdentityAdoptionSource {
        match self {
            Self::Roster { .. } => mob_dsl::IdentityAdoptionSource::Roster,
            Self::Worker { .. } => mob_dsl::IdentityAdoptionSource::Worker,
        }
    }

    /// The generation a worker's host observed (none for a roster source).
    pub(super) fn observed_generation(&self) -> Option<Generation> {
        match self {
            Self::Roster { .. } => None,
            Self::Worker { generation, .. } => Some(*generation),
        }
    }
}

/// One adoption into store custody: an identity change of class `adopt`
/// ([`super::MobHandle::adopt_identity`]).
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct IdentityAdoption {
    pub operation: IdentityOperation,
    pub requester: IdentityChangeRequesterInput,
    pub identity: AgentIdentity,
    pub source: AdoptionSource,
}

impl IdentityAdoption {
    #[must_use]
    pub fn new(
        operation: IdentityOperation,
        requester: IdentityChangeRequesterInput,
        identity: impl Into<AgentIdentity>,
        source: AdoptionSource,
    ) -> Self {
        Self {
            operation,
            requester,
            identity: identity.into(),
            source,
        }
    }
}

/// What an adoption's commit step takes.
pub(super) struct AdoptionCommit {
    pub(super) identity: AgentIdentity,
    pub(super) source: AdoptionSource,
    pub(super) operation: IdentityOperation,
    pub(super) facts: ChangeAdmissionFacts,
    pub(super) grant: Option<IdentityChangeGrant>,
    pub(super) decided: bool,
    pub(super) requester: IdentityChangeRequester,
    pub(super) consent: IdentityChangeConsent,
}

/// What an adoption's commit step answers.
pub(super) enum AdoptionCommitted {
    Committed(IdentityChangeReceipt),
    Replayed(IdentityChangeReceipt),
}

/// The refusal of an adoption MobMachine's verdict names (`None` for an
/// adoption it took).
pub(super) fn adoption_refusal(
    verdict: mob_dsl::IdentityAdoptionVerdict,
    reason: Option<mob_dsl::IdentityNotAdoptableReason>,
    identity: &AgentIdentity,
) -> Result<Option<IdentityChangeRefusal>, MobError> {
    use mob_dsl::IdentityAdoptionVerdict as Verdict;
    Ok(Some(match verdict {
        Verdict::Adopted => return Ok(None),
        Verdict::NotAdoptable => IdentityChangeRefusal::NotAdoptable {
            identity: identity.clone(),
            reason: super::identity_change::NotAdoptableReason::from_machine(reason.ok_or_else(
                || MobError::Internal("MobMachine refused an adoption with no reason".into()),
            )?),
        },
        Verdict::PredecessorMismatch => IdentityChangeRefusal::PredecessorMismatch {
            identity: identity.clone(),
        },
        Verdict::AdmissionUnsettled => IdentityChangeRefusal::AdmissionUnsettled {
            identity: identity.clone(),
        },
        Verdict::DeclaredApplyOpen => IdentityChangeRefusal::DeclaredApplyOpen,
        Verdict::NameUnreadable => IdentityChangeRefusal::NameTaken {
            identity: identity.clone(),
            by: super::IdentityNameHolder::Unreadable,
        },
        Verdict::NameTombstoned => IdentityChangeRefusal::NameTaken {
            identity: identity.clone(),
            by: super::IdentityNameHolder::Removed,
        },
        Verdict::NonAuthoritative => IdentityChangeRefusal::CandidateLaunch,
        Verdict::ChangesHeld => IdentityChangeRefusal::ChangesHeld,
        Verdict::MobNotRunning => IdentityChangeRefusal::MobNotRunning,
    }))
}

impl super::MobHandle {
    /// Adopt an existing member into store custody as one change (class
    /// `adopt`): a roster-owned identity ([`AdoptionSource::Roster`], with
    /// its roster's spec) or a worker member ([`AdoptionSource::Worker`], at
    /// the generation its host observed). The member keeps its runtime,
    /// generation, session, history, labels and comms endpoint: nothing
    /// rotates and nothing is spawned. Its desired record becomes its row in
    /// Core's identity store (revision 1, provenance `adopted` with its
    /// former spawner), its name is reserved as a store-owned identity's,
    /// and it becomes a host root: its former spawner's retirement no
    /// longer reaches it. Its current edges to other identities become its
    /// owned edges (recorded `adopted`); its edges to members that are no
    /// identity stay plain wiring, and the receipt names both. A roster that
    /// keeps listing an adopted name is answered with a quiet note for that
    /// identity alone.
    ///
    /// A retry with the same operation and request answers
    /// [`IdentityChangeOutcome::Replayed`]. Refused typed, with nothing
    /// written: an identity no adoption takes
    /// ([`IdentityChangeRefusal::NotAdoptable`], with its reason: a
    /// store-owned identity, a name the source does not hold, a placed
    /// member, a member bound to no session, a roster-owned identity hosted
    /// on its first use or awaiting its held creation or its host's
    /// preparation, retryable once its host settled it, a worker spawned
    /// with a profile or model override), a stale generation
    /// ([`IdentityChangeRefusal::PredecessorMismatch`]), a
    /// member step in flight ([`IdentityChangeRefusal::AdmissionUnsettled`],
    /// retryable), an open declared apply
    /// ([`IdentityChangeRefusal::DeclaredApplyOpen`], retryable), a roster
    /// spec naming another role than the member's or another identity
    /// ([`IdentityChangeRefusal::InvalidRequest`]), a reserved name, and
    /// every refusal of a change's admission (a changes hold, a candidate
    /// composition, a decision's permit that does not bind the change, an
    /// agent's grant that does not admit the member's role).
    pub async fn adopt_identity(
        &self,
        adoption: IdentityAdoption,
        consent: IdentityChangeConsentInput<'_>,
    ) -> Result<IdentityChangeOutcome, MobError> {
        let IdentityAdoption {
            operation,
            requester,
            identity,
            source,
        } = adoption;
        if self.identity_member.is_none() {
            return Err(refused(IdentityChangeRefusal::StorageUnsupported));
        }
        let invalid = |problem: String| refused(IdentityChangeRefusal::InvalidRequest { problem });
        if operation.operation_id.trim().is_empty() || operation.request_digest.trim().is_empty() {
            return Err(invalid(
                "a change needs an operation id and a request digest".to_string(),
            ));
        }
        crate::identity::validate_identity(&identity)
            .map_err(|error| invalid(error.to_string()))?;
        if let AdoptionSource::Roster { spec } = &source {
            if spec.identity != identity {
                return Err(invalid(format!(
                    "the roster spec names '{}', not the adopted identity '{identity}'",
                    spec.identity
                )));
            }
            spec.validate()
                .map_err(|error| invalid(error.to_string()))?;
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
        let commit = AdoptionCommit {
            identity,
            source,
            facts: consent_facts.at(now_ms()),
            operation,
            grant,
            decided,
            requester,
            consent,
        };
        let committed = self
            .send_actor_command(|reply_tx| {
                super::MobCommand::IdentityChange(Box::new(IdentityChangeCommand::CommitAdoption {
                    commit: Box::new(commit),
                    reply_tx,
                }))
            })
            .await??;
        Ok(match committed {
            AdoptionCommitted::Committed(receipt) => {
                // A roster-owned identity adopted while owed its seat is a
                // store-owned identity owed its seat now: seated from its row,
                // as a start seats one, and its owned edges realized.
                self.reseat_owed_created_entries().await;
                self.realize_owned_identity_edges().await;
                IdentityChangeOutcome::Committed { receipt }
            }
            AdoptionCommitted::Replayed(receipt) => IdentityChangeOutcome::Replayed { receipt },
        })
    }
}
