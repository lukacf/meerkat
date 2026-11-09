//! The governed work a peer delivery was sent under, as its native runtime
//! staged it.

use std::sync::Arc;

use meerkat_authorization_contracts::constraints::ResourceDomain;
use meerkat_authorization_contracts::work_association::{
    InputAuthorityAssociation, OriginalWorkRef, QualifiedIngressNamespace, WorkAuthorityBasis,
};
use meerkat_core::PrincipalRef;
use meerkat_core::connection::RealmId;

/// The governed work a peer delivery was sent under, as the sending native
/// runtime staged it.
///
/// Core mints it when it stages a governed run, from the admitted
/// associations of every original the run's rows retain. It attaches it to
/// the run's work authorization, and carries it with each in-process
/// delivery that run's comms tools make, held opaque by everything between
/// ([`meerkat_core::GovernedSendOrigin`]). At the receiver Core reads it
/// back only for a delivery whose sender the in-process transport
/// established ([`super::PeerIngressCommsOrigin::governed_origin`]).
///
/// It is never serialized, never built from a tool's arguments, and never a
/// grant. A binding of the delivery must keep its requester and name every
/// work it lists as a contributor, with the sending agent as its ingress
/// actor ([`super::BoundOriginalPin::GovernedDelivery`]); the receiver's own
/// host decides everything else about the receiving work.
#[derive(Clone)]
pub struct GovernedPeerOrigin(Arc<OriginFacts>);

/// The facts of one origin. Private: only this module mints them, and a
/// receiver reads back only this type.
#[derive(PartialEq, Eq)]
struct OriginFacts {
    requester: PrincipalRef,
    represented_subject: Option<PrincipalRef>,
    sender_executor: PrincipalRef,
    /// Every staged original of the sending work, in staged order, once.
    originals: Vec<SenderOriginal>,
    /// Every staged original, then every original each one names as a
    /// contributor, in that order, each once.
    sender_work: Vec<OriginalWorkRef>,
}

/// One staged original of the sending work.
#[derive(PartialEq, Eq)]
struct SenderOriginal {
    namespace: QualifiedIngressNamespace,
    original_work: OriginalWorkRef,
    /// Whether its own basis is a grant lineage, whose current ceiling a
    /// work naming it as a contributor is narrowed by. A host-policy or
    /// service-mandate basis narrows no such work.
    grant_basis: bool,
    /// The original's own ingress actor, basis and contributors, as it was
    /// admitted: read only by Core's own tools of the staged work.
    ingress_actor: PrincipalRef,
    authority_basis: WorkAuthorityBasis,
    contributing_work: Vec<OriginalWorkRef>,
}

/// The one staged original of a governed work, as it was admitted: what a
/// Core tool of that work reads of the work it runs in. Never a grant.
#[cfg(all(feature = "local-authorization", not(target_arch = "wasm32")))]
pub(crate) struct StagedOriginal<'a> {
    pub(crate) namespace: &'a QualifiedIngressNamespace,
    pub(crate) original_work: &'a OriginalWorkRef,
    pub(crate) ingress_actor: &'a PrincipalRef,
    pub(crate) authority_basis: &'a WorkAuthorityBasis,
    pub(crate) contributing_work: &'a [OriginalWorkRef],
}

