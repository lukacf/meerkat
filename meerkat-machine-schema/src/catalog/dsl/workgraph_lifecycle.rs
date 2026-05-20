use super::OptionValueExt;
use meerkat_machine_dsl::machine;

#[derive(
    Debug,
    Clone,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct WorkItemKey(pub String);

impl WorkItemKey {
    fn len(&self) -> u64 {
        self.0.len() as u64
    }
}

impl<T: Into<String>> From<T> for WorkItemKey {
    fn from(value: T) -> Self {
        Self(value.into())
    }
}

#[derive(
    Debug,
    Clone,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct WorkEdgeKey {
    pub kind: WorkEdgeKind,
    pub from_item_key: WorkItemKey,
    pub to_item_key: WorkItemKey,
}

#[derive(
    Debug,
    Clone,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct WorkDependencyPathKey {
    pub kind: WorkEdgeKind,
    pub from_item_key: WorkItemKey,
    pub to_item_key: WorkItemKey,
}

#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    serde::Serialize,
    serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum WorkOwnerKind {
    Principal,
    Agent,
    Session,
    Mob,
    #[default]
    Label,
}

#[derive(
    Debug,
    Clone,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct WorkOwnerKey {
    pub kind: WorkOwnerKind,
    pub id: String,
}

#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    serde::Serialize,
    serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum WorkEdgeKind {
    #[default]
    Blocks,
    Parent,
    Related,
    Supersedes,
    DerivedFrom,
}

#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    serde::Serialize,
    serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum WorkGraphErrorKind {
    #[default]
    NotFound,
    StaleRevision,
    Conflict,
    InvalidTransition,
    InvalidInput,
    InvalidTimestampMillis,
    UnsupportedBackend,
    Store,
}

#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    serde::Serialize,
    serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum WorkGraphPublicErrorClass {
    #[default]
    NotFound,
    Conflict,
    InvalidTransition,
    InvalidArguments,
    CapabilityUnavailable,
    StoreError,
}

