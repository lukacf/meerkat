//! The native delivery seam: how the announcement owner reaches Meerkat's
//! continuation owner for a member.
//!
//! Batches are delivered as host continuations in the
//! [`ANNOUNCEMENT_CONTINUATION_NAMESPACE`] namespace, keyed by batch id.
//! Meerkat's continuation owner commits each into the member's durable
//! runtime inbox, binds it to the incarnation current at the first
//! submission, replays an identical submission with the original receipt,
//! refuses other content under the same key, and admits it as one input when
//! the member is served. The announcement owner never decides any of that; it
//! reads it.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Value, json};

use super::availability::{
    BlockCause, EvidenceClass, MemberDeparture, NotServedHereCause, SuspendCause,
};
use super::remote::RemoteMemberDelivery;
use super::sources::MemberFacts;
pub use super::types::MemberRoute;
use super::types::{
    Batch, Member, MemberIdentity, MemberRouteRecord, MobId, MobIncarnation, SubscriptionEnd,
};
pub use crate::runtime::keyed_inject::{AdmittedInput, NativeReceipt, NativeStatus};

/// Events read per page when a member's mob is checked for its end.
const MEMBER_MOB_EVENT_PAGE: usize = 256;

/// The continuation namespace announcement batches are delivered under.
pub const ANNOUNCEMENT_CONTINUATION_NAMESPACE: &str = "meta.mob_announcements";

/// A native read or preparation that could not be made now. Never evidence
/// that something did or did not happen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeUnavailable(pub String);

impl std::fmt::Display for NativeUnavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Why a submission returned no receipt, by what is known about the
/// commit.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum NativeSubmitError {
    /// Nothing was committed for this attempt, on this evidence. The batch
    /// is owed again and a later attempt may submit it: the cause is an
    /// availability state that can change, and it ends nothing.
    NotCommitted {
        cause: SuspendCause,
        evidence: EvidenceClass,
    },
    /// Whether it was committed is not known: only a status read settles
    /// it, and nothing is submitted again before one does.
    OutcomeUnknown {
        cause: SuspendCause,
        evidence: EvidenceClass,
        detail: String,
    },
    /// The delivery can never be submitted as it is.
    Blocked { cause: BlockCause, detail: String },
    /// The member departed for good: nothing was committed, and its
    /// subscriptions end.
    MemberGone(MemberDeparture),
}

impl std::fmt::Display for NativeSubmitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotCommitted { cause, evidence } => {
                write!(f, "nothing was committed: {cause} ({})", evidence.as_str())
            }
            Self::OutcomeUnknown {
                cause,
                evidence,
                detail,
            } => write!(
                f,
                "the outcome is unknown: {cause} ({}): {detail}",
                evidence.as_str()
            ),
            Self::Blocked { cause, detail } => f.write_str(&cause.with_detail(detail)),
            Self::MemberGone(departure) => write!(f, "the member departed: {departure}"),
        }
    }
}

/// Why a native status read gave no status.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum NativeStatusError {
    /// Not readable now, for this cause and on this evidence. Nothing is
    /// implied: the batch keeps waiting for a status.
    Unavailable {
        cause: SuspendCause,
        evidence: EvidenceClass,
        detail: String,
    },
    /// The read rules out any later attempt for the batch.
    Blocked { cause: BlockCause, detail: String },
    /// The member departed for good.
    MemberGone(MemberDeparture),
}

impl NativeStatusError {
    /// This gateway's own native owner could not be read now.
    pub(crate) fn local(detail: impl Into<String>) -> Self {
        Self::Unavailable {
            cause: SuspendCause::NativeUnavailable,
            evidence: EvidenceClass::Local,
            detail: detail.into(),
        }
    }
}

impl std::fmt::Display for NativeStatusError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable {
                cause,
                evidence,
                detail,
            } => write!(
                f,
                "the status is unavailable: {cause} ({}): {detail}",
                evidence.as_str()
            ),
            Self::Blocked { cause, detail } => f.write_str(&cause.with_detail(detail)),
            Self::MemberGone(departure) => write!(f, "the member departed: {departure}"),
        }
    }
}

/// Why a member cannot be subscribed.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum MemberProblem {
    /// Not an identity registered with this gateway.
    Unknown,
    /// Not a durable identity in its public form.
    Invalid(String),
    /// A member this gateway's runtime does not serve (placed on a host, or
    /// bound to an external runtime), or whose route contradicts it, so a
    /// delivery could never be admitted.
    Unusable(String),
    /// The peer gateway cannot be called authenticated.
    UnusablePeer(String),
    /// A read the check needs could not be made now.
    Unavailable(String),
    /// The member's mob id names a mob hosted here and a peer in the
    /// contact directory alike.
    Ambiguous(String),
}

/// Why a member could not be made ready for a delivery.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum PrepareError {
    /// A final fact: its subscriptions end for this reason. Built only from
    /// final facts, never from a registry miss or a placement.
    Gone(SubscriptionEnd),
    /// The member cannot take a delivery now, for this cause and on this
    /// evidence. Not final: the batch waits.
    Suspended {
        cause: SuspendCause,
        evidence: EvidenceClass,
    },
    /// Not now. Never evidence of anything.
    Unavailable(String),
}

impl std::fmt::Display for PrepareError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Gone(end) => write!(f, "the member is gone: {end}"),
            Self::Suspended { cause, evidence } => {
                write!(f, "the member waits: {cause} ({})", evidence.as_str())
            }
            Self::Unavailable(detail) => f.write_str(detail),
        }
    }
}

/// A mob hosted here other than this gateway's own.
enum ChildMob {
    Running(Box<meerkat_mob::MobHandle>),
    /// Restored and held until its owner resolves: it may run again.
    Held,
    /// Neither running nor held: destroyed, or never created here.
    Gone,
}

/// That a member's own mob, a mob a member created, ended after a cursor of
/// its event log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemberMobEnd {
    /// Subscriptions made before this cursor end. `u64::MAX` when the mob
    /// is gone or another incarnation: every subscription made in it ends.
    pub cursor: u64,
    pub why: String,
}

impl MemberMobEnd {
    fn gone(why: &str) -> Self {
        Self {
            cursor: u64::MAX,
            why: why.to_string(),
        }
    }
}

/// What a member's own mob, a mob a member created, did after a cursor of its
/// event log.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum MemberMobScan {
    /// It ended.
    Ended(MemberMobEnd),
    /// It runs, and neither completed nor reset after the cursor through
    /// `through`.
    Running { through: u64 },
}

/// Whether a mob named by an announcement or a subscription is hosted here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum MobPresence {
    /// Managed by this gateway and not destroyed, or restored and held
    /// until its owner resolves.
    Hosted,
    /// Not hosted here: destroyed, or never created here. A mob created
    /// later under the same id is another mob.
    Gone,
}

/// Who created a hosted mob, as Meerkat's durable records say.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum MobOrigin {
    /// A member of `parent`, roster id `creator`, created it: the mob's
    /// owner binding names a session whose own persisted member binding
    /// names that member, and the parent's member-creation journal records
    /// the same session for the same binding. `parent_incarnation` is the
    /// parent's incarnation then; `None` when the parent is this gateway's
    /// own mob.
    Member {
        parent: MobId,
        creator: String,
        parent_incarnation: Option<MobIncarnation>,
    },
    /// No member created it, or the records do not prove one: a final
    /// fact for this incarnation. The detail says which.
    Unproven(String),
    /// The parent the owner session names is not hosted here. Final only
    /// once every mob this gateway hosts has been restored.
    ParentNotHosted(MobId),
}

/// One hosted mob's incarnation and origin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MobOriginFacts {
    pub incarnation: MobIncarnation,
    pub origin: MobOrigin,
}

/// A member binding as Meerkat persists it: the mob, the member's role and
/// its roster id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BindingRecord {
    pub(crate) mob_id: String,
    pub(crate) role: String,
    pub(crate) member: String,
}

/// The parent a member binding names, as this gateway hosts it now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ParentRecord {
    /// Running: its member-creation journal entry for the owner session,
    /// as the session it records and the binding it records.
    Running {
        journal: Option<(String, BindingRecord)>,
        /// The parent's incarnation; `None` for this gateway's own mob.
        incarnation: Option<MobIncarnation>,
    },
    /// Restored and held until its owner resolves: it may run again.
    Held,
    /// Neither running nor held.
    NotHosted,
}

/// What the owner session's persisted metadata says about its member.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OwnerMetadata {
    /// No persisted metadata: an archived session reads so.
    Absent,
    /// Persisted, and bound to no member.
    Unbound,
    /// Persisted, and bound to this member.
    Bound(BindingRecord),
}

/// What Meerkat's durable records say about who created a hosted mob, each
/// read only when the one before it names something.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct OriginRecords {
    /// The mob's set-once owner binding: the session of the member that
    /// created it.
    pub(crate) owner_session: Option<String>,
    /// That session's persisted metadata, read when there is one.
    pub(crate) owner_metadata: Option<OwnerMetadata>,
    /// Where the parent that binding names is.
    pub(crate) parent: Option<ParentRecord>,
}

