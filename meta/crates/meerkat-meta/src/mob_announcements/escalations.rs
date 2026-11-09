//! Escalations: a source or member condition a person should know about,
//! kept as a durable row and reported as events with a monotonic cursor.
//!
//! One escalation is open per cause and subject. Raising an open one keeps
//! it as it is; it is notified again every `renotify_after_ms` under the
//! same id with a higher count until it resolves. Every raise, notification
//! and resolution is also an event row: a host reads them from its last
//! cursor after a reconnect and misses nothing, and the owner pushes each to
//! the host's `meta/on_error` hook as it is written. Nothing here decides a
//! delivery: escalations only report.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use std::collections::BTreeSet;

use super::availability::SuspendMarker;
use super::declarations::DeclarationRecordKind;
use super::sources::{
    AnnouncementSource, AudienceGrant, CollectionPhase, SourceHealthStatus, SourceState,
};
use super::types::{Member, MobId, SourceId, Topic};

/// The reserved topic agents receive health announcements on.
pub const HEALTH_TOPIC: &str = "mob_announcements.health";

/// The key prefix of health announcements.
pub const HEALTH_KEY_PREFIX: &str = "health:";

/// Joins the parts of a subject's key. No part may hold a control
/// character (mob, source and member ids refuse them), so a key names one
/// subject only, as [`Member::key`] does.
const SUBJECT_KEY_SEPARATOR: char = '\u{1f}';

/// The start of every member subject's key.
pub(crate) const MEMBER_SUBJECT_KEY_PREFIX: &str = "member\u{1f}";

/// Why an escalation is open.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum EscalationCause {
    /// A source's collections keep failing.
    FailureStreak,
    /// An active source has not advanced for its stale window: its
    /// collector is not running.
    Stale,
    /// A live source keeps succeeding without an item new to it.
    Quiet,
    /// A source's snapshots keep returning zero records.
    EmptyCoverage,
    /// A source rebaselined: changes may be missing.
    SourceGap,
    /// A source offered other content under a key it already announced.
    Conflicts,
    /// A member's deliveries have waited for an hour.
    Suspended,
    /// A member's deliveries report missing announcements.
    MemberGap,
    /// A member has items held because its coverage changed.
    AudienceHold,
    /// A declared record or role expansion waits on the runtime.
    DeclarationPending,
    /// A member holds items for an operator: their turn ended unhandled
    /// by a held cause, by the hold policy, or past the redelivery bound.
    Unhandled,
    /// A member's turns ended consumed with no receipt saying how, as often
    /// as the threshold in a day.
    HandledUnverified,
}

impl EscalationCause {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::FailureStreak => "failure_streak",
            Self::Stale => "stale",
            Self::Quiet => "quiet",
            Self::EmptyCoverage => "empty_coverage",
            Self::SourceGap => "source_gap",
            Self::Conflicts => "conflicts",
            Self::Suspended => "suspended",
            Self::MemberGap => "member_gap",
            Self::AudienceHold => "audience_hold",
            Self::DeclarationPending => "declaration_pending",
            Self::Unhandled => "unhandled",
            Self::HandledUnverified => "handled_unverified",
        }
    }
}

impl std::fmt::Display for EscalationCause {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What an escalation is about.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[non_exhaustive]
pub enum EscalationSubject {
    Source {
        mob_id: MobId,
        source_id: SourceId,
    },
    Member {
        member: Member,
    },
    Declaration {
        record_kind: DeclarationRecordKind,
        id: String,
    },
}

impl std::fmt::Display for EscalationSubject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Source { mob_id, source_id } => write!(f, "source {source_id} of mob {mob_id}"),
            Self::Member { member } => {
                write!(f, "member {} of mob {}", member.identity, member.mob_id)
            }
            Self::Declaration { record_kind, id } => {
                write!(f, "declared {} {id}", record_kind.as_str())
            }
        }
    }
}

impl EscalationSubject {
    /// The key one open escalation per cause and subject is unique on: its
    /// kind and parts joined by a control character no part may hold.
    #[must_use]
    pub fn key(&self) -> String {
        const SEP: char = SUBJECT_KEY_SEPARATOR;
        match self {
            Self::Source { mob_id, source_id } => format!("source{SEP}{mob_id}{SEP}{source_id}"),
            Self::Member { member } => format!("{MEMBER_SUBJECT_KEY_PREFIX}{}", member.key()),
            Self::Declaration { record_kind, id } => {
                format!("declaration{SEP}{}{SEP}{id}", record_kind.as_str())
            }
        }
    }

