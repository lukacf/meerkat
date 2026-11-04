//! Core's in-process governed peer binder: the receiving work keeps the
//! original requester of the work that sent it.
//!
//! A governed agent's comms send carries, through the in-process lane, the
//! origin of the governed work it was sent under ([`GovernedPeerOrigin`]).
//! [`OriginalRequesterPeerBinder`] binds such a delivery for its receiving
//! agent. Core writes the requester and the sending side of the binding;
//! the host's [`PeerServiceBasis`] supplies only the receiving agent's own
//! side. The result:
//!
//! - **requester**: the sending work's requester;
//! - **ingress actor**: the sending agent;
//! - **contributing work**: every work of the sending work, transitively
//!   ([`GovernedPeerOrigin::sender_work`]). The receiving work's every
//!   operation is then narrowed by the requester's own current ceiling,
//!   through any number of peer hops: it cannot run what the requester may
//!   not;
//! - **namespace**: the sending work's realm and ingress domain (the only
//!   namespace its contributors resolve in), with this delivery's own
//!   occurrence scope (Core's delivery work id);
//! - **original work**: Core's delivery work, under the ingress domain's
//!   authority; **original authentication** and **root event**: Core's
//!   evidence of the in-process sender and of the delivery, in that ingress
//!   domain;
//! - from the host: the receiving agent as logical executor, the subject it
//!   acts for, its own basis for serving peers, its destination, its
//!   controller grant lineage and ceilings, and its pinned controller.
//!
//! The binder refuses, typed ([`PeerIngressBindDenial`]): a delivery without
//! an origin Core established (a network delivery, an ungoverned sender),
//! a sending work it cannot name in one namespace, a sending work with an
//! original that narrows nothing (a host-policy or service-mandate basis), a
//! sending work beyond the association's contributor bound, a receiver the
//! host holds no basis for, and a delivery whose original the host already
//! holds enrolled differently.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use meerkat_authorization_contracts::constraints::{ExecutionRestrictions, ResourceDomain};
use meerkat_authorization_contracts::evidence::{
    EvidenceDigest, EvidenceId, HistoricalEvidenceRef,
};
use meerkat_authorization_contracts::protocol::ContractRequirements;
use meerkat_authorization_contracts::resource::ResourceRef;
use meerkat_authorization_contracts::work_association::{
    GrantLineageRef, InputAuthorityAssociation, InputAuthorityAssociationCandidate,
    MAX_ASSOCIATION_REFERENCES, NativeWorkTarget, OriginalWorkRef, QualifiedIngressNamespace,
    WorkAuthorityBasis,
};
use meerkat_contracts::wire::supervisor_bridge::BridgeMemberIncarnation;
use meerkat_core::comms::PeerId;
use meerkat_core::connection::RealmId;
use meerkat_core::interaction::{PeerIngressKind, PeerInputClass};
use meerkat_core::time_compat::Instant;
use meerkat_core::types::SessionId;
use meerkat_core::{ControllerModelClient, PrincipalRef};
use sha2::{Digest, Sha256};

use super::{
    GovernedPeerOrigin, PeerIngressBindDenial, PeerIngressBindFuture, PeerIngressBindRefusal,
    PeerIngressBindRequest, PeerIngressBinder, PeerIngressBinding, PeerIngressOrigin,
};
use crate::identifiers::{IdempotencyKey, LogicalRuntimeId};
use crate::input_admission_custody::NativeInputAdmissionCustody;

/// How long the binder keeps, at least, what it answered one delivery: far
/// above an in-process sender's whole send budget (`ack_timeout_secs`, 30 s
/// by default), the only window in which it redelivers.
pub const DEFAULT_PEER_CUSTODY_RETENTION: Duration = Duration::from_mins(10);