/// Who created a mob, from its records, or `Err` while that cannot be told
/// yet (its parent is held until its owner resolves). A member created it
/// only when the owner session's binding names it and the parent's journal
/// records that same session for the same mob, role and member.
pub(crate) fn origin_from(records: &OriginRecords) -> Result<MobOrigin, String> {
    let Some(owner) = &records.owner_session else {
        return Ok(MobOrigin::Unproven("no member created it".to_string()));
    };
    let binding = match &records.owner_metadata {
        // An archived owner session reads as absent: the window a mob first
        // seen after its creator respawned falls into. It fails closed.
        None | Some(OwnerMetadata::Absent) => {
            return Ok(MobOrigin::Unproven(
                "its owner session has no persisted metadata".to_string(),
            ));
        }
        Some(OwnerMetadata::Unbound) => {
            return Ok(MobOrigin::Unproven(
                "its owner session is bound to no member".to_string(),
            ));
        }
        Some(OwnerMetadata::Bound(binding)) => binding,
    };
    let parent = match MobId::new(binding.mob_id.clone()) {
        Ok(parent) => parent,
        Err(error) => {
            return Ok(MobOrigin::Unproven(format!(
                "its owner session names no valid mob: {error}"
            )));
        }
    };
    match &records.parent {
        Some(ParentRecord::Running {
            journal,
            incarnation,
        }) => {
            let agrees = journal
                .as_ref()
                .is_some_and(|(session, recorded)| session == owner && recorded == binding);
            if agrees {
                Ok(MobOrigin::Member {
                    parent,
                    creator: binding.member.clone(),
                    parent_incarnation: incarnation.clone(),
                })
            } else {
                Ok(MobOrigin::Unproven(
                    "the parent's member-creation journal does not record its owner session \
                     for that member"
                        .to_string(),
                ))
            }
        }
        Some(ParentRecord::Held) => Err(format!(
            "its parent {parent} is held until its owner resolves"
        )),
        Some(ParentRecord::NotHosted) | None => Ok(MobOrigin::ParentNotHosted(parent)),
    }
}

/// What the announcement owner needs from the hosts of its members.
#[async_trait]
pub trait NativeDelivery: Send + Sync {
    /// Whether this gateway's runtime admits work only through a native work
    /// authorization host, which refuses host continuations. Asked for
    /// members on this gateway only; a peer answers for its own runtime
    /// when a batch reaches it.
    fn governs_work_authority(&self) -> Result<bool, NativeUnavailable>;

    /// How the member would be reached if it were subscribed now: a member
    /// of this gateway's mob, of a mob a member created here (with that
    /// mob's incarnation), or of a mob on a peer gateway. The route a member
    /// was first subscribed with is kept by the store; this answers only for
    /// a member without one.
    async fn route_for(&self, member: &Member) -> Result<MemberRouteRecord, MemberProblem>;

    /// Whether each named mob is hosted here, from the mob owner's own
    /// records.
    async fn mob_presence(
        &self,
        mobs: &BTreeSet<MobId>,
    ) -> Result<BTreeMap<MobId, MobPresence>, NativeUnavailable>;

    /// Changes whenever the set of mobs this gateway manages changes: a mob
    /// was created, restored or destroyed. `None` when nothing signals it.
    fn mob_set_changes(&self) -> Option<tokio::sync::watch::Receiver<u64>> {
        None
    }

    /// The mobs hosted here other than this gateway's own: managed and not
    /// destroyed, or restored and held until their owner resolves.
    async fn hosted_mob_ids(&self) -> Result<BTreeSet<MobId>, NativeUnavailable> {
        Ok(BTreeSet::new())
    }

    /// Whether every mob this gateway hosts across a restart has been
    /// restored: from then on, a mob it does not host is gone for good.
    fn mobs_restored(&self) -> bool {
        true
    }

    /// This gateway's own mob: the root of the lineage, with no record.
    fn own_mob(&self) -> Option<MobId> {
        None
    }

    /// The incarnation of the hosted mob `mob_id` and who created it, from
    /// Meerkat's durable records. `Ok(None)` when it is not running here
    /// now: gone, or held until its owner resolves.
    async fn mob_origin(
        &self,
        _mob_id: &MobId,
    ) -> Result<Option<MobOriginFacts>, NativeUnavailable> {
        Ok(None)
    }

    /// Whether the member can be subscribed: an identity registered with
    /// this gateway and served by its runtime, or one on a peer gateway this
    /// gateway can call authenticated. A peer's roster is not read here;
    /// the peer refuses an unknown member when a batch reaches it.
    async fn check_member(&self, member: &Member, route: MemberRoute) -> Result<(), MemberProblem>;

    /// Make the member's current incarnation resolvable before a
    /// submission binds to it, or report the final fact that it never will
    /// be. Submits nothing. A peer prepares its own member when a batch
    /// reaches it.
    async fn prepare_member(&self, member: &Member, route: MemberRoute)
    -> Result<(), PrepareError>;

    /// Submit the batch's exact continuation for its member.
    async fn submit(
        &self,
        batch: &Batch,
        committed_at_ms: u64,
    ) -> Result<NativeReceipt, NativeSubmitError>;

    /// The authoritative status of the batch's key, read from its
    /// member's continuation owner. Serves nothing.
    async fn status(&self, batch: &Batch) -> Result<NativeStatus, NativeStatusError>;

    /// The native state of an input admitted for the batch's member, as
    /// that member's runtime reports it.
    async fn input(
        &self,
        batch: &Batch,
        session_id: &str,
        input_id: &str,
    ) -> Result<Option<Value>, NativeUnavailable>;

    /// The current cursor of the event log of a mob a member created: a
    /// subscription made now is made at it.
    async fn member_mob_cursor(&self, mob_id: &MobId) -> Result<u64, NativeUnavailable> {
        Err(NativeUnavailable(format!(
            "{mob_id} keeps no event log here"
        )))
    }

    /// Whether the mob a member was subscribed in (`incarnation` of
    /// `mob_id`) is gone for good: destroyed, or created again under the
    /// same id. Read once every mob hosted across a restart is restored.
    async fn member_mob_gone(
        &self,
        _mob_id: &MobId,
        _incarnation: Option<&MobIncarnation>,
    ) -> Result<Option<MemberMobEnd>, NativeUnavailable> {
        Ok(None)
    }

    /// What the mob a member was subscribed in did after `after`: gone (as
    /// [`Self::member_mob_gone`] reads it), or completed or reset per its
    /// own durable event log, or neither through the cursor read.
    async fn member_mob_end(
        &self,
        mob_id: &MobId,
        incarnation: Option<&MobIncarnation>,
        after: u64,
    ) -> Result<MemberMobScan, NativeUnavailable> {
        Ok(match self.member_mob_gone(mob_id, incarnation).await? {
            Some(end) => MemberMobScan::Ended(end),
            None => MemberMobScan::Running { through: after },
        })
    }

    /// What `member`'s mob roster records about it: its role (the profile it
    /// was spawned from) and its labels, the facts role and label grants are
    /// decided over. `None` when this gateway holds no roster for the
    /// member's mob or the roster names no such member: then only a member
    /// grant can cover it.
    async fn member_facts(
        &self,
        member: &Member,
    ) -> Result<Option<MemberFacts>, NativeUnavailable> {
        let _ = member;
        Ok(None)
    }

    /// The members `mob_id`'s roster records with `role` (the profile they
    /// were spawned from), as [`Self::member_facts`] reads each one: the
    /// members a role-selected subscription takes now. Empty when this
    /// gateway holds no roster for the mob.
    async fn members_with_role(
        &self,
        mob_id: &MobId,
        role: &str,
    ) -> Result<Vec<Member>, NativeUnavailable> {
        let _ = (mob_id, role);
        Ok(Vec::new())
    }
}

/// The summary a batch's continuation carries: how many distinct
/// announcements it holds.
pub fn batch_summary(batch: &Batch) -> String {
    let distinct = batch
        .items
        .iter()
        .map(|item| (item.mob_id.as_str(), item.sequence))
        .collect::<std::collections::BTreeSet<_>>()
        .len();
    format!(
        "{distinct} announcement{}",
        if distinct == 1 { "" } else { "s" }
    )
}

/// The exact continuation a delivery is submitted as under the native `key`,
/// from the batch's immutable fields alone, so every attempt submits
/// identical content. A local delivery's key is its batch id.
pub fn continuation_delivery(
    key: &str,
    batch_id: &str,
    content_digest: &str,
    summary: &str,
    body: &str,
) -> Result<meerkat::ContinuationDelivery, String> {
    let key = meerkat::ContinuationKey::new(key).map_err(|error| error.to_string())?;
    Ok(meerkat::ContinuationDelivery {
        key,
        result: meerkat::ContinuationResultRef {
            producer: meerkat::ContinuationProducer::Host {
                namespace: ANNOUNCEMENT_CONTINUATION_NAMESPACE.to_string(),
            },
            producer_id: batch_id.to_string(),
            result_digest: content_digest.to_string(),
            summary: Some(summary.to_string()),
        },
        body: meerkat::ContinuationBody::from(body.to_string()),
        handling: meerkat::ContinuationHandling::Queue,
    })
}