    /// The mob the subject belongs to; a declaration names none.
    #[must_use]
    pub fn mob_id(&self) -> Option<&MobId> {
        match self {
            Self::Source { mob_id, .. } => Some(mob_id),
            Self::Member { member } => Some(&member.mob_id),
            Self::Declaration { .. } => None,
        }
    }

    /// A short digest of the subject, for health announcement keys.
    #[must_use]
    pub fn digest(&self) -> String {
        use std::fmt::Write as _;
        let digest = Sha256::digest(self.key().as_bytes());
        let mut out = String::with_capacity(16);
        for byte in digest.iter().take(8) {
            let _ = write!(out, "{byte:02x}");
        }
        out
    }
}

/// What the latest advance of a quiet source's window observed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum QuietEvidence {
    /// The provider answered a delta: likely genuinely idle.
    DeltaConfirmed,
    /// A snapshot returned zero records: suspicious.
    SnapshotEmpty,
    /// A snapshot returned records, none new.
    SnapshotNonempty,
    /// The latest advance did not say what it observed.
    Unreported,
}

/// A durable escalation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Escalation {
    pub escalation_id: String,
    pub cause: EscalationCause,
    pub subject: EscalationSubject,
    /// When the condition began.
    pub since_ms: u64,
    pub raised_at_ms: u64,
    /// How many times it was notified again after its raise.
    pub renotify_count: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_at_ms: Option<u64>,
    /// For a quiet source: what the window's latest advance observed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<QuietEvidence>,
}

/// What an escalation event records.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum EscalationEventKind {
    Raised,
    Renotified,
    Resolved,
}

impl EscalationEventKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Raised => "raised",
            Self::Renotified => "renotified",
            Self::Resolved => "resolved",
        }
    }
}

impl std::fmt::Display for EscalationEventKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One raise, notification or resolution, at its cursor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct EscalationEvent {
    pub cursor: u64,
    pub escalation_id: String,
    pub event: EscalationEventKind,
    pub cause: EscalationCause,
    pub subject: EscalationSubject,
    pub since_ms: u64,
    pub at_ms: u64,
    pub renotify_count: u32,
}

/// An escalation event and the escalation as it stands after it: what the
/// owner pushes to the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EscalationNotice {
    pub event: EscalationEvent,
    pub escalation: Escalation,
}

/// The typed body of a health announcement: never adapter text.
#[must_use]
pub(crate) fn health_body(escalation: &Escalation, event: EscalationEventKind) -> String {
    serde_json::json!({
        "event": event,
        "escalation_id": escalation.escalation_id,
        "cause": escalation.cause,
        "subject": escalation.subject,
        "since_ms": escalation.since_ms,
        "renotify_count": escalation.renotify_count,
        "evidence": escalation.evidence,
    })
    .to_string()
}

/// The key of the health announcement for `escalation` at its current
/// count: unique per episode, since a raise is a new escalation with an id
/// of its own (its since is in the body only), and the same for a retry
/// within one.
#[must_use]
pub(crate) fn health_key(escalation: &Escalation) -> String {
    format!(
        "{HEALTH_KEY_PREFIX}{}:{}:{}:{}",
        escalation.cause,
        escalation.subject.digest(),
        escalation.escalation_id,
        escalation.renotify_count
    )
}

/// The reserved health topic.
pub(crate) fn health_topic() -> Result<Topic, super::types::AnnouncementStoreError> {
    Topic::new(HEALTH_TOPIC)
}

/// What one escalation pass needs from the owner's configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct EscalationPass {
    /// How long an open escalation waits before it is notified again.
    pub renotify_after_ms: u64,
    /// Who receives member escalations on the health topic.
    pub health_audience: Vec<AudienceGrant>,
    /// How long events are kept.
    pub retention_ms: u64,
    /// How many handled_unverified turns of a member in a day escalate.
    pub handled_unverified_threshold: u32,
    /// The mobs hosted here: health is announced only into them, while the
    /// host hears of every subject.
    pub hosted: BTreeSet<MobId>,
}

