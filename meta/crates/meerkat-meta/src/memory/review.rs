//! Quarantine review: the typed decision over one quarantined record.
//!
//! §10.1 promises that quarantined writes wait "until steward/operator
//! review"; §8.5 defines the review itself. Both reviewers share this one
//! transaction shape so a release means the same thing whoever decides it:
//!
//! - **release** re-stages the origin's content in the SAME scope at
//!   `agent_observed` with `derived_from = [origin]` (the §10.2 ceiling walks
//!   that edge forever) and tombstones the origin. A quarantined UPDATE
//!   (origin `supersedes` an active prior) is released as a supersede of
//!   that prior, so the lineage does not fork into two active versions.
//! - **tombstone** retires the origin; nothing else changes.
//!
//! The review's own audit rows are its durable evidence: every applied op
//! carries `detail.review` ([`ReviewAudit`]). A repeated decision is
//! recognized from that record of the committed transaction, never from a
//! row that merely looks like a successor, so a replay returns the original
//! decision and writes nothing. The successor id is a function of the origin
//! id, so a second release of one origin cannot mint a second successor; an
//! unrelated row already holding that id is refused, not adopted. Everything
//! here is pure structure; the store applies it in one transaction through
//! the staged validator (lattice, transitive ceiling, secret chokepoint)
//! like every other write.

use serde::{Deserialize, Serialize};

use super::records::{
    MemoryAuthor, MemoryId, MemoryKind, MemoryRecord, MemoryScope, NewMemoryRecord, RecordStatus,
    TrustTier,
};
use super::staged::StagedOp;
use crate::identity_first::agent_memory::AgentMemoryError;

/// Suffix that turns an origin id into its release successor's id.
pub const RELEASE_SUCCESSOR_SUFFIX: &str = "-released";

/// Longest reviewer rationale the review transaction accepts (bytes).
pub const MAX_REVIEW_RATIONALE_BYTES: usize = 400;

/// The verdict a reviewer renders on one quarantined record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuarantineDecision {
    Release,
    Tombstone,
}

impl QuarantineDecision {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Release => "release",
            Self::Tombstone => "tombstone",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "release" => Some(Self::Release),
            "tombstone" => Some(Self::Tombstone),
            _ => None,
        }
    }
}

/// Who decides. The review's batch is authored by this principal, and the
/// audit records it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum QuarantineReviewer {
    /// An operator through a console. `principal` is the authenticated
    /// console principal. It is `None` only on a host that runs without app
    /// authentication; the audit then says no principal was known instead
    /// of naming anyone.
    Operator { principal: Option<String> },
    /// A memory steward dream run.
    Steward { run_id: String },
}

impl QuarantineReviewer {
    /// The batch author: operators are non-LLM principals, the steward is
    /// the LLM-authored judgment stage (§10.2 lattice rules key off this).
    pub fn author(&self) -> MemoryAuthor {
        match self {
            Self::Operator { .. } => MemoryAuthor::Operator,
            Self::Steward { run_id } => MemoryAuthor::Steward {
                run_id: run_id.clone(),
            },
        }
    }
}

/// One review decision. `scope` is the scope the caller is authorized for:
/// the transaction refuses a record that lives anywhere else, so a review
/// can never leave the scope it was authorized in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuarantineReviewRequest {
    pub scope: MemoryScope,
    pub memory_id: MemoryId,
    pub decision: QuarantineDecision,
    /// [`super::records::content_hash`] of the title and body the reviewer
    /// read. Record content is never edited in place, so the id plus this
    /// hash pins exactly the content being decided.
    pub expected_content_hash: String,
    pub reviewer: QuarantineReviewer,
    pub rationale: Option<String>,
}

/// One record as it stands now, read back from the store rather than
/// assumed from the request. For a replay this is the current state, which
/// may have moved on since the decision (a successor forgotten later stays
/// forgotten).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReviewedRecordReceipt {
    pub memory_id: MemoryId,
    pub scope: MemoryScope,
    pub kind: MemoryKind,
    pub status: RecordStatus,
    pub trust: TrustTier,
    /// The durable §10.2 taint marker.
    pub ever_quarantined: bool,
    pub content_hash: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supersedes: Option<MemoryId>,
    pub derived_from: Vec<MemoryId>,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
}

