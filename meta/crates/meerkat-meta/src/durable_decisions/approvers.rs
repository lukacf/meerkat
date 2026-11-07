//! The one approver map (the access configuration's `approvers`) as the
//! decision verbs read it: whether the stdio operator or a person a
//! declared ingress attests holds an approver id, which identities one
//! person links across ingresses, and whether the party that decided a
//! decision still holds its approver id.
//!
//! Holders are read from the live configuration when a decision is made
//! and again when it commits or executes: removing a holder revokes before
//! or after the decision, and a link between the decider and the requester
//! made before the commit refuses the decide.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use meerkat_core::approval::consent::{
    ConsentEligibility, ConsentEligibilityAnswer, ConsentIneligible,
};
use meerkat_core::approval::decision::{DecisionEvidence, DecisionPrincipalKey, StoredDecision};
use meerkat_core::{PrincipalKind, PrincipalRef, RealmId, TrustDomainId};
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
    /// host has one.
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

/// The stdio operator of a host on `realm`: the operating-system user that
/// started it, as the realm names it.
///
/// # Errors
/// The text of the refusal, when the user cannot be named.
pub(crate) fn local_operator_principal(realm: &RealmId) -> Result<PrincipalRef, String> {
    meerkat::governed_realm::stock::StockPrincipals::for_local_operator(realm)
        .map(|principals| principals.operator)
        .map_err(|error| error.to_string())
}

/// How the live map lists an identity for one approver id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Holding {
    /// The identities of the person the identity matched, when it matched a
    /// `people` entry: one person, whose requests none of them may approve.
    linked: Vec<DeciderIdentity>,
}

impl Holding {
    /// The identity among the linked ones that is `requester`, as its
    /// principal.
    fn linked_principal(
        &self,
        requester: &DecisionPrincipalKey,
        operator: Option<&PrincipalRef>,
    ) -> Option<PrincipalRef> {
        self.linked
            .iter()
            .filter_map(|identity| identity.principal(operator))
            .find(|principal| {
                DecisionPrincipalKey::of(principal).is_ok_and(|key| key == *requester)
            })
    }
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
    holding
        .and_then(|holding| holding.linked_principal(&row.descriptor.requester, operator))
        .unwrap_or(own)
}

/// What a decide input says of its decider, checked against the live map
/// when the decision commits: the decision's requester, the key the input
/// carries for the decider, and the stdio operator's principal, which names
/// the operator among a person's identities.
#[derive(Debug, Clone)]
pub(crate) struct DecideInput {
    requester: DecisionPrincipalKey,
    carried: Option<DecisionPrincipalKey>,
    operator: Option<PrincipalRef>,
}

impl DecideInput {
    /// The decide of `row` whose input carries `decider`.
    pub(crate) fn new(
        row: &StoredDecision,
        decider: &PrincipalRef,
        operator: Option<&PrincipalRef>,
    ) -> Self {
        Self {
            requester: row.descriptor.requester.clone(),
            carried: DecisionPrincipalKey::of(decider).ok(),
            operator: operator.cloned(),
        }
    }

    /// The decide recorded for `decision`, decided as `decider`.
    fn recorded(
        decision: &StoredDecision,
        decider: &DecisionPrincipalKey,
        operator: Option<&PrincipalRef>,
    ) -> Self {
        Self {
            requester: decision.descriptor.requester.clone(),
            carried: Some(decider.clone()),
            operator: operator.cloned(),
        }
    }
}

/// Eligible while the live approver map lists one identity for one approver
/// id, read when the decision commits, and does not link that identity to
/// the decision's requester unless the decide input already carries the
/// requester's key (the machine then refuses it as self approval); for the
/// stdio operator, also only for a decision its connection presented, while
/// the connection is open.
pub(crate) struct LiveHolderEligibility {
    access: Option<AccessController>,
    approver_id: String,
    identity: DeciderIdentity,
    input: DecideInput,
    presented: bool,
    connection_open: Option<Arc<AtomicBool>>,
}

impl LiveHolderEligibility {
    /// The eligibility of a person a declared ingress attests.
    pub(crate) fn attested(
        access: Option<AccessController>,
        approver_id: String,
        identity: DeciderIdentity,
        input: DecideInput,
    ) -> Self {
        Self {
            access,
            approver_id,
            identity,
            input,
            presented: true,
            connection_open: None,
        }
    }

