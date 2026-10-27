//! The announcement owner: announcement and subscription commands, delivery
//! observation, and the dispatcher that seals and delivers batches.
//!
//! The owner decides nothing about native delivery. It persists its intent
//! before every native effect, reads the native owner's evidence after it,
//! and moves a batch only through [`BatchState::apply`] as the store applies
//! it. The dispatcher runs when something changed (an announcement was
//! captured for a subscription, a subscription resumed, a batch is due) or
//! when a computed deadline passes. Its timer is a hint: no batch moves
//! without evidence.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;
use tokio::sync::{Mutex, Notify};

use super::availability::{EvidenceClass, SuspendCause, SuspendMarker};
use super::escalations::{
    Escalation, EscalationEvent, EscalationEventKind, EscalationNotice, EscalationPass,
    EscalationSubject, MemberRead, StatusCounts,
};
use super::handling::{BatchHandling, Classified, HandlingRead, MAX_FOLLOW, classify};
pub use super::native::AdmittedInput;
use super::native::{
    MemberMobScan, MemberProblem, MemberRoute, MobOrigin, MobPresence, NativeDelivery,
    NativeStatus, NativeStatusError, NativeSubmitError, OwnerRefusal, PrepareError,
    route_contradiction,
};
use super::sources::{
    AdvanceSource, AnnouncementSource, AuditQuery, AuditRow, MemberFacts, PutSource,
    RecordSourceFailure, RosterFacts, SourceAdvance, SourceChange, SourceQuery, SourceState,
    SourceStream, UnhandledPolicy,
};
use super::store::{AnnouncementStore, SubmissionCheck};
use super::types::{
    AnnounceOutcome, AnnounceRequest, Announcement, AnnouncementStoreError, Batch, BatchEvent,
    BatchGap, BatchId, BatchPosition, BatchQuery, BatchState, BatchStateTag, ChildMemberLimits,
    GapReason, LateOutcome, MAX_PAGE, Member, MemberRouteRecord, MobId, MobIncarnation, MobLineage,
    NativeReceipt, OperationId, PutSubscription, SealingPolicy, SourceId, SourceKey, Subscription,
    SubscriptionChange, SubscriptionChangeOutcome, SubscriptionEnd, SubscriptionId,
    SubscriptionState, SuccessorBatch, merge_gaps, render_item, successor_batch_with_notice,
    wire_budget,
};

#[path = "owner_declarations.rs"]
mod declarations_engine;

/// The owner's error for a member a put cannot subscribe.
fn member_problem(member: &Member, problem: MemberProblem) -> AnnouncementError {
    match problem {
        MemberProblem::Unknown => AnnouncementError::UnknownMember(member.clone()),
        MemberProblem::Invalid(detail) => AnnouncementError::InvalidMember(detail),
        MemberProblem::Unusable(detail) => AnnouncementError::UnusableMember(detail),
        MemberProblem::UnusablePeer(detail) => AnnouncementError::UnusablePeer(detail),
        MemberProblem::Unavailable(detail) => AnnouncementError::NativeUnavailable(detail),
        MemberProblem::Ambiguous(_) => AnnouncementError::AmbiguousMob(member.mob_id.clone()),
    }
}

/// Announcements and settled batches are kept this long after they settle.
pub const DEFAULT_RETENTION_MS: u64 = 7 * 24 * 60 * 60 * 1_000;

/// The shortest wait before an open escalation is notified again.
pub const MIN_RENOTIFY_AFTER_MS: u64 = 60 * 1_000;

/// How long an open escalation waits before it is notified again by
/// default: a day.
pub const DEFAULT_RENOTIFY_AFTER_MS: u64 = 24 * 60 * 60 * 1_000;
/// The shortest retention window: a retry of an announcement or a subscription
/// change inside it still meets its original result.
pub const MIN_RETENTION_MS: u64 = 60 * 60 * 1_000;
/// Unsettled batches the dispatcher advances per round.
pub(super) const ROUND_BATCHES: usize = 64;
/// Members the dispatcher considers for sealing per round.
pub(super) const ROUND_MEMBERS: usize = 64;

/// How often, at least, a member's held items are checked afresh while it
/// has nothing else to seal: a held item its member passes again is
/// released within this bound and the dispatcher's longest idle.
pub const HOLD_RECHECK_MS: u64 = 5 * 60 * 1_000;
/// Batches that settled without their delivery being confirmed, read again
/// per round.
const ROUND_LATE_READS: usize = 16;
/// Most handling reads in one round.
const ROUND_HANDLING_READS: usize = 16;
/// Longest the dispatcher sleeps without a signal, in ms.
const MAX_IDLE_MS: u64 = 5 * 60 * 1_000;
/// How often retention runs.
const PRUNE_INTERVAL_MS: u64 = 60 * 60 * 1_000;
/// The pause between retention passes while retention is behind, so
/// commands get the store in between.
const CATCH_UP_PAUSE: Duration = Duration::from_millis(10);
/// How often a batch that keeps failing is reported again.
const FAILURE_REPORT_INTERVAL_MS: u64 = 10 * 60 * 1_000;
/// The first pause before a dispatcher that ended without a stop request is
/// started again; it doubles up to [`RESTART_PAUSE_MAX`].
const RESTART_PAUSE_MIN: Duration = Duration::from_secs(1);
const RESTART_PAUSE_MAX: Duration = Duration::from_mins(1);
/// One retention pass removes at most this many announcements, this many
/// batches and this many receipts; passes repeat while one stops at a limit.
const PRUNE_LIMIT: usize = 1_000;

/// Where escalation events reach the host: the gateway's `meta/on_error`
/// hook.
pub type ErrorSink = Arc<dyn Fn(crate::unified_runtime::ErrorEvent) + Send + Sync>;

/// The host notification for one escalation event.
fn escalation_error_event(notice: EscalationNotice) -> crate::unified_runtime::ErrorEvent {
    use crate::unified_runtime::ErrorEvent;
    let EscalationNotice {
        event,
        mut escalation,
    } = notice;
    match event.event {
        EscalationEventKind::Resolved => ErrorEvent::MobAnnouncementEscalationResolved {
            escalation_id: event.escalation_id,
            cause: event.cause,
            subject: event.subject,
            resolved_at_ms: event.at_ms,
            cursor: event.cursor,
        },
        kind => {
            // The record as it stood at this event: open, at the event's
            // count, though a later notification or the resolution may
            // already have changed the stored one.
            escalation.renotify_count = event.renotify_count;
            escalation.resolved_at_ms = None;
            ErrorEvent::MobAnnouncementEscalated {
                escalation,
                event: kind,
                renotify_count: event.renotify_count,
                cursor: event.cursor,
            }
        }
    }
}

/// Configurable announcement policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnnouncementConfig {
    pub policy: SealingPolicy,
    pub retention_ms: u64,
    /// Shortest pause before a batch the native owner has not settled yet
    /// is read again. The pause grows with the batch's age.
    pub settle_check_min_ms: u64,
    /// Longest such pause.
    pub settle_check_max_ms: u64,
    /// Per-member limits on the member tools.
    pub member_limits: super::tools::MemberLimits,
    /// How far a keyed inject request's issue time may be from a peer's
    /// clock: a request older than this is refused as stale, so a batch
    /// for a peer member whose last attempt is older may expire. It is the
    /// peers' window, so it should match their `control_request_window_ms`:
    /// a smaller one shortens the quiet window before such a batch expires
    /// (at 0, to the slowest settle pause). Every gateway sets it from the
    /// window its own control listener serves with.
    pub peer_request_window_ms: u64,
    /// Bounds on subscriptions for members of mobs hosted here other than
    /// this gateway's own.
    pub child_limits: ChildMemberLimits,
    /// The ceiling of the member tools in mobs members create. With it, the
    /// member tool bundle is offered to child mobs: a child mob profile
    /// mounts it only by naming it in its own `tools.bundles` opt-in, and
    /// only its members' calls are attributed through this gateway's
    /// runtime. Without it the bundle is the host's alone: a child create
    /// that names it is refused, and a member a lowered ceiling no longer
    /// covers is built without it.
    pub member_tools_in_child_mobs: bool,
    /// What happens to a plain announcement whose admitted turn ended
    /// unhandled: report (the default, as 0.9.4 plus the handling
    /// observation), redeliver with notice, or hold for an operator. A
    /// source item follows its own source's `delivery.on_unhandled`.
    pub on_unhandled: UnhandledPolicy,
    /// How long an open escalation waits before it is notified again.
    pub renotify_after_ms: u64,
    /// Who agents among a mob's members receive member escalations on the
    /// health topic; empty, the default, leaves them to the host.
    pub health_audience: Vec<super::sources::AudienceGrant>,
    /// How many of a member's turns ending handled_unverified in a day
    /// escalate.
    pub handled_unverified_threshold: u32,
    /// The sources and subscriptions this gateway declares, as its options
    /// name them (inline, or a sidecar read at init): reconciled at compose,
    /// before the dispatcher starts.
    pub declarations: Option<Arc<super::declarations::DeclarationsSource>>,
}

impl AnnouncementConfig {
    /// Refuse a configuration no host composes, whichever surface built
    /// it: a policy, window or limit out of range. Every constructor runs
    /// this one check: the options parser,
    /// [`super::AnnouncementComposition::new`] and
    /// [`AnnouncementService::new`].
    pub fn validate(&self) -> Result<(), super::AnnouncementOptionsError> {
        use super::AnnouncementOptionsError::Invalid;
        self.policy
            .validate()
            .map_err(|error| Invalid(format!("batch: {error}")))?;
        if self.retention_ms < MIN_RETENTION_MS {
            return Err(Invalid(format!(
                "retention_ms must be at least {MIN_RETENTION_MS}"
            )));
        }
        if self.settle_check_min_ms == 0 || self.settle_check_min_ms > self.settle_check_max_ms {
            return Err(Invalid("settle checks need 0 < min <= max".to_string()));
        }
        self.member_limits
            .validate()
            .map_err(|error| Invalid(format!("member_limits: {error}")))?;
        self.child_limits
            .validate()
            .map_err(|error| Invalid(format!("child_member_limits: {error}")))?;
        if self.renotify_after_ms < MIN_RENOTIFY_AFTER_MS {
            return Err(Invalid(format!(
                "renotify_after_ms must be at least {MIN_RENOTIFY_AFTER_MS}"
            )));
        }
        for grant in &self.health_audience {
            grant
                .validate()
                .map_err(|error| Invalid(format!("health_audience: {error}")))?;
        }
        if self.handled_unverified_threshold == 0 {
            return Err(Invalid(
                "handled_unverified_threshold must be at least 1".to_string(),
            ));
        }
        Ok(())
    }
}

impl Default for AnnouncementConfig {
    fn default() -> Self {
        Self {
            policy: SealingPolicy::default(),
            retention_ms: DEFAULT_RETENTION_MS,
            settle_check_min_ms: 1_000,
            settle_check_max_ms: 30_000,
            member_limits: super::tools::MemberLimits::default(),
            peer_request_window_ms: crate::runtime::keyed_inject::DEFAULT_KEYED_INJECT_WINDOW_MS,
            child_limits: ChildMemberLimits::default(),
            member_tools_in_child_mobs: false,
            on_unhandled: UnhandledPolicy::Report,
            renotify_after_ms: DEFAULT_RENOTIFY_AFTER_MS,
            health_audience: Vec::new(),
            handled_unverified_threshold: 3,
            declarations: None,
        }
    }
}

/// Announcement refusals and failures, as callers see them.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum AnnouncementError {
    Store(AnnouncementStoreError),
    /// The configuration the owner was built with is refused; see
    /// [`AnnouncementConfig::validate`].
    Configuration(super::AnnouncementOptionsError),
    /// This host's runtime admits work only through a native work
    /// authorization host, which refuses host continuations.
    GovernedRuntime,
    /// The member is not an identity registered with this host.
    UnknownMember(Member),
    /// The member is not a durable identity in its public form.
    InvalidMember(String),
    /// This gateway's runtime does not serve the member (it is placed on a
    /// host, or bound to an external runtime), so a delivery could never be
    /// admitted.
    UnusableMember(String),
    /// The named mobs are not hosted on this gateway.
    UnknownMobs(Vec<MobId>),
    /// The member's peer gateway cannot be called authenticated: no
    /// contact, no pinned gateway key, an opt-out of signed control
    /// responses, or no signing key on this gateway.
    UnusablePeer(String),
    /// A native read the request needs could not be made now.
    NativeUnavailable(String),
    /// A member asked for a mob that is not its own, above it or below it
    /// in the lineage of mobs members created.
    UnrelatedMob(MobId),
    /// The member's mob id names a mob hosted here and a peer in the
    /// contact directory alike, or would now route differently from how the
    /// member was first subscribed.
    AmbiguousMob(MobId),
    /// This gateway is a candidate launch: it does not speak for its mob
    /// composition, so it delivers nothing and changes no announcement
    /// state. Reads still answer; a change belongs to the authoritative
    /// launch.
    CandidateLaunch,
}

impl std::fmt::Display for AnnouncementError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Store(error) => error.fmt(f),
            Self::Configuration(error) => error.fmt(f),
            Self::GovernedRuntime => f.write_str(
                "this host's runtime admits work only through a native work authorization \
                 host, which refuses host continuations",
            ),
            Self::UnknownMember(member) => {
                write!(f, "{member} is not an identity registered with this host")
            }
            Self::InvalidMember(detail) => write!(f, "invalid member: {detail}"),
            Self::UnusableMember(detail) => {
                write!(f, "the member cannot take a delivery: {detail}")
            }
            Self::UnknownMobs(mobs) => {
                let names = mobs
                    .iter()
                    .map(MobId::as_str)
                    .collect::<Vec<_>>()
                    .join(", ");
                write!(f, "not hosted on this gateway: {names}")
            }
            Self::UnusablePeer(detail) => {
                write!(f, "the member's peer gateway is not usable: {detail}")
            }
            Self::NativeUnavailable(detail) => write!(f, "native state unavailable: {detail}"),
            Self::UnrelatedMob(mob_id) => write!(
                f,
                "mob {mob_id} is not the member's own mob, above it or below it"
            ),
            Self::AmbiguousMob(mob_id) => write!(
                f,
                "mob {mob_id} names a mob hosted here and a peer gateway alike"
            ),
            Self::CandidateLaunch => f.write_str(
                "this gateway is a candidate launch: it delivers nothing and changes no mob \
                 announcement state; reads still answer, and changes belong to the \
                 authoritative launch",
            ),
        }
    }
}

impl std::error::Error for AnnouncementError {}

impl From<AnnouncementStoreError> for AnnouncementError {
    fn from(error: AnnouncementStoreError) -> Self {
        Self::Store(error)
    }
}