/// The review annotation carried in each applied op's audit `detail.review`
/// (§8.5 one audit entry per op): who decided what, over which content, and
/// what the decision produced. This is the evidence a replay is recognized
/// by, and it keeps the quarantine reason the origin's tombstone clears from
/// the row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewAudit {
    pub verdict: QuarantineDecision,
    pub reviewer: QuarantineReviewer,
    pub origin: MemoryId,
    /// The release successor (`None` for a tombstone).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub successor: Option<MemoryId>,
    pub expected_content_hash: String,
    pub origin_quarantine_reason: String,
    /// Gated promotions of the origin that had outlived
    /// [`super::capabilities::GATED_PROMOTION_EXPIRY_MS`] and were expired
    /// by this review (their staged batches discarded).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub expired_promotions: Vec<String>,
    /// Live gated promotions of the origin that an operator's tombstone
    /// invalidated in the same transaction (operator invalidation): their
    /// mappings were expired and their staged batches discarded, so no later
    /// approval can publish the discarded content.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub invalidated_promotions: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rationale: Option<String>,
}

/// One committed review as its audit rows record it: the historical
/// decision, kept apart from the records' current (mutable) status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReviewDecision {
    pub audit_token: String,
    pub decided_at_ms: u64,
    pub review: ReviewAudit,
}

/// What a review did. The `Already*` variants are idempotent replays: the
/// same decision was committed before (`decision` is that committed review)
/// and nothing was written now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuarantineReviewOutcome {
    Released {
        origin: ReviewedRecordReceipt,
        successor: ReviewedRecordReceipt,
        /// The active prior a quarantined update superseded on release.
        superseded_prior: Option<MemoryId>,
        decision: ReviewDecision,
    },
    AlreadyReleased {
        origin: ReviewedRecordReceipt,
        successor: ReviewedRecordReceipt,
        decision: ReviewDecision,
    },
    Tombstoned {
        origin: ReviewedRecordReceipt,
        decision: ReviewDecision,
    },
    AlreadyTombstoned {
        origin: ReviewedRecordReceipt,
        decision: ReviewDecision,
    },
}

impl QuarantineReviewOutcome {
    pub fn outcome_str(&self) -> &'static str {
        match self {
            Self::Released { .. } => "released",
            Self::AlreadyReleased { .. } => "already_released",
            Self::Tombstoned { .. } => "tombstoned",
            Self::AlreadyTombstoned { .. } => "already_tombstoned",
        }
    }

    /// Whether this call applied the decision (as opposed to a replay).
    pub fn applied(&self) -> bool {
        matches!(self, Self::Released { .. } | Self::Tombstoned { .. })
    }

    pub fn origin(&self) -> &ReviewedRecordReceipt {
        match self {
            Self::Released { origin, .. }
            | Self::AlreadyReleased { origin, .. }
            | Self::Tombstoned { origin, .. }
            | Self::AlreadyTombstoned { origin, .. } => origin,
        }
    }

    pub fn successor(&self) -> Option<&ReviewedRecordReceipt> {
        match self {
            Self::Released { successor, .. } | Self::AlreadyReleased { successor, .. } => {
                Some(successor)
            }
            Self::Tombstoned { .. } | Self::AlreadyTombstoned { .. } => None,
        }
    }

    pub fn decision(&self) -> &ReviewDecision {
        match self {
            Self::Released { decision, .. }
            | Self::AlreadyReleased { decision, .. }
            | Self::Tombstoned { decision, .. }
            | Self::AlreadyTombstoned { decision, .. } => decision,
        }
    }
}

/// Why a review applied nothing. Typed so callers branch on the reason, not
/// on message text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuarantineReviewRefusal {
    /// No record with this id lives in the authorized scope. Deliberately the
    /// same answer for "missing" and "in another scope".
    NotFound,
    /// The record's content is not the content the reviewer read. Neither
    /// hash is echoed: knowing the stored hash must never stand in for
    /// reading the record.
    ContentMismatch,
    /// The record is not awaiting review. `released_as` names the release
    /// successor when a committed review released it.
    NotQuarantined {
        status: &'static str,
        released_as: Option<MemoryId>,
    },
    /// A gated promotion of this record awaits a decision through the gating
    /// flow, which owns its publication until it is decided or expires at
    /// `expires_at_ms`; a review after that expires it. Only an operator's
    /// tombstone is not refused: it invalidates the promotion instead.
    GatePending {
        pending_id: String,
        expires_at_ms: u64,
    },
    /// Another record already holds this origin's release successor id. It
    /// is not this review's successor, so nothing is adopted or written.
    SuccessorConflict { successor_id: MemoryId },
    /// Release refused by the §10.4 secret gate. The class is named; the
    /// matched text never is. Tombstone remains the exit.
    SecretDetected { class: &'static str },
    /// Release of a quarantined update whose prior is no longer active
    /// (absent, tombstoned or superseded): releasing would resurrect a
    /// forgotten fact or fork a newer version.
    StaleUpdate {
        prior: MemoryId,
        prior_status: &'static str,
    },
}