    /// The eligibility of the stdio operator: `presented` says whether its
    /// connection presented the decision; `open` is the connection's.
    pub(crate) fn operator(
        access: Option<AccessController>,
        approver_id: String,
        input: DecideInput,
        presented: bool,
        open: Arc<AtomicBool>,
    ) -> Self {
        Self {
            access,
            approver_id,
            identity: DeciderIdentity::Operator,
            input,
            presented,
            connection_open: Some(open),
        }
    }
}

impl ConsentEligibility for LiveHolderEligibility {
    fn check_current(&self) -> Result<(), ConsentIneligible> {
        let Some(access) = self.access.as_ref() else {
            return Err(ConsentIneligible);
        };
        let config = access.snapshot().0;
        let Some(held) = holding(&config, &self.approver_id, &self.identity) else {
            return Err(ConsentIneligible);
        };
        // The requester link, re-derived from the map read now: a decide
        // read before the map linked the decider to the requester carries
        // the decider's own key, and is not eligible.
        let linked = held
            .linked_principal(&self.input.requester, self.input.operator.as_ref())
            .is_some();
        if linked && self.input.carried.as_ref() != Some(&self.input.requester) {
            return Err(ConsentIneligible);
        }
        let open = self
            .connection_open
            .as_ref()
            .is_none_or(|open| open.load(Ordering::SeqCst));
        if self.presented && open {
            Ok(())
        } else {
            Err(ConsentIneligible)
        }
    }
}

/// Meta's answer to whether the party that decided a decision still holds
/// its approver id, from one snapshot of the live approver map: `Eligible`
/// while the map lists the identity that decided (the stdio operator of
/// this host's realm, or a person a declared ingress attests) for the
/// decision's approver id, and `Refused` otherwise. Without an access
/// controller there is no map and no holder. Meta vouches for operator
/// evidence only when its decider is this host's operator, and for no other
/// party (a reviewer, or a person another surface attested): those answer
/// `Refused`.
pub(crate) struct MapApproverCurrentness {
    access: Option<AccessController>,
    operator: Option<PrincipalRef>,
}

impl MapApproverCurrentness {
    /// The currentness of a host whose meerkat-level realm is `realm`,
    /// which names its stdio operator.
    pub(crate) fn new(access: Option<AccessController>, realm: Option<&RealmId>) -> Self {
        Self {
            access,
            operator: realm.and_then(|realm| local_operator_principal(realm).ok()),
        }
    }
}

impl DecisionApproverCurrentness for MapApproverCurrentness {
    fn approver_answer(&self, decision: &StoredDecision) -> ConsentEligibilityAnswer {
        let Some(access) = self.access.as_ref() else {
            return ConsentEligibilityAnswer::Refused;
        };
        let config = access.snapshot().0;
        let approver_id = decision.descriptor.approver_id.as_str();
        match decided_identity(&config, decision, self.operator.as_ref()) {
            Some((identity, decider)) if holding(&config, approver_id, &identity).is_some() => {
                ConsentEligibilityAnswer::Eligible(Arc::new(LiveHolderEligibility::attested(
                    Some(access.clone()),
                    approver_id.to_string(),
                    identity,
                    DecideInput::recorded(decision, &decider, self.operator.as_ref()),
                )))
            }
            _ => ConsentEligibilityAnswer::Refused,
        }
    }
}

/// The identity of the map that decided `decision`, and the key it decided
/// as: its human evidence names this host's operator (the ingress
/// `operator` and the operator's own key), or the ingress and key of a
/// person the map lists for the decision's approver id.
fn decided_identity(
    config: &AccessControlConfig,
    decision: &StoredDecision,
    operator: Option<&PrincipalRef>,
) -> Option<(DeciderIdentity, DecisionPrincipalKey)> {
    let outcome = decision.outcome.as_ref()?;
    let DecisionEvidence::Human {
        decider, ingress, ..
    } = &outcome.evidence
    else {
        return None;
    };
    if ingress.as_str() == OPERATOR_INGRESS {
        let operator = DecisionPrincipalKey::of(operator?).ok()?;
        return (operator == *decider).then(|| (DeciderIdentity::Operator, decider.clone()));
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
        .map(|identity| (identity, decider.clone()))
}