/// A live read of a batch's native delivery. Each variant is the native
/// owner's own status; none implies that the member processed anything.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
#[non_exhaustive]
pub enum NativeObservation {
    /// No native submission has started for this batch.
    NotSubmitted,
    /// The native owner holds no commit for the batch's key.
    NotCommitted,
    /// Committed to the member's inbox, not yet admitted.
    Pending {
        receipt: NativeReceipt,
        #[serde(skip_serializing_if = "Option::is_none")]
        admitted: Option<AdmittedInput>,
    },
    /// Admitted as one input of the member's session.
    Applied {
        receipt: NativeReceipt,
        session_id: String,
        input_id: String,
    },
    /// Committed to an incarnation that retired before admitting it.
    Stranded { receipt: NativeReceipt },
    /// Refused by the native owner.
    Refused {
        receipt: NativeReceipt,
        reason: String,
    },
    /// The native status could not be read now; nothing is implied.
    Unavailable { detail: String },
}

/// Everything known about one delivery: the immutable batch with its owner
/// state, the native owner's live status, once admitted the native input's
/// own state and what became of its turn (null until admitted), and why
/// the member's deliveries wait, if they do.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DeliveryObservation {
    pub batch: Batch,
    pub native: NativeObservation,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input: Option<Value>,
    pub handling: Option<BatchHandling>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub marker: Option<SuspendMarker>,
}

/// What one dispatcher round did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RoundReport {
    pub sealed: u32,
    pub advanced: u32,
    /// The earliest time something is due again, if anything is.
    pub next_due_ms: Option<u64>,
}

type Clock = Arc<dyn Fn() -> u64 + Send + Sync>;

/// When a batch nothing native holds expires.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expiry {
    NotYet,
    Now,
    /// Once the peer's request window has passed since the last attempt.
    AfterQuietWindow(u64),
}

/// Which subscriptions still want a batch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Interest {
    Active,
    Paused,
    None,
}

fn system_clock() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}

/// The dispatcher's walk over unsettled batches, in (sealed time, id)
/// order. Batches that stay unsettled for long (a long turn, a paused
/// subscription, an unresolvable member, an unreadable status) cannot take
/// every place of every round, and none is read faster than its own pace.
#[derive(Debug, Default)]
struct Walk {
    /// Where the next round resumes while a lap is under way.
    position: Option<BatchPosition>,
    /// The newest unsettled batch when the lap started: the lap ends there,
    /// so batches sealed meanwhile wait for the next lap and full windows
    /// of newer batches cannot leave an older one behind.
    lap_end: Option<BatchPosition>,
    /// When each recently read batch is read again: its native retry time,
    /// or, after a failure, the longest settle pause. Only future times are
    /// kept, and only for batches still unsettled when last seen.
    due: BTreeMap<BatchPosition, u64>,
    /// When a batch that keeps failing was last reported.
    reported: BTreeMap<BatchPosition, u64>,
    /// Members whose backlog failed to seal since it last sealed: reported
    /// once until it seals again or is gone.
    unsealed: BTreeSet<Member>,
}

impl Walk {
    /// Record a batch just read: when to read it again. A batch that needs
    /// no timer (settled, or held by a paused subscription) keeps no entry.
    fn read(&mut self, position: BatchPosition, due: Option<u64>) {
        self.reported.remove(&position);
        match due {
            Some(at) => {
                self.due.insert(position, at);
            }
            None => {
                self.due.remove(&position);
            }
        }
    }

    /// A window holds every unsettled batch in the range it covers, from
    /// the walk's position through its last batch, or to the end when it is
    /// not full. A batch the walk remembers in that range and the window
    /// lacks has settled or was pruned since: it leaves the walk.
    fn drop_settled(&mut self, window: &[Batch], full: bool) {
        let held = window
            .iter()
            .map(BatchPosition::of)
            .collect::<BTreeSet<_>>();
        let after = self.position.clone();
        let through = if full { held.last().cloned() } else { None };
        let kept = |position: &BatchPosition| {
            held.contains(position)
                || after.as_ref().is_some_and(|after| position <= after)
                || through.as_ref().is_some_and(|through| position > through)
        };
        self.due.retain(|position, _| kept(position));
        self.reported.retain(|position, _| kept(position));
    }
}

struct Inner {
    store: Arc<dyn AnnouncementStore>,
    native: Arc<dyn NativeDelivery>,
    config: AnnouncementConfig,
    wake: Arc<Notify>,
    /// Held for a whole round; it owns the walk over unsettled batches.
    round: Mutex<Walk>,
    /// Why each member's deliveries wait now: the latest classified outcome
    /// of an attempt for it. A projection for operators, rebuilt from the
    /// next attempt after a restart; nothing reads it to decide a
    /// transition.
    markers: std::sync::Mutex<BTreeMap<Member, SuspendMarker>>,
    clock: Clock,
    stopped: AtomicBool,
    /// The schedule authority sources' runners are kept on, once the gateway
    /// that owns one installs it.
    runners: std::sync::Mutex<Option<Arc<dyn SourceRunners>>>,
    /// Runner reconciles pass one at a time, each reading the source's
    /// record as it stands then.
    runners_gate: tokio::sync::Mutex<()>,
    /// Whether the persisted markers were read into `markers`.
    markers_loaded: AtomicBool,
    /// When each member's marker was last persisted.
    marker_writes: std::sync::Mutex<BTreeMap<Member, u64>>,
    /// Where escalation events reach the host.
    error_sink: std::sync::Mutex<Option<ErrorSink>>,
    /// The last escalation event pushed to the host; read from the store at
    /// the first pass, so events from before a restart are not pushed again.
    pushed_cursor: std::sync::Mutex<Option<u64>>,
    /// Applies and reconciles of declarations pass one at a time, each
    /// reading the applied declarations and marks as they stand then.
    declarations_gate: tokio::sync::Mutex<()>,
    /// Whether this boot warned that its options declare the applied
    /// revision with other content.
    options_conflict_warned: AtomicBool,
    /// Set once when the owner belongs to a candidate launch, one that does
    /// not speak for its mob composition (see
    /// [`AnnouncementService::quiesce_for_candidate`]). It holds for the
    /// process: promotion is a fresh authoritative boot.
    quiesced_candidate: AtomicBool,
}

/// Keeps sources' runner schedules in line with their records: Meta's
/// schedule authority, composed by the gateway that owns one. A source with
/// a runner has one durable schedule firing its host runnable with the
/// source it collects; a changed cadence updates it in place; no runner, or
/// an ended source, ends it.
#[async_trait::async_trait]
pub trait SourceRunners: Send + Sync {
    /// Bring `source`'s runner schedule in line with this record of it.
    async fn reconcile(&self, source: &AnnouncementSource) -> Result<(), String>;

    /// Bring every runner schedule in line with `live`, every source that is
    /// not ended: theirs are kept, made or changed, and the schedule of a
    /// source not among them (ended, or gone) ends.
    async fn reconcile_all(&self, live: &[AnnouncementSource]) -> Result<(), String>;
}

/// What the last check before a submission decided.
enum LastCheck {
    Passed,
    /// An item fails, and the batch may not be cancelled now.
    Failed,
    Cancelled(Box<Batch>),
    /// The member's roster could not be read: decided at a later round.
    Deferred,
}

/// What a submission left to do.
enum SubmitStep {
    /// The batch moved: drive it on.
    Moved(Box<Batch>),
    /// Whether it was committed is not known: read its status next, at the
    /// settle pace.
    ReadStatus,
    /// Nothing was committed and the batch is owed again: submit it again at
    /// the slowest settle pace.
    Owed,
}

/// Why a member was not made ready for a submission.
enum PrepareStep {
    /// A final fact ended its subscriptions.
    Ended,
    /// It cannot take a delivery now, for a typed reason: look again at the
    /// slowest settle pace.
    Waits,
    /// A read it needs could not be made now: look again at the settle pace.
    NotYet,
}

/// What a status read left to do.
enum StatusStep {
    Status(NativeStatus),
    /// No status now: look again at the settle pace.
    Wait,
    /// The read moved the batch.
    Moved(Box<Batch>),
}

impl Drop for Inner {
    fn drop(&mut self) {
        // Let a waiting dispatcher notice the service is gone.
        self.wake.notify_one();
    }
}

/// The announcement owner. Cheap to clone; clones share one owner.
#[derive(Clone)]
pub struct AnnouncementService {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for AnnouncementService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AnnouncementService")
            .field("config", &self.inner.config)
            .finish_non_exhaustive()
    }
}