impl QuarantineReviewRefusal {
    pub fn reason_str(&self) -> &'static str {
        match self {
            Self::NotFound => "not_found",
            Self::ContentMismatch => "content_mismatch",
            Self::NotQuarantined { .. } => "not_quarantined",
            Self::GatePending { .. } => "gate_pending",
            Self::SuccessorConflict { .. } => "successor_conflict",
            Self::SecretDetected { .. } => "secret_detected",
            Self::StaleUpdate { .. } => "stale_update",
        }
    }
}

impl std::fmt::Display for QuarantineReviewRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => write!(f, "no such memory record in this scope"),
            Self::ContentMismatch => write!(
                f,
                "the record's content is not the content that was reviewed"
            ),
            Self::NotQuarantined {
                status,
                released_as: Some(successor),
            } => write!(
                f,
                "record is {status}, not quarantined: it was released as '{successor}'"
            ),
            Self::NotQuarantined {
                status,
                released_as: None,
            } => write!(f, "record is {status}, not quarantined"),
            Self::GatePending {
                pending_id,
                expires_at_ms,
            } => write!(
                f,
                "a gated promotion of this record awaits a gating decision ('{pending_id}'); \
                 it expires at {expires_at_ms} ms"
            ),
            Self::SuccessorConflict { successor_id } => write!(
                f,
                "another record already holds the release successor id '{successor_id}'"
            ),
            Self::SecretDetected { class } => write!(
                f,
                "release refused: content matches the '{class}' secret pattern class \
                 (§10.4); tombstone is the only exit"
            ),
            Self::StaleUpdate {
                prior,
                prior_status,
            } => write!(
                f,
                "release refused: this quarantined update's prior '{prior}' is \
                 {prior_status}, not active"
            ),
        }
    }
}

#[derive(Debug)]
pub enum QuarantineReviewError {
    Refused(QuarantineReviewRefusal),
    Store(AgentMemoryError),
}