/// What Core asks the host's [`PeerServiceBasis`] for one governed peer
/// delivery it binds. Built only by Core.
#[non_exhaustive]
pub struct PeerServiceRequest {
    /// The receiving session's runtime.
    pub runtime_id: LogicalRuntimeId,
    /// The receiving session.
    pub session_id: SessionId,
    /// The member incarnation the delivery is fenced to, when it is fenced.
    pub expected_member: Option<BridgeMemberIncarnation>,
    /// The delivering peer, as the in-process transport established it.
    pub sender: PeerId,
    pub class: PeerInputClass,
    pub kind: PeerIngressKind,
    /// The governed work the delivery was sent under.
    pub origin: GovernedPeerOrigin,
    /// Core's admission key for this delivery.
    pub event_key: IdempotencyKey,
    /// Core's work id for this delivery: the receiving work's original work
    /// and occurrence scope.
    pub delivery_work: EvidenceId,
}

impl std::fmt::Debug for PeerServiceRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PeerServiceRequest")
            .field("runtime_id", &self.runtime_id)
            .field("session_id", &self.session_id)
            .field("sender", &self.sender)
            .field("origin", &self.origin)
            .finish_non_exhaustive()
    }
}

/// The receiving agent's own side of one governed peer binding, as the
/// host's owners hold it now for this receiver and this requester.
pub struct PeerServiceGrant {
    /// The receiving agent: the work's logical executor.
    pub receiver: PrincipalRef,
    /// The principal the receiving agent acts for in this work: the subject
    /// its basis resolves for. The sending work's subject is data
    /// ([`GovernedPeerOrigin::represented_subject`]), never adopted.
    pub represented_subject: Option<PrincipalRef>,
    /// The receiving agent's own basis for serving peers for this
    /// requester. Every operation of the work is narrowed further by every
    /// work of the sending work, so the receiving agent's own access never
    /// widens what the requester may.
    pub authority_basis: WorkAuthorityBasis,
    /// The receiving session's destination, on the receiving runtime.
    pub target: NativeWorkTarget,
    /// The receiving agent's own controller grant lineage.
    pub controller_grant_lineage: Vec<GrantLineageRef>,
    pub controller_ceiling: ExecutionRestrictions,
    pub admitted_ceiling: ExecutionRestrictions,
    /// The receiving session's live controller client, pinned without
    /// waiting on its turn.
    pub controller: ControllerModelClient,
}

impl std::fmt::Debug for PeerServiceGrant {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PeerServiceGrant")
            .field("receiver", &self.receiver)
            .field("represented_subject", &self.represented_subject)
            .finish_non_exhaustive()
    }
}

/// Why the host's [`PeerServiceBasis`] bound nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum PeerServiceRefusal {
    /// The host holds no basis on which this receiver serves this requester
    /// through peers. Final for the delivery: Core refuses it
    /// `no_peer_service_basis`, and records the refusal.
    NoBasis,
    /// The host already holds another association enrolled for this
    /// delivery's original. Final: the first enrollment stands, and Core
    /// refuses the delivery `enrollment_conflict` and records the refusal.
    EnrollmentConflict,
    /// Not answerable now, for a typed cause (a cold or held receiver, a
    /// controller not pinned yet). Pending: Core records nothing, and a
    /// durable delivery stays the session's FIFO head.
    NotReady(meerkat_core::authorization::ControllerReadinessFailure),
    /// The host could not be read now. Pending, as `NotReady`.
    Unavailable(String),
}

/// The host's answer for one governed peer delivery.
#[cfg(not(target_arch = "wasm32"))]
pub type PeerServiceFuture<'a> = std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<PeerServiceGrant, PeerServiceRefusal>> + Send + 'a>,
>;

/// The host's answer for one governed peer delivery.
#[cfg(target_arch = "wasm32")]
pub type PeerServiceFuture<'a> = std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<PeerServiceGrant, PeerServiceRefusal>> + 'a>,
>;

