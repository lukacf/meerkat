//! The one approver map (the access configuration's `approvers`) as the
//! decision verbs read it: whether the stdio operator or a person a
//! declared ingress attests holds an approver id, which identities one
//! person links across ingresses, and whether the party that decided a
//! decision still holds its approver id.
//!
//! Holders are read from the live configuration when a decision is made
//! and again when it commits or executes: removing a holder revokes before
//! or after the decision.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use meerkat_core::approval::consent::{
    ConsentEligibility, ConsentEligibilityAnswer, ConsentIneligible,
};
use meerkat_core::approval::decision::{DecisionEvidence, DecisionPrincipalKey, StoredDecision};
use meerkat_core::{PrincipalKind, PrincipalRef, TrustDomainId};
use meerkat_runtime::decision::DecisionApproverCurrentness;

use crate::access::{
    AccessControlConfig, AccessController, AccessIngressSubject, OPERATOR_INGRESS,
};

/// The trust-domain prefix of a person a declared ingress attests: the
/// domain is this prefix followed by the ingress's name, and the person's id
/// is the subject the ingress authenticated.
pub const INGRESS_TRUST_DOMAIN_PREFIX: &str = "meta-ingress-";

/// An identity that decides through Meta's stdio host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DeciderIdentity {
    /// The stdio operator.
    Operator,
    /// The person `subject` that the declared ingress `ingress` attests.
    Ingress { ingress: String, subject: String },
}

impl DeciderIdentity {
    fn is(&self, listed: &AccessIngressSubject) -> bool {
        match self {
            Self::Operator => listed.ingress == OPERATOR_INGRESS && listed.subject.is_none(),
            Self::Ingress { ingress, subject } => {
                listed.ingress == *ingress && listed.subject.as_deref() == Some(subject.as_str())
            }
        }
    }

    fn of_listed(listed: &AccessIngressSubject) -> Option<Self> {
        match (listed.ingress.as_str(), listed.subject.as_deref()) {
            (OPERATOR_INGRESS, None) => Some(Self::Operator),
            (OPERATOR_INGRESS, Some(_)) | (_, None) => None,
            (ingress, Some(subject)) => Some(Self::Ingress {
                ingress: ingress.to_string(),
                subject: subject.to_string(),
            }),
        }
    }

    /// This identity's principal; the operator's is `operator`, when the
    /// connection has one.
    fn principal(&self, operator: Option<&PrincipalRef>) -> Option<PrincipalRef> {
        match self {
            Self::Operator => operator.cloned(),
            Self::Ingress { ingress, subject } => ingress_person(ingress, subject).ok(),
        }
    }
}

/// The principal of the person `subject` that the declared ingress
/// `ingress` attests: a human in the ingress's own identity domain.
///
/// # Errors
/// The text of the principal refusal, for a subject or ingress no principal
/// can carry.
pub(crate) fn ingress_person(ingress: &str, subject: &str) -> Result<PrincipalRef, String> {
    let domain = TrustDomainId::new(format!("{INGRESS_TRUST_DOMAIN_PREFIX}{ingress}"))
        .map_err(|error| error.to_string())?;
    PrincipalRef::in_domain(PrincipalKind::Human, subject, domain)
        .map_err(|error| error.to_string())
}

/// How the live map lists an identity for one approver id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Holding {
    /// The identities of the person the identity matched, when it matched a
    /// `people` entry: one person, whose requests none of them may approve.
    linked: Vec<DeciderIdentity>,
}

/// Whether `config` lists `identity` as a holder of `approver_id`, directly
/// in `ingress_subjects` or as one of a person's identities in `people`.
pub(crate) fn holding(
    config: &AccessControlConfig,
    approver_id: &str,
    identity: &DeciderIdentity,
) -> Option<Holding> {
    let holders = config.approvers.get(approver_id)?;
    let person = holders.people.iter().find(|person| {
        person
            .ingress_subjects
            .iter()
            .any(|listed| identity.is(listed))
    });
    match person {
        Some(person) => Some(Holding {
            linked: person
                .ingress_subjects
                .iter()
                .filter_map(DeciderIdentity::of_listed)
                .collect(),
        }),
        None if holders
            .ingress_subjects
            .iter()
            .any(|listed| identity.is(listed)) =>
        {
            Some(Holding { linked: Vec::new() })
        }
        None => None,
    }
}