/// The exact continuation a batch is delivered as.
pub fn continuation_for(batch: &Batch) -> Result<meerkat::ContinuationDelivery, String> {
    continuation_delivery(
        batch.id.as_str(),
        batch.id.as_str(),
        &batch.content_digest,
        &batch_summary(batch),
        &batch.body,
    )
}

/// Why this gateway's runtime names no continuation owner for a batch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OwnerRefusal {
    /// The member is on a peer gateway: its batches go through that peer.
    NoPeerPath,
    /// The batch's route is local, but its member names another mob than
    /// this gateway's. The record contradicts itself, and no member of this
    /// gateway's mob is delivered to in that member's place.
    LocalRouteNamesAnotherMob { member: Member, own: MobId },
    /// The batch's route is local, but this gateway names no mob of its
    /// own, so the route cannot be shown to name the member's mob: it is
    /// refused as one that contradicts itself.
    LocalRouteWithoutOwnMob { member: Member },
}

impl std::fmt::Display for OwnerRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoPeerPath => f.write_str("this gateway has no delivery path to peer members"),
            Self::LocalRouteNamesAnotherMob { member, own } => write!(
                f,
                "the batch for {member} is routed to this gateway's mob {own}, \
                 which is not the member's"
            ),
            Self::LocalRouteWithoutOwnMob { member } => write!(
                f,
                "the batch for {member} is routed to this gateway, which names no mob of its own"
            ),
        }
    }
}

/// The one guard on a member's kept route: a local route names a member of
/// this gateway's own mob, `own`. Run before anything is prepared, ended,
/// submitted or read for the member, so no member of this gateway's mob is
/// touched in another mob's member's place. A gateway that names no mob of
/// its own has no local route that holds: the guard fails closed.
pub(crate) fn route_contradiction(
    own: Option<&MobId>,
    member: &Member,
    route: MemberRoute,
) -> Option<OwnerRefusal> {
    if route != MemberRoute::Local {
        return None;
    }
    match own {
        Some(own) if member.mob_id == *own => None,
        Some(own) => Some(OwnerRefusal::LocalRouteNamesAnotherMob {
            member: member.clone(),
            own: own.clone(),
        }),
        None => Some(OwnerRefusal::LocalRouteWithoutOwnMob {
            member: member.clone(),
        }),
    }
}

impl OwnerRefusal {
    /// Whether the refusal rules the batch out: a self-contradicting route is
    /// never deliverable, while a member on a peer is only not this seam's to
    /// reach now. The one reading a submission, a status read and a put use.
    fn rules_out(&self) -> bool {
        matches!(
            self,
            Self::LocalRouteNamesAnotherMob { .. } | Self::LocalRouteWithoutOwnMob { .. }
        )
    }

    /// What a submission meets for this refusal. Nothing was submitted.
    pub(crate) fn submit_error(&self) -> NativeSubmitError {
        if self.rules_out() {
            NativeSubmitError::Blocked {
                cause: BlockCause::NativeInvalid,
                detail: self.to_string(),
            }
        } else {
            NativeSubmitError::NotCommitted {
                cause: SuspendCause::NativeUnavailable,
                evidence: EvidenceClass::Local,
            }
        }
    }

    /// What a status read meets for this refusal. Nothing was read.
    pub(crate) fn status_error(&self) -> NativeStatusError {
        if self.rules_out() {
            NativeStatusError::Blocked {
                cause: BlockCause::NativeInvalid,
                detail: self.to_string(),
            }
        } else {
            NativeStatusError::local(self.to_string())
        }
    }

    /// What a put meets for this refusal: a member whose route contradicts
    /// it could never be delivered to, and a member on a peer is not this
    /// seam's to reach.
    pub(crate) fn member_problem(&self) -> MemberProblem {
        if self.rules_out() {
            MemberProblem::Unusable(self.to_string())
        } else {
            MemberProblem::UnusablePeer(self.to_string())
        }
    }
}

/// The continuation owner of `member` by its kept route, on the gateway
/// whose own mob is `own`. A member of `own` is named by its roster id, a
/// member of a mob a member created by its plain roster id, and a local
/// route always names the member's own mob.
pub(crate) fn continuation_owner(
    own: &MobId,
    member: &Member,
    route: MemberRoute,
) -> Result<meerkat::ContinuationOwner, OwnerRefusal> {
    if let Some(refusal) = route_contradiction(Some(own), member, route) {
        return Err(refusal);
    }
    match route {
        MemberRoute::Local => Ok(meerkat::ContinuationOwner::Member {
            mob_id: member.mob_id.to_string(),
            identity: crate::member_comms_id::roster_member_id_for_identity(
                member.identity.as_str(),
            )
            .to_string(),
        }),
        MemberRoute::Child => Ok(meerkat::ContinuationOwner::Member {
            mob_id: member.mob_id.to_string(),
            identity: member.identity.to_string(),
        }),
        MemberRoute::Peer => Err(OwnerRefusal::NoPeerPath),
    }
}

fn native_receipt(receipt: &meerkat::ContinuationReceipt) -> NativeReceipt {
    NativeReceipt {
        delivery_id: receipt.delivery_id.clone(),
        address: receipt.address.clone(),
        submission_digest: receipt.submission_digest.clone(),
        committed_at_ms: receipt.committed_at_ms,
    }
}

fn reason_text(reason: meerkat_runtime::RuntimeDeliveryRefusalReason) -> String {
    serde_json::to_value(reason)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_else(|| format!("{reason:?}"))
}

fn native_status(status: meerkat::ContinuationStatus) -> NativeStatus {
    match status {
        meerkat::ContinuationStatus::NotCommitted => NativeStatus::NotCommitted,
        meerkat::ContinuationStatus::Pending {
            receipt, admitted, ..
        } => NativeStatus::Pending {
            receipt: native_receipt(&receipt),
            admitted: admitted.map(|(session, input)| AdmittedInput {
                session_id: session.to_string(),
                input_id: input.to_string(),
            }),
        },
        meerkat::ContinuationStatus::Applied {
            receipt,
            session,
            input,
        } => NativeStatus::Applied {
            receipt: native_receipt(&receipt),
            session_id: session.to_string(),
            input_id: input.to_string(),
        },
        meerkat::ContinuationStatus::Stranded { receipt, .. } => NativeStatus::Stranded {
            receipt: native_receipt(&receipt),
        },
        meerkat::ContinuationStatus::Refused {
            receipt, reason, ..
        } => NativeStatus::Refused {
            receipt: native_receipt(&receipt),
            reason: reason_text(reason),
        },
    }
}

/// The agent mob tool state's listings of the mobs hosted here. A seam:
/// which mobs a reader finds depends on the order it reads them in.
#[async_trait]
pub(crate) trait HostedListings: Send + Sync {
    /// A running mob's handle.
    type Handle: Send;

    /// Every managed mob with its state. Fails until the restore of
    /// persisted mobs is done.
    async fn mob_list(
        &self,
    ) -> Result<Vec<(meerkat_mob::MobId, meerkat_mob::MobState)>, meerkat_mob::MobError>;

    /// The restored mobs held until their owner resolves. Empty when the
    /// restore fails.
    async fn held_child_mobs(&self) -> Vec<(meerkat_mob::MobId, meerkat_core::types::SessionId)>;

    /// The handles of the managed mobs not destroyed. Fails until the
    /// restore is done.
    async fn mob_handles_snapshot(
        &self,
    ) -> Result<Vec<(meerkat_mob::MobId, Self::Handle)>, meerkat_mob::MobError>;
}

#[async_trait]
impl HostedListings for meerkat_mob_mcp::MobMcpState {
    type Handle = meerkat_mob::MobHandle;

    /// Meerkat lists each mob with its lifecycle observation. The hosted set
    /// needs only its phase: a mob whose actor has stopped is still hosted
    /// here, and a restart recovers it.
    async fn mob_list(
        &self,
    ) -> Result<Vec<(meerkat_mob::MobId, meerkat_mob::MobState)>, meerkat_mob::MobError> {
        Ok(meerkat_mob_mcp::MobMcpState::mob_list(self)
            .await?
            .into_iter()
            .map(|(mob_id, observation)| (mob_id, observation.phase))
            .collect())
    }

    async fn held_child_mobs(&self) -> Vec<(meerkat_mob::MobId, meerkat_core::types::SessionId)> {
        meerkat_mob_mcp::MobMcpState::held_child_mobs(self).await
    }

    async fn mob_handles_snapshot(
        &self,
    ) -> Result<Vec<(meerkat_mob::MobId, meerkat_mob::MobHandle)>, meerkat_mob::MobError> {
        meerkat_mob_mcp::MobMcpState::mob_handles_snapshot(self).await
    }
}

/// Every mob hosted here, this gateway's own included: managed and not
/// destroyed, or restored and held until its owner resolves.
///
/// The managed listing is read first only to finish the restore of
/// persisted mobs: it fails rather than answer before the restore, where
/// the held listing answers empty. Held mobs are then read before managed
/// ones, because activation adds a held mob to the managed ones before it
/// drops the hold: a mob that activates between the two reads is in one of
/// them.
pub(crate) async fn hosted_set<L: HostedListings>(
    listings: &L,
) -> Result<BTreeSet<String>, NativeUnavailable> {
    managed_listing(listings).await?;
    let held = held_listing(listings).await;
    let managed = managed_listing(listings)
        .await?
        .into_iter()
        .filter(|(_, state)| *state != meerkat_mob::MobState::Destroyed)
        .map(|(mob_id, _)| mob_id.as_str().to_string());
    Ok(held.into_iter().chain(managed).collect())
}