machine! {
    machine WorkGraphLifecycleMachine {
        version: 2,
        rust: "self" / "catalog::dsl::workgraph_lifecycle",

        state {
            lifecycle_phase: WorkLifecycleState,
            revision: u64,
            item_key: Option<WorkItemKey>,
            external_ref_tokens: Seq<String>,
            evidence_ref_tokens: Seq<String>,
            unresolved_blocker_count: u64,
            claim_owner_key: Option<WorkOwnerKey>,
            claimed_at_utc_ms: Option<u64>,
            lease_expires_at_utc_ms: Option<u64>,
            due_at_utc_ms: Option<u64>,
            not_before_utc_ms: Option<u64>,
            snoozed_until_utc_ms: Option<u64>,
            terminal_at_utc_ms: Option<u64>,
            evidence_count: u64,
        }

        init(Absent) {
            revision = 0,
            item_key = None,
            external_ref_tokens = EmptySeq,
            evidence_ref_tokens = EmptySeq,
            unresolved_blocker_count = 0,
            claim_owner_key = None,
            claimed_at_utc_ms = None,
            lease_expires_at_utc_ms = None,
            due_at_utc_ms = None,
            not_before_utc_ms = None,
            snoozed_until_utc_ms = None,
            terminal_at_utc_ms = None,
            evidence_count = 0,
        }

        terminal [Completed, Cancelled, Failed]

        phase WorkLifecycleState {
            Absent,
            Open,
            InProgress,
            Blocked,
            Completed,
            Cancelled,
            Failed,
        }

        input WorkGraphLifecycleInput {
            Create {
                item_key: WorkItemKey,
                external_ref_tokens: Seq<String>,
                evidence_ref_tokens: Seq<String>,
                evidence_ref_count: u64,
                due_at_utc_ms: Option<u64>,
                not_before_utc_ms: Option<u64>,
                snoozed_until_utc_ms: Option<u64>,
                unresolved_blocker_count: u64,
                requested_status: Option<Enum<WorkLifecycleState>>,
            },
            CreateOpen {
                item_key: WorkItemKey,
                external_ref_tokens: Seq<String>,
                evidence_ref_tokens: Seq<String>,
                evidence_ref_count: u64,
                due_at_utc_ms: Option<u64>,
                not_before_utc_ms: Option<u64>,
                snoozed_until_utc_ms: Option<u64>,
                unresolved_blocker_count: u64,
            },
            CreateBlocked {
                item_key: WorkItemKey,
                external_ref_tokens: Seq<String>,
                evidence_ref_tokens: Seq<String>,
                evidence_ref_count: u64,
                due_at_utc_ms: Option<u64>,
                not_before_utc_ms: Option<u64>,
                snoozed_until_utc_ms: Option<u64>,
                unresolved_blocker_count: u64,
            },
            Update {
                expected_revision: u64,
                external_ref_tokens: Seq<String>,
                due_at_utc_ms: Option<u64>,
                not_before_utc_ms: Option<u64>,
                snoozed_until_utc_ms: Option<u64>,
                unresolved_blocker_count: u64,
            },
            Claim {
                expected_revision: u64,
                owner_key: WorkOwnerKey,
                now_utc_ms: u64,
                lease_expires_at_utc_ms: Option<u64>,
            },
            Release { expected_revision: u64 },
            Block { expected_revision: u64 },
            RefreshEligibility { unresolved_blocker_count: u64 },
            ClassifyReadiness { now_utc_ms: u64 },
            ClassifyBlockerSatisfaction,
            ClassifyTerminality,
            ValidateLink {
                kind: WorkEdgeKind,
                from_item_key: WorkItemKey,
                to_item_key: WorkItemKey,
                edge_key: WorkEdgeKey,
                reverse_path_key: WorkDependencyPathKey,
                topology_item_keys: Set<WorkItemKey>,
                topology_edge_keys: Set<WorkEdgeKey>,
                blocks_reachability: Set<WorkDependencyPathKey>,
                parent_reachability: Set<WorkDependencyPathKey>,
            },
            Close { expected_revision: u64, at_utc_ms: u64, requested_status: Option<Enum<WorkLifecycleState>> },
            CloseCompleted { expected_revision: u64, at_utc_ms: u64 },
            CloseCancelled { expected_revision: u64, at_utc_ms: u64 },
            CloseFailed { expected_revision: u64, at_utc_ms: u64 },
            AddEvidence { expected_revision: u64, evidence_ref_tokens: Seq<String>, evidence_ref_count: u64 },
            ClassifyPublicError { error_kind: Enum<WorkGraphErrorKind> },
        }

        effect WorkGraphLifecycleEffect {
            Created,
            Updated,
            Claimed { owner_key: WorkOwnerKey },
            Released,
            Blocked,
            BlockerSatisfied,
            BlockerUnsatisfied,
            LifecycleTerminal,
            LifecycleNonTerminal,
            WorkReady,
            WorkNotReady,
            LinkValidated,
            Closed { terminal_state: WorkLifecycleState },
            EvidenceAdded,
            PublicErrorClassified { public_class: Enum<WorkGraphPublicErrorClass> },
        }

        invariant absent_has_zero_revision {
            self.lifecycle_phase != Phase::Absent || self.revision == 0
        }

        invariant live_has_positive_revision {
            self.lifecycle_phase == Phase::Absent || self.revision > 0
        }

        invariant absent_has_no_item_projection {
            self.lifecycle_phase != Phase::Absent
                || (self.item_key == None
                    && for_all(token in self.external_ref_tokens, false)
                    && for_all(token in self.evidence_ref_tokens, false))
        }

        invariant live_has_item_key {
            self.lifecycle_phase == Phase::Absent || self.item_key != None
        }

        invariant terminal_has_terminal_time {
            (self.lifecycle_phase != Phase::Completed && self.lifecycle_phase != Phase::Cancelled && self.lifecycle_phase != Phase::Failed)
                || self.terminal_at_utc_ms != None
        }

        invariant claim_only_in_progress {
            self.claim_owner_key == None || self.lifecycle_phase == Phase::InProgress
        }

        invariant blocked_has_no_claim {
            self.lifecycle_phase != Phase::Blocked || self.claim_owner_key == None
        }

        invariant terminal_has_no_claim {
            (self.lifecycle_phase != Phase::Completed && self.lifecycle_phase != Phase::Cancelled && self.lifecycle_phase != Phase::Failed)
                || self.claim_owner_key == None
        }

        disposition Created => local,
        disposition Updated => local,
        disposition Claimed => local,
        disposition Released => local,
        disposition Blocked => local,
        disposition BlockerSatisfied => local,
        disposition BlockerUnsatisfied => local,
        disposition LifecycleTerminal => local,
        disposition LifecycleNonTerminal => local,
        disposition WorkReady => local,
        disposition WorkNotReady => local,
        disposition LinkValidated => local,
        disposition Closed => local,
        disposition EvidenceAdded => local,
        disposition PublicErrorClassified => local,

        transition CreateDefaultOrOpen {
            on input Create { item_key, external_ref_tokens, evidence_ref_tokens, evidence_ref_count, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, unresolved_blocker_count, requested_status }
            guard "absent" { self.lifecycle_phase == Phase::Absent }
            guard "default_or_open" { requested_status == None || requested_status == Some(WorkLifecycleState::Open) }
            guard "item_key_present" { workgraph_item_key_present(item_key) }
            update {
                self.revision = 1;
                self.item_key = Some(item_key);
                self.external_ref_tokens = external_ref_tokens;
                self.evidence_ref_tokens = evidence_ref_tokens;
                self.evidence_count = evidence_ref_count;
                self.unresolved_blocker_count = unresolved_blocker_count;
                self.due_at_utc_ms = due_at_utc_ms;
                self.not_before_utc_ms = not_before_utc_ms;
                self.snoozed_until_utc_ms = snoozed_until_utc_ms;
            }
            to Open
            emit Created
        }

        transition CreateRequestedBlocked {
            on input Create { item_key, external_ref_tokens, evidence_ref_tokens, evidence_ref_count, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, unresolved_blocker_count, requested_status }
            guard "absent" { self.lifecycle_phase == Phase::Absent }
            guard "requested_blocked" { requested_status == Some(WorkLifecycleState::Blocked) }
            guard "item_key_present" { workgraph_item_key_present(item_key) }
            update {
                self.revision = 1;
                self.item_key = Some(item_key);
                self.external_ref_tokens = external_ref_tokens;
                self.evidence_ref_tokens = evidence_ref_tokens;
                self.evidence_count = evidence_ref_count;
                self.unresolved_blocker_count = unresolved_blocker_count;
                self.due_at_utc_ms = due_at_utc_ms;
                self.not_before_utc_ms = not_before_utc_ms;
                self.snoozed_until_utc_ms = snoozed_until_utc_ms;
            }
            to Blocked
            emit Created
        }

        transition CreateOpen {
            on input CreateOpen { item_key, external_ref_tokens, evidence_ref_tokens, evidence_ref_count, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, unresolved_blocker_count }
            guard { self.lifecycle_phase == Phase::Absent }
            guard "item_key_present" { workgraph_item_key_present(item_key) }
            update {
                self.revision = 1;
                self.item_key = Some(item_key);
                self.external_ref_tokens = external_ref_tokens;
                self.evidence_ref_tokens = evidence_ref_tokens;
                self.evidence_count = evidence_ref_count;
                self.unresolved_blocker_count = unresolved_blocker_count;
                self.due_at_utc_ms = due_at_utc_ms;
                self.not_before_utc_ms = not_before_utc_ms;
                self.snoozed_until_utc_ms = snoozed_until_utc_ms;
            }
            to Open
            emit Created
        }

        transition CreateBlocked {
            on input CreateBlocked { item_key, external_ref_tokens, evidence_ref_tokens, evidence_ref_count, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, unresolved_blocker_count }
            guard { self.lifecycle_phase == Phase::Absent }
            guard "item_key_present" { workgraph_item_key_present(item_key) }
            update {
                self.revision = 1;
                self.item_key = Some(item_key);
                self.external_ref_tokens = external_ref_tokens;
                self.evidence_ref_tokens = evidence_ref_tokens;
                self.evidence_count = evidence_ref_count;
                self.unresolved_blocker_count = unresolved_blocker_count;
                self.due_at_utc_ms = due_at_utc_ms;
                self.not_before_utc_ms = not_before_utc_ms;
                self.snoozed_until_utc_ms = snoozed_until_utc_ms;
            }
            to Blocked
            emit Created
        }

        transition UpdateOpen {
            on input Update { expected_revision, external_ref_tokens, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, unresolved_blocker_count }
            guard { self.lifecycle_phase == Phase::Open && self.revision == expected_revision }
            update {
                self.revision += 1;
                self.external_ref_tokens = external_ref_tokens;
                self.unresolved_blocker_count = unresolved_blocker_count;
                self.due_at_utc_ms = due_at_utc_ms;
                self.not_before_utc_ms = not_before_utc_ms;
                self.snoozed_until_utc_ms = snoozed_until_utc_ms;
            }
            to Open
            emit Updated
        }

        transition UpdateInProgress {
            on input Update { expected_revision, external_ref_tokens, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, unresolved_blocker_count }
            guard { self.lifecycle_phase == Phase::InProgress && self.revision == expected_revision }
            update {
                self.revision += 1;
                self.external_ref_tokens = external_ref_tokens;
                self.unresolved_blocker_count = unresolved_blocker_count;
                self.due_at_utc_ms = due_at_utc_ms;
                self.not_before_utc_ms = not_before_utc_ms;
                self.snoozed_until_utc_ms = snoozed_until_utc_ms;
            }
            to InProgress
            emit Updated
        }

        transition UpdateBlocked {
            on input Update { expected_revision, external_ref_tokens, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, unresolved_blocker_count }
            guard { self.lifecycle_phase == Phase::Blocked && self.revision == expected_revision }
            update {
                self.revision += 1;
                self.external_ref_tokens = external_ref_tokens;
                self.unresolved_blocker_count = unresolved_blocker_count;
                self.due_at_utc_ms = due_at_utc_ms;
                self.not_before_utc_ms = not_before_utc_ms;
                self.snoozed_until_utc_ms = snoozed_until_utc_ms;
            }
            to Blocked
            emit Updated
        }

        transition ClaimOpen {
            on input Claim { expected_revision, owner_key, now_utc_ms, lease_expires_at_utc_ms }
            guard "revision_matches" { self.lifecycle_phase == Phase::Open && self.revision == expected_revision }
            guard "dependencies_satisfied" { self.unresolved_blocker_count == 0 }
            guard "due_eligible" { if self.due_at_utc_ms == None { true } else { self.due_at_utc_ms.get("value") <= now_utc_ms } }
            guard "not_before_eligible" { if self.not_before_utc_ms == None { true } else { self.not_before_utc_ms.get("value") <= now_utc_ms } }
            guard "snooze_eligible" { if self.snoozed_until_utc_ms == None { true } else { self.snoozed_until_utc_ms.get("value") <= now_utc_ms } }
            update {
                self.revision += 1;
                self.claim_owner_key = Some(owner_key);
                self.claimed_at_utc_ms = Some(now_utc_ms);
                self.lease_expires_at_utc_ms = lease_expires_at_utc_ms;
            }
            to InProgress
            emit Claimed { owner_key: owner_key }
        }

        transition ClaimExpiredInProgress {
            on input Claim { expected_revision, owner_key, now_utc_ms, lease_expires_at_utc_ms }
            guard "revision_matches" { self.lifecycle_phase == Phase::InProgress && self.revision == expected_revision }
            guard "prior_claim_present" { self.claim_owner_key != None }
            guard "prior_claim_has_lease" { self.lease_expires_at_utc_ms != None }
            guard "prior_claim_expired" { if self.lease_expires_at_utc_ms == None { false } else { self.lease_expires_at_utc_ms.get("value") <= now_utc_ms } }
            guard "dependencies_satisfied" { self.unresolved_blocker_count == 0 }
            guard "due_eligible" { if self.due_at_utc_ms == None { true } else { self.due_at_utc_ms.get("value") <= now_utc_ms } }
            guard "not_before_eligible" { if self.not_before_utc_ms == None { true } else { self.not_before_utc_ms.get("value") <= now_utc_ms } }
            guard "snooze_eligible" { if self.snoozed_until_utc_ms == None { true } else { self.snoozed_until_utc_ms.get("value") <= now_utc_ms } }
            update {
                self.revision += 1;
                self.claim_owner_key = Some(owner_key);
                self.claimed_at_utc_ms = Some(now_utc_ms);
                self.lease_expires_at_utc_ms = lease_expires_at_utc_ms;
            }
            to InProgress
            emit Claimed { owner_key: owner_key }
        }

        transition ReleaseInProgress {
            on input Release { expected_revision }
            guard { self.lifecycle_phase == Phase::InProgress && self.revision == expected_revision && self.claim_owner_key != None }
            update {
                self.revision += 1;
                self.claim_owner_key = None;
                self.claimed_at_utc_ms = None;
                self.lease_expires_at_utc_ms = None;
            }
            to Open
            emit Released
        }

        transition BlockOpen {
            on input Block { expected_revision }
            guard { self.lifecycle_phase == Phase::Open && self.revision == expected_revision }
            update {
                self.revision += 1;
                self.claim_owner_key = None;
                self.claimed_at_utc_ms = None;
                self.lease_expires_at_utc_ms = None;
            }
            to Blocked
            emit Blocked
        }

        transition BlockInProgress {
            on input Block { expected_revision }
            guard { self.lifecycle_phase == Phase::InProgress && self.revision == expected_revision }
            update {
                self.revision += 1;
                self.claim_owner_key = None;
                self.claimed_at_utc_ms = None;
                self.lease_expires_at_utc_ms = None;
            }
            to Blocked
            emit Blocked
        }

        transition BlockBlocked {
            on input Block { expected_revision }
            guard { self.lifecycle_phase == Phase::Blocked && self.revision == expected_revision }
            update {
                self.revision += 1;
                self.claim_owner_key = None;
                self.claimed_at_utc_ms = None;
                self.lease_expires_at_utc_ms = None;
            }
            to Blocked
            emit Blocked
        }

        transition RefreshEligibilityOpen {
            on input RefreshEligibility { unresolved_blocker_count }
            guard { self.lifecycle_phase == Phase::Open }
            update {
                self.unresolved_blocker_count = unresolved_blocker_count;
            }
            to Open
            emit Updated
        }

        transition RefreshEligibilityInProgress {
            on input RefreshEligibility { unresolved_blocker_count }
            guard { self.lifecycle_phase == Phase::InProgress }
            update {
                self.unresolved_blocker_count = unresolved_blocker_count;
            }
            to InProgress
            emit Updated
        }

        transition RefreshEligibilityBlocked {
            on input RefreshEligibility { unresolved_blocker_count }
            guard { self.lifecycle_phase == Phase::Blocked }
            update {
                self.unresolved_blocker_count = unresolved_blocker_count;
            }
            to Blocked
            emit Updated
        }

        transition ClassifyReadinessOpenReady {
            on input ClassifyReadiness { now_utc_ms }
            guard { self.lifecycle_phase == Phase::Open }
            guard "dependencies_satisfied" { self.unresolved_blocker_count == 0 }
            guard "due_eligible" { if self.due_at_utc_ms == None { true } else { self.due_at_utc_ms.get("value") <= now_utc_ms } }
            guard "not_before_eligible" { if self.not_before_utc_ms == None { true } else { self.not_before_utc_ms.get("value") <= now_utc_ms } }
            guard "snooze_eligible" { if self.snoozed_until_utc_ms == None { true } else { self.snoozed_until_utc_ms.get("value") <= now_utc_ms } }
            to Open
            emit WorkReady
        }

        transition ClassifyReadinessExpiredInProgressReady {
            on input ClassifyReadiness { now_utc_ms }
            guard { self.lifecycle_phase == Phase::InProgress }
            guard "prior_claim_present" { self.claim_owner_key != None }
            guard "prior_claim_has_lease" { self.lease_expires_at_utc_ms != None }
            guard "prior_claim_expired" { if self.lease_expires_at_utc_ms == None { false } else { self.lease_expires_at_utc_ms.get("value") <= now_utc_ms } }
            guard "dependencies_satisfied" { self.unresolved_blocker_count == 0 }
            guard "due_eligible" { if self.due_at_utc_ms == None { true } else { self.due_at_utc_ms.get("value") <= now_utc_ms } }
            guard "not_before_eligible" { if self.not_before_utc_ms == None { true } else { self.not_before_utc_ms.get("value") <= now_utc_ms } }
            guard "snooze_eligible" { if self.snoozed_until_utc_ms == None { true } else { self.snoozed_until_utc_ms.get("value") <= now_utc_ms } }
            to InProgress
            emit WorkReady
        }

        transition ClassifyReadinessAbsentNotReady {
            on input ClassifyReadiness { now_utc_ms }
            guard { self.lifecycle_phase == Phase::Absent }
            to Absent
            emit WorkNotReady
        }

        transition ClassifyReadinessOpenNotReady {
            on input ClassifyReadiness { now_utc_ms }
            guard { self.lifecycle_phase == Phase::Open }
            guard "not_ready" {
                self.unresolved_blocker_count != 0
                    || (if self.due_at_utc_ms == None { false } else { now_utc_ms < self.due_at_utc_ms.get("value") })
                    || (if self.not_before_utc_ms == None { false } else { now_utc_ms < self.not_before_utc_ms.get("value") })
                    || (if self.snoozed_until_utc_ms == None { false } else { now_utc_ms < self.snoozed_until_utc_ms.get("value") })
            }
            to Open
            emit WorkNotReady
        }

        transition ClassifyReadinessInProgressNotReady {
            on input ClassifyReadiness { now_utc_ms }
            guard { self.lifecycle_phase == Phase::InProgress }
            guard "not_ready" {
                self.claim_owner_key == None
                    || self.lease_expires_at_utc_ms == None
                    || (if self.lease_expires_at_utc_ms == None { false } else { now_utc_ms < self.lease_expires_at_utc_ms.get("value") })
                    || self.unresolved_blocker_count != 0
                    || (if self.due_at_utc_ms == None { false } else { now_utc_ms < self.due_at_utc_ms.get("value") })
                    || (if self.not_before_utc_ms == None { false } else { now_utc_ms < self.not_before_utc_ms.get("value") })
                    || (if self.snoozed_until_utc_ms == None { false } else { now_utc_ms < self.snoozed_until_utc_ms.get("value") })
            }
            to InProgress
            emit WorkNotReady
        }

        transition ClassifyReadinessBlockedNotReady {
            on input ClassifyReadiness { now_utc_ms }
            guard { self.lifecycle_phase == Phase::Blocked }
            to Blocked
            emit WorkNotReady
        }

        transition ClassifyReadinessCompletedNotReady {
            on input ClassifyReadiness { now_utc_ms }
            guard { self.lifecycle_phase == Phase::Completed }
            to Completed
            emit WorkNotReady
        }

        transition ClassifyReadinessCancelledNotReady {
            on input ClassifyReadiness { now_utc_ms }
            guard { self.lifecycle_phase == Phase::Cancelled }
            to Cancelled
            emit WorkNotReady
        }

        transition ClassifyReadinessFailedNotReady {
            on input ClassifyReadiness { now_utc_ms }
            guard { self.lifecycle_phase == Phase::Failed }
            to Failed
            emit WorkNotReady
        }

        transition ClassifyBlockerSatisfiedCompleted {
            on input ClassifyBlockerSatisfaction
            guard { self.lifecycle_phase == Phase::Completed }
            to Completed
            emit BlockerSatisfied
        }

        transition ClassifyBlockerUnsatisfiedAbsent {
            on input ClassifyBlockerSatisfaction
            guard { self.lifecycle_phase == Phase::Absent }
            to Absent
            emit BlockerUnsatisfied
        }

        transition ClassifyBlockerUnsatisfiedOpen {
            on input ClassifyBlockerSatisfaction
            guard { self.lifecycle_phase == Phase::Open }
            to Open
            emit BlockerUnsatisfied
        }

        transition ClassifyBlockerUnsatisfiedInProgress {
            on input ClassifyBlockerSatisfaction
            guard { self.lifecycle_phase == Phase::InProgress }
            to InProgress
            emit BlockerUnsatisfied
        }

        transition ClassifyBlockerUnsatisfiedBlocked {
            on input ClassifyBlockerSatisfaction
            guard { self.lifecycle_phase == Phase::Blocked }
            to Blocked
            emit BlockerUnsatisfied
        }

        transition ClassifyBlockerUnsatisfiedCancelled {
            on input ClassifyBlockerSatisfaction
            guard { self.lifecycle_phase == Phase::Cancelled }
            to Cancelled
            emit BlockerUnsatisfied
        }

        transition ClassifyBlockerUnsatisfiedFailed {
            on input ClassifyBlockerSatisfaction
            guard { self.lifecycle_phase == Phase::Failed }
            to Failed
            emit BlockerUnsatisfied
        }

        transition ClassifyTerminalityAbsent {
            on input ClassifyTerminality
            guard { self.lifecycle_phase == Phase::Absent }
            to Absent
            emit LifecycleNonTerminal
        }

        transition ClassifyTerminalityOpen {
            on input ClassifyTerminality
            guard { self.lifecycle_phase == Phase::Open }
            to Open
            emit LifecycleNonTerminal
        }

        transition ClassifyTerminalityInProgress {
            on input ClassifyTerminality
            guard { self.lifecycle_phase == Phase::InProgress }
            to InProgress
            emit LifecycleNonTerminal
        }

        transition ClassifyTerminalityBlocked {
            on input ClassifyTerminality
            guard { self.lifecycle_phase == Phase::Blocked }
            to Blocked
            emit LifecycleNonTerminal
        }

        transition ClassifyTerminalityCompleted {
            on input ClassifyTerminality
            guard { self.lifecycle_phase == Phase::Completed }
            to Completed
            emit LifecycleTerminal
        }

        transition ClassifyTerminalityCancelled {
            on input ClassifyTerminality
            guard { self.lifecycle_phase == Phase::Cancelled }
            to Cancelled
            emit LifecycleTerminal
        }

        transition ClassifyTerminalityFailed {
            on input ClassifyTerminality
            guard { self.lifecycle_phase == Phase::Failed }
            to Failed
            emit LifecycleTerminal
        }

        transition ValidateLink {
            on input ValidateLink { kind, from_item_key, to_item_key, edge_key, reverse_path_key, topology_item_keys, topology_edge_keys, blocks_reachability, parent_reachability }
            guard "stateless_checker" { self.lifecycle_phase == Phase::Absent }
            guard "from_endpoint_exists" { topology_item_keys.contains(from_item_key) }
            guard "to_endpoint_exists" { topology_item_keys.contains(to_item_key) }
            guard "not_self_edge" { from_item_key != to_item_key }
            guard "not_duplicate_edge" { topology_edge_keys.contains(edge_key) == false }
            guard "blocks_acyclic" {
                kind != WorkEdgeKind::Blocks || blocks_reachability.contains(reverse_path_key) == false
            }
            guard "parent_acyclic" {
                kind != WorkEdgeKind::Parent || parent_reachability.contains(reverse_path_key) == false
            }
            to Absent
            emit LinkValidated
        }

        transition CloseOpenDefaultOrCompleted {
            on input Close { expected_revision, at_utc_ms, requested_status }
            guard "revision_matches" { self.lifecycle_phase == Phase::Open && self.revision == expected_revision }
            guard "default_or_completed" { requested_status == None || requested_status == Some(WorkLifecycleState::Completed) }
            update {
                self.revision += 1;
                self.claim_owner_key = None;
                self.claimed_at_utc_ms = None;
                self.lease_expires_at_utc_ms = None;
                self.terminal_at_utc_ms = Some(at_utc_ms);
            }
            to Completed
            emit Closed { terminal_state: self.lifecycle_phase }
        }

        transition CloseInProgressDefaultOrCompleted {
            on input Close { expected_revision, at_utc_ms, requested_status }
            guard "revision_matches" { self.lifecycle_phase == Phase::InProgress && self.revision == expected_revision }
            guard "default_or_completed" { requested_status == None || requested_status == Some(WorkLifecycleState::Completed) }
            update {
                self.revision += 1;
                self.claim_owner_key = None;
                self.claimed_at_utc_ms = None;
                self.lease_expires_at_utc_ms = None;
                self.terminal_at_utc_ms = Some(at_utc_ms);
            }
            to Completed
            emit Closed { terminal_state: self.lifecycle_phase }
        }

        transition CloseBlockedDefaultOrCompleted {
            on input Close { expected_revision, at_utc_ms, requested_status }
            guard "revision_matches" { self.lifecycle_phase == Phase::Blocked && self.revision == expected_revision }
            guard "default_or_completed" { requested_status == None || requested_status == Some(WorkLifecycleState::Completed) }
            update {
                self.revision += 1;
                self.claim_owner_key = None;
                self.claimed_at_utc_ms = None;
                self.lease_expires_at_utc_ms = None;
                self.terminal_at_utc_ms = Some(at_utc_ms);
            }
            to Completed
            emit Closed { terminal_state: self.lifecycle_phase }
        }

        transition CloseOpenRequestedCancelled {
            on input Close { expected_revision, at_utc_ms, requested_status }
            guard "revision_matches" { self.lifecycle_phase == Phase::Open && self.revision == expected_revision }
            guard "requested_cancelled" { requested_status == Some(WorkLifecycleState::Cancelled) }
            update {
                self.revision += 1;
                self.claim_owner_key = None;
                self.claimed_at_utc_ms = None;
                self.lease_expires_at_utc_ms = None;
                self.terminal_at_utc_ms = Some(at_utc_ms);
            }
            to Cancelled
            emit Closed { terminal_state: self.lifecycle_phase }
        }

        transition CloseInProgressRequestedCancelled {
            on input Close { expected_revision, at_utc_ms, requested_status }
            guard "revision_matches" { self.lifecycle_phase == Phase::InProgress && self.revision == expected_revision }
            guard "requested_cancelled" { requested_status == Some(WorkLifecycleState::Cancelled) }
            update {
                self.revision += 1;
                self.claim_owner_key = None;
                self.claimed_at_utc_ms = None;
                self.lease_expires_at_utc_ms = None;
                self.terminal_at_utc_ms = Some(at_utc_ms);
            }
            to Cancelled
            emit Closed { terminal_state: self.lifecycle_phase }
        }

        transition CloseBlockedRequestedCancelled {
            on input Close { expected_revision, at_utc_ms, requested_status }
            guard "revision_matches" { self.lifecycle_phase == Phase::Blocked && self.revision == expected_revision }
            guard "requested_cancelled" { requested_status == Some(WorkLifecycleState::Cancelled) }
            update {
                self.revision += 1;
                self.claim_owner_key = None;
                self.claimed_at_utc_ms = None;
                self.lease_expires_at_utc_ms = None;
                self.terminal_at_utc_ms = Some(at_utc_ms);
            }
            to Cancelled
            emit Closed { terminal_state: self.lifecycle_phase }
        }

        transition CloseOpenRequestedFailed {
            on input Close { expected_revision, at_utc_ms, requested_status }
            guard "revision_matches" { self.lifecycle_phase == Phase::Open && self.revision == expected_revision }
            guard "requested_failed" { requested_status == Some(WorkLifecycleState::Failed) }
            update {
                self.revision += 1;
                self.claim_owner_key = None;
                self.claimed_at_utc_ms = None;
                self.lease_expires_at_utc_ms = None;
                self.terminal_at_utc_ms = Some(at_utc_ms);
            }
            to Failed
            emit Closed { terminal_state: self.lifecycle_phase }
        }

        transition CloseInProgressRequestedFailed {
            on input Close { expected_revision, at_utc_ms, requested_status }
            guard "revision_matches" { self.lifecycle_phase == Phase::InProgress && self.revision == expected_revision }
            guard "requested_failed" { requested_status == Some(WorkLifecycleState::Failed) }
            update {
                self.revision += 1;
                self.claim_owner_key = None;
                self.claimed_at_utc_ms = None;
                self.lease_expires_at_utc_ms = None;
                self.terminal_at_utc_ms = Some(at_utc_ms);
            }
            to Failed
            emit Closed { terminal_state: self.lifecycle_phase }
        }

        transition CloseBlockedRequestedFailed {
            on input Close { expected_revision, at_utc_ms, requested_status }
            guard "revision_matches" { self.lifecycle_phase == Phase::Blocked && self.revision == expected_revision }
            guard "requested_failed" { requested_status == Some(WorkLifecycleState::Failed) }
            update {
                self.revision += 1;
                self.claim_owner_key = None;
                self.claimed_at_utc_ms = None;
                self.lease_expires_at_utc_ms = None;
                self.terminal_at_utc_ms = Some(at_utc_ms);
            }
            to Failed
            emit Closed { terminal_state: self.lifecycle_phase }
        }

        transition CloseOpenCompleted {
            on input CloseCompleted { expected_revision, at_utc_ms }
            guard { self.lifecycle_phase == Phase::Open && self.revision == expected_revision }
            update {
                self.revision += 1;
                self.claim_owner_key = None;
                self.claimed_at_utc_ms = None;
                self.lease_expires_at_utc_ms = None;
                self.terminal_at_utc_ms = Some(at_utc_ms);
            }
            to Completed
            emit Closed { terminal_state: self.lifecycle_phase }
        }

        transition CloseInProgressCompleted {
            on input CloseCompleted { expected_revision, at_utc_ms }
            guard { self.lifecycle_phase == Phase::InProgress && self.revision == expected_revision }
            update {
                self.revision += 1;
                self.claim_owner_key = None;
                self.claimed_at_utc_ms = None;
                self.lease_expires_at_utc_ms = None;
                self.terminal_at_utc_ms = Some(at_utc_ms);
            }
            to Completed
            emit Closed { terminal_state: self.lifecycle_phase }
        }

        transition CloseBlockedCompleted {
            on input CloseCompleted { expected_revision, at_utc_ms }
            guard { self.lifecycle_phase == Phase::Blocked && self.revision == expected_revision }
            update {
                self.revision += 1;
                self.claim_owner_key = None;
                self.claimed_at_utc_ms = None;
                self.lease_expires_at_utc_ms = None;
                self.terminal_at_utc_ms = Some(at_utc_ms);
            }
            to Completed
            emit Closed { terminal_state: self.lifecycle_phase }
        }

        transition CloseOpenCancelled {
            on input CloseCancelled { expected_revision, at_utc_ms }
            guard { self.lifecycle_phase == Phase::Open && self.revision == expected_revision }
            update {
                self.revision += 1;
                self.claim_owner_key = None;
                self.claimed_at_utc_ms = None;
                self.lease_expires_at_utc_ms = None;
                self.terminal_at_utc_ms = Some(at_utc_ms);
            }
            to Cancelled
            emit Closed { terminal_state: self.lifecycle_phase }
        }

        transition CloseInProgressCancelled {
            on input CloseCancelled { expected_revision, at_utc_ms }
            guard { self.lifecycle_phase == Phase::InProgress && self.revision == expected_revision }
            update {
                self.revision += 1;
                self.claim_owner_key = None;
                self.claimed_at_utc_ms = None;
                self.lease_expires_at_utc_ms = None;
                self.terminal_at_utc_ms = Some(at_utc_ms);
            }
            to Cancelled
            emit Closed { terminal_state: self.lifecycle_phase }
        }

        transition CloseBlockedCancelled {
            on input CloseCancelled { expected_revision, at_utc_ms }
            guard { self.lifecycle_phase == Phase::Blocked && self.revision == expected_revision }
            update {
                self.revision += 1;
                self.claim_owner_key = None;
                self.claimed_at_utc_ms = None;
                self.lease_expires_at_utc_ms = None;
                self.terminal_at_utc_ms = Some(at_utc_ms);
            }
            to Cancelled
            emit Closed { terminal_state: self.lifecycle_phase }
        }

        transition CloseOpenFailed {
            on input CloseFailed { expected_revision, at_utc_ms }
            guard { self.lifecycle_phase == Phase::Open && self.revision == expected_revision }
            update {
                self.revision += 1;
                self.claim_owner_key = None;
                self.claimed_at_utc_ms = None;
                self.lease_expires_at_utc_ms = None;
                self.terminal_at_utc_ms = Some(at_utc_ms);
            }
            to Failed
            emit Closed { terminal_state: self.lifecycle_phase }
        }

        transition CloseInProgressFailed {
            on input CloseFailed { expected_revision, at_utc_ms }
            guard { self.lifecycle_phase == Phase::InProgress && self.revision == expected_revision }
            update {
                self.revision += 1;
                self.claim_owner_key = None;
                self.claimed_at_utc_ms = None;
                self.lease_expires_at_utc_ms = None;
                self.terminal_at_utc_ms = Some(at_utc_ms);
            }
            to Failed
            emit Closed { terminal_state: self.lifecycle_phase }
        }

        transition CloseBlockedFailed {
            on input CloseFailed { expected_revision, at_utc_ms }
            guard { self.lifecycle_phase == Phase::Blocked && self.revision == expected_revision }
            update {
                self.revision += 1;
                self.claim_owner_key = None;
                self.claimed_at_utc_ms = None;
                self.lease_expires_at_utc_ms = None;
                self.terminal_at_utc_ms = Some(at_utc_ms);
            }
            to Failed
            emit Closed { terminal_state: self.lifecycle_phase }
        }

        transition AddEvidenceOpen {
            on input AddEvidence { expected_revision, evidence_ref_tokens, evidence_ref_count }
            guard { self.lifecycle_phase == Phase::Open && self.revision == expected_revision }
            guard "evidence_refs_preserve_existing" { for_all(token in self.evidence_ref_tokens, evidence_ref_tokens.count(token) == self.evidence_ref_tokens.count(token)) }
            guard "evidence_count_advances_one" { evidence_ref_count == self.evidence_count + 1 }
            update {
                self.revision += 1;
                self.evidence_ref_tokens = evidence_ref_tokens;
                self.evidence_count = evidence_ref_count;
            }
            to Open
            emit EvidenceAdded
        }

        transition AddEvidenceInProgress {
            on input AddEvidence { expected_revision, evidence_ref_tokens, evidence_ref_count }
            guard { self.lifecycle_phase == Phase::InProgress && self.revision == expected_revision }
            guard "evidence_refs_preserve_existing" { for_all(token in self.evidence_ref_tokens, evidence_ref_tokens.count(token) == self.evidence_ref_tokens.count(token)) }
            guard "evidence_count_advances_one" { evidence_ref_count == self.evidence_count + 1 }
            update {
                self.revision += 1;
                self.evidence_ref_tokens = evidence_ref_tokens;
                self.evidence_count = evidence_ref_count;
            }
            to InProgress
            emit EvidenceAdded
        }

        transition AddEvidenceBlocked {
            on input AddEvidence { expected_revision, evidence_ref_tokens, evidence_ref_count }
            guard { self.lifecycle_phase == Phase::Blocked && self.revision == expected_revision }
            guard "evidence_refs_preserve_existing" { for_all(token in self.evidence_ref_tokens, evidence_ref_tokens.count(token) == self.evidence_ref_tokens.count(token)) }
            guard "evidence_count_advances_one" { evidence_ref_count == self.evidence_count + 1 }
            update {
                self.revision += 1;
                self.evidence_ref_tokens = evidence_ref_tokens;
                self.evidence_count = evidence_ref_count;
            }
            to Blocked
            emit EvidenceAdded
        }

        transition AddEvidenceCompleted {
            on input AddEvidence { expected_revision, evidence_ref_tokens, evidence_ref_count }
            guard { self.lifecycle_phase == Phase::Completed && self.revision == expected_revision }
            guard "evidence_refs_preserve_existing" { for_all(token in self.evidence_ref_tokens, evidence_ref_tokens.count(token) == self.evidence_ref_tokens.count(token)) }
            guard "evidence_count_advances_one" { evidence_ref_count == self.evidence_count + 1 }
            update {
                self.revision += 1;
                self.evidence_ref_tokens = evidence_ref_tokens;
                self.evidence_count = evidence_ref_count;
            }
            to Completed
            emit EvidenceAdded
        }

        transition AddEvidenceCancelled {
            on input AddEvidence { expected_revision, evidence_ref_tokens, evidence_ref_count }
            guard { self.lifecycle_phase == Phase::Cancelled && self.revision == expected_revision }
            guard "evidence_refs_preserve_existing" { for_all(token in self.evidence_ref_tokens, evidence_ref_tokens.count(token) == self.evidence_ref_tokens.count(token)) }
            guard "evidence_count_advances_one" { evidence_ref_count == self.evidence_count + 1 }
            update {
                self.revision += 1;
                self.evidence_ref_tokens = evidence_ref_tokens;
                self.evidence_count = evidence_ref_count;
            }
            to Cancelled
            emit EvidenceAdded
        }

        transition AddEvidenceFailed {
            on input AddEvidence { expected_revision, evidence_ref_tokens, evidence_ref_count }
            guard { self.lifecycle_phase == Phase::Failed && self.revision == expected_revision }
            guard "evidence_refs_preserve_existing" { for_all(token in self.evidence_ref_tokens, evidence_ref_tokens.count(token) == self.evidence_ref_tokens.count(token)) }
            guard "evidence_count_advances_one" { evidence_ref_count == self.evidence_count + 1 }
            update {
                self.revision += 1;
                self.evidence_ref_tokens = evidence_ref_tokens;
                self.evidence_count = evidence_ref_count;
            }
            to Failed
            emit EvidenceAdded
        }

        helper workgraph_item_key_present(item_key: WorkItemKey) -> bool {
            item_key.len() > 0
        }

        // --- Public error class ---
        //
        // WorkGraph owns the semantic public result class for WorkGraph
        // domain errors. Surfaces translate this generated class into
        // transport status/tool codes without reclassifying the error.

        transition ClassifyPublicErrorNotFound {
            on input ClassifyPublicError { error_kind }
            guard { self.lifecycle_phase == Phase::Absent && error_kind == WorkGraphErrorKind::NotFound }
            to Absent
            emit PublicErrorClassified { public_class: WorkGraphPublicErrorClass::NotFound }
        }

        transition ClassifyPublicErrorStaleRevision {
            on input ClassifyPublicError { error_kind }
            guard { self.lifecycle_phase == Phase::Absent && error_kind == WorkGraphErrorKind::StaleRevision }
            to Absent
            emit PublicErrorClassified { public_class: WorkGraphPublicErrorClass::Conflict }
        }

        transition ClassifyPublicErrorConflict {
            on input ClassifyPublicError { error_kind }
            guard { self.lifecycle_phase == Phase::Absent && error_kind == WorkGraphErrorKind::Conflict }
            to Absent
            emit PublicErrorClassified { public_class: WorkGraphPublicErrorClass::Conflict }
        }

        transition ClassifyPublicErrorInvalidTransition {
            on input ClassifyPublicError { error_kind }
            guard { self.lifecycle_phase == Phase::Absent && error_kind == WorkGraphErrorKind::InvalidTransition }
            to Absent
            emit PublicErrorClassified { public_class: WorkGraphPublicErrorClass::InvalidTransition }
        }

        transition ClassifyPublicErrorInvalidInput {
            on input ClassifyPublicError { error_kind }
            guard { self.lifecycle_phase == Phase::Absent && error_kind == WorkGraphErrorKind::InvalidInput }
            to Absent
            emit PublicErrorClassified { public_class: WorkGraphPublicErrorClass::InvalidArguments }
        }

        transition ClassifyPublicErrorInvalidTimestampMillis {
            on input ClassifyPublicError { error_kind }
            guard { self.lifecycle_phase == Phase::Absent && error_kind == WorkGraphErrorKind::InvalidTimestampMillis }
            to Absent
            emit PublicErrorClassified { public_class: WorkGraphPublicErrorClass::InvalidArguments }
        }

        transition ClassifyPublicErrorUnsupportedBackend {
            on input ClassifyPublicError { error_kind }
            guard { self.lifecycle_phase == Phase::Absent && error_kind == WorkGraphErrorKind::UnsupportedBackend }
            to Absent
            emit PublicErrorClassified { public_class: WorkGraphPublicErrorClass::CapabilityUnavailable }
        }

        transition ClassifyPublicErrorStore {
            on input ClassifyPublicError { error_kind }
            guard { self.lifecycle_phase == Phase::Absent && error_kind == WorkGraphErrorKind::Store }
            to Absent
            emit PublicErrorClassified { public_class: WorkGraphPublicErrorClass::StoreError }
        }
    }
}