impl std::fmt::Display for QuarantineReviewError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Refused(refusal) => write!(f, "quarantine review refused: {refusal}"),
            Self::Store(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for QuarantineReviewError {}

impl From<AgentMemoryError> for QuarantineReviewError {
    fn from(err: AgentMemoryError) -> Self {
        Self::Store(err)
    }
}

/// The id of `origin`'s release successor.
pub fn release_successor_id(origin: &str) -> MemoryId {
    format!("{origin}{RELEASE_SUCCESSOR_SUFFIX}")
}

/// The content copy used for quarantine releases and promotions: same
/// title/body/tags, no evidence (derived_from carries lineage and the
/// §10.2 ceiling walks it).
pub fn release_copy(record: &MemoryRecord) -> NewMemoryRecord {
    NewMemoryRecord {
        kind: record.kind,
        title: record.title.clone(),
        description: record.description.clone(),
        body: record.body.clone(),
        tags: record.tags.clone(),
        evidence: Vec::new(),
        verification: record.provenance.verification.clone(),
    }
}

/// The ops of one release: the successor first (so its insert inherits the
/// still-quarantined origin's `ever_quarantined` bit and the LLM
/// tombstone-recreation guard never sees the copy), then the origin's
/// tombstone. A quarantined update supersedes its prior; the validator
/// refuses the batch when that prior is no longer active.
pub fn quarantine_release_ops(
    origin: &MemoryRecord,
    create_rationale: String,
    tombstone_rationale: String,
) -> Vec<StagedOp> {
    let successor = Some(release_successor_id(&origin.id));
    let record = release_copy(origin);
    let derived_from = vec![origin.id.clone()];
    let first = match &origin.supersedes {
        Some(prior) => StagedOp::Supersede {
            id: successor,
            prior: prior.clone(),
            record,
            trust: TrustTier::AgentObserved,
            derived_from,
            rationale: Some(create_rationale),
        },
        None => StagedOp::Create {
            id: successor,
            scope: origin.scope.clone(),
            record,
            trust: TrustTier::AgentObserved,
            derived_from,
            rationale: Some(create_rationale),
            created_at_ms: None,
            updated_at_ms: None,
        },
    };
    vec![
        first,
        StagedOp::Tombstone {
            id: origin.id.clone(),
            rationale: Some(tombstone_rationale),
        },
    ]
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::memory::records::{MemoryProvenance, UsageStats};

    fn quarantined(supersedes: Option<&str>) -> MemoryRecord {
        MemoryRecord {
            id: "mem-origin".to_string(),
            scope: MemoryScope::Identity {
                realm: "default".to_string(),
                identity: "lead:main".to_string(),
            },
            kind: MemoryKind::Preference,
            title: "Reading preference".to_string(),
            description: "When recommending books".to_string(),
            body: "Prefers slow literary fantasy.".to_string(),
            tags: vec!["epistemic:operator_said".to_string()],
            provenance: MemoryProvenance {
                evidence: Vec::new(),
                author: MemoryAuthor::Agent {
                    identity: "lead:main".to_string(),
                },
                profile: None,
                verification: None,
            },
            trust: TrustTier::AgentObserved,
            status: RecordStatus::Quarantined {
                reason: "session tainted".to_string(),
            },
            supersedes: supersedes.map(str::to_string),
            derived_from: Vec::new(),
            working_set_rank: None,
            created_at_ms: 1,
            updated_at_ms: 1,
            usage: UsageStats::default(),
            ever_quarantined: true,
        }
    }

    #[test]
    fn reviewers_author_their_batches() {
        assert_eq!(
            QuarantineReviewer::Operator { principal: None }.author(),
            MemoryAuthor::Operator
        );
        assert_eq!(
            QuarantineReviewer::Steward {
                run_id: "dream-1".to_string()
            }
            .author(),
            MemoryAuthor::Steward {
                run_id: "dream-1".to_string()
            }
        );
    }

    #[test]
    fn decision_round_trips_its_wire_names() {
        for decision in [QuarantineDecision::Release, QuarantineDecision::Tombstone] {
            assert_eq!(QuarantineDecision::parse(decision.as_str()), Some(decision));
        }
        assert_eq!(QuarantineDecision::parse("hold"), None);
    }

    #[test]
    fn release_of_a_fresh_write_creates_the_named_successor_then_tombstones() {
        let origin = quarantined(None);
        let ops = quarantine_release_ops(&origin, "create".into(), "tombstone".into());
        assert_eq!(ops.len(), 2);
        let StagedOp::Create {
            id,
            scope,
            record,
            trust,
            derived_from,
            ..
        } = &ops[0]
        else {
            panic!("first op must create the successor: {ops:?}");
        };
        assert_eq!(id.as_deref(), Some("mem-origin-released"));
        assert_eq!(scope, &origin.scope);
        assert_eq!(*trust, TrustTier::AgentObserved);
        assert_eq!(derived_from, &vec!["mem-origin".to_string()]);
        assert_eq!(record.body, origin.body);
        assert_eq!(record.tags, origin.tags);
        assert!(record.evidence.is_empty());
        assert!(matches!(&ops[1], StagedOp::Tombstone { id, .. } if id == "mem-origin"));
    }

    #[test]
    fn release_of_a_quarantined_update_supersedes_its_prior() {
        let origin = quarantined(Some("mem-prior"));
        let ops = quarantine_release_ops(&origin, "create".into(), "tombstone".into());
        let StagedOp::Supersede {
            id,
            prior,
            trust,
            derived_from,
            ..
        } = &ops[0]
        else {
            panic!("first op must supersede the prior: {ops:?}");
        };
        assert_eq!(id.as_deref(), Some("mem-origin-released"));
        assert_eq!(prior, "mem-prior");
        assert_eq!(*trust, TrustTier::AgentObserved);
        assert_eq!(derived_from, &vec!["mem-origin".to_string()]);
        assert!(matches!(&ops[1], StagedOp::Tombstone { id, .. } if id == "mem-origin"));
    }
}