/// The held mobs and the running mobs' handles, read in the order
/// [`hosted_set`] reads them in: the restore finished, then held, then
/// running. A mob in neither is not one still being restored, nor one
/// about to run.
pub(crate) async fn hosted_handles<L: HostedListings>(
    listings: &L,
) -> Result<(BTreeSet<String>, Vec<(meerkat_mob::MobId, L::Handle)>), NativeUnavailable> {
    managed_listing(listings).await?;
    let held = held_listing(listings).await;
    let running = listings
        .mob_handles_snapshot()
        .await
        .map_err(|error| NativeUnavailable(error.to_string()))?;
    Ok((held, running))
}

/// The mobs hosted here other than `own`. An id this store cannot hold is
/// skipped, so the others stay readable, and warned about once: `warned`
/// keeps the ids already reported.
pub(crate) async fn hosted_mob_ids_in<L: HostedListings>(
    listings: &L,
    own: &MobId,
    warned: &std::sync::Mutex<BTreeSet<String>>,
) -> Result<BTreeSet<MobId>, NativeUnavailable> {
    Ok(hosted_set(listings)
        .await?
        .into_iter()
        .filter(|mob_id| mob_id != own.as_str())
        .filter_map(|mob_id| match MobId::new(mob_id.clone()) {
            Ok(mob_id) => Some(mob_id),
            Err(error) => {
                let first = warned
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .insert(mob_id.clone());
                if first {
                    tracing::warn!(mob = %mob_id, %error, "hosted mob id not usable here");
                }
                None
            }
        })
        .collect())
}

/// The managed mobs, once the restore of persisted mobs is finished.
async fn managed_listing<L: HostedListings>(
    listings: &L,
) -> Result<Vec<(meerkat_mob::MobId, meerkat_mob::MobState)>, NativeUnavailable> {
    listings
        .mob_list()
        .await
        .map_err(|error| NativeUnavailable(error.to_string()))
}

/// The ids of the restored mobs held until their owner resolves.
async fn held_listing<L: HostedListings>(listings: &L) -> BTreeSet<String> {
    listings
        .held_child_mobs()
        .await
        .into_iter()
        .map(|(mob_id, _)| mob_id.as_str().to_string())
        .collect()
}

/// The production seam over this gateway's mob runtime.
///
/// It holds the composition's non-owning identity runtime, the agent mob
/// tool state (whose continuation service is resolved on every call, so a
/// rebound state is followed) and the runtime machine. None of them keeps
/// the mob publicly owned.
pub struct MobNativeDelivery {
    mob_id: MobId,
    identities: Arc<crate::identity_first::IdentityRuntime>,
    mob_tools: Arc<meerkat_mob_mcp::MobMcpState>,
    runtime: Arc<meerkat_runtime::MeerkatMachine>,
    sessions: Arc<dyn meerkat_mob::MobSessionService>,
    /// Hosted mob ids this store cannot hold, warned about once each.
    unusable_ids: std::sync::Mutex<BTreeSet<String>>,
}

impl MobNativeDelivery {
    pub(crate) fn new(
        mob_id: MobId,
        identities: Arc<crate::identity_first::IdentityRuntime>,
        mob_tools: Arc<meerkat_mob_mcp::MobMcpState>,
        runtime: Arc<meerkat_runtime::MeerkatMachine>,
        sessions: Arc<dyn meerkat_mob::MobSessionService>,
    ) -> Self {
        Self {
            mob_id,
            identities,
            mob_tools,
            runtime,
            sessions,
            unusable_ids: std::sync::Mutex::new(BTreeSet::new()),
        }
    }

    fn service(&self) -> Result<Arc<meerkat::ContinuationOwnerService>, NativeUnavailable> {
        self.mob_tools.continuation_service().ok_or_else(|| {
            NativeUnavailable("the mob's continuation service is not bound".to_string())
        })
    }

    fn owner(&self, identity: &MemberIdentity) -> meerkat::ContinuationOwner {
        meerkat::ContinuationOwner::Member {
            mob_id: self.mob_id.to_string(),
            identity: crate::member_comms_id::roster_member_id_for_identity(identity.as_str())
                .to_string(),
        }
    }

    fn agent_identity(
        identity: &MemberIdentity,
    ) -> Result<crate::identity_first::AgentIdentity, NativeUnavailable> {
        crate::identity_first::AgentIdentity::parse(identity.as_str())
            .map_err(|error| NativeUnavailable(error.to_string()))
    }

    /// Whether this gateway's runtime refuses host continuations.
    pub fn governs_work_authority(&self) -> Result<bool, NativeUnavailable> {
        Ok(self.service()?.governs_work_authority())
    }

    /// Whether the identity is registered with this gateway, and if so
    /// whether this gateway's runtime serves it.
    pub async fn identity_service(
        &self,
        identity: &MemberIdentity,
    ) -> Result<IdentityService, NativeUnavailable> {
        let identity = Self::agent_identity(identity)?;
        match self.identities.not_served_here(&identity).await {
            Ok(None) => Ok(IdentityService::Local),
            Ok(Some(crate::identity_first::NotServedHere::PlacedOnHost)) => {
                Ok(IdentityService::Elsewhere(NotServedHereCause::PlacedOnHost))
            }
            Ok(Some(crate::identity_first::NotServedHere::ExternalBinding)) => Ok(
                IdentityService::Elsewhere(NotServedHereCause::ExternalBinding),
            ),
            Err(crate::identity_first::IdentityRuntimeError::UnknownIdentity(_)) => {
                Ok(IdentityService::Unknown)
            }
            Err(error) => Err(NativeUnavailable(error.to_string())),
        }
    }

    /// Make the identity's current incarnation resolvable. An identity not
    /// registered here, or one this runtime does not serve, waits: a miss or
    /// a placement can change, so neither ends anything.
    pub async fn prepare_identity(&self, identity: &MemberIdentity) -> Result<(), PrepareError> {
        match self
            .identity_service(identity)
            .await
            .map_err(|error| PrepareError::Unavailable(error.0))?
        {
            IdentityService::Unknown => {
                return Err(PrepareError::Suspended {
                    cause: SuspendCause::MemberUnknown,
                    evidence: EvidenceClass::Local,
                });
            }
            IdentityService::Elsewhere(cause) => {
                return Err(PrepareError::Suspended {
                    cause: SuspendCause::MemberNotServedHere { cause: Some(cause) },
                    evidence: EvidenceClass::Local,
                });
            }
            IdentityService::Local => {}
        }
        let identity =
            Self::agent_identity(identity).map_err(|error| PrepareError::Unavailable(error.0))?;
        // Tracked, so a caller that stops waiting (a keyed inject past its
        // deadline) drops only the wait: the materialization runs to its end
        // under the identity runtime's foreground owner, and the member is
        // ready for the next attempt.
        self.identities
            .materialize_tracked(&identity)
            .await
            .map(|_| ())
            .map_err(|error| PrepareError::Unavailable(error.to_string()))
    }

    /// Whether each named mob is hosted here: managed and not destroyed, or
    /// restored and held until its owner resolves.
    pub async fn hosted_mobs(
        &self,
        mobs: &BTreeSet<MobId>,
    ) -> Result<BTreeMap<MobId, MobPresence>, NativeUnavailable> {
        let hosted = hosted_set(&*self.mob_tools).await?;
        Ok(mobs
            .iter()
            .map(|mob_id| {
                let presence = if hosted.contains(mob_id.as_str()) {
                    MobPresence::Hosted
                } else {
                    MobPresence::Gone
                };
                (mob_id.clone(), presence)
            })
            .collect())
    }

    /// The mobs hosted here other than this gateway's own.
    ///
    /// Read through the agent mob tool state's own listings, which finish
    /// its restore of persisted child mobs before they answer, so a mob
    /// missing here is not one still being restored.
    pub async fn hosted_mob_ids(&self) -> Result<BTreeSet<MobId>, NativeUnavailable> {
        hosted_mob_ids_in(&*self.mob_tools, &self.mob_id, &self.unusable_ids).await
    }