/// The trusted host's receiving side of [`OriginalRequesterPeerBinder`]:
/// whether, and on which of its own grants, a receiving agent serves a
/// requester through peers, and its custody of what Core binds.
///
/// Contract:
/// - [`Self::basis_for`] is a read: side-effect free, bounded and
///   cancel-safe. Core may drop its future at any await, and a dropped call
///   must leave the host as it was. It never awaits a member turn or
///   anything that waits on the receiving session's ingress; what it cannot
///   answer now is `NotReady`.
/// - [`Self::enroll`] puts the exact association Core minted for a delivery
///   in the host's own custody of originals, under the delivery's original
///   work, before the binding returns: the custody its admission
///   authenticates the original against (for a scoped composition, its
///   `AuthenticatedOriginalSource`). It is first-writer-wins per realm,
///   ingress domain and original work: the same association again is
///   accepted (Core enrolls on every bind, verbatim re-binds included), and
///   a different one is refused `EnrollmentConflict`, never overwritten, so
///   an admitted work's later operations are always checked against the
///   facts it was admitted with. It is synchronous and bounded and performs
///   no I/O on a lock Core holds. Core holds its custody lock across it, so
///   it must never call back into the binder: a call back deadlocks.
/// - The custody it returns is settled by native admission exactly once
///   (`NotAdmitted` for every refusal of Core's own), as for every bound
///   original. It keeps the enrolled original at least as long as the
///   sender may redeliver: an in-process sender redelivers only within its
///   own send budget.
pub trait PeerServiceBasis: Send + Sync {
    /// The receiving agent's own side of a binding of `request`, or why
    /// there is none.
    fn basis_for<'a>(&'a self, request: &'a PeerServiceRequest) -> PeerServiceFuture<'a>;

    /// Enroll `association`, the exact original Core minted for `request`,
    /// first-writer-wins (see the trait's contract), and return the custody
    /// native admission settles.
    ///
    /// # Errors
    /// `EnrollmentConflict` for an original enrolled with another
    /// association; `NotReady` or `Unavailable` when the custody cannot be
    /// written now.
    fn enroll(
        &self,
        request: &PeerServiceRequest,
        association: &InputAuthorityAssociation,
    ) -> Result<Option<Box<dyn NativeInputAdmissionCustody>>, PeerServiceRefusal>;
}

/// Core's in-process governed peer binder: binds a governed in-process
/// delivery for its receiving agent so that the receiving work keeps the
/// sending work's original requester (see the module docs).
///
/// A host composition installs it with
/// `MeerkatMachine::install_peer_ingress_binder`, over its own
/// [`PeerServiceBasis`]. It meets the [`PeerIngressBinder`] contract:
/// - **Custody.** It keeps, per receiving session and event key, the
///   association it minted, or the refusal it answered for a host-dependent
///   cause, for at least [`DEFAULT_PEER_CUSTODY_RETENTION`] (or the
///   retention its host set). The custody is process-local: an in-process
///   delivery not admitted yet lives in the receiver's in-memory queue and
///   ends with the process, and an admitted one is deduplicated by Core's
///   delivery input id whatever is bound again. A record older than the
///   retention is dropped; a re-bind after that mints again, and the host's
///   first-writer-wins enrollment refuses an association that differs.
/// - **Verbatim re-bind.** A later bind of a key with a minted record binds
///   that exact association again, also after Core refused it, with the
///   host's current controller client of the same selection. A different
///   selection is refused (`Denied`) and recorded.
/// - **Recorded refusals.** A refusal for a host-dependent cause
///   (`no_peer_service_basis`, `enrollment_conflict`, a malformed host
///   grant) is recorded against the key and answered again. A refusal Core
///   derives from the delivery's own origin is derived again identically.
///   A pending answer (`NotReady`, `Unavailable`) records nothing.
/// - **Cancel safety.** The only await is the host's
///   [`PeerServiceBasis::basis_for`], a side-effect-free read. After it
///   returns, the binder re-reads its custody under its lock (a record a
///   concurrent bind made meanwhile wins verbatim), then mints, binds,
///   enrolls through [`PeerServiceBasis::enroll`] and records, in that
///   order, under that lock, with no await. A bind dropped during
///   `basis_for` has minted, enrolled and recorded nothing, as a `NotReady`
///   answer.
/// - **Host panics.** A panic in [`PeerServiceBasis::enroll`] ends that bind
///   (its future panics) with nothing recorded. It cannot leave the custody
///   map half-updated, so the binder recovers the lock the panic poisoned and
///   later binds proceed.
/// - **Supervisor deliveries.** A supervisor-bridge member delivery is not
///   this binder's: it goes to the host's own binder when one is set
///   ([`Self::with_supervisor_binder`]), else it answers `NotOffered`.
pub struct OriginalRequesterPeerBinder {
    basis: Arc<dyn PeerServiceBasis>,
    supervisor: Option<Arc<dyn PeerIngressBinder>>,
    retention: Duration,
    custody: Mutex<HashMap<CustodyKey, Record>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct CustodyKey {
    session_id: SessionId,
    event_key: String,
}

struct Record {
    answer: Answer,
    at: Instant,
}

#[derive(Clone)]
enum Answer {
    Minted(InputAuthorityAssociation),
    Refused(PeerIngressBindRefusal),
}

impl OriginalRequesterPeerBinder {
    /// The binder over the host's `basis`.
    #[must_use]
    pub fn new(basis: Arc<dyn PeerServiceBasis>) -> Self {
        Self {
            basis,
            supervisor: None,
            retention: DEFAULT_PEER_CUSTODY_RETENTION,
            custody: Mutex::new(HashMap::new()),
        }
    }