fn min_due(current: Option<u64>, candidate: Option<u64>) -> Option<u64> {
    match (current, candidate) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

/// The longest chain of mobs a lineage walk follows, up or down.
const MAX_LINEAGE_DEPTH: usize = 64;

/// The mobs a member of `own` reaches by the lineage `records`: its own
/// mob, the chain of mobs above it, and every mob below it. Siblings and
/// cousins are not reached. Each mob is visited once, so records that loop
/// (an id reused under a new parent) end the walk instead of repeating it.
pub(crate) fn reach_from(records: &[MobLineage], own: &MobId) -> BTreeSet<MobId> {
    let incarnation_of = records
        .iter()
        .map(|record| (&record.mob_id, &record.incarnation))
        .collect::<BTreeMap<_, _>>();
    // An edge holds only to the parent incarnation it was derived under: a
    // parent created again under the same id adopts no child of the old
    // one. The root, this gateway's own mob, has no record.
    let parent_of = records
        .iter()
        .filter_map(|record| {
            let parent = record.parent.as_ref()?;
            let holds = match (&record.parent_incarnation, incarnation_of.get(parent)) {
                (None, None) => true,
                (Some(derived_under), Some(live)) => derived_under == *live,
                _ => false,
            };
            holds.then_some((&record.mob_id, parent))
        })
        .collect::<BTreeMap<_, _>>();
    let mut children = BTreeMap::<&MobId, Vec<&MobId>>::new();
    for (child, parent) in &parent_of {
        children.entry(*parent).or_default().push(*child);
    }
    let mut reach = BTreeSet::from([own.clone()]);
    let mut at = own;
    for _ in 0..MAX_LINEAGE_DEPTH {
        let Some(parent) = parent_of.get(at) else {
            break;
        };
        if !reach.insert((*parent).clone()) {
            break;
        }
        at = parent;
    }
    let mut frontier = vec![own];
    for _ in 0..MAX_LINEAGE_DEPTH {
        let mut next = Vec::new();
        for mob_id in frontier {
            for child in children.get(mob_id).into_iter().flatten() {
                if reach.insert((*child).clone()) {
                    next.push(*child);
                }
            }
        }
        if next.is_empty() {
            break;
        }
        frontier = next;
    }
    reach
}

fn new_batch_id() -> Result<BatchId, AnnouncementError> {
    Ok(BatchId::new(format!("batch-{}", uuid::Uuid::new_v4()))?)
}

impl AnnouncementService {
    pub fn new(
        store: Arc<dyn AnnouncementStore>,
        native: Arc<dyn NativeDelivery>,
        config: AnnouncementConfig,
    ) -> Result<Self, AnnouncementError> {
        Self::with_clock(store, native, config, Arc::new(system_clock))
    }

    pub(crate) fn with_clock(
        store: Arc<dyn AnnouncementStore>,
        native: Arc<dyn NativeDelivery>,
        config: AnnouncementConfig,
        clock: Clock,
    ) -> Result<Self, AnnouncementError> {
        config
            .validate()
            .map_err(AnnouncementError::Configuration)?;
        Ok(Self {
            inner: Arc::new(Inner {
                store,
                native,
                config,
                wake: Arc::new(Notify::new()),
                round: Mutex::new(Walk::default()),
                markers: std::sync::Mutex::new(BTreeMap::new()),
                clock,
                stopped: AtomicBool::new(false),
                runners: std::sync::Mutex::new(None),
                runners_gate: tokio::sync::Mutex::new(()),
                markers_loaded: AtomicBool::new(false),
                marker_writes: std::sync::Mutex::new(BTreeMap::new()),
                error_sink: std::sync::Mutex::new(None),
                pushed_cursor: std::sync::Mutex::new(None),
                declarations_gate: tokio::sync::Mutex::new(()),
                options_conflict_warned: AtomicBool::new(false),
                quiesced_candidate: AtomicBool::new(false),
            }),
        })
    }

    pub fn config(&self) -> &AnnouncementConfig {
        &self.inner.config
    }

    /// Send every escalation event to `sink` as it is written.
    pub fn set_error_sink(&self, sink: ErrorSink) {
        *self
            .inner
            .error_sink
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(sink);
    }

    /// Apply declarations (contract 4.4): `Some` carries them (a gateway
    /// with inline declarations or none); `None` reads the gateway's sidecar
    /// again. A lower revision than the applied one is refused
    /// [`AnnouncementStoreError::DeclarationRollback`], the applied revision
    /// with other content [`AnnouncementStoreError::DeclarationDigestConflict`],
    /// and the applied revision with the same content changes nothing.
    pub async fn apply_declarations(
        &self,
        declarations: Option<super::declarations::Declarations>,
    ) -> Result<super::declarations::DeclarationsView, AnnouncementError> {
        self.ensure_authoritative()?;
        self.apply_declared(declarations).await
    }

    /// What the declarations manage now (contract 4.6).
    pub async fn declarations(
        &self,
    ) -> Result<super::declarations::DeclarationsView, AnnouncementError> {
        self.declared_view().await
    }

    /// Reconcile the applied declarations again: at compose, before the
    /// dispatcher starts, and at each trigger that can admit a pending
    /// record (a mob set change, a health pass).
    pub async fn reconcile_declarations(&self) -> Result<(), AnnouncementError> {
        self.ensure_authoritative()?;
        self.reconcile_declared().await
    }

    /// Reconcile the declarations once, then start the dispatcher, so its
    /// first round runs on what they made and dropped. A reconcile that
    /// fails, or is still running after `bound`, is logged and the
    /// dispatcher starts anyway: a declaration fault never holds delivery.
    pub async fn start_after_reconcile(&self, bound: Duration) -> tokio::task::JoinHandle<()> {
        if self.is_quiesced_candidate() {
            // A candidate reconciles nothing, and its dispatcher ends at once.
            return self.supervise();
        }
        let reconciling = self.clone();
        // Its own task: past the bound the reconcile goes on while the
        // dispatcher starts.
        let reconcile = tokio::spawn(async move { reconciling.reconcile_declarations().await });
        match tokio::time::timeout(bound, reconcile).await {
            Ok(Ok(Ok(()))) => {}
            Ok(Ok(Err(error))) => tracing::warn!(
                kind = %super::rpc::refusal_kind(&error),
                %error,
                "mob announcement declarations not reconciled before the dispatcher started"
            ),
            Ok(Err(error)) => tracing::warn!(
                %error,
                "mob announcement declarations reconcile ended before the dispatcher started"
            ),
            Err(_) => tracing::warn!(
                bound_ms = u64::try_from(bound.as_millis()).unwrap_or(u64::MAX),
                "mob announcement declarations still reconciling when the dispatcher started"
            ),
        }
        self.supervise()
    }

    /// Hold the declarations gate a reconcile takes, so a test can keep one
    /// waiting.
    #[cfg(test)]
    pub(crate) async fn hold_declarations(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.inner.declarations_gate.lock().await
    }

    /// Create a role template, or replace its selector at its revision
    /// (`subscriptions/put` with `members`, contract 4.5).
    pub async fn put_role_template(
        &self,
        operation: &OperationId,
        request: super::declarations::PutRoleTemplate,
    ) -> Result<super::declarations::RoleTemplate, AnnouncementError> {
        self.ensure_authoritative()?;
        self.put_template(operation, request).await
    }

    /// Why the member's deliveries wait now, if they do: the latest
    /// classified outcome of an attempt for it.
    pub fn suspend_marker(&self, member: &Member) -> Option<SuspendMarker> {
        self.inner
            .markers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(member)
            .cloned()
    }

    /// Record why the member's deliveries wait. The same wait on the same
    /// evidence keeps the time it was first seen, across restarts too, and
    /// shows its latest detail. The marker is the latest attempt's: it can
    /// outlive the wait it names (after a cancel, or once the batch settles)
    /// until the member's next attempt. It is persisted when its cause or
    /// evidence changes, and at most once per failure report interval
    /// otherwise; nothing reads it to decide a transition.
    async fn mark(&self, member: &Member, cause: SuspendCause, evidence: EvidenceClass, now: u64) {
        self.load_markers().await;
        let (marker, changed) = {
            let mut markers = self
                .inner
                .markers
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let same = markers
                .get(member)
                .filter(|marker| marker.evidence == evidence && marker.cause.same_wait(&cause))
                .map(|marker| marker.since_ms);
            let marker = SuspendMarker {
                cause,
                evidence,
                since_ms: same.unwrap_or(now),
            };
            markers.insert(member.clone(), marker.clone());
            (marker, same.is_none())
        };
        let due = {
            let mut writes = self
                .inner
                .marker_writes
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let due = changed
                || writes
                    .get(member)
                    .is_none_or(|at| now.saturating_sub(*at) >= FAILURE_REPORT_INTERVAL_MS);
            if due {
                writes.insert(member.clone(), now);
            }
            due
        };
        if due
            && let Err(error) = self
                .inner
                .store
                .put_member_marker(member, Some(marker), now)
                .await
        {
            tracing::warn!(member = %member, %error, "announcement member marker not persisted");
        }
    }

    /// The member's deliveries move again.
    async fn unmark(&self, member: &Member) {
        self.load_markers().await;
        let removed = self
            .inner
            .markers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(member)
            .is_some();
        self.inner
            .marker_writes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(member);
        if removed
            && let Err(error) = self
                .inner
                .store
                .put_member_marker(member, None, self.now())
                .await
        {
            tracing::warn!(member = %member, %error, "announcement member marker not removed");
        }
    }

    /// Read the persisted markers once, so a wait seen before a restart
    /// keeps its time.
    async fn load_markers(&self) {
        if self.inner.markers_loaded.load(Ordering::SeqCst) {
            return;
        }
        match self.inner.store.member_markers().await {
            Ok(persisted) => {
                let mut markers = self
                    .inner
                    .markers
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                for (member, marker) in persisted {
                    markers.entry(member).or_insert(marker);
                }
                self.inner.markers_loaded.store(true, Ordering::SeqCst);
            }
            Err(error) => {
                tracing::warn!(%error, "announcement member markers not read");
            }
        }
    }

    fn now(&self) -> u64 {
        (self.inner.clock)()
    }

    /// The open escalations, of `subject` when one is named.
    pub async fn open_escalations(
        &self,
        subject: Option<&EscalationSubject>,
    ) -> Result<Vec<Escalation>, AnnouncementError> {
        Ok(self.inner.store.open_escalations(subject).await?)
    }

    /// The escalation events after `after`, and the cursor to read on from.
    pub async fn escalation_events(
        &self,
        after: u64,
        limit: usize,
    ) -> Result<(Vec<EscalationEvent>, u64), AnnouncementError> {
        Ok(self.inner.store.escalation_events(after, limit).await?)
    }

    /// The cursor of the last escalation event.
    pub async fn last_escalation_cursor(&self) -> Result<u64, AnnouncementError> {
        Ok(self.inner.store.last_escalation_cursor().await?)
    }

    /// What members/get reads about `member`.
    pub async fn member_read(&self, member: &Member) -> Result<MemberRead, AnnouncementError> {
        Ok(self.inner.store.member_read(member).await?)
    }

    /// What members/list reads.
    pub async fn member_reads(
        &self,
        after: Option<&Member>,
        limit: usize,
        escalated: bool,
    ) -> Result<Vec<MemberRead>, AnnouncementError> {
        Ok(self
            .inner
            .store
            .member_reads(after, limit, escalated)
            .await?)
    }

    /// The live subscriptions a source's audience and routes bind.
    pub async fn source_bound_subscriptions(
        &self,
        mob_id: &MobId,
        source_id: &SourceId,
    ) -> Result<u32, AnnouncementError> {
        Ok(self
            .inner
            .store
            .source_bound_subscriptions(mob_id, source_id)
            .await?)
    }

    /// meta/status's mob announcement counts.
    pub async fn status_counts(&self) -> Result<StatusCounts, AnnouncementError> {
        Ok(self.inner.store.status_counts(self.now()).await?)
    }

    /// The one health status of `source` now: stale is computed at read.
    pub fn health_status(
        &self,
        source: &super::sources::AnnouncementSource,
    ) -> super::sources::SourceHealthStatus {
        super::escalations::health_status(source, self.now())
    }

    /// Whether this gateway schedules sources' runners: a gateway with a
    /// schedule service installed them.
    pub fn runners_installed(&self) -> bool {
        self.inner
            .runners
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_some()
    }

    /// Whether this gateway delivers to its own members: the fact the
    /// subscription check reads.
    pub fn local_delivery(&self) -> serde_json::Value {
        match self.inner.native.governs_work_authority() {
            Ok(false) => serde_json::json!({ "kind": "ready" }),
            Ok(true) => serde_json::json!({ "kind": "refused", "cause": "governed_runtime" }),
            Err(error) => serde_json::json!({ "kind": "unavailable", "detail": error.0 }),
        }
    }

    /// Ask the dispatcher for a round.
    pub fn wake(&self) {
        self.inner.wake.notify_one();
    }

    /// Stop the dispatcher. Commands still work; nothing is delivered
    /// until a new dispatcher starts.
    pub fn stop(&self) {
        self.inner.stopped.store(true, Ordering::SeqCst);
        self.inner.wake.notify_one();
    }

    /// Whether a stop was requested and not undone by a later start.
    #[cfg(test)]
    pub(crate) fn is_stopped(&self) -> bool {
        self.inner.stopped.load(Ordering::SeqCst)
    }

    /// Quiesce this owner for a candidate launch, one that does not speak for
    /// its mob composition: it boots against the state its promoted,
    /// authoritative launch adopts, or a copy of it, so a batch it delivered
    /// would be a second delivery of work the authoritative launch owes, and
    /// a change it accepted would be made without that authority. Once
    /// quiesced the owner runs no dispatcher (it seals, submits and settles
    /// nothing), follows no mob set, reconciles no declarations and keeps no
    /// source runner schedule, and every change, from the RPC, the member
    /// tools or a collector in this process, is refused with
    /// [`AnnouncementError::CandidateLaunch`] before anything is written.
    /// Reads still answer. It holds for the process: promotion is a fresh
    /// authoritative boot, never an in-process switch.
    pub(crate) fn quiesce_for_candidate(&self) {
        self.inner.quiesced_candidate.store(true, Ordering::SeqCst);
        // A dispatcher already running ends at its next wake.
        self.inner.wake.notify_one();
    }

    /// Whether a candidate launch quiesced this owner.
    pub(crate) fn is_quiesced_candidate(&self) -> bool {
        self.inner.quiesced_candidate.load(Ordering::SeqCst)
    }

    /// Refuse a change on a candidate launch's owner, before anything is
    /// read for it or written.
    fn ensure_authoritative(&self) -> Result<(), AnnouncementError> {
        if self.is_quiesced_candidate() {
            Err(AnnouncementError::CandidateLaunch)
        } else {
            Ok(())
        }
    }

    fn ensure_deliverable(&self) -> Result<(), AnnouncementError> {
        match self.inner.native.governs_work_authority() {
            Ok(false) => Ok(()),
            Ok(true) => Err(AnnouncementError::GovernedRuntime),
            Err(error) => Err(AnnouncementError::NativeUnavailable(error.0)),
        }
    }

    /// Refuse mobs that are not hosted here, from the mob owner's own
    /// records.
    async fn ensure_hosted(
        &self,
        mobs: &BTreeSet<MobId>,
    ) -> Result<BTreeSet<MobId>, AnnouncementError> {
        let presence = self
            .inner
            .native
            .mob_presence(mobs)
            .await
            .map_err(|error| AnnouncementError::NativeUnavailable(error.0))?;
        let gone = mobs
            .iter()
            .filter(|mob_id| presence.get(*mob_id) != Some(&MobPresence::Hosted))
            .cloned()
            .collect::<Vec<_>>();
        if gone.is_empty() {
            Ok(mobs.clone())
        } else {
            Err(AnnouncementError::UnknownMobs(gone))
        }
    }

    /// Announce into a mob hosted here.
    pub async fn announce(
        &self,
        request: AnnounceRequest,
    ) -> Result<AnnounceOutcome, AnnouncementError> {
        self.ensure_authoritative()?;
        if request
            .topics
            .iter()
            .any(|topic| topic.as_str() == super::escalations::HEALTH_TOPIC)
        {
            return Err(AnnouncementError::Store(AnnouncementStoreError::Invalid(
                format!(
                    "the topic {} is reserved for health announcements",
                    super::escalations::HEALTH_TOPIC
                ),
            )));
        }
        self.ensure_hosted(&BTreeSet::from([request.mob_id.clone()]))
            .await?;
        // The roster facts of the members whose subscriptions take a topic a
        // source owns: the store captures only covered members there.
        let mut subscribers = self
            .inner
            .store
            .owned_topic_subscribers(&request.mob_id, &request.topics)
            .await?;
        // The role templates the announcement matches take the members with
        // their role now: a member without a child is checked here, and the
        // store makes its child in the capture.
        let mut templates = self
            .inner
            .store
            .role_templates(Some(&request.mob_id))
            .await?;
        templates.retain(|found| !found.template.topics.is_disjoint(&request.topics));
        let (roles, admitted) = self.expand_roles(&templates).await;
        subscribers.extend(admitted);
        let facts = self.roster_facts(subscribers).await;
        let outcome = self
            .inner
            .store
            .announce_with_roles(request, &facts, &roles, self.now())
            .await?;
        if !outcome.captured_for.is_empty() {
            self.wake();
        }
        Ok(outcome)
    }

    pub async fn announcement(
        &self,
        mob_id: &MobId,
        source_key: &SourceKey,
    ) -> Result<Option<Announcement>, AnnouncementError> {
        Ok(self.inner.store.announcement(mob_id, source_key).await?)
    }

    /// Declare a source of a mob hosted here, replace its declaration at its
    /// revision, or start its next generation. The source verbs are public
    /// so a collector in this process calls them directly, never through
    /// its own gateway's RPC.
    pub async fn put_source(
        &self,
        operation: &OperationId,
        request: PutSource,
    ) -> Result<AnnouncementSource, AnnouncementError> {
        self.ensure_authoritative()?;
        self.ensure_hosted(&BTreeSet::from([request.mob_id.clone()]))
            .await?;
        let source = self
            .inner
            .store
            .put_source(operation, request, self.now())
            .await?;
        self.reconcile_runner(&source).await;
        Ok(source)
    }

    /// Pause, resume or end a source at its revision.
    pub async fn change_source(
        &self,
        operation: &OperationId,
        mob_id: &MobId,
        source_id: &SourceId,
        expected_revision: u64,
        change: SourceChange,
    ) -> Result<AnnouncementSource, AnnouncementError> {
        self.ensure_authoritative()?;
        let source = self
            .inner
            .store
            .change_source(
                operation,
                mob_id,
                source_id,
                expected_revision,
                change,
                self.now(),
            )
            .await?;
        self.reconcile_runner(&source).await;
        Ok(source)
    }

    /// Install the schedule authority sources' runners are kept on, and
    /// bring every runner schedule in line with the sources: one declared
    /// before the install, or while the gateway was down, gets its schedule
    /// now, and one of a source ended meanwhile ends. A failure is logged;
    /// the next install, or a source's next change, reconciles again.
    pub async fn install_source_runners(&self, runners: Arc<dyn SourceRunners>) {
        if self.is_quiesced_candidate() {
            // Runner schedules are the authoritative launch's: a candidate
            // installs none and makes, changes or ends no schedule.
            tracing::info!(
                "mob announcement source runners not installed: this launch is a candidate, so \
                 it keeps no runner schedule and no collector runs from it"
            );
            return;
        }
        let _gate = self.inner.runners_gate.lock().await;
        *self
            .inner
            .runners
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::clone(&runners));
        let mut live = Vec::new();
        for state in [SourceState::Active, SourceState::Paused] {
            let mut after = None;
            loop {
                let page = match self
                    .inner
                    .store
                    .sources(&SourceQuery {
                        mob_id: None,
                        after,
                        state: Some(state),
                        limit: MAX_PAGE,
                    })
                    .await
                {
                    Ok(page) => page,
                    Err(error) => {
                        // A partial read would end live sources' schedules.
                        tracing::warn!(%error, "sources' runner schedules were not reconciled");
                        return;
                    }
                };
                let more = page.len() == MAX_PAGE;
                after = page
                    .last()
                    .map(|source| (source.mob_id.clone(), source.source_id.clone()));
                live.extend(page);
                if !more {
                    break;
                }
            }
        }
        if let Err(error) = runners.reconcile_all(&live).await {
            tracing::warn!(%error, "sources' runner schedules were not reconciled");
        }
    }

    /// Bring the runner schedule of the source `changed` names in line with
    /// its record as it stands now, if a schedule authority is installed.
    /// The record is read again under the gate, so the reconciles of racing
    /// changes end on the last of them. A failure is logged: the change is
    /// committed, and the next change or install reconciles again.
    async fn reconcile_runner(&self, changed: &AnnouncementSource) {
        let _gate = self.inner.runners_gate.lock().await;
        let runners = self
            .inner
            .runners
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let Some(runners) = runners else {
            return;
        };
        let outcome = match self
            .inner
            .store
            .source(&changed.mob_id, &changed.source_id)
            .await
        {
            Ok(Some(source)) => runners.reconcile(&source).await,
            Ok(None) => Ok(()),
            Err(error) => Err(error.to_string()),
        };
        if let Err(error) = outcome {
            tracing::warn!(
                mob_id = %changed.mob_id,
                source_id = %changed.source_id,
                %error,
                "a source's runner schedule was not reconciled"
            );
        }
    }

    /// A source whole, with its cursor's token: what a collector reads
    /// before it collects.
    pub async fn source(
        &self,
        mob_id: &MobId,
        source_id: &SourceId,
    ) -> Result<Option<AnnouncementSource>, AnnouncementError> {
        Ok(self.inner.store.source(mob_id, source_id).await?)
    }

    /// Sources in `(mob_id, source_id)` order, with their cursor tokens
    /// (the RPC list strips them).
    pub async fn sources(
        &self,
        query: &SourceQuery,
    ) -> Result<Vec<AnnouncementSource>, AnnouncementError> {
        Ok(self.inner.store.sources(query).await?)
    }

    /// Advance a source of a mob hosted here by one collector step: its
    /// items, their captures and its cursor commit together or not at all.
    /// A retry of the step with its operation id answers the original,
    /// marked replayed.
    pub async fn advance_source(
        &self,
        operation: &OperationId,
        request: AdvanceSource,
    ) -> Result<SourceAdvance, AnnouncementError> {
        self.ensure_authoritative()?;
        self.ensure_hosted(&BTreeSet::from([request.mob_id.clone()]))
            .await?;
        let mut subscribers = self
            .inner
            .store
            .source_subscribers(&request.mob_id, &request.source_id)
            .await?;
        // Only the templates the source's own or forward topics reach.
        let templates = self
            .inner
            .store
            .source_role_templates(&request.mob_id, &request.source_id)
            .await?;
        let (roles, admitted) = self.expand_roles(&templates).await;
        subscribers.extend(admitted);
        let facts = self.roster_facts(subscribers).await;
        let subject = EscalationSubject::Source {
            mob_id: request.mob_id.clone(),
            source_id: request.source_id.clone(),
        };
        let advance = self
            .inner
            .store
            .advance_source_with_roles(operation, request, &facts, &roles, self.now())
            .await?;
        super::faults::after_source_advance();
        // An advance can end what an open escalation of its source reports
        // (a failure streak, a quiet window): the next round settles it
        // rather than the dispatcher's longest idle.
        let escalated = match self.inner.store.open_escalations(Some(&subject)).await {
            Ok(open) => !open.is_empty(),
            // Not read: the next round settles it all the same.
            Err(_) => true,
        };
        if advance.captured || escalated {
            self.wake();
        }
        Ok(advance)
    }

    /// Record a source collector's failure at the revision and cursor
    /// version it read.
    pub async fn record_source_failure(
        &self,
        operation: &OperationId,
        request: RecordSourceFailure,
    ) -> Result<AnnouncementSource, AnnouncementError> {
        self.ensure_authoritative()?;
        let source = self
            .inner
            .store
            .record_source_failure(operation, request, self.now())
            .await?;
        self.wake();
        Ok(source)
    }

    /// Audited refusals and withholdings of sources' audiences and routes.
    pub async fn audit(&self, query: &AuditQuery) -> Result<Vec<AuditRow>, AnnouncementError> {
        Ok(self.inner.store.audit(query).await?)
    }

    /// The member's roster facts, or none when its roster cannot be read
    /// now: the store then covers it by member grants alone.
    async fn member_facts(&self, member: &Member) -> Option<MemberFacts> {
        match self.inner.native.member_facts(member).await {
            Ok(facts) => facts,
            Err(error) => {
                tracing::warn!(
                    %member,
                    error = %error.0,
                    "a member's roster facts could not be read; a member grant alone covers it"
                );
                None
            }
        }
    }

    /// The roster facts of `members`, as far as they can be read.
    async fn roster_facts(&self, members: BTreeSet<Member>) -> RosterFacts {
        let mut facts = RosterFacts::default();
        for member in members {
            if let Some(found) = self.member_facts(&member).await {
                facts.insert(member, found);
            }
        }
        facts
    }

    /// The streams a source's current generation onboarded.
    pub async fn source_streams(
        &self,
        mob_id: &MobId,
        source_id: &SourceId,
    ) -> Result<Vec<SourceStream>, AnnouncementError> {
        Ok(self.inner.store.source_streams(mob_id, source_id).await?)
    }

    pub async fn put_subscription(
        &self,
        operation: &OperationId,
        mut request: PutSubscription,
    ) -> Result<Subscription, AnnouncementError> {
        self.ensure_authoritative()?;
        let route = self.member_route(&request.member).await?;
        // A peer answers for its own runtime when a batch reaches it.
        if route.route != MemberRoute::Peer {
            self.ensure_deliverable()?;
        }
        // A member of a mob a member created is subscribed at that mob's
        // current event cursor, read before the member is checked: a
        // completion or reset the check does not see is in the log after it
        // and ends the subscription.
        request.member_mob_cursor = match route.route {
            MemberRoute::Child => Some(
                self.inner
                    .native
                    .member_mob_cursor(&request.member.mob_id)
                    .await
                    .map_err(|error| AnnouncementError::NativeUnavailable(error.0))?,
            ),
            MemberRoute::Local | MemberRoute::Peer => None,
        };
        if let Err(problem) = self
            .inner
            .native
            .check_member(&request.member, route.route)
            .await
        {
            return Err(member_problem(&request.member, problem));
        }
        request.route = Some(route);
        request.child_limits = self.inner.config.child_limits;
        let mobs = self.ensure_hosted(&request.selector.mobs).await?;
        let member_mob = request.member.mob_id.clone();
        // A topic a source owns takes the member only if its audience
        // covers it, as the member's roster facts say.
        let facts = self.member_facts(&request.member).await;
        let subscription = self
            .inner
            .store
            .put_subscription_with_facts(operation, request, facts, self.now())
            .await
            .map_err(|error| match error {
                // Another put kept the member on another route meanwhile.
                AnnouncementStoreError::RouteKept { .. } => {
                    AnnouncementError::AmbiguousMob(member_mob)
                }
                other => AnnouncementError::Store(other),
            })?;
        // A mob destroyed between the check and the commit would otherwise
        // stay named until the mob set changes again.
        if let Err(error) = self.drop_ended(&mobs).await {
            tracing::warn!(%error, "destroyed mobs not reconciled after a subscription change");
        }
        self.wake();
        Ok(subscription)
    }

    /// The route a put subscribes the member on: the route it is kept on,
    /// or the route it takes now when it has none. A kept route never
    /// changes. A member kept for a mob a member created that is now gone or
    /// created again first ends there and loses its kept route; a member
    /// whose mob id would now route another way is refused as ambiguous.
    async fn member_route(&self, member: &Member) -> Result<MemberRouteRecord, AnnouncementError> {
        let current = self
            .inner
            .native
            .route_for(member)
            .await
            .map_err(|problem| member_problem(member, problem))?;
        let Some(kept) = self.inner.store.member_route(member).await? else {
            return Ok(current);
        };
        if kept == current {
            return Ok(kept);
        }
        if kept.route != MemberRoute::Child {
            return Err(AnnouncementError::AmbiguousMob(member.mob_id.clone()));
        }
        let why = if current.route == MemberRoute::Child {
            "created again"
        } else {
            "destroyed"
        };
        let ended = self
            .inner
            .store
            .unbind_member_route(
                member,
                SubscriptionEnd::MemberMobEnded {
                    why: why.to_string(),
                },
                self.now(),
            )
            .await?;
        tracing::info!(
            member = %member,
            why,
            subscriptions = ended.len(),
            "member's mob ended; its subscriptions ended there"
        );
        Ok(current)
    }

    /// Create or replace a subscription a member asks for itself: every mob
    /// it names must be its own mob, above it, or below it in the lineage
    /// of mobs members created. A named mob without a live lineage record
    /// is derived first. A mob still outside that reach is refused with
    /// [`AnnouncementError::UnrelatedMob`], and nothing changes.
    pub async fn put_member_subscription(
        &self,
        operation: &OperationId,
        request: PutSubscription,
    ) -> Result<Subscription, AnnouncementError> {
        // Before the reach check, which records the lineage of a mob seen for
        // the first time.
        self.ensure_authoritative()?;
        self.ensure_within_reach(&request.member.mob_id, &request.selector.mobs)
            .await?;
        self.put_subscription(operation, request).await
    }

    /// The mobs a member of `own` may name in a subscription it makes
    /// itself, by the live lineage records.
    pub async fn reach(&self, own: &MobId) -> Result<BTreeSet<MobId>, AnnouncementError> {
        Ok(reach_from(&self.inner.store.lineage().await?, own))
    }

    async fn ensure_within_reach(
        &self,
        own: &MobId,
        mobs: &BTreeSet<MobId>,
    ) -> Result<(), AnnouncementError> {
        let records = self.inner.store.lineage().await?;
        if mobs.is_subset(&reach_from(&records, own)) {
            return Ok(());
        }
        // A mob seen for the first time gets its record before the check.
        let recorded = records
            .into_iter()
            .map(|record| (record.mob_id, record.incarnation))
            .collect::<BTreeMap<_, _>>();
        for mob_id in mobs.iter().chain([own]) {
            if !recorded.contains_key(mob_id) {
                self.derive_lineage(mob_id, None).await?;
            }
        }
        let reach = self.reach(own).await?;
        match mobs.iter().find(|mob_id| !reach.contains(*mob_id)) {
            Some(mob_id) => Err(AnnouncementError::UnrelatedMob(mob_id.clone())),
            None => Ok(()),
        }
    }

    /// Record the lineage of the hosted mob `mob_id` unless its live record
    /// is already for the incarnation hosted now: a relation is derived
    /// once. A missing parent is final only once every mob this gateway
    /// hosts has been restored; before that nothing is written. A read
    /// that fails writes nothing.
    async fn derive_lineage(
        &self,
        mob_id: &MobId,
        recorded: Option<&MobIncarnation>,
    ) -> Result<(), AnnouncementError> {
        // This gateway's own mob is the root: it has no record.
        if self.inner.native.own_mob().as_ref() == Some(mob_id) {
            return Ok(());
        }
        let Some(facts) = self
            .inner
            .native
            .mob_origin(mob_id)
            .await
            .map_err(|error| AnnouncementError::NativeUnavailable(error.0))?
        else {
            return Ok(());
        };
        if recorded == Some(&facts.incarnation) {
            return Ok(());
        }
        let (parent, parent_incarnation, creator) = match facts.origin {
            MobOrigin::Member {
                parent,
                creator,
                parent_incarnation,
            } => (Some(parent), parent_incarnation, Some(creator)),
            MobOrigin::Unproven(detail) => {
                tracing::info!(mob = %mob_id, %detail, "mob has no member origin");
                (None, None, None)
            }
            MobOrigin::ParentNotHosted(parent) => {
                if !self.inner.native.mobs_restored() {
                    return Ok(());
                }
                tracing::info!(mob = %mob_id, parent = %parent, "the mob's parent is gone");
                (None, None, None)
            }
        };
        let now = self.now();
        self.inner
            .store
            .record_lineage(
                &MobLineage {
                    mob_id: mob_id.clone(),
                    incarnation: facts.incarnation,
                    parent,
                    parent_incarnation,
                    creator,
                    recorded_at_ms: now,
                },
                now,
            )
            .await?;
        Ok(())
    }

    /// Derive the lineage of every hosted mob whose current incarnation has
    /// no live record, and end the records of mobs no longer hosted. A mob
    /// whose read fails is tried again at the next reconcile; the others
    /// are recorded meanwhile.
    pub async fn reconcile_lineage(&self) -> Result<(), AnnouncementError> {
        self.ensure_authoritative()?;
        let hosted = self
            .inner
            .native
            .hosted_mob_ids()
            .await
            .map_err(|error| AnnouncementError::NativeUnavailable(error.0))?;
        let records = self.inner.store.lineage().await?;
        let gone = records
            .iter()
            .filter(|record| !hosted.contains(&record.mob_id))
            .map(|record| record.mob_id.clone())
            .collect::<BTreeSet<_>>();
        if !gone.is_empty() {
            self.inner.store.end_lineage(&gone, self.now()).await?;
        }
        let recorded = records
            .into_iter()
            .map(|record| (record.mob_id, record.incarnation))
            .collect::<BTreeMap<_, _>>();
        let mut failed = None;
        for mob_id in &hosted {
            if let Err(error) = self.derive_lineage(mob_id, recorded.get(mob_id)).await {
                tracing::warn!(mob = %mob_id, %error, "mob lineage not derived yet");
                failed = Some(error);
            }
        }
        failed.map_or(Ok(()), Err)
    }

    /// Drop the mobs among `mobs` that are no longer hosted here from every
    /// live subscription that names them; a subscription left with none
    /// ends. Returns the subscriptions that changed.
    async fn drop_ended(
        &self,
        mobs: &BTreeSet<MobId>,
    ) -> Result<Vec<SubscriptionId>, AnnouncementError> {
        if mobs.is_empty() {
            return Ok(Vec::new());
        }
        let presence = self
            .inner
            .native
            .mob_presence(mobs)
            .await
            .map_err(|error| AnnouncementError::NativeUnavailable(error.0))?;
        let gone = mobs
            .iter()
            .filter(|mob_id| presence.get(*mob_id) == Some(&MobPresence::Gone))
            .cloned()
            .collect::<BTreeSet<_>>();
        if gone.is_empty() {
            return Ok(Vec::new());
        }
        let changed = self.inner.store.drop_ended_mobs(&gone, self.now()).await?;
        if !changed.is_empty() {
            self.wake();
        }
        Ok(changed)
    }

    /// Reconcile every mob the store knows against the mob owner's records:
    /// destroyed mobs leave the subscriptions that name them, and their logs
    /// are marked ended for retention. Then reconcile the declarations and
    /// the children of role templates ([`Self::reconcile_declarations`]),
    /// and the lineage records ([`Self::reconcile_lineage`]).
    pub async fn reconcile_mobs(&self) -> Result<Vec<SubscriptionId>, AnnouncementError> {
        self.ensure_authoritative()?;
        let known = self.inner.store.known_mobs().await?;
        let changed = self.drop_ended(&known).await?;
        self.reconcile_child_members().await?;
        // A mob set change can admit a pending declared record, and move
        // members in and out of role templates' roles.
        if let Err(error) = self.reconcile_declarations().await {
            tracing::warn!(%error, "declarations not reconciled after a mob set change");
        }
        self.reconcile_lineage().await?;
        Ok(changed)
    }

    /// End the subscriptions of members kept for a mob a member created
    /// that is now gone or was created again. A mob that cannot be read now
    /// (held until its owner resolves, or unavailable) is left to the next
    /// reconcile or delivery.
    async fn reconcile_child_members(&self) -> Result<(), AnnouncementError> {
        let routes = self.inner.store.child_member_routes().await?;
        let mut gone: BTreeMap<(MobId, Option<MobIncarnation>), Option<String>> = BTreeMap::new();
        let now = self.now();
        for (member, kept) in routes {
            let key = (member.mob_id.clone(), kept.incarnation.clone());
            let why = match gone.get(&key) {
                Some(why) => why.clone(),
                None => {
                    match self
                        .inner
                        .native
                        .member_mob_gone(&member.mob_id, kept.incarnation.as_ref())
                        .await
                    {
                        Ok(end) => {
                            let why = end.map(|end| end.why);
                            gone.insert(key, why.clone());
                            why
                        }
                        Err(error) => {
                            tracing::warn!(
                                mob = %member.mob_id,
                                error = %error,
                                "member's mob not read; its members are reconciled later"
                            );
                            gone.insert(key, None);
                            None
                        }
                    }
                }
            };
            if let Some(why) = why {
                self.end_member(&member, SubscriptionEnd::MemberMobEnded { why }, now)
                    .await;
            }
        }
        Ok(())
    }

    /// Follow the mob owner's set of mobs: reconcile once now, then on every
    /// change. A reconcile that cannot read the mob owner is tried again
    /// after the slowest settle pause. Holds the service weakly and ends
    /// with it; `None` when nothing signals mob set changes, or when a
    /// candidate launch quiesced the owner: its reconciles would write.
    pub fn watch_mobs(&self) -> Option<tokio::task::JoinHandle<()>> {
        if self.is_quiesced_candidate() {
            return None;
        }
        let mut changes = self.inner.native.mob_set_changes()?;
        let weak = Arc::downgrade(&self.inner);
        Some(tokio::spawn(async move {
            loop {
                let retry = {
                    let Some(inner) = weak.upgrade() else {
                        return;
                    };
                    if inner.stopped.load(Ordering::SeqCst)
                        || inner.quiesced_candidate.load(Ordering::SeqCst)
                    {
                        return;
                    }
                    let service = AnnouncementService { inner };
                    match service.reconcile_mobs().await {
                        Ok(_) => None,
                        Err(error) => {
                            tracing::warn!(%error, "destroyed mobs not reconciled; retrying");
                            Some(Duration::from_millis(
                                service.inner.config.settle_check_max_ms,
                            ))
                        }
                    }
                };
                let changed = match retry {
                    None => changes.changed().await.is_ok(),
                    Some(pause) => tokio::select! {
                        changed = changes.changed() => changed.is_ok(),
                        () = tokio::time::sleep(pause) => true,
                    },
                };
                if !changed {
                    return;
                }
            }
        }))
    }

    /// End every live subscription of a member that is gone for good.
    async fn end_member(&self, member: &Member, end: SubscriptionEnd, now: u64) {
        self.unmark(member).await;
        match self.inner.store.end_member(member, end.clone(), now).await {
            Ok(ended) if !ended.is_empty() => {
                tracing::info!(
                    member = %member,
                    reason = %end,
                    subscriptions = ended.len(),
                    "member subscriptions ended"
                );
            }
            Ok(_) => {}
            Err(error) => {
                tracing::warn!(member = %member, %error, "member subscriptions not ended");
            }
        }
    }

    pub async fn change_subscription(
        &self,
        operation: &OperationId,
        id: &SubscriptionId,
        expected_revision: u64,
        change: SubscriptionChange,
    ) -> Result<SubscriptionChangeOutcome, AnnouncementError> {
        self.ensure_authoritative()?;
        let outcome = self
            .inner
            .store
            .change_subscription(operation, id, expected_revision, change, self.now())
            .await?;
        self.wake();
        Ok(outcome)
    }

    pub async fn subscription(
        &self,
        id: &SubscriptionId,
    ) -> Result<Option<Subscription>, AnnouncementError> {
        Ok(self.inner.store.subscription(id).await?)
    }

    pub async fn subscriptions(
        &self,
        member: Option<&Member>,
        after: Option<&SubscriptionId>,
        limit: usize,
    ) -> Result<Vec<Subscription>, AnnouncementError> {
        Ok(self.inner.store.subscriptions(member, after, limit).await?)
    }

    pub async fn deliveries(&self, query: &BatchQuery) -> Result<Vec<Batch>, AnnouncementError> {
        Ok(self.inner.store.batches(query).await?)
    }

    /// [`Self::deliveries`], each batch with its handling once it was
    /// admitted.
    pub async fn deliveries_with_handling(
        &self,
        query: &BatchQuery,
    ) -> Result<Vec<(Batch, Option<BatchHandling>)>, AnnouncementError> {
        let batches = self.inner.store.batches(query).await?;
        let mut handlings = self
            .inner
            .store
            .handlings(batches.iter().map(|batch| batch.id.clone()).collect())
            .await?;
        Ok(batches
            .into_iter()
            .map(|batch| {
                let handling = handlings.remove(&batch.id);
                (batch, handling)
            })
            .collect())
    }

    /// An operator's remedy for a delivered batch whose turn ended
    /// unhandled and whose items are held: each held item is checked afresh
    /// against its member's coverage, and what passes is owed again, under
    /// a notice, for the member's next batch. A roster that cannot be read
    /// decides nothing: the remedy is refused, to be asked again. A retry
    /// answers what the first call recorded, whatever the roster says now.
    pub async fn redeliver_held(
        &self,
        operation: &OperationId,
        id: &BatchId,
    ) -> Result<BatchHandling, AnnouncementError> {
        self.ensure_authoritative()?;
        if let Some(recorded) = self.inner.store.redeliver_receipt(operation, id).await? {
            return Ok(recorded);
        }
        let batch = self
            .inner
            .store
            .batch(id)
            .await?
            .ok_or(AnnouncementStoreError::UnknownBatch)?;
        let facts = self
            .member_facts_read(&batch.member)
            .await
            .map_err(|error| AnnouncementError::NativeUnavailable(error.0))?;
        let handling = self
            .inner
            .store
            .redeliver_held(operation, id, facts, self.now())
            .await?;
        self.wake();
        Ok(handling)
    }

    /// An operator's dismissal of what a delivered batch holds after its
    /// turn ended unhandled: each held item settles as handled by an
    /// operator and is never delivered again.
    pub async fn dismiss_held(
        &self,
        operation: &OperationId,
        id: &BatchId,
    ) -> Result<BatchHandling, AnnouncementError> {
        self.ensure_authoritative()?;
        Ok(self
            .inner
            .store
            .dismiss_held(operation, id, self.now())
            .await?)
    }

    /// The delivery's immutable batch, its owner state, and the native
    /// owner's live status. Reads only; serves and changes nothing.
    pub async fn observe_delivery(
        &self,
        id: &BatchId,
    ) -> Result<DeliveryObservation, AnnouncementError> {
        let batch = self
            .inner
            .store
            .batch(id)
            .await?
            .ok_or(AnnouncementStoreError::UnknownBatch)?;
        self.observe(batch).await
    }

    /// Advance one delivery as far as native evidence allows now, then
    /// observe it.
    pub async fn reconcile_delivery(
        &self,
        id: &BatchId,
    ) -> Result<DeliveryObservation, AnnouncementError> {
        // Driving a batch submits it: a candidate observes instead.
        self.ensure_authoritative()?;
        let batch = {
            let mut walk = self.inner.round.lock().await;
            let batch = self
                .inner
                .store
                .batch(id)
                .await?
                .ok_or(AnnouncementStoreError::UnknownBatch)?;
            let position = BatchPosition::of(&batch);
            let now = self.now();
            let due = self.drive(batch, now).await?;
            // The walk paces its next look at the batch from this read; a
            // batch this settled leaves the walk.
            walk.read(position, due);
            let batch = self
                .inner
                .store
                .batch(id)
                .await?
                .ok_or(AnnouncementStoreError::UnknownBatch)?;
            // A batch that settled without its delivery being confirmed is
            // read once more for what became of it. An operator's reconcile
            // reads it even after its own late reads ended; an answer that
            // leaves it unknown schedules them again, one per quiet window,
            // until retention prunes the batch.
            if BatchStateTag::UNCONFIRMED.contains(&batch.state.tag())
                && batch.state.late().is_none_or(|late| !late.is_final())
            {
                self.read_late_one(&batch, now).await?;
                self.inner
                    .store
                    .batch(id)
                    .await?
                    .ok_or(AnnouncementStoreError::UnknownBatch)?
            } else {
                batch
            }
        };
        // A batch this settled frees its member for the next one now,
        // not at the dispatcher's next timer.
        self.wake();
        self.observe(batch).await
    }

    async fn observe(&self, batch: Batch) -> Result<DeliveryObservation, AnnouncementError> {
        let marker = self.suspend_marker(&batch.member);
        let handling = self.inner.store.handling(&batch.id).await?;
        if batch.state.tag() == BatchStateTag::Sealed {
            return Ok(DeliveryObservation {
                batch,
                native: NativeObservation::NotSubmitted,
                input: None,
                handling,
                marker,
            });
        }
        // Nothing is read for a batch whose route contradicts its member,
        // here as on every drive path: the one guard answers instead.
        if let Some(refusal) = self.route_refusal(&batch) {
            return Ok(DeliveryObservation {
                batch,
                native: NativeObservation::Unavailable {
                    detail: refusal.to_string(),
                },
                input: None,
                handling,
                marker,
            });
        }
        let native = match self.inner.native.status(&batch).await {
            Err(error) => NativeObservation::Unavailable {
                detail: error.to_string(),
            },
            Ok(status) => Self::native_observation(status),
        };
        let input = match &native {
            NativeObservation::Applied {
                session_id,
                input_id,
                ..
            } => match self.inner.native.input(&batch, session_id, input_id).await {
                Ok(input) => input,
                Err(error) => Some(serde_json::json!({ "unavailable": error.0 })),
            },
            _ => None,
        };
        Ok(DeliveryObservation {
            batch,
            native,
            input,
            handling,
            marker,
        })
    }

    fn native_observation(status: NativeStatus) -> NativeObservation {
        match status {
            NativeStatus::NotCommitted => NativeObservation::NotCommitted,
            NativeStatus::Pending { receipt, admitted } => {
                NativeObservation::Pending { receipt, admitted }
            }
            NativeStatus::Applied {
                receipt,
                session_id,
                input_id,
            } => NativeObservation::Applied {
                receipt,
                session_id,
                input_id,
            },
            NativeStatus::Stranded { receipt } => NativeObservation::Stranded { receipt },
            NativeStatus::Refused { receipt, reason } => {
                NativeObservation::Refused { receipt, reason }
            }
        }
    }

    /// One dispatcher round: advance a window of unsettled batches from
    /// native evidence, then seal and deliver due backlogs.
    pub async fn run_round(&self) -> Result<RoundReport, AnnouncementError> {
        self.ensure_authoritative()?;
        let mut guard = self.inner.round.lock().await;
        let walk = &mut *guard;
        let now = self.now();
        let config = &self.inner.config;
        let mut report = RoundReport::default();
        // No next look is ever written further ahead than the slowest settle
        // pause, and no report is in the future; one that is predates a step
        // back of the clock.
        walk.due
            .retain(|_, at| *at > now && *at - now <= config.settle_check_max_ms);
        walk.reported
            .retain(|_, at| *at <= now && now - *at < FAILURE_REPORT_INTERVAL_MS);
        if walk.position.is_none() {
            walk.lap_end = self.inner.store.newest_unsettled_batch().await?;
        }
        let mut window = self
            .inner
            .store
            .unsettled_batches(walk.position.as_ref(), ROUND_BATCHES)
            .await?;
        let full = window.len() == ROUND_BATCHES;
        walk.drop_settled(&window, full);
        let lap_end = walk.lap_end.clone();
        window.retain(|batch| {
            lap_end
                .as_ref()
                .is_some_and(|end| BatchPosition::of(batch) <= *end)
        });
        // A full window short of the lap's end resumes after it in the next
        // round, soon. Anything else ends the lap, which starts over.
        walk.position = match window.last().map(BatchPosition::of) {
            Some(last) if full && lap_end.as_ref().is_some_and(|end| last < *end) => {
                report.next_due_ms = Some(now.saturating_add(config.settle_check_min_ms));
                Some(last)
            }
            _ => {
                // The lap ends: the next one starts by the time the first
                // batch it remembers is due again.
                report.next_due_ms = min_due(report.next_due_ms, walk.due.values().min().copied());
                None
            }
        };
        // A batch or backlog the round cannot advance is reported and
        // retried at the slowest pace; it never ends the round for the
        // others.
        for batch in window {
            let position = BatchPosition::of(&batch);
            if let Some(&at) = walk.due.get(&position) {
                report.next_due_ms = min_due(report.next_due_ms, Some(at));
                continue;
            }
            match self.advance(batch, now).await {
                Ok((moved, due)) => {
                    report.advanced += u32::from(moved);
                    self.read_again(walk, &mut report, position, due);
                }
                Err(error) => self.retry_later(walk, &mut report, position, now, &error),
            }
        }
        // What became of a delivery that was not confirmed in time is read
        // again at the quiet window's pace; a failing read waits for the
        // next round and holds back nothing else.
        if let Err(error) = self.read_late(now).await {
            tracing::warn!(%error, "announcement late reads failed this round");
        }
        // What became of each admitted batch's turn is read at the settle
        // pace until a final read; a failing read waits for the next round,
        // and the round after it comes no sooner than the shortest pause.
        match self.read_handling(now).await {
            Ok(next) => {
                let next = next.map(|at| at.max(now.saturating_add(config.settle_check_min_ms)));
                report.next_due_ms = min_due(report.next_due_ms, next);
            }
            Err(error) => {
                tracing::warn!(%error, "announcement handling reads failed this round");
            }
        }
        // Held items are checked afresh before the backlogs are read, so
        // what a check releases seals in this round.
        if let Err(error) = self.recheck_holds(now).await {
            tracing::warn!(%error, "held announcements were not checked this round");
        }
        let policy = config.policy;
        let backlogs = self.inner.store.backlogs_at(ROUND_MEMBERS, now).await?;
        walk.unsealed
            .retain(|member| backlogs.iter().any(|backlog| backlog.member == *member));
        for backlog in backlogs {
            match policy.due_at(&backlog) {
                Some(due) if due <= now => {
                    let id = new_batch_id()?;
                    let sealed = self.seal_backlog(&backlog.member, &id, &policy, now).await;
                    if sealed.is_ok() {
                        walk.unsealed.remove(&backlog.member);
                    }
                    match sealed {
                        Ok(Some(batch)) => {
                            report.sealed += 1;
                            let position = BatchPosition::of(&batch);
                            match self.drive(batch, now).await {
                                Ok(due) => self.read_again(walk, &mut report, position, due),
                                Err(error) => {
                                    self.retry_later(walk, &mut report, position, now, &error);
                                }
                            }
                        }
                        Ok(None) => {}
                        Err(error) => {
                            // Reported once while it keeps failing: a mob
                            // held until its owner resolves fails every
                            // round until it runs again.
                            if walk.unsealed.insert(backlog.member.clone()) {
                                tracing::warn!(
                                    member = %backlog.member,
                                    %error,
                                    "announcement backlog not sealed this round"
                                );
                            } else {
                                tracing::debug!(
                                    member = %backlog.member,
                                    %error,
                                    "announcement backlog still not sealed"
                                );
                            }
                            report.next_due_ms = min_due(
                                report.next_due_ms,
                                Some(now.saturating_add(config.settle_check_max_ms)),
                            );
                        }
                    }
                }
                Some(due) => report.next_due_ms = min_due(report.next_due_ms, Some(due)),
                None => {}
            }
        }
        self.escalation_pass(now).await;
        Ok(report)
    }

    /// One escalation pass: raise and resolve every source and member
    /// condition from the records as they stand, notify open escalations
    /// again when due, and push the events it wrote to the host. A failure
    /// leaves the next round to try again.
    async fn escalation_pass(&self, now: u64) {
        self.load_markers().await;
        if self
            .inner
            .pushed_cursor
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_none()
        {
            match self.inner.store.last_escalation_cursor().await {
                Ok(cursor) => {
                    *self
                        .inner
                        .pushed_cursor
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(cursor);
                }
                Err(error) => {
                    tracing::warn!(%error, "announcement escalation cursor not read");
                    return;
                }
            }
        }
        let facts = match self.inner.store.health_subscribers().await {
            Ok(members) => self.roster_facts(members).await,
            Err(error) => {
                tracing::warn!(%error, "announcement health subscribers not read");
                RosterFacts::default()
            }
        };
        // Health is announced only into the mobs hosted here: this gateway's
        // own and the others it hosts.
        let hosted = match self.inner.native.hosted_mob_ids().await {
            Ok(mut hosted) => {
                hosted.extend(self.inner.native.own_mob());
                hosted
            }
            Err(error) => {
                tracing::warn!(
                    error = %error.0,
                    "hosted mobs not read; no health is announced this round"
                );
                BTreeSet::new()
            }
        };
        // A pending declared record is retried at every health pass, and
        // what it raised or resolved is pushed with the rest.
        self.retry_pending_declared().await;
        let pass = EscalationPass {
            renotify_after_ms: self.inner.config.renotify_after_ms,
            health_audience: self.inner.config.health_audience.clone(),
            retention_ms: self.inner.config.retention_ms,
            handled_unverified_threshold: self.inner.config.handled_unverified_threshold,
            hosted,
        };
        if let Err(error) = self.inner.store.escalation_pass(pass, &facts, now).await {
            tracing::warn!(%error, "announcement escalation pass failed; the next round retries");
        }
        self.push_escalations().await;
    }

    /// Push every escalation event after the last one pushed to the host's
    /// error sink, in cursor order.
    async fn push_escalations(&self) {
        let sink = self
            .inner
            .error_sink
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        loop {
            let Some(after) = *self
                .inner
                .pushed_cursor
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
            else {
                return;
            };
            let page = match self
                .inner
                .store
                .escalation_notices(after, super::types::MAX_PAGE)
                .await
            {
                Ok(page) => page,
                Err(error) => {
                    tracing::warn!(%error, "announcement escalation events not read");
                    return;
                }
            };
            for notice in page.notices {
                let cursor = notice.event.cursor;
                if let Some(sink) = &sink {
                    sink(escalation_error_event(notice));
                }
                *self
                    .inner
                    .pushed_cursor
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(cursor);
            }
            // Past every event read: one whose escalation is gone included.
            *self
                .inner
                .pushed_cursor
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(page.next);
            if !page.full {
                return;
            }
        }
    }

    /// Seal a member's due backlog into one batch on its kept route. The
    /// log of a member's own mob, for a member of a mob a member created,
    /// is read first: a subscription that mob's end ended carries nothing
    /// into the batch, so a batch never mixes what was captured before a
    /// reset with what a subscription made after it wants.
    async fn seal_backlog(
        &self,
        member: &Member,
        id: &BatchId,
        policy: &SealingPolicy,
        now: u64,
    ) -> Result<Option<Batch>, AnnouncementError> {
        // The store seals on the route it keeps, in its own transaction, and
        // takes the batch's wire budget from it; it refuses a member without
        // one, since no route is assumed. Here the kept route only says
        // whether a child member's mob log is read first.
        let route = self
            .inner
            .store
            .member_route(member)
            .await?
            .map(|kept| kept.route);
        if route == Some(MemberRoute::Child) {
            self.member_mob_ended(member, now).await?;
        }
        // The store checks what it seals, and what it holds, against the
        // member's coverage and routes as its roster says now. A roster
        // that cannot be read decides nothing: the seal waits for a read.
        let facts = match self.member_facts_read(member).await {
            Ok(facts) => facts,
            Err(error) => {
                tracing::warn!(
                    %member,
                    error = %error.0,
                    "a member's roster could not be read; its seal waits"
                );
                return Ok(None);
            }
        };
        Ok(self
            .inner
            .store
            .seal_batch_with_facts(member, id, policy, facts, now)
            .await?)
    }

    /// A member's roster facts, or the error that kept them from being
    /// read: a check that holds or releases items never takes an
    /// unreadable roster for an uncovered member.
    async fn member_facts_read(
        &self,
        member: &Member,
    ) -> Result<Option<MemberFacts>, super::native::NativeUnavailable> {
        self.inner.native.member_facts(member).await
    }

    /// Check afresh the held items of every member due a check: one whose
    /// holds were last checked a bound ago, so a held item whose member
    /// passes again is released though nothing new comes for it. Each
    /// member is checked on its own: one whose roster cannot be read, or
    /// whose check fails, is checked again at a later round, and the
    /// members after it are still checked in this one.
    async fn recheck_holds(&self, now: u64) -> Result<(), AnnouncementError> {
        let members = self
            .inner
            .store
            .members_with_holds_due(now.saturating_sub(HOLD_RECHECK_MS), ROUND_MEMBERS)
            .await?;
        for member in members {
            match self.member_facts_read(&member).await {
                Ok(facts) => {
                    if let Err(error) = self
                        .inner
                        .store
                        .release_holds(&member, facts, self.inner.config.policy.max_count, now)
                        .await
                    {
                        tracing::warn!(
                            %member,
                            %error,
                            "a member's held items could not be checked; they wait"
                        );
                    }
                }
                Err(error) => tracing::warn!(
                    %member,
                    error = %error.0,
                    "a member's roster could not be read; its held items wait"
                ),
            }
        }
        Ok(())
    }

    /// For a batch to a peer that nothing has settled (submitting, or
    /// sealed): whether a request for it may still commit, inside the quiet
    /// window after its last change. A sealed batch's record does not say
    /// whether an earlier submission was answered that nothing was
    /// committed, so every sealed peer batch waits that window out before a
    /// failing check cancels it: a delay, never a leak. `None` for any
    /// other batch.
    fn peer_request_open(&self, batch: &Batch, now: u64) -> Option<bool> {
        if batch.route != MemberRoute::Peer {
            return None;
        }
        matches!(
            batch.state,
            BatchState::Submitting { .. } | BatchState::Sealed
        )
        .then(|| now <= self.peer_window_end(batch))
    }

    /// When the quiet window after a peer batch's last request ends.
    fn peer_window_end(&self, batch: &Batch) -> u64 {
        batch.updated_at_ms.saturating_add(self.quiet_ms())
    }

    /// The last check before a native submission: every item of `batch` is
    /// checked again against its member's coverage and routes. With
    /// `may_cancel`, a failing item cancels the batch (its failing items
    /// are held and the rest reopen); without it, the failure comes back
    /// and nothing changes. A roster that cannot be read defers it.
    async fn last_check(
        &self,
        batch: &Batch,
        may_cancel: bool,
        now: u64,
    ) -> Result<LastCheck, AnnouncementError> {
        let facts = match self.member_facts_read(&batch.member).await {
            Ok(facts) => facts,
            Err(error) => {
                tracing::warn!(
                    batch_id = %batch.id,
                    error = %error.0,
                    "a member's roster could not be read; the batch's last check waits"
                );
                return Ok(LastCheck::Deferred);
            }
        };
        Ok(
            match self
                .inner
                .store
                .check_before_submission(&batch.id, batch.state.tag(), facts, may_cancel, now)
                .await?
            {
                SubmissionCheck::Passed => LastCheck::Passed,
                SubmissionCheck::Cancelled(cancelled) => LastCheck::Cancelled(cancelled),
                _ => LastCheck::Failed,
            },
        )
    }

    /// Count what an admitted batch delivered past a revocation: on a
    /// gateway without a governed binder nothing Meta controls stands
    /// between a commit and its admission, so an item its member stopped
    /// passing between them still reached it. A failure to count is
    /// logged; the delivery stands.
    async fn audit_delivered_across_revocation(&self, batch: &Batch, now: u64) {
        let facts = match self.member_facts_read(&batch.member).await {
            Ok(facts) => facts,
            Err(error) => {
                tracing::warn!(
                    batch_id = %batch.id,
                    error = %error.0,
                    "a member's roster could not be read; an admission went uncounted"
                );
                return;
            }
        };
        if let Err(error) = self
            .inner
            .store
            .audit_delivered_across_revocation(&batch.id, facts, now)
            .await
        {
            tracing::warn!(
                batch_id = %batch.id,
                %error,
                "a delivery across a revocation could not be audited"
            );
        }
    }

    /// Record when a batch the round just read is read again.
    fn read_again(
        &self,
        walk: &mut Walk,
        report: &mut RoundReport,
        position: BatchPosition,
        due: Option<u64>,
    ) {
        report.next_due_ms = min_due(report.next_due_ms, due);
        walk.read(position, due);
    }

    /// A batch the round could not advance is retried at the slowest settle
    /// pace and reported at most once per interval.
    fn retry_later(
        &self,
        walk: &mut Walk,
        report: &mut RoundReport,
        position: BatchPosition,
        now: u64,
        error: &AnnouncementError,
    ) {
        let at = now.saturating_add(self.inner.config.settle_check_max_ms);
        report.next_due_ms = min_due(report.next_due_ms, Some(at));
        walk.due.insert(position.clone(), at);
        match walk.reported.entry(position) {
            std::collections::btree_map::Entry::Occupied(entry) => {
                tracing::debug!(batch = %entry.key().id, %error, "announcement batch still not advanced");
            }
            std::collections::btree_map::Entry::Vacant(entry) => {
                tracing::warn!(
                    batch = %entry.key().id,
                    %error,
                    "announcement batch not advanced; retried at the slowest settle pace"
                );
                entry.insert(now);
            }
        }
    }

    /// How many batches the walk keeps a next look for, and how many a
    /// failure report for.
    #[cfg(test)]
    pub(crate) async fn walk_entries(&self) -> (usize, usize) {
        let walk = self.inner.round.lock().await;
        (walk.due.len(), walk.reported.len())
    }

    /// Seal one member's due backlog now, as a round does.
    #[cfg(test)]
    pub(crate) async fn seal_member(
        &self,
        member: &Member,
    ) -> Result<Option<Batch>, AnnouncementError> {
        let id = new_batch_id()?;
        let policy = self.inner.config.policy;
        self.seal_backlog(member, &id, &policy, self.now()).await
    }

    /// When each batch the walk reports as failing was last reported.
    #[cfg(test)]
    pub(crate) async fn walk_reported_at(&self) -> Vec<u64> {
        let walk = self.inner.round.lock().await;
        walk.reported.values().copied().collect()
    }

    /// Drive one unsettled batch: whether its state moved, and when it
    /// should be looked at again.
    async fn advance(
        &self,
        batch: Batch,
        now: u64,
    ) -> Result<(bool, Option<u64>), AnnouncementError> {
        let before = batch.state.tag();
        let id = batch.id.clone();
        let due = self.drive(batch, now).await?;
        let moved = self
            .inner
            .store
            .batch(&id)
            .await?
            .is_some_and(|after| after.state.tag() != before);
        Ok((moved, due))
    }

    /// Start the dispatcher under a supervisor. Both hold the service weakly
    /// and end when it is dropped or stopped. A dispatcher that ends any
    /// other way (a panic) is reported and started again after a pause that
    /// doubles up to a minute. A candidate launch's owner runs no round: its
    /// dispatcher ends at once.
    pub fn start(&self) -> tokio::task::JoinHandle<()> {
        self.inner.stopped.store(false, Ordering::SeqCst);
        self.supervise()
    }

    /// The dispatcher under its supervisor, leaving a stop request as it
    /// stands: a stop asked for before a delayed start holds.
    fn supervise(&self) -> tokio::task::JoinHandle<()> {
        let first = self.spawn_dispatcher();
        let weak = Arc::downgrade(&self.inner);
        tokio::spawn(async move {
            let mut dispatcher = first;
            let mut pause = RESTART_PAUSE_MIN;
            loop {
                let Err(error) = dispatcher.await else {
                    return;
                };
                tracing::error!(
                    %error,
                    "mob announcement dispatcher ended unexpectedly; it starts again"
                );
                tokio::time::sleep(pause).await;
                pause = (pause * 2).min(RESTART_PAUSE_MAX);
                let Some(inner) = weak.upgrade() else {
                    return;
                };
                if inner.stopped.load(Ordering::SeqCst)
                    || inner.quiesced_candidate.load(Ordering::SeqCst)
                {
                    return;
                }
                dispatcher = AnnouncementService { inner }.spawn_dispatcher();
            }
        })
    }

    /// One dispatcher task. It never clears a stop request, so a restart
    /// racing a stop ends at once.
    fn spawn_dispatcher(&self) -> tokio::task::JoinHandle<()> {
        let weak = Arc::downgrade(&self.inner);
        let wake = Arc::clone(&self.inner.wake);
        tokio::spawn(async move {
            let mut last_prune = 0_u64;
            // Set while the last retention pass stopped at its limit: the
            // next pass follows after a short pause instead of an interval.
            let mut retention_behind = false;
            // When the next catch-up pass may run. Wakes bring rounds, never
            // an early pass, so commands get the store between passes.
            let mut next_pass_at = tokio::time::Instant::now();
            // When the next round is due. A wake or the round's own timer
            // runs it; retention passes in between do not.
            let mut round_due = 0_u64;
            let mut round_wanted = true;
            loop {
                let pause = {
                    let Some(inner) = weak.upgrade() else {
                        return;
                    };
                    // A candidate launch's owner seals, submits, settles and
                    // prunes nothing.
                    if inner.stopped.load(Ordering::SeqCst)
                        || inner.quiesced_candidate.load(Ordering::SeqCst)
                    {
                        return;
                    }
                    let service = AnnouncementService { inner };
                    let now = service.now();
                    let pass_due = if retention_behind {
                        tokio::time::Instant::now() >= next_pass_at
                    } else {
                        now.saturating_sub(last_prune) >= PRUNE_INTERVAL_MS
                    };
                    if pass_due {
                        let retention_ms = service.inner.config.retention_ms;
                        // What waited longer than the window with nothing
                        // native holding it expires first, so the prune
                        // after it is never held back by it. A batch owed
                        // again waits out the quiet window after its last
                        // request, as the drive loop's expiry does.
                        let store = &service.inner.store;
                        let quiet_ms = service.quiet_ms();
                        match store.expire(now, retention_ms, quiet_ms, PRUNE_LIMIT).await {
                            Ok(expired) => {
                                match store.prune(now, retention_ms, PRUNE_LIMIT).await {
                                    Ok(report) => retention_behind = expired.more || report.more,
                                    Err(error) => {
                                        tracing::warn!(%error, "announcement retention pass failed");
                                        retention_behind = false;
                                    }
                                }
                            }
                            Err(error) => {
                                tracing::warn!(%error, "announcement expiry pass failed");
                                retention_behind = false;
                            }
                        }
                        if retention_behind {
                            next_pass_at = tokio::time::Instant::now() + CATCH_UP_PAUSE;
                        } else {
                            last_prune = now;
                        }
                    }
                    let now = service.now();
                    // A round is never further off than the longest idle: one
                    // that is was scheduled before the clock stepped back.
                    if round_wanted
                        || now >= round_due
                        || round_due.saturating_sub(now) > MAX_IDLE_MS
                    {
                        let latest = now.saturating_add(MAX_IDLE_MS);
                        round_due = match service.run_round().await {
                            Ok(report) => report.next_due_ms.map_or(latest, |due| due.min(latest)),
                            Err(error) => {
                                tracing::warn!(%error, "announcement dispatcher round failed");
                                now.saturating_add(service.inner.config.settle_check_max_ms)
                            }
                        };
                    }
                    let round_pause = Duration::from_millis(
                        round_due.saturating_sub(service.now()).min(MAX_IDLE_MS),
                    );
                    // A retention backlog continues after a short pause, not
                    // an interval.
                    if retention_behind {
                        round_pause.min(
                            next_pass_at.saturating_duration_since(tokio::time::Instant::now()),
                        )
                    } else {
                        round_pause
                    }
                };
                round_wanted = tokio::select! {
                    biased;
                    () = wake.notified() => true,
                    // The round's own timer, or the pause between retention
                    // passes.
                    () = tokio::time::sleep(pause) => !retention_behind,
                };
            }
        })
    }

    /// Which subscriptions still want this batch's announcements: delivery
    /// needs one active subscription; paused ones hold the batch until they
    /// resume; when every one is cancelled the batch is no longer wanted.
    async fn interest(&self, batch: &Batch) -> Result<Interest, AnnouncementError> {
        let subscriptions = batch
            .items
            .iter()
            .map(|item| item.subscription_id.clone())
            .collect::<BTreeSet<_>>();
        let mut interest = Interest::None;
        for id in subscriptions {
            match self
                .inner
                .store
                .subscription(&id)
                .await?
                .map(|subscription| subscription.state)
            {
                Some(SubscriptionState::Active) => return Ok(Interest::Active),
                Some(SubscriptionState::Paused) => interest = Interest::Paused,
                Some(SubscriptionState::Cancelled | SubscriptionState::Ended) | None => {}
            }
        }
        Ok(interest)
    }

    async fn successor_for(
        &self,
        batch: &Batch,
    ) -> Result<Option<SuccessorBatch>, AnnouncementError> {
        if self.interest(batch).await? == Interest::None {
            return Ok(None);
        }
        let id = new_batch_id()?;
        let mut seen = BTreeSet::new();
        let mut items = Vec::new();
        let mut not_retained = Vec::new();
        for item in &batch.items {
            if !seen.insert((item.mob_id.clone(), item.sequence)) {
                continue;
            }
            // Retention may have removed an announcement while the native
            // owner held this batch: the successor reports it as a gap.
            match self
                .inner
                .store
                .announcement(&item.mob_id, &item.source_key)
                .await?
            {
                Some(announcement) => items.push(render_item(&announcement)),
                None => not_retained.push(BatchGap {
                    mob_id: item.mob_id.clone(),
                    count: 1,
                    reason: GapReason::NotRetained,
                }),
            }
        }
        // What the stranded batch redelivered after an unhandled turn keeps
        // its notice.
        let notice = self
            .inner
            .store
            .redelivery_notice(&batch.member, &batch.items)
            .await?;
        Ok(Some(successor_batch_with_notice(
            id,
            &batch.id,
            merge_gaps(&batch.gaps, &not_retained),
            &notice,
            &items,
            wire_budget(batch.route),
        )))
    }

    /// For a member kept for a mob a member created: end its subscriptions
    /// made before that mob ended (destroyed, created again, completed or
    /// reset), read from the mob's own durable records; otherwise record how
    /// far its event log was read, so the next check reads on from there.
    /// Returns whether any subscription ended.
    async fn member_mob_ended(&self, member: &Member, now: u64) -> Result<bool, AnnouncementError> {
        let Some(kept) = self.inner.store.member_route(member).await? else {
            return Ok(false);
        };
        if kept.route != MemberRoute::Child {
            return Ok(false);
        }
        let Some(from) = self.inner.store.member_mob_check_from(member).await? else {
            return Ok(false);
        };
        let scan = self
            .inner
            .native
            .member_mob_end(&member.mob_id, kept.incarnation.as_ref(), from)
            .await
            .map_err(|error| AnnouncementError::NativeUnavailable(error.0))?;
        match scan {
            MemberMobScan::Running { through } => {
                if through > from {
                    self.inner
                        .store
                        .record_member_mob_check(member, through)
                        .await?;
                }
                Ok(false)
            }
            MemberMobScan::Ended(end) => {
                let ended = self
                    .inner
                    .store
                    .end_member_before(
                        member,
                        end.cursor,
                        SubscriptionEnd::MemberMobEnded {
                            why: end.why.clone(),
                        },
                        now,
                    )
                    .await?;
                if !ended.is_empty() {
                    tracing::info!(
                        member = %member,
                        why = %end.why,
                        subscriptions = ended.len(),
                        "member's mob ended; its subscriptions ended there"
                    );
                }
                Ok(!ended.is_empty())
            }
        }
    }

    /// Whether the mob a member was kept for is destroyed: not hosted here
    /// once every mob is restored.
    async fn member_mob_destroyed(&self, member: &Member) -> Result<bool, AnnouncementError> {
        let named = BTreeSet::from([member.mob_id.clone()]);
        let presence = self
            .inner
            .native
            .mob_presence(&named)
            .await
            .map_err(|error| AnnouncementError::NativeUnavailable(error.0))?;
        Ok(presence.get(&member.mob_id) == Some(&MobPresence::Gone))
    }

    /// Whether a batch whose submission is not settled by native evidence
    /// expires: one nothing native holds (still sealed or owed again, or one
    /// the native owner reads as not committed), or one whose outcome no
    /// status read has brought. Once it was sealed before the retention
    /// window, every item it holds is at least that old. For a member on a
    /// peer gateway its last submission must also be older than the quiet
    /// window, so a request still in flight is refused as stale instead of
    /// landing after the expiry; until then nothing is submitted again.
    fn expiry(&self, batch: &Batch, now: u64) -> Expiry {
        let config = &self.inner.config;
        if now.saturating_sub(batch.sealed_at_ms) < config.retention_ms {
            return Expiry::NotYet;
        }
        let quiet = self.quiet_ms();
        if batch.route != MemberRoute::Peer || now.saturating_sub(batch.updated_at_ms) > quiet {
            return Expiry::Now;
        }
        Expiry::AfterQuietWindow(batch.updated_at_ms.saturating_add(quiet).saturating_add(1))
    }

    /// The quiet window after a submission to a peer. A peer accepts a
    /// request while its issue time is within the window of its own clock,
    /// and commits it by that deadline or not at all: the wait covers this
    /// gateway's window and the slowest settle pause past it, for a request
    /// issued late in a round or read by a clock a little behind.
    fn quiet_ms(&self) -> u64 {
        let config = &self.inner.config;
        config
            .peer_request_window_ms
            .saturating_add(config.settle_check_max_ms)
    }

    /// Whether a committed batch settles with its admission not confirmed:
    /// the retention window has passed since its commit (when nothing moved
    /// it since, its last update) and a verified status still reads it as
    /// pending (`read`), or no status could be read for the quiet window
    /// after that too. Any verified status after the retention window
    /// settles the batch, so then none lies in that last quiet window.
    fn admission_unconfirmed(&self, batch: &Batch, now: u64, read: bool) -> bool {
        let mut bound = self.inner.config.retention_ms;
        if !read {
            bound = bound.saturating_add(self.quiet_ms());
        }
        now.saturating_sub(batch.updated_at_ms) >= bound
    }

    /// Read again the batches that settled without their delivery being
    /// confirmed whose next read is due, and keep on each what a read finds
    /// that it does not already state. A batch is read once per quiet window
    /// until what it keeps is final or retention removes it; it never
    /// reopens, and the member's later batches go on as they were.
    async fn read_late(&self, now: u64) -> Result<(), AnnouncementError> {
        for batch in self
            .inner
            .store
            .late_reads_due(now, ROUND_LATE_READS)
            .await?
        {
            self.read_late_one(&batch, now).await?;
        }
        Ok(())
    }

    async fn read_late_one(&self, batch: &Batch, now: u64) -> Result<(), AnnouncementError> {
        // A route that contradicts its member is never read.
        if self.route_refusal(batch).is_some() {
            self.inner
                .store
                .record_late_read(&batch.id, None, None)
                .await?;
            return Ok(());
        }
        let again = Some(now.saturating_add(self.quiet_ms()));
        let (late, next) = match self.inner.native.status(batch).await {
            Ok(status) => {
                // A status about another delivery contradicts the batch: it
                // is kept as it was read, and nothing more is read.
                let contradicts = batch.state.late_names_other_delivery(&status);
                let late = batch
                    .state
                    .late_news(status)
                    .map(|status| LateOutcome { status, at_ms: now });
                let done = contradicts
                    || late
                        .as_ref()
                        .or(batch.state.late())
                        .is_some_and(LateOutcome::is_final);
                (late, if done { None } else { again })
            }
            Err(NativeStatusError::Unavailable { .. }) => (None, again),
            // The member departed for good: its subscriptions end, and
            // nothing more is read for the batch.
            Err(NativeStatusError::MemberGone(departure)) => {
                self.end_member(
                    &batch.member,
                    SubscriptionEnd::MemberDeparted { departure },
                    now,
                )
                .await;
                (None, None)
            }
            // The read rules out any later one.
            Err(NativeStatusError::Blocked { .. }) => (None, None),
        };
        self.inner
            .store
            .record_late_read(&batch.id, late, next)
            .await?;
        Ok(())
    }

    /// Read what became of the turn of each admitted batch due a read.
    /// Returns when the next read is due.
    async fn read_handling(&self, now: u64) -> Result<Option<u64>, AnnouncementError> {
        for batch in self
            .inner
            .store
            .handling_reads_due(now, ROUND_HANDLING_READS)
            .await?
        {
            if let Err(error) = self.read_handling_one(&batch, now).await {
                tracing::warn!(batch = %batch.id, %error, "a batch's handling was not read");
            }
        }
        Ok(self.inner.store.next_handling_read().await?)
    }

    async fn read_handling_one(&self, batch: &Batch, now: u64) -> Result<(), AnnouncementError> {
        let BatchState::Delivered {
            session_id,
            input_id,
            ..
        } = &batch.state
        else {
            return Ok(());
        };
        // A route that contradicts its member is never read.
        let read = if self.route_refusal(batch).is_some() {
            HandlingRead::Unknown
        } else {
            self.handling_read(batch, session_id, input_id, now).await
        };
        self.inner
            .store
            .record_handling(&batch.id, read, self.inner.config.on_unhandled, now)
            .await?;
        Ok(())
    }

    /// What the admitted input's turn came to: its receipt first, its
    /// terminal outcome and abandon reason without one, followed through a
    /// superseding or coalescing input to the one that absorbed it. Read
    /// again at the settle pace while it is not final or not readable;
    /// unknown once the retention window has passed since the admission.
    async fn handling_read(
        &self,
        batch: &Batch,
        session_id: &str,
        input_id: &str,
        now: u64,
    ) -> HandlingRead {
        let expired = now.saturating_sub(batch.updated_at_ms) > self.inner.config.retention_ms;
        let again = if expired {
            HandlingRead::Unknown
        } else {
            HandlingRead::Again {
                next_read_at_ms: now.saturating_add(self.settle_pause_ms(batch, now)),
            }
        };
        let mut input_id = input_id.to_string();
        for _ in 0..=MAX_FOLLOW {
            let reading = match self.inner.native.input(batch, session_id, &input_id).await {
                Ok(Some(reading)) => reading,
                Ok(None) | Err(_) => return again,
            };
            match classify(&reading) {
                Classified::Pending | Classified::Unreadable => return again,
                Classified::Handled => return HandlingRead::Handled,
                Classified::HandledUnverified => return HandlingRead::HandledUnverified,
                Classified::Unhandled { cause, held } => {
                    return HandlingRead::Unhandled { cause, held };
                }
                Classified::Unknown => return HandlingRead::Unknown,
                Classified::Follow { input_id: next } => {
                    // A peer answers for the batch's own input only: the
                    // input that absorbed it is not readable there.
                    if batch.route == MemberRoute::Peer {
                        return HandlingRead::Unknown;
                    }
                    input_id = next;
                }
            }
        }
        HandlingRead::Unknown
    }

    fn settle_pause_ms(&self, batch: &Batch, now: u64) -> u64 {
        let age = now.saturating_sub(batch.sealed_at_ms);
        (age / 10).clamp(
            self.inner.config.settle_check_min_ms,
            self.inner.config.settle_check_max_ms,
        )
    }

    async fn apply(
        &self,
        batch: &Batch,
        event: BatchEvent,
        now: u64,
    ) -> Result<Batch, AnnouncementError> {
        let admitted = matches!(event, BatchEvent::NativeApplied { .. });
        let next = self
            .inner
            .store
            .transition_batch(&batch.id, batch.state.tag(), event, now)
            .await?;
        if admitted {
            self.audit_delivered_across_revocation(&next, now).await;
        }
        Ok(next)
    }

    /// Advance one batch as far as native evidence allows now. Returns when
    /// it should be looked at again, or `None` once it is settled.
    async fn drive(&self, mut batch: Batch, now: u64) -> Result<Option<u64>, AnnouncementError> {
        // A route that contradicts its member blocks the batch before
        // anything is prepared, ended, submitted or read for it: no member
        // of this gateway's mob is touched in that member's place.
        if BatchStateTag::UNSETTLED.contains(&batch.state.tag())
            && let Some(refusal) = self.route_refusal(&batch)
        {
            self.apply(
                &batch,
                BatchEvent::Block {
                    cause: refusal.to_string(),
                },
                now,
            )
            .await?;
            return Ok(None);
        }
        let mut submitted_this_round = false;
        // A batch owed again after an answer that nothing was committed is
        // tried again at the slowest settle pace.
        let owed_at = Some(now.saturating_add(self.inner.config.settle_check_max_ms));
        for _ in 0..8 {
            let retry_at = Some(now.saturating_add(self.settle_pause_ms(&batch, now)));
            match batch.state.clone() {
                BatchState::Sealed => {
                    let interest = self.interest(&batch).await?;
                    if interest == Interest::None {
                        batch = self.apply(&batch, BatchEvent::Cancel, now).await?;
                        continue;
                    }
                    // Nothing native holds a sealed batch: one owed past the
                    // retention window expires, paused or not, and the next
                    // batch reports its announcements.
                    match self.expiry(&batch, now) {
                        Expiry::NotYet => {}
                        Expiry::Now => {
                            batch = self.apply(&batch, BatchEvent::Expire, now).await?;
                            continue;
                        }
                        Expiry::AfterQuietWindow(due) => return Ok(Some(due)),
                    }
                    // A resume wakes the dispatcher; no timer needed.
                    if interest == Interest::Paused {
                        return Ok(None);
                    }
                    if submitted_this_round {
                        return Ok(owed_at);
                    }
                    if batch.route == MemberRoute::Child
                        && self.member_mob_ended(&batch.member, now).await?
                    {
                        batch = self.reload(&batch).await?;
                        continue;
                    }
                    match self.prepare(&batch, now).await {
                        Ok(()) => {}
                        Err(PrepareStep::Ended) => {
                            batch = self.reload(&batch).await?;
                            continue;
                        }
                        Err(PrepareStep::Waits) => return Ok(owed_at),
                        Err(PrepareStep::NotYet) => return Ok(retry_at),
                    }
                    // The last point Meta controls: a member that stopped
                    // passing since the seal gets nothing of it. A peer
                    // batch submitted before is never cancelled while its
                    // last request may still commit, and past that window
                    // what the request did is read first.
                    let peer = self.peer_request_open(&batch, now);
                    match self.last_check(&batch, peer.is_none(), now).await? {
                        LastCheck::Passed => {}
                        LastCheck::Cancelled(cancelled) => {
                            batch = *cancelled;
                            continue;
                        }
                        LastCheck::Deferred => return Ok(retry_at),
                        LastCheck::Failed if peer == Some(true) => {
                            return Ok(Some(self.peer_window_end(&batch).saturating_add(1)));
                        }
                        LastCheck::Failed => match self.inner.native.status(&batch).await {
                            Ok(NativeStatus::NotCommitted) => {
                                match self.last_check(&batch, true, now).await? {
                                    LastCheck::Cancelled(cancelled) => {
                                        batch = *cancelled;
                                        continue;
                                    }
                                    LastCheck::Passed => {}
                                    LastCheck::Failed | LastCheck::Deferred => {
                                        return Ok(retry_at);
                                    }
                                }
                            }
                            // An earlier request committed: the batch settles
                            // from what the native owner holds.
                            Ok(status) => {
                                batch = self
                                    .apply(&batch, BatchEvent::BeginSubmit, self.now())
                                    .await?;
                                batch = self.settle_from(&batch, status, now).await?;
                                continue;
                            }
                            Err(_) => return Ok(retry_at),
                        },
                    }
                    // Stamped when the request is issued, not when the
                    // round began: a peer's window runs from the issue.
                    batch = self
                        .apply(&batch, BatchEvent::BeginSubmit, self.now())
                        .await?;
                    super::faults::after_begin_submit();
                    submitted_this_round = true;
                    match self.submit(&batch, now).await? {
                        SubmitStep::Moved(next) => batch = *next,
                        SubmitStep::ReadStatus => return Ok(retry_at),
                        SubmitStep::Owed => return Ok(owed_at),
                    }
                }
                BatchState::Submitting { .. } => {
                    let status = match self.read_status(&batch, now).await? {
                        StatusStep::Status(status) => status,
                        // The outcome stays unknown. Past the retention
                        // window, and past the quiet window since its last
                        // submission for a member on a peer, it is not
                        // learned in time: the batch settles as not
                        // confirmed, is never submitted again, and the
                        // member's next batch goes on.
                        StatusStep::Wait => match self.expiry(&batch, now) {
                            Expiry::Now => {
                                batch = self
                                    .apply(&batch, BatchEvent::ExpireUnverified, now)
                                    .await?;
                                continue;
                            }
                            Expiry::NotYet | Expiry::AfterQuietWindow(_) => return Ok(retry_at),
                        },
                        StatusStep::Moved(next) => {
                            batch = *next;
                            continue;
                        }
                    };
                    match status {
                        NativeStatus::NotCommitted => {
                            let interest = self.interest(&batch).await?;
                            if interest == Interest::None {
                                batch = self
                                    .apply(&batch, BatchEvent::CancelNotCommitted, now)
                                    .await?;
                                continue;
                            }
                            // Nothing native holds it and it waited longer than
                            // the retention window: its announcements expire,
                            // paused or not, and the next batch reports them.
                            match self.expiry(&batch, now) {
                                Expiry::NotYet => {}
                                Expiry::Now => {
                                    batch = self
                                        .apply(&batch, BatchEvent::ExpireNotCommitted, now)
                                        .await?;
                                    continue;
                                }
                                // A peer's request may still be in flight: no
                                // further attempt, and the expiry waits until
                                // the peer would refuse it as stale.
                                Expiry::AfterQuietWindow(due) => return Ok(Some(due)),
                            }
                            if interest == Interest::Paused {
                                return Ok(None);
                            }
                            if submitted_this_round {
                                return Ok(retry_at);
                            }
                            if batch.route == MemberRoute::Child
                                && self.member_mob_ended(&batch.member, now).await?
                            {
                                batch = self.reload(&batch).await?;
                                continue;
                            }
                            match self.prepare(&batch, now).await {
                                Ok(()) => {}
                                Err(PrepareStep::Ended) => {
                                    batch = self.reload(&batch).await?;
                                    continue;
                                }
                                Err(PrepareStep::Waits) => return Ok(owed_at),
                                Err(PrepareStep::NotYet) => return Ok(retry_at),
                            }
                            // Every resubmission is checked again first. A
                            // peer's earlier request may still commit inside
                            // the quiet window: then the batch waits.
                            let open = self.peer_request_open(&batch, now) == Some(true);
                            match self.last_check(&batch, !open, now).await? {
                                LastCheck::Passed => {}
                                LastCheck::Cancelled(cancelled) => {
                                    batch = *cancelled;
                                    continue;
                                }
                                LastCheck::Deferred => return Ok(retry_at),
                                LastCheck::Failed => {
                                    return Ok(Some(
                                        self.peer_window_end(&batch).saturating_add(1),
                                    ));
                                }
                            }
                            batch = self
                                .apply(&batch, BatchEvent::RetryNotCommitted, self.now())
                                .await?;
                            submitted_this_round = true;
                            match self.submit(&batch, now).await? {
                                SubmitStep::Moved(next) => batch = *next,
                                SubmitStep::ReadStatus => return Ok(retry_at),
                                SubmitStep::Owed => return Ok(owed_at),
                            }
                        }
                        status => batch = self.settle_from(&batch, status, now).await?,
                    }
                }
                BatchState::Committed { .. } => {
                    let status = match self.read_status(&batch, now).await? {
                        StatusStep::Status(status) => status,
                        // No status can be read, past the retention window
                        // and the quiet window after it since the commit:
                        // the admission is not confirmed in time. The member's
                        // inbox committed the batch, so the next one goes on
                        // without passing it.
                        StatusStep::Wait if self.admission_unconfirmed(&batch, now, false) => {
                            batch = self
                                .apply(&batch, BatchEvent::ExpireUnconfirmed, now)
                                .await?;
                            continue;
                        }
                        StatusStep::Wait => return Ok(retry_at),
                        StatusStep::Moved(next) => {
                            batch = *next;
                            continue;
                        }
                    };
                    match status {
                        // Evidence about another delivery never settles this
                        // one, nor leaves it waiting on that delivery.
                        NativeStatus::Pending { receipt, .. } => {
                            match batch.state.other_delivery(&receipt) {
                                Some(cause) => {
                                    batch = self
                                        .apply(&batch, BatchEvent::Block { cause }, now)
                                        .await?;
                                }
                                None => {
                                    // Meerkat keeps a delivery to a member of a mob it
                                    // does not host waiting, in case that mob is
                                    // restored. A destroyed mob a member created never
                                    // is: the delivery strands, after the member's
                                    // subscriptions there end.
                                    if batch.route != MemberRoute::Child
                                        || !self.member_mob_destroyed(&batch.member).await?
                                    {
                                        // Still pending past the retention
                                        // window since the commit: the
                                        // admission is not confirmed in
                                        // time, and an admission or a
                                        // stranding after this is kept as
                                        // its late outcome.
                                        if self.admission_unconfirmed(&batch, now, true) {
                                            batch = self
                                                .apply(&batch, BatchEvent::ExpireUnconfirmed, now)
                                                .await?;
                                            continue;
                                        }
                                        return Ok(retry_at);
                                    }
                                    self.inner
                                        .store
                                        .end_member(
                                            &batch.member,
                                            SubscriptionEnd::MemberMobEnded {
                                                why: "destroyed".to_string(),
                                            },
                                            now,
                                        )
                                        .await?;
                                    let successor = self.successor_for(&batch).await?;
                                    batch = self
                                        .apply(
                                            &batch,
                                            BatchEvent::NativeStranded { receipt, successor },
                                            now,
                                        )
                                        .await?;
                                }
                            }
                        }
                        NativeStatus::NotCommitted => {
                            batch = self
                                .apply(
                                    &batch,
                                    BatchEvent::Block {
                                        cause: "the native owner holds no commit for a batch \
                                                it issued a receipt for"
                                            .into(),
                                    },
                                    now,
                                )
                                .await?;
                        }
                        status => batch = self.settle_from(&batch, status, now).await?,
                    }
                }
                BatchState::Stranded { .. } => match batch.successor.clone() {
                    Some(successor) => {
                        let Some(next) = self.inner.store.batch(&successor).await? else {
                            return Ok(None);
                        };
                        batch = next;
                    }
                    None => return Ok(None),
                },
                BatchState::Delivered { .. }
                | BatchState::Refused { .. }
                | BatchState::Cancelled
                | BatchState::Expired
                | BatchState::Blocked { .. }
                | BatchState::ExpiredUnverified { .. }
                | BatchState::CommittedUnconfirmed { .. } => return Ok(None),
            }
        }
        Ok(Some(
            now.saturating_add(self.inner.config.settle_check_min_ms),
        ))
    }

    /// Whether the batch's kept route contradicts its member, by the one
    /// guard the native seam runs too. A native seam that names no own mob
    /// leaves no local route standing.
    fn route_refusal(&self, batch: &Batch) -> Option<OwnerRefusal> {
        route_contradiction(
            self.inner.native.own_mob().as_ref(),
            &batch.member,
            batch.route,
        )
    }

    /// The batch as the store holds it now.
    async fn reload(&self, batch: &Batch) -> Result<Batch, AnnouncementError> {
        Ok(self
            .inner
            .store
            .batch(&batch.id)
            .await?
            .ok_or(AnnouncementStoreError::UnknownBatch)?)
    }

    /// Read the batch's native status. No status leaves the batch as it
    /// was, with the reason marked; only a read that rules the batch out
    /// blocks it, and only the member's departure ends anything.
    async fn read_status(&self, batch: &Batch, now: u64) -> Result<StatusStep, AnnouncementError> {
        match self.inner.native.status(batch).await {
            // An answered read ends the wait it marked; what it says decides
            // the batch.
            Ok(status) => {
                self.unmark(&batch.member).await;
                Ok(StatusStep::Status(status))
            }
            Err(NativeStatusError::Unavailable {
                cause,
                evidence,
                detail,
            }) => {
                tracing::warn!(
                    batch = %batch.id,
                    cause = %cause,
                    evidence = evidence.as_str(),
                    detail = %detail,
                    "announcement native status unavailable"
                );
                self.mark(&batch.member, cause, evidence, now).await;
                Ok(StatusStep::Wait)
            }
            Err(NativeStatusError::Blocked { cause, detail }) => Ok(StatusStep::Moved(Box::new(
                self.apply(
                    batch,
                    BatchEvent::Block {
                        cause: cause.with_detail(&detail),
                    },
                    now,
                )
                .await?,
            ))),
            // The member departed for good: its subscriptions end. The
            // batch's own outcome stays as unknown as it was.
            Err(NativeStatusError::MemberGone(departure)) => {
                self.end_member(
                    &batch.member,
                    SubscriptionEnd::MemberDeparted { departure },
                    now,
                )
                .await;
                Ok(StatusStep::Wait)
            }
        }
    }

    /// Make the batch's member ready for a submission.
    async fn prepare(&self, batch: &Batch, now: u64) -> Result<(), PrepareStep> {
        match self
            .inner
            .native
            .prepare_member(&batch.member, batch.route)
            .await
        {
            Ok(()) => Ok(()),
            // A final fact: the member's subscriptions end and the batch is
            // cancelled.
            Err(PrepareError::Gone(end)) => {
                self.end_member(&batch.member, end, now).await;
                Err(PrepareStep::Ended)
            }
            // The member cannot take a delivery now; nothing ends.
            Err(PrepareError::Suspended { cause, evidence }) => {
                tracing::warn!(
                    batch = %batch.id,
                    member = %batch.member,
                    cause = %cause,
                    "announcement member cannot take a delivery now; the batch waits"
                );
                self.mark(&batch.member, cause, evidence, now).await;
                Err(PrepareStep::Waits)
            }
            Err(PrepareError::Unavailable(error)) => {
                tracing::warn!(
                    batch = %batch.id,
                    member = %batch.member,
                    error = %error,
                    "announcement member is not resolvable yet; the batch waits"
                );
                Err(PrepareStep::NotYet)
            }
        }
    }

    /// Settle or advance a submitting or committed batch from a native
    /// status other than "not committed".
    async fn settle_from(
        &self,
        batch: &Batch,
        status: NativeStatus,
        now: u64,
    ) -> Result<Batch, AnnouncementError> {
        let event = match status {
            NativeStatus::NotCommitted => return Ok(batch.clone()),
            NativeStatus::Pending { receipt, .. } => {
                if batch.state.tag() == BatchStateTag::Committed {
                    return Ok(batch.clone());
                }
                BatchEvent::NativeCommitted { receipt }
            }
            NativeStatus::Applied {
                receipt,
                session_id,
                input_id,
            } => BatchEvent::NativeApplied {
                receipt,
                session_id,
                input_id,
            },
            NativeStatus::Refused { receipt, reason } => BatchEvent::NativeRefused {
                reason,
                receipt: Some(receipt),
            },
            NativeStatus::Stranded { receipt } => BatchEvent::NativeStranded {
                receipt,
                successor: self.successor_for(batch).await?,
            },
        };
        self.apply(batch, event, now).await
    }

    /// Submit the batch's exact continuation, and read what the answer says
    /// about the commit: committed, nothing committed (the batch is owed
    /// again), unknown (its status settles it), ruled out, or the member
    /// gone for good.
    async fn submit(&self, batch: &Batch, now: u64) -> Result<SubmitStep, AnnouncementError> {
        let event = match self.inner.native.submit(batch, now).await {
            Ok(receipt) => {
                self.unmark(&batch.member).await;
                BatchEvent::NativeCommitted { receipt }
            }
            Err(NativeSubmitError::NotCommitted { cause, evidence }) => {
                tracing::warn!(
                    batch = %batch.id,
                    member = %batch.member,
                    cause = %cause,
                    evidence = evidence.as_str(),
                    "announcement submission committed nothing; the batch is owed again"
                );
                self.mark(&batch.member, cause, evidence, now).await;
                self.apply(batch, BatchEvent::SubmitRefused, now).await?;
                return Ok(SubmitStep::Owed);
            }
            Err(NativeSubmitError::OutcomeUnknown {
                cause,
                evidence,
                detail,
            }) => {
                tracing::warn!(
                    batch = %batch.id,
                    cause = %cause,
                    evidence = evidence.as_str(),
                    detail = %detail,
                    "announcement submission outcome unknown; the native status will settle it"
                );
                self.mark(&batch.member, cause, evidence, now).await;
                return Ok(SubmitStep::ReadStatus);
            }
            Err(NativeSubmitError::Blocked { cause, detail }) => BatchEvent::Block {
                cause: cause.with_detail(&detail),
            },
            // The member departed for good: nothing was committed, the batch
            // settles as refused and the member's subscriptions end.
            Err(NativeSubmitError::MemberGone(departure)) => {
                let end = SubscriptionEnd::MemberDeparted { departure };
                let refused = self
                    .apply(
                        batch,
                        BatchEvent::NativeRefused {
                            reason: end.to_string(),
                            receipt: None,
                        },
                        now,
                    )
                    .await?;
                self.end_member(&batch.member, end, now).await;
                return Ok(SubmitStep::Moved(Box::new(refused)));
            }
        };
        self.apply(batch, event, now)
            .await
            .map(|next| SubmitStep::Moved(Box::new(next)))
    }
}