    /// The incarnation of a running hosted mob and who created it, from
    /// three durable records: the mob's set-once owner binding, that
    /// session's persisted member binding, and the parent mob's
    /// member-creation journal for that session. The live roster is not
    /// read: the relation must hold across a repoint of the creator.
    pub async fn mob_origin(
        &self,
        mob_id: &MobId,
    ) -> Result<Option<MobOriginFacts>, NativeUnavailable> {
        let unavailable = |error: &dyn std::fmt::Display| NativeUnavailable(error.to_string());
        // A parent in neither listing is not one still being restored, nor
        // one about to run.
        let (held, handles) = hosted_handles(&*self.mob_tools).await?;
        let Some(handle) = handles
            .iter()
            .find(|(id, _)| id.as_str() == mob_id.as_str())
            .map(|(_, handle)| handle.clone())
        else {
            return Ok(None);
        };
        let incarnation = Self::incarnation_of(&handle, mob_id).await?;
        let owner = handle
            .owner_bridge_session_lifecycle_authority()
            .map(|authority| authority.bridge_session_id);
        let mut records = OriginRecords {
            owner_session: owner.as_ref().map(ToString::to_string),
            ..OriginRecords::default()
        };
        if let Some(owner) = &owner {
            let metadata = match self
                .sessions
                .load_persisted_session_metadata(owner)
                .await
                .map_err(|error| unavailable(&error))?
                .filter(|view| view.session_id == *owner)
            {
                None => OwnerMetadata::Absent,
                Some(view) => match view
                    .session_metadata
                    .and_then(|metadata| metadata.mob_member_binding)
                {
                    None => OwnerMetadata::Unbound,
                    Some(binding) => OwnerMetadata::Bound(BindingRecord {
                        mob_id: binding.mob_id,
                        role: binding.role,
                        member: binding.member,
                    }),
                },
            };
            if let OwnerMetadata::Bound(binding) = &metadata {
                let parent = handles
                    .iter()
                    .find(|(id, _)| id.as_str() == binding.mob_id)
                    .map(|(_, handle)| handle.clone());
                records.parent = Some(match parent {
                    Some(parent) => ParentRecord::Running {
                        // The root has no incarnation on its edges, and an id
                        // this store cannot hold makes a record with no
                        // parent (origin_from), so neither reads one.
                        incarnation: match MobId::new(binding.mob_id.clone()) {
                            Ok(parent_id) if parent_id != self.mob_id => {
                                Some(Self::incarnation_of(&parent, &parent_id).await?)
                            }
                            _ => None,
                        },
                        journal: parent
                            .read_handle()
                            .member_creation_for_session(owner)
                            .await
                            .map_err(|error| unavailable(&error))?
                            .map(|snapshot| {
                                (
                                    snapshot.session_id.to_string(),
                                    BindingRecord {
                                        mob_id: snapshot.member_binding.mob_id,
                                        role: snapshot.member_binding.role,
                                        member: snapshot.member_binding.member,
                                    },
                                )
                            }),
                    },
                    None if held.contains(&binding.mob_id) => ParentRecord::Held,
                    None => ParentRecord::NotHosted,
                });
            }
            records.owner_metadata = Some(metadata);
        }
        let origin = origin_from(&records).map_err(NativeUnavailable)?;
        Ok(Some(MobOriginFacts {
            incarnation,
            origin,
        }))
    }

    /// One running mob's incarnation: its owner binding and the time of the
    /// first event in its own log.
    async fn incarnation_of(
        handle: &meerkat_mob::MobHandle,
        mob_id: &MobId,
    ) -> Result<MobIncarnation, NativeUnavailable> {
        let first_event_at_ms = handle
            .events()
            .poll(0, 1)
            .await
            .map_err(|error| NativeUnavailable(error.to_string()))?
            .first()
            .map(|event| u64::try_from(event.timestamp.timestamp_millis()).unwrap_or(0))
            .ok_or_else(|| NativeUnavailable(format!("mob {mob_id} has no events yet")))?;
        Ok(MobIncarnation {
            owner_session: handle
                .owner_bridge_session_lifecycle_authority()
                .map(|authority| authority.bridge_session_id.to_string()),
            first_event_at_ms,
        })
    }

    /// A mob hosted here other than this gateway's own: running, held until
    /// its owner resolves, or gone.
    ///
    /// Read through [`hosted_handles`]: a mob that is neither held nor
    /// running is not one still being restored, nor one about to run.
    async fn child_mob(&self, mob_id: &MobId) -> Result<ChildMob, NativeUnavailable> {
        let (held, running) = hosted_handles(&*self.mob_tools).await?;
        let handle = running
            .into_iter()
            .find(|(id, _)| *mob_id != self.mob_id && id.as_str() == mob_id.as_str())
            .map(|(_, handle)| handle);
        Ok(match handle {
            Some(handle) => ChildMob::Running(Box::new(handle)),
            None if held.contains(mob_id.as_str()) => ChildMob::Held,
            None => ChildMob::Gone,
        })
    }

    /// The roster facts of a member of this gateway's mob (its registered
    /// row) or of a mob hosted here a member created (its roster entry).
    pub async fn member_facts(
        &self,
        member: &Member,
    ) -> Result<Option<MemberFacts>, NativeUnavailable> {
        if member.mob_id == self.mob_id {
            let identity = Self::agent_identity(&member.identity)?;
            return match self.identities.registered_spec(&identity).await {
                Ok(registered) => Ok(Some(MemberFacts::new(
                    registered.spec.profile.as_str(),
                    registered.spec.labels,
                ))),
                Err(crate::identity_first::IdentityRuntimeError::UnknownIdentity(_)) => Ok(None),
                Err(error) => Err(NativeUnavailable(error.to_string())),
            };
        }
        let Some(handle) = self.running_child(&member.mob_id).await? else {
            return Ok(None);
        };
        match handle
            .get_member(&meerkat_mob::AgentIdentity::from(member.identity.as_str()))
            .await
        {
            Ok(Some(entry)) => Ok(Some(MemberFacts::new(entry.role.as_str(), entry.labels))),
            Ok(None) => Ok(None),
            Err(error) => Err(NativeUnavailable(error.to_string())),
        }
    }

    /// The members of this gateway's mob whose registered row names the
    /// profile `role`, or of a mob hosted here a member created whose roster
    /// entry has the role: the members a role template takes now. None for
    /// a mob that is gone; unavailable while it is held.
    pub async fn members_with_role(
        &self,
        mob_id: &MobId,
        role: &str,
    ) -> Result<Vec<Member>, NativeUnavailable> {
        let identities: Vec<String> = if *mob_id == self.mob_id {
            self.identities
                .registered_with_profile(role)
                .await
                .into_iter()
                .map(|identity| identity.as_str().to_string())
                .collect()
        } else {
            let Some(handle) = self.running_child(mob_id).await? else {
                return Ok(Vec::new());
            };
            handle
                .list_members()
                .await
                .into_iter()
                .filter(|entry| entry.role.as_str() == role)
                .map(|entry| entry.agent_identity.as_str().to_string())
                .collect()
        };
        Ok(identities
            .into_iter()
            .filter_map(|identity| match MemberIdentity::new(identity) {
                Ok(identity) => Some(Member {
                    mob_id: mob_id.clone(),
                    identity,
                }),
                Err(error) => {
                    tracing::warn!(mob = %mob_id, %error, "a roster member has no usable identity");
                    None
                }
            })
            .collect())
    }

    /// The running handle of a mob hosted here other than this gateway's
    /// own; `None` when it is gone; unavailable while it is held.
    async fn running_child(
        &self,
        mob_id: &MobId,
    ) -> Result<Option<meerkat_mob::MobHandle>, NativeUnavailable> {
        match self.child_mob(mob_id).await? {
            ChildMob::Running(handle) => Ok(Some(*handle)),
            ChildMob::Held => Err(NativeUnavailable(format!(
                "{mob_id} is held until its owner resolves"
            ))),
            ChildMob::Gone => Ok(None),
        }
    }

    /// The incarnation of a running mob hosted here other than this
    /// gateway's own, or `None` when it is gone. Unavailable while the mob
    /// is held until its owner resolves.
    pub async fn child_incarnation(
        &self,
        mob_id: &MobId,
    ) -> Result<Option<MobIncarnation>, NativeUnavailable> {
        match self.running_child(mob_id).await? {
            Some(handle) => Ok(Some(Self::incarnation_of(&handle, mob_id).await?)),
            None => Ok(None),
        }
    }

    /// What Meerkat records for a call from `session` that is not on this
    /// gateway's own roster: the member binding persisted with the session,
    /// and that member's roster entry in its mob now. Read only from durable
    /// session metadata and the mob's own roster.
    pub(crate) async fn child_caller_records(
        &self,
        session: &meerkat_core::types::SessionId,
    ) -> Result<super::tools::ChildCallerRecords, String> {
        let binding = self
            .sessions
            .load_persisted_session_metadata(session)
            .await
            .map_err(|error| format!("the calling session's records cannot be read: {error}"))?
            .filter(|view| view.session_id == *session)
            .and_then(|view| view.session_metadata)
            .and_then(|metadata| metadata.mob_member_binding);
        let Some(binding) = binding else {
            return Ok(super::tools::ChildCallerRecords::default());
        };
        let mob_id = MobId::new(binding.mob_id).map_err(|error| {
            format!("the calling session's member binding names no usable mob: {error}")
        })?;
        let member = MemberIdentity::new(binding.member).map_err(|error| {
            format!("the calling session's member binding names no usable member: {error}")
        })?;
        let entry = if mob_id == self.mob_id {
            None
        } else {
            match self.running_child(&mob_id).await.map_err(|error| error.0)? {
                Some(handle) => handle
                    .get_member(&meerkat_mob::AgentIdentity::from(member.as_str()))
                    .await
                    .map_err(|error| error.to_string())?
                    .map(|entry| super::tools::RosterBinding {
                        session: entry.bridge_session_id().cloned(),
                        role: entry.role,
                    }),
                None => None,
            }
        };
        Ok(super::tools::ChildCallerRecords {
            binding: Some(super::tools::SessionMemberBinding {
                mob_id,
                role: meerkat_mob::ProfileName::from(binding.role),
                member,
            }),
            entry,
        })
    }