    /// The same binder, offering supervisor-bridge member deliveries to the
    /// host's own `binder` instead of answering them `NotOffered`.
    #[must_use]
    pub fn with_supervisor_binder(mut self, binder: Arc<dyn PeerIngressBinder>) -> Self {
        self.supervisor = Some(binder);
        self
    }

    /// The same binder, keeping each record at least `retention`. A host
    /// whose in-process senders' send budget exceeds the default keeps its
    /// records at least that long.
    #[must_use]
    pub fn with_custody_retention(mut self, retention: Duration) -> Self {
        self.retention = retention;
        self
    }

    async fn bind(
        &self,
        request: PeerIngressBindRequest,
    ) -> Result<PeerIngressBinding, PeerIngressBindRefusal> {
        let comms = match &request.origin {
            PeerIngressOrigin::Comms(comms) => comms,
            PeerIngressOrigin::SupervisorDelivery(_) => {
                return match &self.supervisor {
                    Some(binder) => binder.bind_peer_ingress(request).await,
                    None => Err(PeerIngressBindRefusal::NotOffered),
                };
            }
        };
        let refused = |denial| Err(PeerIngressBindRefusal::Refused(denial));
        // Refusals derived from the delivery's own origin: a redelivery of
        // the same envelope carries the same origin, so they derive again
        // identically and are not recorded.
        let Some(origin) = comms.governed_origin.clone() else {
            return refused(PeerIngressBindDenial::OriginUnverifiable);
        };
        let Some((realm, ingress)) = origin.namespace() else {
            return refused(PeerIngressBindDenial::OriginUnverifiable);
        };
        let (realm, ingress) = (realm.clone(), ingress.clone());
        if !origin.bounded_by_grants() {
            return refused(PeerIngressBindDenial::SenderBasisUnbounded);
        }
        if origin.sender_work().len() > MAX_ASSOCIATION_REFERENCES {
            return refused(PeerIngressBindDenial::ContributorBoundExceeded);
        }
        let key = CustodyKey {
            session_id: request.session_id.clone(),
            event_key: request.event_key.to_string(),
        };
        if let Some(Answer::Refused(refusal)) = self.recorded(&key) {
            return Err(refusal);
        }
        let service = PeerServiceRequest {
            runtime_id: request.runtime_id.clone(),
            session_id: request.session_id.clone(),
            expected_member: request.expected_member.clone(),
            sender: comms.sender,
            class: comms.class,
            kind: comms.kind,
            origin,
            event_key: request.event_key.clone(),
            delivery_work: request.delivery_work.clone(),
        };
        // The one await: a side-effect-free read of the host.
        let answer = self.basis.basis_for(&service).await;
        // From here on nothing awaits: custody is re-read, and the binding
        // minted, enrolled and recorded, under one lock.
        // A host panic in `enroll` poisons this lock but cannot leave the
        // map half-updated: the sweep and the re-read end before `enroll`,
        // and the record is one insert after it. So the lock is recovered,
        // and later binds proceed instead of staying pending.
        let mut custody = self.custody.lock().unwrap_or_else(PoisonError::into_inner);
        let now = Instant::now();
        custody.retain(|_, record| now.saturating_duration_since(record.at) < self.retention);
        let recorded = custody.get(&key).map(|record| record.answer.clone());
        let (recorded, grant) = match (recorded, answer) {
            (Some(Answer::Refused(refusal)), _) => return Err(refusal),
            (_, Err(PeerServiceRefusal::NotReady(failure))) => {
                return Err(PeerIngressBindRefusal::NotReady(failure));
            }
            (_, Err(PeerServiceRefusal::Unavailable(detail))) => {
                return Err(PeerIngressBindRefusal::Unavailable(detail));
            }
            (_, Err(refusal)) => {
                let refusal = PeerIngressBindRefusal::Refused(match refusal {
                    PeerServiceRefusal::EnrollmentConflict => {
                        PeerIngressBindDenial::EnrollmentConflict
                    }
                    _ => PeerIngressBindDenial::NoPeerServiceBasis,
                });
                record(&mut custody, key, Answer::Refused(refusal.clone()), now);
                return Err(refusal);
            }
            (recorded, Ok(grant)) => (recorded, grant),
        };
        let minted = match recorded {
            Some(Answer::Minted(association)) => Ok(association),
            _ => mint(&service, &grant, &realm, &ingress),
        };
        let bound = minted.and_then(|association| {
            let authentication = association.candidate().original_authentication.clone();
            let bound = super::bind_original_input(
                request.input,
                association.clone(),
                request.event_key.clone(),
                service.origin.requester().clone(),
                service.origin.sender_executor().clone(),
                realm.clone(),
                authentication,
                grant.controller,
            )
            .map_err(|error| error.to_string())?;
            Ok((association, bound))
        });
        let (association, bound) = match bound {
            Ok(bound) => bound,
            Err(detail) => {
                // A grant Core cannot mint or bind from (a malformed host
                // answer, or a controller client of another selection than
                // the recorded association's) is a host fault: refused for
                // good, and recorded.
                tracing::error!(
                    session_id = %request.session_id,
                    %detail,
                    "original requester peer binder: the host's grant cannot bind the delivery"
                );
                let refusal = PeerIngressBindRefusal::Denied;
                record(&mut custody, key, Answer::Refused(refusal.clone()), now);
                return Err(refusal);
            }
        };
        let enrolled = match self.basis.enroll(&service, &association) {
            Ok(enrolled) => enrolled,
            Err(PeerServiceRefusal::NotReady(failure)) => {
                return Err(PeerIngressBindRefusal::NotReady(failure));
            }
            Err(PeerServiceRefusal::Unavailable(detail)) => {
                return Err(PeerIngressBindRefusal::Unavailable(detail));
            }
            Err(refusal) => {
                let refusal = PeerIngressBindRefusal::Refused(match refusal {
                    PeerServiceRefusal::NoBasis => PeerIngressBindDenial::NoPeerServiceBasis,
                    _ => PeerIngressBindDenial::EnrollmentConflict,
                });
                record(&mut custody, key, Answer::Refused(refusal.clone()), now);
                return Err(refusal);
            }
        };
        record(&mut custody, key, Answer::Minted(association), now);
        let binding = PeerIngressBinding::new(bound);
        Ok(match enrolled {
            Some(custody) => binding.with_admission_custody(custody),
            None => binding,
        })
    }

