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
pub struct WorkOwnerKey(pub String);

impl<T: Into<String>> From<T> for WorkOwnerKey {
    fn from(value: T) -> Self {
        Self(value.into())
    }
}

machine! {
    machine WorkGraphLifecycleMachine {
        version: 1,
        rust: "self" / "catalog::dsl::workgraph_lifecycle",

        state {
            lifecycle_phase: WorkLifecycleState,
            revision: u64,
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
            CreateOpen {
                due_at_utc_ms: Option<u64>,
                not_before_utc_ms: Option<u64>,
                snoozed_until_utc_ms: Option<u64>,
                unresolved_blocker_count: u64,
            },
            CreateBlocked {
                due_at_utc_ms: Option<u64>,
                not_before_utc_ms: Option<u64>,
                snoozed_until_utc_ms: Option<u64>,
                unresolved_blocker_count: u64,
            },
            Update {
                expected_revision: u64,
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
            ValidateLink {
                endpoints_exist: bool,
                self_edge: bool,
                duplicate_edge: bool,
                would_create_cycle: bool,
            },
            CloseCompleted { expected_revision: u64, at_utc_ms: u64 },
            CloseCancelled { expected_revision: u64, at_utc_ms: u64 },
            CloseFailed { expected_revision: u64, at_utc_ms: u64 },
            AddEvidence { expected_revision: u64 },
        }

        effect WorkGraphLifecycleEffect {
            Created,
            Updated,
            Claimed { owner_key: WorkOwnerKey },
            Released,
            Blocked,
            LinkValidated,
            Closed { terminal_state: WorkLifecycleState },
            EvidenceAdded,
        }

        invariant absent_has_zero_revision {
            self.lifecycle_phase != Phase::Absent || self.revision == 0
        }

        invariant live_has_positive_revision {
            self.lifecycle_phase == Phase::Absent || self.revision > 0
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
        disposition LinkValidated => local,
        disposition Closed => local,
        disposition EvidenceAdded => local,

        transition CreateOpen {
            on input CreateOpen { due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, unresolved_blocker_count }
            guard { self.lifecycle_phase == Phase::Absent }
            update {
                self.revision = 1;
                self.unresolved_blocker_count = unresolved_blocker_count;
                self.due_at_utc_ms = due_at_utc_ms;
                self.not_before_utc_ms = not_before_utc_ms;
                self.snoozed_until_utc_ms = snoozed_until_utc_ms;
            }
            to Open
            emit Created
        }

        transition CreateBlocked {
            on input CreateBlocked { due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, unresolved_blocker_count }
            guard { self.lifecycle_phase == Phase::Absent }
            update {
                self.revision = 1;
                self.unresolved_blocker_count = unresolved_blocker_count;
                self.due_at_utc_ms = due_at_utc_ms;
                self.not_before_utc_ms = not_before_utc_ms;
                self.snoozed_until_utc_ms = snoozed_until_utc_ms;
            }
            to Blocked
            emit Created
        }

        transition UpdateOpen {
            on input Update { expected_revision, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, unresolved_blocker_count }
            guard { self.lifecycle_phase == Phase::Open && self.revision == expected_revision }
            update {
                self.revision += 1;
                self.unresolved_blocker_count = unresolved_blocker_count;
                self.due_at_utc_ms = due_at_utc_ms;
                self.not_before_utc_ms = not_before_utc_ms;
                self.snoozed_until_utc_ms = snoozed_until_utc_ms;
            }
            to Open
            emit Updated
        }

        transition UpdateInProgress {
            on input Update { expected_revision, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, unresolved_blocker_count }
            guard { self.lifecycle_phase == Phase::InProgress && self.revision == expected_revision }
            update {
                self.revision += 1;
                self.unresolved_blocker_count = unresolved_blocker_count;
                self.due_at_utc_ms = due_at_utc_ms;
                self.not_before_utc_ms = not_before_utc_ms;
                self.snoozed_until_utc_ms = snoozed_until_utc_ms;
            }
            to InProgress
            emit Updated
        }

        transition UpdateBlocked {
            on input Update { expected_revision, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, unresolved_blocker_count }
            guard { self.lifecycle_phase == Phase::Blocked && self.revision == expected_revision }
            update {
                self.revision += 1;
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
        }

        transition RefreshEligibilityInProgress {
            on input RefreshEligibility { unresolved_blocker_count }
            guard { self.lifecycle_phase == Phase::InProgress }
            update {
                self.unresolved_blocker_count = unresolved_blocker_count;
            }
            to InProgress
        }

        transition RefreshEligibilityBlocked {
            on input RefreshEligibility { unresolved_blocker_count }
            guard { self.lifecycle_phase == Phase::Blocked }
            update {
                self.unresolved_blocker_count = unresolved_blocker_count;
            }
            to Blocked
        }

        transition ValidateLink {
            on input ValidateLink { endpoints_exist, self_edge, duplicate_edge, would_create_cycle }
            guard "stateless_checker" { self.lifecycle_phase == Phase::Absent }
            guard "endpoints_exist" { endpoints_exist == true }
            guard "not_self_edge" { self_edge == false }
            guard "not_duplicate_edge" { duplicate_edge == false }
            guard "acyclic" { would_create_cycle == false }
            to Absent
            emit LinkValidated
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
            on input AddEvidence { expected_revision }
            guard { self.lifecycle_phase == Phase::Open && self.revision == expected_revision }
            update {
                self.revision += 1;
                self.evidence_count += 1;
            }
            to Open
            emit EvidenceAdded
        }

        transition AddEvidenceInProgress {
            on input AddEvidence { expected_revision }
            guard { self.lifecycle_phase == Phase::InProgress && self.revision == expected_revision }
            update {
                self.revision += 1;
                self.evidence_count += 1;
            }
            to InProgress
            emit EvidenceAdded
        }

        transition AddEvidenceBlocked {
            on input AddEvidence { expected_revision }
            guard { self.lifecycle_phase == Phase::Blocked && self.revision == expected_revision }
            update {
                self.revision += 1;
                self.evidence_count += 1;
            }
            to Blocked
            emit EvidenceAdded
        }

        transition AddEvidenceCompleted {
            on input AddEvidence { expected_revision }
            guard { self.lifecycle_phase == Phase::Completed && self.revision == expected_revision }
            update {
                self.revision += 1;
                self.evidence_count += 1;
            }
            to Completed
            emit EvidenceAdded
        }

        transition AddEvidenceCancelled {
            on input AddEvidence { expected_revision }
            guard { self.lifecycle_phase == Phase::Cancelled && self.revision == expected_revision }
            update {
                self.revision += 1;
                self.evidence_count += 1;
            }
            to Cancelled
            emit EvidenceAdded
        }

        transition AddEvidenceFailed {
            on input AddEvidence { expected_revision }
            guard { self.lifecycle_phase == Phase::Failed && self.revision == expected_revision }
            update {
                self.revision += 1;
                self.evidence_count += 1;
            }
            to Failed
            emit EvidenceAdded
        }
    }
}