    /// A member of a mob a member created can be subscribed while that mob
    /// runs and has not completed, and the member is on its roster now. Its
    /// absence later ends nothing: a respawn takes it off for a while. A mob
    /// whose actor has stopped admits nothing until a restart recovers it.
    async fn check_child_member(&self, member: &Member) -> Result<(), MemberProblem> {
        let Some(handle) = self
            .running_child(&member.mob_id)
            .await
            .map_err(|error| MemberProblem::Unavailable(error.0))?
        else {
            return Err(MemberProblem::Unusable(format!(
                "{} does not run here",
                member.mob_id
            )));
        };
        let observation = handle.lifecycle_observation();
        if matches!(
            observation.phase,
            meerkat_mob::MobState::Completed | meerkat_mob::MobState::Destroyed
        ) {
            return Err(MemberProblem::Unusable(format!(
                "{} has completed",
                member.mob_id
            )));
        }
        if observation.failure.is_some() {
            return Err(MemberProblem::Unusable(format!(
                "{} has stopped",
                member.mob_id
            )));
        }
        match handle
            .get_member(&meerkat_mob::AgentIdentity::from(member.identity.as_str()))
            .await
        {
            Ok(Some(_)) => Ok(()),
            Ok(None) => Err(MemberProblem::Unknown),
            Err(error) => Err(MemberProblem::Unavailable(error.to_string())),
        }
    }

    /// A member of a mob a member created is ready while that mob runs here;
    /// once it is gone for good the member's subscriptions end. A held mob
    /// may run again: its members wait.
    async fn prepare_child_member(&self, member: &Member) -> Result<(), PrepareError> {
        match self.running_child(&member.mob_id).await {
            Ok(Some(_)) => Ok(()),
            Ok(None) => Err(PrepareError::Gone(SubscriptionEnd::MemberMobEnded {
                why: "destroyed".to_string(),
            })),
            Err(error) => Err(PrepareError::Unavailable(error.0)),
        }
    }

    /// The continuation owner of a member by its kept route.
    fn owner_for(
        &self,
        member: &Member,
        route: MemberRoute,
    ) -> Result<meerkat::ContinuationOwner, OwnerRefusal> {
        continuation_owner(&self.mob_id, member, route)
    }

    /// The current cursor of a running child mob's event log.
    pub async fn member_mob_cursor(&self, mob_id: &MobId) -> Result<u64, NativeUnavailable> {
        let handle = self
            .running_child(mob_id)
            .await?
            .ok_or_else(|| NativeUnavailable(format!("{mob_id} does not run here")))?;
        handle
            .events()
            .latest_cursor()
            .await
            .map_err(|error| NativeUnavailable(error.to_string()))
    }

    /// The running handle of the mob a member was subscribed in, or the
    /// final fact that it is gone: destroyed (neither running nor held once
    /// every mob is restored), or created again (another incarnation under
    /// the same id). Unavailable while it is held until its owner resolves.
    async fn member_mob_handle(
        &self,
        mob_id: &MobId,
        incarnation: Option<&MobIncarnation>,
    ) -> Result<Result<meerkat_mob::MobHandle, MemberMobEnd>, NativeUnavailable> {
        let Some(handle) = self.running_child(mob_id).await? else {
            return Ok(Err(MemberMobEnd::gone("destroyed")));
        };
        if let Some(subscribed_in) = incarnation
            && Self::incarnation_of(&handle, mob_id).await? != *subscribed_in
        {
            return Ok(Err(MemberMobEnd::gone("created again")));
        }
        Ok(Ok(handle))
    }

    /// Whether the mob a member was subscribed in is gone for good.
    pub async fn member_mob_gone(
        &self,
        mob_id: &MobId,
        incarnation: Option<&MobIncarnation>,
    ) -> Result<Option<MemberMobEnd>, NativeUnavailable> {
        Ok(self.member_mob_handle(mob_id, incarnation).await?.err())
    }

    /// What the mob a member was subscribed in did after `after`, from its
    /// own durable records: gone, or the first completion or reset in its
    /// event log after `after`, or neither through the last event read.
    pub async fn member_mob_end(
        &self,
        mob_id: &MobId,
        incarnation: Option<&MobIncarnation>,
        after: u64,
    ) -> Result<MemberMobScan, NativeUnavailable> {
        let handle = match self.member_mob_handle(mob_id, incarnation).await? {
            Ok(handle) => handle,
            Err(end) => return Ok(MemberMobScan::Ended(end)),
        };
        let view = handle.events();
        let mut through = after;
        loop {
            let page = view
                .poll(through, MEMBER_MOB_EVENT_PAGE)
                .await
                .map_err(|error| NativeUnavailable(error.to_string()))?;
            for event in &page {
                let why = match event.kind {
                    meerkat_mob::event::MobEventKind::MobCompleted => "completed",
                    meerkat_mob::event::MobEventKind::MobReset => "reset",
                    _ => continue,
                };
                return Ok(MemberMobScan::Ended(MemberMobEnd {
                    cursor: event.cursor,
                    why: why.to_string(),
                }));
            }
            match page.last() {
                Some(last) if last.cursor > through => through = last.cursor,
                _ => return Ok(MemberMobScan::Running { through }),
            }
            if page.len() < MEMBER_MOB_EVENT_PAGE {
                return Ok(MemberMobScan::Running { through });
            }
        }
    }

    /// Submit one continuation for the identity.
    pub async fn submit_delivery(
        &self,
        identity: &MemberIdentity,
        delivery: meerkat::ContinuationDelivery,
        committed_at_ms: u64,
    ) -> Result<NativeReceipt, NativeSubmitError> {
        self.submit_delivery_to(&self.owner(identity), delivery, committed_at_ms)
            .await
    }

    /// Submit one continuation for a continuation owner.
    async fn submit_delivery_to(
        &self,
        owner: &meerkat::ContinuationOwner,
        delivery: meerkat::ContinuationDelivery,
        committed_at_ms: u64,
    ) -> Result<NativeReceipt, NativeSubmitError> {
        // The service is not composed yet: nothing was submitted.
        let service = self
            .service()
            .map_err(|_| NativeSubmitError::NotCommitted {
                cause: SuspendCause::NativeUnavailable,
                evidence: EvidenceClass::Local,
            })?;
        service
            .submit(owner, delivery, committed_at_ms)
            .await
            .map(|receipt| native_receipt(&receipt))
            .map_err(|error| match error {
                meerkat::ContinuationSubmitError::Unavailable(detail) => {
                    NativeSubmitError::OutcomeUnknown {
                        cause: SuspendCause::NativeUnavailable,
                        evidence: EvidenceClass::Local,
                        detail,
                    }
                }
                meerkat::ContinuationSubmitError::OwnerRetired => NativeSubmitError::NotCommitted {
                    cause: SuspendCause::MemberNotLive,
                    evidence: EvidenceClass::Local,
                },
                meerkat::ContinuationSubmitError::NoAdmissibleWorkBinding => {
                    NativeSubmitError::NotCommitted {
                        cause: SuspendCause::Governed,
                        evidence: EvidenceClass::Local,
                    }
                }
                meerkat::ContinuationSubmitError::Conflict { existing } => {
                    NativeSubmitError::Blocked {
                        cause: BlockCause::Conflict,
                        detail: format!(
                            "the native key {} is bound to other content",
                            existing.key.as_str()
                        ),
                    }
                }
                meerkat::ContinuationSubmitError::Invalid(detail) => NativeSubmitError::Blocked {
                    cause: BlockCause::NativeInvalid,
                    detail,
                },
            })
    }

    /// The continuation owner's status of a native key for the identity.
    pub async fn status_of(
        &self,
        identity: &MemberIdentity,
        key: &str,
    ) -> Result<NativeStatus, NativeStatusError> {
        self.status_at(&self.owner(identity), key).await
    }

    /// The continuation owner's status of a native key for an owner.
    async fn status_at(
        &self,
        owner: &meerkat::ContinuationOwner,
        key: &str,
    ) -> Result<NativeStatus, NativeStatusError> {
        let key =
            meerkat::ContinuationKey::new(key).map_err(|error| NativeStatusError::Blocked {
                cause: BlockCause::NativeKeyInvalid,
                detail: error.to_string(),
            })?;
        self.service()
            .map_err(|error| NativeStatusError::local(error.0))?
            .continuation_status(owner, &key)
            .await
            .map(native_status)
            .map_err(|error| match error {
                meerkat::ContinuationStatusError::Inconsistent(detail) => {
                    NativeStatusError::Blocked {
                        cause: BlockCause::NativeInconsistent,
                        detail,
                    }
                }
                other => NativeStatusError::local(other.to_string()),
            })
    }