    /// The live record for `key`, if any.
    fn recorded(&self, key: &CustodyKey) -> Option<Answer> {
        let custody = self.custody.lock().unwrap_or_else(PoisonError::into_inner);
        let record = custody.get(key)?;
        (Instant::now().saturating_duration_since(record.at) < self.retention)
            .then(|| record.answer.clone())
    }
}

/// Record `answer` for `key`, keeping a minted record's first time: what was
/// answered first stands for the retention.
fn record(custody: &mut HashMap<CustodyKey, Record>, key: CustodyKey, answer: Answer, at: Instant) {
    let at = custody.get(&key).map_or(at, |record| record.at);
    custody.insert(key, Record { answer, at });
}

impl PeerIngressBinder for OriginalRequesterPeerBinder {
    fn bind_peer_ingress(&self, request: PeerIngressBindRequest) -> PeerIngressBindFuture<'_> {
        Box::pin(self.bind(request))
    }
}

impl std::fmt::Debug for OriginalRequesterPeerBinder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OriginalRequesterPeerBinder")
            .field("retention", &self.retention)
            .field("supervisor_binder", &self.supervisor.is_some())
            .finish_non_exhaustive()
    }
}

/// The association Core binds for `request` under `grant`, in the sending
/// work's `realm` and `ingress` domain (see the module docs).
fn mint(
    request: &PeerServiceRequest,
    grant: &PeerServiceGrant,
    realm: &RealmId,
    ingress: &ResourceDomain,
) -> Result<InputAuthorityAssociation, String> {
    let evidence = |revision: &str, digest: EvidenceDigest| -> Result<_, String> {
        Ok(HistoricalEvidenceRef {
            resource: ResourceRef {
                domain: ResourceDomain {
                    authority: ingress.authority.clone(),
                    namespace: "peer-deliveries".into(),
                },
                resource_id: request.delivery_work.as_str().into(),
            },
            revision: EvidenceId::new(revision).map_err(|error| error.to_string())?,
            digest,
        })
    };
    let event_key = request.event_key.to_string();
    let original_authentication = evidence(
        "in-process-sender",
        digest(&[
            b"meerkat.peer-delivery-sender.v1",
            request.sender.to_string().as_bytes(),
            event_key.as_bytes(),
        ]),
    )?;
    let root_event = evidence(
        "delivered",
        digest(&[b"meerkat.peer-delivery-event.v1", event_key.as_bytes()]),
    )?;
    InputAuthorityAssociation::new(InputAuthorityAssociationCandidate {
        requester: request.origin.requester().clone(),
        ingress_actor: request.origin.sender_executor().clone(),
        represented_subject: grant.represented_subject.clone(),
        original_authentication,
        logical_executor: grant.receiver.clone(),
        target: grant.target.clone(),
        original_work: OriginalWorkRef {
            authority: ingress.authority.clone(),
            work: request.delivery_work.clone(),
        },
        root_event,
        contributing_work: request.origin.sender_work().to_vec(),
        authority_basis: grant.authority_basis.clone(),
        controller_grant_lineage: grant.controller_grant_lineage.clone(),
        controller_model: Some(grant.controller.selection().clone()),
        controller_ceiling: grant.controller_ceiling.clone(),
        admitted_ceiling: grant.admitted_ceiling.clone(),
        source_observations: Vec::new(),
        ingress_namespace: QualifiedIngressNamespace {
            realm: realm.clone(),
            ingress: ingress.clone(),
            occurrence_scope: request.delivery_work.clone(),
        },
        contract: ContractRequirements::local_governed_v1(),
    })
    .map_err(|error| error.to_string())
}

/// SHA-256 over length-delimited `parts`.
fn digest(parts: &[&[u8]]) -> EvidenceDigest {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update((part.len() as u64).to_be_bytes());
        hasher.update(part);
    }
    EvidenceDigest::from_array(hasher.finalize().into())
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
mod tests;