impl serde::Serialize for WorkLifecycleState {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(match self {
            Self::Absent => "absent",
            Self::Open => "open",
            Self::InProgress => "in_progress",
            Self::Blocked => "blocked",
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
            Self::Failed => "failed",
        })
    }
}

impl<'de> serde::Deserialize<'de> for WorkLifecycleState {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = <String as serde::Deserialize>::deserialize(deserializer)?;
        match value.as_str() {
            "absent" | "Absent" => Ok(Self::Absent),
            "open" | "Open" => Ok(Self::Open),
            "in_progress" | "InProgress" => Ok(Self::InProgress),
            "blocked" | "Blocked" => Ok(Self::Blocked),
            "completed" | "Completed" => Ok(Self::Completed),
            "cancelled" | "Cancelled" => Ok(Self::Cancelled),
            "failed" | "Failed" => Ok(Self::Failed),
            other => Err(serde::de::Error::custom(format!(
                "invalid WorkLifecycleState `{other}`"
            ))),
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkGraphLifecycleMachineStateWire {
    lifecycle_phase: WorkLifecycleState,
    revision: u64,
    #[serde(default)]
    item_key: Option<WorkItemKey>,
    #[serde(default)]
    external_ref_tokens: Vec<String>,
    #[serde(default)]
    evidence_ref_tokens: Vec<String>,
    unresolved_blocker_count: u64,
    claim_owner_key: Option<WorkOwnerKey>,
    claimed_at_utc_ms: Option<u64>,
    lease_expires_at_utc_ms: Option<u64>,
    due_at_utc_ms: Option<u64>,
    not_before_utc_ms: Option<u64>,
    snoozed_until_utc_ms: Option<u64>,
    terminal_at_utc_ms: Option<u64>,
    evidence_count: u64,
}

impl From<&WorkGraphLifecycleMachineState> for WorkGraphLifecycleMachineStateWire {
    fn from(state: &WorkGraphLifecycleMachineState) -> Self {
        Self {
            lifecycle_phase: state.lifecycle_phase,
            revision: state.revision,
            item_key: state.item_key.clone(),
            external_ref_tokens: state.external_ref_tokens.clone(),
            evidence_ref_tokens: state.evidence_ref_tokens.clone(),
            unresolved_blocker_count: state.unresolved_blocker_count,
            claim_owner_key: state.claim_owner_key.clone(),
            claimed_at_utc_ms: state.claimed_at_utc_ms,
            lease_expires_at_utc_ms: state.lease_expires_at_utc_ms,
            due_at_utc_ms: state.due_at_utc_ms,
            not_before_utc_ms: state.not_before_utc_ms,
            snoozed_until_utc_ms: state.snoozed_until_utc_ms,
            terminal_at_utc_ms: state.terminal_at_utc_ms,
            evidence_count: state.evidence_count,
        }
    }
}

impl From<WorkGraphLifecycleMachineStateWire> for WorkGraphLifecycleMachineState {
    fn from(wire: WorkGraphLifecycleMachineStateWire) -> Self {
        Self {
            lifecycle_phase: wire.lifecycle_phase,
            revision: wire.revision,
            item_key: wire.item_key,
            external_ref_tokens: wire.external_ref_tokens,
            evidence_ref_tokens: wire.evidence_ref_tokens,
            unresolved_blocker_count: wire.unresolved_blocker_count,
            claim_owner_key: wire.claim_owner_key,
            claimed_at_utc_ms: wire.claimed_at_utc_ms,
            lease_expires_at_utc_ms: wire.lease_expires_at_utc_ms,
            due_at_utc_ms: wire.due_at_utc_ms,
            not_before_utc_ms: wire.not_before_utc_ms,
            snoozed_until_utc_ms: wire.snoozed_until_utc_ms,
            terminal_at_utc_ms: wire.terminal_at_utc_ms,
            evidence_count: wire.evidence_count,
        }
    }
}

impl serde::Serialize for WorkGraphLifecycleMachineState {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        WorkGraphLifecycleMachineStateWire::from(self).serialize(serializer)
    }
}

impl<'de> serde::Deserialize<'de> for WorkGraphLifecycleMachineState {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        WorkGraphLifecycleMachineStateWire::deserialize(deserializer).map(Self::from)
    }
}