    /// The runtime's own state of an admitted input.
    pub async fn input_state(
        &self,
        session_id: &str,
        input_id: &str,
    ) -> Result<Option<Value>, NativeUnavailable> {
        use meerkat_runtime::SessionServiceRuntimeExt as _;
        let session = meerkat_core::types::SessionId::parse(session_id)
            .map_err(|error| NativeUnavailable(error.to_string()))?;
        let input = uuid::Uuid::parse_str(input_id)
            .map(meerkat_core::lifecycle::InputId)
            .map_err(|error| NativeUnavailable(error.to_string()))?;
        let Some(state) = self
            .runtime
            .input_state(&session, &input)
            .await
            .map_err(|error| NativeUnavailable(error.to_string()))?
        else {
            return Ok(None);
        };
        let completion = self
            .runtime
            .input_terminal_completion(&session, &input)
            .await;
        Ok(Some(input_reading(
            state.seed.phase,
            state.seed.terminal_outcome,
            completion,
        )))
    }
}

/// An admitted input's reading, `{phase, terminal_outcome, completion}`,
/// from its stored state and Meerkat's exact completion read of it.
///
/// `completion` is a final receipt's completion type. It is null when there
/// is no receipt to wait for, and the terminal outcome or the phase decides:
/// the input has not ended yet, or it ended in a way that never carries a
/// receipt (superseded, coalesced, consumed on accept, or abandoned without
/// one). An input that has ended but whose receipt is not final yet is read
/// again, never as one without a receipt: the receipt follows the end of its
/// run by a moment and says how the turn ended. A failed read is read again
/// too.
fn input_reading(
    phase: meerkat_runtime::InputLifecycleState,
    terminal_outcome: Option<meerkat_runtime::InputTerminalOutcome>,
    completion: Result<
        Option<meerkat_runtime::CompletionOutcome>,
        meerkat_runtime::RuntimeDriverError,
    >,
) -> Value {
    use meerkat_runtime::InputLifecycleState as Phase;
    let ended = terminal_outcome.is_some()
        || matches!(
            phase,
            Phase::Consumed | Phase::Superseded | Phase::Coalesced | Phase::Abandoned
        );
    let completion = match completion {
        Ok(Some(outcome)) => serde_json::to_value(outcome)
            .ok()
            .and_then(|value| value.get("completion_type").cloned())
            .unwrap_or(Value::Null),
        Ok(None) if ended => {
            json!({ "unavailable": "the input has ended and its receipt is not final yet" })
        }
        Ok(None) | Err(meerkat_runtime::RuntimeDriverError::InputTerminalWithoutReceipt { .. }) => {
            Value::Null
        }
        Err(error) => json!({ "unavailable": error.to_string() }),
    };
    json!({
        "phase": serde_json::to_value(phase).unwrap_or(Value::Null),
        "terminal_outcome": serde_json::to_value(terminal_outcome).unwrap_or(Value::Null),
        "completion": completion,
    })
}

/// Whether an identity of this gateway's mob is registered, and whether this
/// gateway's runtime serves it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum IdentityService {
    /// Not registered with this gateway.
    Unknown,
    /// Served by this gateway's runtime: a local session-backed member.
    Local,
    /// Registered, but its session is not this runtime's to serve.
    Elsewhere(NotServedHereCause),
}

/// What an operator reads for a member this runtime does not serve.
pub(crate) fn not_served_here_detail(
    identity: &MemberIdentity,
    cause: NotServedHereCause,
) -> String {
    match cause {
        NotServedHereCause::PlacedOnHost => format!("{identity} is placed on a host"),
        NotServedHereCause::ExternalBinding => {
            format!("{identity} is bound to an external runtime")
        }
    }
}

/// Members of this gateway's mob, and of mobs its members created, are
/// delivered to through its own runtime. A member on a peer gateway has no
/// path from this seam.
#[async_trait]
impl NativeDelivery for MobNativeDelivery {
    fn governs_work_authority(&self) -> Result<bool, NativeUnavailable> {
        MobNativeDelivery::governs_work_authority(self)
    }

    async fn member_facts(
        &self,
        member: &Member,
    ) -> Result<Option<MemberFacts>, NativeUnavailable> {
        MobNativeDelivery::member_facts(self, member).await
    }

    async fn members_with_role(
        &self,
        mob_id: &MobId,
        role: &str,
    ) -> Result<Vec<Member>, NativeUnavailable> {
        MobNativeDelivery::members_with_role(self, mob_id, role).await
    }

    async fn route_for(&self, member: &Member) -> Result<MemberRouteRecord, MemberProblem> {
        if member.mob_id == self.mob_id {
            return Ok(MemberRouteRecord {
                route: MemberRoute::Local,
                incarnation: None,
            });
        }
        match self
            .child_incarnation(&member.mob_id)
            .await
            .map_err(|error| MemberProblem::Unavailable(error.0))?
        {
            Some(incarnation) => Ok(MemberRouteRecord {
                route: MemberRoute::Child,
                incarnation: Some(incarnation),
            }),
            None => Ok(MemberRouteRecord {
                route: MemberRoute::Peer,
                incarnation: None,
            }),
        }
    }

    async fn mob_presence(
        &self,
        mobs: &BTreeSet<MobId>,
    ) -> Result<BTreeMap<MobId, MobPresence>, NativeUnavailable> {
        self.hosted_mobs(mobs).await
    }

    fn mob_set_changes(&self) -> Option<tokio::sync::watch::Receiver<u64>> {
        Some(self.mob_tools.mob_set_changes())
    }

    fn own_mob(&self) -> Option<MobId> {
        Some(self.mob_id.clone())
    }

    async fn hosted_mob_ids(&self) -> Result<BTreeSet<MobId>, NativeUnavailable> {
        MobNativeDelivery::hosted_mob_ids(self).await
    }

    async fn mob_origin(
        &self,
        mob_id: &MobId,
    ) -> Result<Option<MobOriginFacts>, NativeUnavailable> {
        MobNativeDelivery::mob_origin(self, mob_id).await
    }

    async fn check_member(&self, member: &Member, route: MemberRoute) -> Result<(), MemberProblem> {
        if let Some(refusal) = route_contradiction(Some(&self.mob_id), member, route) {
            return Err(refusal.member_problem());
        }
        match route {
            MemberRoute::Local => match self.identity_service(&member.identity).await {
                Ok(IdentityService::Local) => Ok(()),
                Ok(IdentityService::Unknown) => Err(MemberProblem::Unknown),
                Ok(IdentityService::Elsewhere(cause)) => Err(MemberProblem::Unusable(
                    not_served_here_detail(&member.identity, cause),
                )),
                Err(error) => Err(MemberProblem::Unavailable(error.0)),
            },
            MemberRoute::Child => self.check_child_member(member).await,
            MemberRoute::Peer => Err(MemberProblem::UnusablePeer(format!(
                "this gateway has no delivery path to members of {}",
                member.mob_id
            ))),
        }
    }

    async fn prepare_member(
        &self,
        member: &Member,
        route: MemberRoute,
    ) -> Result<(), PrepareError> {
        match route {
            MemberRoute::Local => self.prepare_identity(&member.identity).await,
            MemberRoute::Child => self.prepare_child_member(member).await,
            MemberRoute::Peer => Err(PrepareError::Unavailable(
                "this gateway has no delivery path to peer members".to_string(),
            )),
        }
    }

    async fn submit(
        &self,
        batch: &Batch,
        committed_at_ms: u64,
    ) -> Result<NativeReceipt, NativeSubmitError> {
        let owner = self
            .owner_for(&batch.member, batch.route)
            .map_err(|refusal| refusal.submit_error())?;
        let delivery = continuation_for(batch).map_err(|detail| NativeSubmitError::Blocked {
            cause: BlockCause::NativeInvalid,
            detail,
        })?;
        let receipt = self
            .submit_delivery_to(&owner, delivery, committed_at_ms)
            .await?;
        super::faults::after_native_commit();
        Ok(receipt)
    }

    /// No owner path reads a batch whose route contradicts its member: the
    /// owner's one route guard blocks it before any drive step and observes
    /// it without a read. A direct call is refused by the same guard before
    /// anything is read, as blocked.
    async fn status(&self, batch: &Batch) -> Result<NativeStatus, NativeStatusError> {
        let owner = self
            .owner_for(&batch.member, batch.route)
            .map_err(|refusal| refusal.status_error())?;
        self.status_at(&owner, batch.id.as_str()).await
    }

    async fn input(
        &self,
        _batch: &Batch,
        session_id: &str,
        input_id: &str,
    ) -> Result<Option<Value>, NativeUnavailable> {
        self.input_state(session_id, input_id).await
    }

    async fn member_mob_cursor(&self, mob_id: &MobId) -> Result<u64, NativeUnavailable> {
        MobNativeDelivery::member_mob_cursor(self, mob_id).await
    }

    async fn member_mob_gone(
        &self,
        mob_id: &MobId,
        incarnation: Option<&MobIncarnation>,
    ) -> Result<Option<MemberMobEnd>, NativeUnavailable> {
        MobNativeDelivery::member_mob_gone(self, mob_id, incarnation).await
    }

    async fn member_mob_end(
        &self,
        mob_id: &MobId,
        incarnation: Option<&MobIncarnation>,
        after: u64,
    ) -> Result<MemberMobScan, NativeUnavailable> {
        MobNativeDelivery::member_mob_end(self, mob_id, incarnation, after).await
    }
}