/// A page of escalation notices, and the cursor to read on from past every
/// event read, those whose escalation is gone included.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct EscalationNoticePage {
    pub notices: Vec<EscalationNotice>,
    pub next: u64,
    /// Whether the page was full: more may follow.
    pub full: bool,
}

/// Whether `source` is stale at `now_ms`: active, not failing, and no
/// successful advance for its stale window. A failing collector is
/// failure_streak's, not stale's.
#[must_use]
pub(crate) fn is_stale(source: &AnnouncementSource, now_ms: u64) -> bool {
    let last_success = source
        .health
        .last_success_at_ms
        .unwrap_or(source.created_at_ms);
    source.state == SourceState::Active
        && source.health.consecutive_failures == 0
        && now_ms.saturating_sub(last_success) >= source.health_policy.stale_after_ms
}

/// The one health status of `source` at `now_ms`: a paused or ended source
/// is that; an active one is failing while a failure stands, in any
/// collector phase; else stale, computed here because time makes it; else
/// its collection phase, or ok once live. Quiet is never a status: its
/// escalation and quiet monitoring report it.
#[must_use]
pub(crate) fn health_status(source: &AnnouncementSource, now_ms: u64) -> SourceHealthStatus {
    match source.state {
        SourceState::Paused => SourceHealthStatus::Paused,
        SourceState::Ended => SourceHealthStatus::Ended,
        SourceState::Active if source.health.consecutive_failures > 0 => {
            SourceHealthStatus::Failing
        }
        SourceState::Active if is_stale(source, now_ms) => SourceHealthStatus::Stale,
        SourceState::Active => match source.collection {
            CollectionPhase::Baselining => SourceHealthStatus::Baselining,
            CollectionPhase::Backfilling => SourceHealthStatus::Backfilling,
            CollectionPhase::Live => SourceHealthStatus::Ok,
        },
    }
}

/// A member's backlog as members/get reads it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct MemberBacklogRead {
    pub eligible_count: u64,
    pub eligible_bytes: u64,
    pub oldest_captured_at_ms: Option<u64>,
    pub last_sealed_at_ms: Option<u64>,
    pub outstanding: bool,
    pub held_audience: u64,
    pub held_unhandled: u64,
    pub held_refused: u64,
    pub withheld_audience: u64,
}

/// What members/get reads about one member.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct MemberRead {
    pub member: Member,
    pub backlog: MemberBacklogRead,
    /// The persisted suspend marker.
    pub marker: Option<SuspendMarker>,
    pub escalations: Vec<Escalation>,
    /// What became of the member's admitted batches' turns.
    pub handling: MemberHandlingRead,
}

/// A member's admitted batches by handling, as members/get reads them:
/// handled_unverified is never folded into handled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[non_exhaustive]
pub struct MemberHandlingRead {
    pub pending: u64,
    pub handled: u64,
    pub handled_unverified: u64,
    pub unhandled: u64,
    /// Items the member holds for an operator now.
    pub held: u64,
    /// When its oldest unhandled turn was read, if it has one.
    pub oldest_unhandled_at_ms: Option<u64>,
}

/// meta/status's mob announcement counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct StatusCounts {
    pub sources_active: u64,
    pub sources_paused: u64,
    pub sources_failing: u64,
    pub sources_stale: u64,
    pub members_suspended: u64,
    /// Items held for an operator, over members.
    pub unhandled_held: u64,
    pub open_escalations: u64,
    pub last_event_cursor: u64,
    /// Admitted batches by handling, over members.
    pub handling_pending: u64,
    pub handling_handled: u64,
    pub handling_handled_unverified: u64,
    pub handling_unhandled: u64,
}

/// The window the handled_unverified threshold counts in.
pub(crate) const HANDLED_UNVERIFIED_WINDOW_MS: u64 = 24 * 60 * 60 * 1_000;

/// How long a member's deliveries wait before they escalate as suspended.
pub(crate) const SUSPENDED_ESCALATION_MS: u64 = 60 * 60 * 1_000;

/// How long the escalation of a condition that happens, rather than holds,
/// stays open after it last happened: a source's conflict or gap, a batch
/// that reported gaps, a capture withheld because coverage changed. A pass
/// reads back further when the last pass before it is older: what happened
/// while no round ran is raised by the next one.
pub(crate) const EVENT_WINDOW_MS: u64 = 24 * 60 * 60 * 1_000;