/// The principal that decides `row` as `own`: `own`, or, when the map links
/// the decider in one person with the decision's requester, the requester's
/// own principal, so that the decision machine refuses the decide as
/// `self_approval` (the machine sees one person only through one key).
pub(crate) fn decider_principal(
    row: &StoredDecision,
    holding: Option<&Holding>,
    own: PrincipalRef,
    operator: Option<&PrincipalRef>,
) -> PrincipalRef {
    let requester = &row.descriptor.requester;
    holding
        .into_iter()
        .flat_map(|holding| holding.linked.iter())
        .filter_map(|identity| identity.principal(operator))
        .find(|principal| DecisionPrincipalKey::of(principal).is_ok_and(|key| key == *requester))
        .unwrap_or(own)
}

/// Eligible while the live approver map lists one identity for one approver
/// id, read when the decision commits; for the stdio operator, also only
/// for a decision its connection presented, while the connection is open.
pub(crate) struct LiveHolderEligibility {
    access: Option<AccessController>,
    approver_id: String,
    identity: DeciderIdentity,
    presented: bool,
    connection_open: Option<Arc<AtomicBool>>,
}

impl LiveHolderEligibility {
    /// The eligibility of a person a declared ingress attests.
    pub(crate) fn attested(
        access: Option<AccessController>,
        approver_id: String,
        identity: DeciderIdentity,
    ) -> Self {
        Self {
            access,
            approver_id,
            identity,
            presented: true,
            connection_open: None,
        }
    }

    /// The eligibility of the stdio operator: `presented` says whether its
    /// connection presented the decision; `open` is the connection's.
    pub(crate) fn operator(
        access: Option<AccessController>,
        approver_id: String,
        presented: bool,
        open: Arc<AtomicBool>,
    ) -> Self {
        Self {
            access,
            approver_id,
            identity: DeciderIdentity::Operator,
            presented,
            connection_open: Some(open),
        }
    }
}

impl ConsentEligibility for LiveHolderEligibility {
    fn check_current(&self) -> Result<(), ConsentIneligible> {
        let listed = self.access.as_ref().is_some_and(|access| {
            holding(&access.snapshot().0, &self.approver_id, &self.identity).is_some()
        });
        let open = self
            .connection_open
            .as_ref()
            .is_none_or(|open| open.load(Ordering::SeqCst));
        if listed && self.presented && open {
            Ok(())
        } else {
            Err(ConsentIneligible)
        }
    }
}

/// Meta's answer to whether the party that decided a decision still holds
/// its approver id, from one snapshot of the live approver map: `Eligible`
/// while the map lists the identity that decided (the stdio operator, or a
/// person a declared ingress attests) for the decision's approver id, and
/// `Refused` otherwise. Without an access controller there is no map and no
/// holder. Meta vouches for no other party (a reviewer, or a person another
/// surface attested): those answer `Refused`.
pub(crate) struct MapApproverCurrentness {
    access: Option<AccessController>,
}

impl MapApproverCurrentness {
    pub(crate) fn new(access: Option<AccessController>) -> Self {
        Self { access }
    }
}

impl DecisionApproverCurrentness for MapApproverCurrentness {
    fn approver_answer(&self, decision: &StoredDecision) -> ConsentEligibilityAnswer {
        let Some(access) = self.access.as_ref() else {
            return ConsentEligibilityAnswer::Refused;
        };
        let config = access.snapshot().0;
        let approver_id = decision.descriptor.approver_id.as_str();
        match decided_identity(&config, decision) {
            Some(identity) if holding(&config, approver_id, &identity).is_some() => {
                ConsentEligibilityAnswer::Eligible(Arc::new(LiveHolderEligibility::attested(
                    Some(access.clone()),
                    approver_id.to_string(),
                    identity,
                )))
            }
            _ => ConsentEligibilityAnswer::Refused,
        }
    }
}

/// The identity of the map that decided `decision`: its human evidence's
/// ingress is the operator's, or the ingress and key of a person the map
/// lists for the decision's approver id.
fn decided_identity(
    config: &AccessControlConfig,
    decision: &StoredDecision,
) -> Option<DeciderIdentity> {
    let outcome = decision.outcome.as_ref()?;
    let DecisionEvidence::Human {
        decider, ingress, ..
    } = &outcome.evidence
    else {
        return None;
    };
    if ingress.as_str() == OPERATOR_INGRESS {
        return Some(DeciderIdentity::Operator);
    }
    let holders = config
        .approvers
        .get(decision.descriptor.approver_id.as_str())?;
    holders
        .all_ingress_subjects()
        .filter(|listed| listed.ingress == ingress.as_str())
        .filter_map(DeciderIdentity::of_listed)
        .find(|identity| {
            identity
                .principal(None)
                .and_then(|principal| DecisionPrincipalKey::of(&principal).ok())
                .is_some_and(|key| key == *decider)
        })
}