/// The owner's seam: this gateway's runtime for members of its own mob and
/// of mobs its members created, and keyed inject over the signed control
/// channel for members of mobs on peer gateways.
pub struct AnnouncementNativeDelivery {
    local: Arc<MobNativeDelivery>,
    remote: Option<Arc<RemoteMemberDelivery>>,
}

impl AnnouncementNativeDelivery {
    pub(crate) fn new(
        local: Arc<MobNativeDelivery>,
        remote: Option<Arc<RemoteMemberDelivery>>,
    ) -> Self {
        Self { local, remote }
    }

    fn remote(&self) -> Result<&RemoteMemberDelivery, String> {
        self.remote
            .as_deref()
            .ok_or_else(|| "this gateway has no contact directory for peer members".to_string())
    }
}

#[async_trait]
impl NativeDelivery for AnnouncementNativeDelivery {
    fn governs_work_authority(&self) -> Result<bool, NativeUnavailable> {
        self.local.governs_work_authority()
    }

    /// A member on a peer gateway has no roster here: only a member grant
    /// covers it.
    async fn member_facts(
        &self,
        member: &Member,
    ) -> Result<Option<MemberFacts>, NativeUnavailable> {
        self.local.member_facts(member).await
    }

    /// A mob on a peer gateway has no roster here: a template of its
    /// members takes none.
    async fn members_with_role(
        &self,
        mob_id: &MobId,
        role: &str,
    ) -> Result<Vec<Member>, NativeUnavailable> {
        self.local.members_with_role(mob_id, role).await
    }

    /// A mob id hosted here as a mob a member created and also filed in the
    /// contact directory names two mobs: it is refused, never guessed.
    async fn route_for(&self, member: &Member) -> Result<MemberRouteRecord, MemberProblem> {
        let route = NativeDelivery::route_for(self.local.as_ref(), member).await?;
        if route.route == MemberRoute::Child
            && self
                .remote
                .as_deref()
                .is_some_and(|remote| remote.files(&member.mob_id))
        {
            return Err(MemberProblem::Ambiguous(format!(
                "{} is a mob hosted here and also a peer in the contact directory",
                member.mob_id
            )));
        }
        Ok(route)
    }

    async fn mob_presence(
        &self,
        mobs: &BTreeSet<MobId>,
    ) -> Result<BTreeMap<MobId, MobPresence>, NativeUnavailable> {
        self.local.hosted_mobs(mobs).await
    }

    fn mob_set_changes(&self) -> Option<tokio::sync::watch::Receiver<u64>> {
        NativeDelivery::mob_set_changes(self.local.as_ref())
    }

    fn own_mob(&self) -> Option<MobId> {
        NativeDelivery::own_mob(self.local.as_ref())
    }

    async fn hosted_mob_ids(&self) -> Result<BTreeSet<MobId>, NativeUnavailable> {
        self.local.hosted_mob_ids().await
    }

    async fn mob_origin(
        &self,
        mob_id: &MobId,
    ) -> Result<Option<MobOriginFacts>, NativeUnavailable> {
        self.local.mob_origin(mob_id).await
    }

    async fn check_member(&self, member: &Member, route: MemberRoute) -> Result<(), MemberProblem> {
        match route {
            MemberRoute::Local | MemberRoute::Child => {
                NativeDelivery::check_member(self.local.as_ref(), member, route).await
            }
            MemberRoute::Peer => self
                .remote()
                .map_err(MemberProblem::UnusablePeer)?
                .check_member(member),
        }
    }

    async fn prepare_member(
        &self,
        member: &Member,
        route: MemberRoute,
    ) -> Result<(), PrepareError> {
        match route {
            MemberRoute::Local | MemberRoute::Child => {
                NativeDelivery::prepare_member(self.local.as_ref(), member, route).await
            }
            // The peer prepares its own member when the batch reaches it.
            MemberRoute::Peer => Ok(()),
        }
    }

    async fn submit(
        &self,
        batch: &Batch,
        committed_at_ms: u64,
    ) -> Result<NativeReceipt, NativeSubmitError> {
        match batch.route {
            MemberRoute::Local | MemberRoute::Child => {
                NativeDelivery::submit(self.local.as_ref(), batch, committed_at_ms).await
            }
            // Without a contact directory and key the peer is never asked.
            MemberRoute::Peer => {
                self.remote()
                    .map_err(|_| NativeSubmitError::NotCommitted {
                        cause: SuspendCause::LocalNotAuthenticatable,
                        evidence: EvidenceClass::Local,
                    })?
                    .submit(batch)
                    .await
            }
        }
    }

    async fn status(&self, batch: &Batch) -> Result<NativeStatus, NativeStatusError> {
        match batch.route {
            MemberRoute::Local | MemberRoute::Child => {
                NativeDelivery::status(self.local.as_ref(), batch).await
            }
            MemberRoute::Peer => {
                self.remote()
                    .map_err(|detail| NativeStatusError::Unavailable {
                        cause: SuspendCause::LocalNotAuthenticatable,
                        evidence: EvidenceClass::Local,
                        detail,
                    })?
                    .status(batch)
                    .await
            }
        }
    }

    async fn input(
        &self,
        batch: &Batch,
        session_id: &str,
        input_id: &str,
    ) -> Result<Option<Value>, NativeUnavailable> {
        match batch.route {
            MemberRoute::Local | MemberRoute::Child => {
                self.local.input_state(session_id, input_id).await
            }
            MemberRoute::Peer => self.remote().map_err(NativeUnavailable)?.input(batch).await,
        }
    }

    async fn member_mob_cursor(&self, mob_id: &MobId) -> Result<u64, NativeUnavailable> {
        self.local.member_mob_cursor(mob_id).await
    }

    async fn member_mob_gone(
        &self,
        mob_id: &MobId,
        incarnation: Option<&MobIncarnation>,
    ) -> Result<Option<MemberMobEnd>, NativeUnavailable> {
        self.local.member_mob_gone(mob_id, incarnation).await
    }

    async fn member_mob_end(
        &self,
        mob_id: &MobId,
        incarnation: Option<&MobIncarnation>,
        after: u64,
    ) -> Result<MemberMobScan, NativeUnavailable> {
        self.local.member_mob_end(mob_id, incarnation, after).await
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod reading_tests {
    use meerkat_core::lifecycle::InputId;
    use meerkat_runtime::{
        CompletionOutcome, InputAbandonReason, InputLifecycleState, InputTerminalOutcome,
        RuntimeDriverError,
    };

    use super::input_reading;
    use crate::mob_announcements::handling::{Classified, classify};

    /// An input that has ended is read again until its receipt is final: a
    /// run's end commits a moment before its receipt does, and a read in
    /// between is not a turn that ended without a receipt.
    #[test]
    fn an_ended_input_is_read_again_until_its_receipt_is_final() {
        let consumed = Some(InputTerminalOutcome::Consumed);
        let between = input_reading(InputLifecycleState::Consumed, consumed.clone(), Ok(None));
        assert_eq!(classify(&between), Classified::Unreadable, "{between}");
        let receipted = input_reading(
            InputLifecycleState::Consumed,
            consumed,
            Ok(Some(CompletionOutcome::CompletedWithoutResult)),
        );
        assert_eq!(classify(&receipted), Classified::Handled, "{receipted}");
        let running = input_reading(InputLifecycleState::Staged, None, Ok(None));
        assert_eq!(classify(&running), Classified::Pending, "{running}");
    }

    /// An end that never carries a receipt is read by its outcome: consumed
    /// on accept is handled_unverified, a superseded input follows the one
    /// that absorbed it, and a turn a stop kept past its requeue max age is
    /// unhandled. Any other failed read is read again.
    #[test]
    fn an_end_without_a_receipt_is_read_by_its_outcome() {
        let read = |phase, terminal: InputTerminalOutcome| {
            let error = RuntimeDriverError::InputTerminalWithoutReceipt {
                input_id: InputId(uuid::Uuid::nil()),
                terminal: terminal.clone(),
            };
            classify(&input_reading(phase, Some(terminal), Err(error)))
        };
        assert_eq!(
            read(
                InputLifecycleState::Consumed,
                InputTerminalOutcome::Consumed
            ),
            Classified::HandledUnverified
        );
        let absorber = InputId(uuid::Uuid::from_u128(7));
        assert_eq!(
            read(
                InputLifecycleState::Superseded,
                InputTerminalOutcome::Superseded {
                    superseded_by: absorber.clone()
                }
            ),
            Classified::Follow {
                input_id: serde_json::to_value(&absorber)
                    .unwrap()
                    .as_str()
                    .unwrap()
                    .to_string()
            }
        );
        assert_eq!(
            read(
                InputLifecycleState::Abandoned,
                InputTerminalOutcome::Abandoned {
                    reason: InputAbandonReason::RequeueExpired
                }
            ),
            Classified::Unhandled {
                cause: "requeue_expired".into(),
                held: false
            }
        );
        let failed = input_reading(
            InputLifecycleState::Consumed,
            Some(InputTerminalOutcome::Consumed),
            Err(RuntimeDriverError::RecoveryBackoff {
                reason: "the store is busy".into(),
            }),
        );
        assert_eq!(classify(&failed), Classified::Unreadable, "{failed}");
    }
}
