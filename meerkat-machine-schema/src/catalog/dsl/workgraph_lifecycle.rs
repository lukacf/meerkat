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
pub struct WorkEdgeKey(pub String);

impl<T: Into<String>> From<T> for WorkEdgeKey {
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
pub struct WorkDependencyPathKey(pub String);

impl<T: Into<String>> From<T> for WorkDependencyPathKey {
    fn from(value: T) -> Self {
        Self(value.into())
    }
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

machine! {
    machine WorkGraphLifecycleMachine {
        version: 1,
        rust: "self" / "catalog::dsl::workgraph_lifecycle",

        state {
            lifecycle_phase: WorkLifecycleState,
            revision: u64,
            unresolved_blocker_count: u64,
            topology_item_keys: Set<WorkItemKey>,
            topology_edge_keys: Set<WorkEdgeKey>,
            blocks_reachability: Set<WorkDependencyPathKey>,
            parent_reachability: Set<WorkDependencyPathKey>,
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
            topology_item_keys = EmptySet,
            topology_edge_keys = EmptySet,
            blocks_reachability = EmptySet,
            parent_reachability = EmptySet,
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
                kind: WorkEdgeKind,
                from_item_key: WorkItemKey,
                to_item_key: WorkItemKey,
                edge_key: WorkEdgeKey,
                reverse_path_key: WorkDependencyPathKey,
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

        invariant topology_snapshot_is_stateless {
            self.topology_item_keys == EmptySet
                || self.topology_edge_keys == EmptySet
                || self.lifecycle_phase == Phase::Absent
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
            on input ValidateLink { kind, from_item_key, to_item_key, edge_key, reverse_path_key }
            guard "stateless_checker" { self.lifecycle_phase == Phase::Absent }
            guard "from_endpoint_exists" { self.topology_item_keys.contains(from_item_key) }
            guard "to_endpoint_exists" { self.topology_item_keys.contains(to_item_key) }
            guard "not_self_edge" { from_item_key != to_item_key }
            guard "not_duplicate_edge" { self.topology_edge_keys.contains(edge_key) == false }
            guard "blocks_acyclic" {
                kind != WorkEdgeKind::Blocks || self.blocks_reachability.contains(reverse_path_key) == false
            }
            guard "parent_acyclic" {
                kind != WorkEdgeKind::Parent || self.parent_reachability.contains(reverse_path_key) == false
            }
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
struct WorkGraphLifecycleMachineStateWire {
    lifecycle_phase: WorkLifecycleState,
    revision: u64,
    unresolved_blocker_count: u64,
    #[serde(default)]
    topology_item_keys: std::collections::BTreeSet<WorkItemKey>,
    #[serde(default)]
    topology_edge_keys: std::collections::BTreeSet<WorkEdgeKey>,
    #[serde(default)]
    blocks_reachability: std::collections::BTreeSet<WorkDependencyPathKey>,
    #[serde(default)]
    parent_reachability: std::collections::BTreeSet<WorkDependencyPathKey>,
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
            unresolved_blocker_count: state.unresolved_blocker_count,
            topology_item_keys: state.topology_item_keys.clone(),
            topology_edge_keys: state.topology_edge_keys.clone(),
            blocks_reachability: state.blocks_reachability.clone(),
            parent_reachability: state.parent_reachability.clone(),
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
            unresolved_blocker_count: wire.unresolved_blocker_count,
            topology_item_keys: wire.topology_item_keys,
            topology_edge_keys: wire.topology_edge_keys,
            blocks_reachability: wire.blocks_reachability,
            parent_reachability: wire.parent_reachability,
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