impl GovernedPeerOrigin {
    /// The origin of the work staged from `associations`: every original of
    /// one staged run. `None` for no original, or for originals that do not
    /// share one requester, represented subject and executor (a staged
    /// batch always does).
    pub(crate) fn from_associations<'a>(
        associations: impl IntoIterator<Item = &'a InputAuthorityAssociation>,
    ) -> Option<Self> {
        let mut associations = associations.into_iter().peekable();
        let first = associations.peek()?.candidate();
        let requester = first.requester.clone();
        let represented_subject = first.represented_subject.clone();
        let sender_executor = first.logical_executor.clone();
        let mut originals: Vec<SenderOriginal> = Vec::new();
        let mut contributors: Vec<&OriginalWorkRef> = Vec::new();
        for association in associations {
            let claims = association.candidate();
            if claims.requester != requester
                || claims.represented_subject != represented_subject
                || claims.logical_executor != sender_executor
            {
                return None;
            }
            if originals
                .iter()
                .any(|original| original.original_work == claims.original_work)
            {
                continue;
            }
            originals.push(SenderOriginal {
                namespace: claims.ingress_namespace.clone(),
                original_work: claims.original_work.clone(),
                grant_basis: matches!(
                    claims.authority_basis,
                    WorkAuthorityBasis::GrantLineage { .. }
                        | WorkAuthorityBasis::TargetedGrantLineages { .. }
                ),
                ingress_actor: claims.ingress_actor.clone(),
                authority_basis: claims.authority_basis.clone(),
                contributing_work: claims.contributing_work.clone(),
            });
            contributors.extend(claims.contributing_work.iter());
        }
        let mut sender_work: Vec<OriginalWorkRef> = Vec::new();
        for work in originals
            .iter()
            .map(|original| &original.original_work)
            .chain(contributors)
        {
            if !sender_work.contains(work) {
                sender_work.push(work.clone());
            }
        }
        Some(Self(Arc::new(OriginFacts {
            requester,
            represented_subject,
            sender_executor,
            originals,
            sender_work,
        })))
    }

    /// The opaque carrier of this origin, for the run's work authorization
    /// and the in-process lane.
    pub(crate) fn to_send_origin(&self) -> meerkat_core::GovernedSendOrigin {
        meerkat_core::GovernedSendOrigin::__from_runtime_facts(
            Arc::clone(&self.0) as Arc<dyn std::any::Any + Send + Sync>
        )
    }

    /// The origin a carrier holds, when a native runtime minted it. A
    /// carrier holding anything else is no origin.
    pub(crate) fn from_send_origin(origin: &meerkat_core::GovernedSendOrigin) -> Option<Self> {
        Arc::clone(origin.__runtime_facts())
            .downcast::<OriginFacts>()
            .ok()
            .map(Self)
    }

    /// The sending work's requester: the requester a binding keeps.
    pub fn requester(&self) -> &PrincipalRef {
        &self.0.requester
    }

    /// The principal the sending agent acted for. Data for the receiving
    /// host: the receiving work's represented subject is its own.
    pub fn represented_subject(&self) -> Option<&PrincipalRef> {
        self.0.represented_subject.as_ref()
    }

    /// The sending agent: the receiving work's ingress actor.
    pub fn sender_executor(&self) -> &PrincipalRef {
        &self.0.sender_executor
    }

    /// The sending work, transitively: every staged original, then every
    /// original each one names as a contributor, in that order, each once.
    /// A work that names an earlier peer work names that work's own
    /// contributors too, so every hop of a chain of peers lists the
    /// original requester's work. A binding names all of them.
    pub fn sender_work(&self) -> &[OriginalWorkRef] {
        &self.0.sender_work
    }

    /// The realm and ingress domain every staged original of the sending
    /// work was admitted in, when they share one. Contributors are resolved
    /// in the receiving work's own namespace, so the receiving work is bound
    /// in this one; `None` for a work whose originals span two.
    pub fn namespace(&self) -> Option<(&RealmId, &ResourceDomain)> {
        let mut originals = self.0.originals.iter();
        let first = &originals.next()?.namespace;
        originals
            .all(|original| {
                original.namespace.realm == first.realm
                    && original.namespace.ingress == first.ingress
            })
            .then_some((&first.realm, &first.ingress))
    }

    /// Whether every staged original of the sending work has a grant-lineage
    /// basis, so each one narrows a work that names it. A host-policy or
    /// service-mandate original adds no ceiling to such a work.
    pub fn bounded_by_grants(&self) -> bool {
        self.0.originals.iter().all(|original| original.grant_basis)
    }

    /// The staged work's one original, as it was admitted, when the work
    /// staged exactly one. `None` for a work that staged several.
    #[cfg(all(feature = "local-authorization", not(target_arch = "wasm32")))]
    pub(crate) fn single_original(&self) -> Option<StagedOriginal<'_>> {
        let [original] = self.0.originals.as_slice() else {
            return None;
        };
        Some(StagedOriginal {
            namespace: &original.namespace,
            original_work: &original.original_work,
            ingress_actor: &original.ingress_actor,
            authority_basis: &original.authority_basis,
            contributing_work: &original.contributing_work,
        })
    }
}

impl PartialEq for GovernedPeerOrigin {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0) || self.0 == other.0
    }
}

impl Eq for GovernedPeerOrigin {}

impl std::fmt::Debug for GovernedPeerOrigin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GovernedPeerOrigin")
            .field("requester", &self.0.requester)
            .field("represented_subject", &self.0.represented_subject)
            .field("sender_executor", &self.0.sender_executor)
            .field("originals", &self.0.originals.len())
            .field("sender_work", &self.0.sender_work.len())
            .finish()
    }
}
