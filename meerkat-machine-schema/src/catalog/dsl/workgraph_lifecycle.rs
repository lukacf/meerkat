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
pub enum WorkCompletionPolicy {
    #[default]
    SelfAttest,
    HostConfirmed,
    PrincipalConfirmed,
    Supervisor,
    ReviewerQuorum,
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
pub enum WorkEvidenceKind {
    #[default]
    SelfAttest,
    HostConfirmation,
    PrincipalConfirmation,
    SupervisorConfirmation,
    ReviewerConfirmation,
}

/// Typed discriminant the shell extracts from a `WorkGraphError` and feeds back
/// into the machine. This is a pure typed extraction (one variant per
/// `WorkGraphError` variant); the shell performs NO grouping — the
/// variant->public-class POLICY lives in the `ClassifyWorkGraphPublicError`
/// transitions below, owned by this canonical machine.
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
    AttentionNotFound,
    StaleRevision,
    Conflict,
    InvalidTransition,
    InvalidInput,
    InvalidTimestampMillis,
    Store,
    UnsupportedBackend,
}

/// Machine-owned public error classification surfaced to REST/RPC callers. The
/// machine is the sole authority for the many-to-one grouping of internal error
/// kinds into this public vocabulary (see the `ClassifyWorkGraphPublicError`
/// transitions). Shells mirror the emitted class; they do not decide it.
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
            completion_policy: Enum<WorkCompletionPolicy>,
            completion_supervisor_owner_key: Option<WorkOwnerKey>,
            completion_reviewer_quorum_threshold: Option<u64>,
            terminal_at_utc_ms: Option<u64>,
            evidence_count: u64,
            // Machine-owned, per-kind confirmation accounting. The producer
            // classifies each piece of confirmation evidence into a typed
            // WorkEvidenceKind; the machine records it here so the completion
            // policy satisfaction decision is computed from owned state, not a
            // shell reducer scanning string `evidence.kind` values.
            host_confirmation_count: u64,
            principal_confirmation_count: u64,
            supervisor_confirmation_owner_keys: Set<WorkOwnerKey>,
            reviewer_confirmation_owner_keys: Set<WorkOwnerKey>,
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
            completion_policy = WorkCompletionPolicy::SelfAttest,
            completion_supervisor_owner_key = None,
            completion_reviewer_quorum_threshold = None,
            terminal_at_utc_ms = None,
            evidence_count = 0,
            host_confirmation_count = 0,
            principal_confirmation_count = 0,
            supervisor_confirmation_owner_keys = EmptySet,
            reviewer_confirmation_owner_keys = EmptySet,
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
                completion_policy: Enum<WorkCompletionPolicy>,
                completion_supervisor_owner_key: Option<WorkOwnerKey>,
                completion_reviewer_quorum_threshold: Option<u64>,
                unresolved_blocker_count: u64,
            },
            CreateBlocked {
                due_at_utc_ms: Option<u64>,
                not_before_utc_ms: Option<u64>,
                snoozed_until_utc_ms: Option<u64>,
                completion_policy: Enum<WorkCompletionPolicy>,
                completion_supervisor_owner_key: Option<WorkOwnerKey>,
                completion_reviewer_quorum_threshold: Option<u64>,
                unresolved_blocker_count: u64,
            },
            Update {
                expected_revision: u64,
                due_at_utc_ms: Option<u64>,
                not_before_utc_ms: Option<u64>,
                snoozed_until_utc_ms: Option<u64>,
                completion_policy: Enum<WorkCompletionPolicy>,
                completion_supervisor_owner_key: Option<WorkOwnerKey>,
                completion_reviewer_quorum_threshold: Option<u64>,
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
            AddEvidence {
                expected_revision: u64,
                evidence_kind: Enum<WorkEvidenceKind>,
                confirming_owner_key: Option<WorkOwnerKey>,
            },
            // Public error-class classification. The shell extracts a typed
            // WorkGraphErrorKind from a WorkGraphError (pure typed extraction)
            // and feeds it back here; this machine owns the variant->class
            // POLICY and emits WorkGraphPublicErrorClassified.
            ClassifyWorkGraphPublicError { kind: Enum<WorkGraphErrorKind> },
            // Terminality classification. This machine owns the lifecycle_phase;
            // the terminality verdict (which phases are terminal) is a machine
            // fact. The shell extracts no fact — it drives this input over the
            // recovered machine state and mirrors the emitted
            // WorkItemTerminalityClassified.terminal, failing closed.
            ClassifyTerminality {},
            // Per-blocking-edge satisfaction classification. The shell extracts
            // only the raw blocker lifecycle phase (a pure observation projected
            // from the blocker item's own machine state) plus whether the blocker
            // was resolvable at all; this machine owns the satisfaction POLICY
            // (a blocking edge is satisfied iff its blocker reached terminal
            // SUCCESS, i.e. Completed) and emits BlockerSatisfactionClassified.
            // The shell mirrors the verdict and mechanically fans-in (counts)
            // the unsatisfied blockers — the count it feeds to RefreshEligibility
            // / Claim, which this machine revalidates via dependencies_satisfied.
            ClassifyBlockerSatisfied {
                blocker_present: bool,
                blocker_lifecycle_phase: Enum<WorkLifecycleState>,
            },
        }

        effect WorkGraphLifecycleEffect {
            Created,
            Updated,
            Claimed { owner_key: WorkOwnerKey },
            Released,
            Blocked,
            LinkValidated,
            Closed { terminal_state: WorkLifecycleState, at_utc_ms: u64 },
            EvidenceAdded,
            WorkGraphPublicErrorClassified {
                kind: Enum<WorkGraphErrorKind>,
                public_class: Enum<WorkGraphPublicErrorClass>,
            },
            WorkItemTerminalityClassified { terminal: bool },
            BlockerSatisfactionClassified { satisfied: bool },
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

        helper completion_policy_payload_valid(
            policy: WorkCompletionPolicy,
            supervisor_owner_key: Option<WorkOwnerKey>,
            reviewer_quorum_threshold: Option<u64>
        ) -> bool {
            if policy == WorkCompletionPolicy::Supervisor {
                supervisor_owner_key != None && reviewer_quorum_threshold == None
            } else {
                if policy == WorkCompletionPolicy::ReviewerQuorum {
                    supervisor_owner_key == None
                        && reviewer_quorum_threshold != None
                        && reviewer_quorum_threshold.get("value") > 0
                } else {
                    supervisor_owner_key == None && reviewer_quorum_threshold == None
                }
            }
        }

        helper completion_policy_is_satisfied(
            policy: WorkCompletionPolicy,
            supervisor_owner_key: Option<WorkOwnerKey>,
            reviewer_quorum_threshold: Option<u64>,
            host_confirmation_count: u64,
            principal_confirmation_count: u64,
            supervisor_confirmation_owner_keys: Set<WorkOwnerKey>,
            reviewer_confirmation_owner_keys: Set<WorkOwnerKey>
        ) -> bool {
            if policy == WorkCompletionPolicy::SelfAttest {
                true
            } else {
                if policy == WorkCompletionPolicy::HostConfirmed {
                    host_confirmation_count > 0
                } else {
                    if policy == WorkCompletionPolicy::PrincipalConfirmed {
                        principal_confirmation_count > 0
                    } else {
                        if policy == WorkCompletionPolicy::Supervisor {
                            supervisor_owner_key != None
                                && supervisor_confirmation_owner_keys.contains(supervisor_owner_key.get("value"))
                        } else {
                            reviewer_quorum_threshold != None
                                && reviewer_confirmation_owner_keys.len() >= reviewer_quorum_threshold.get("value")
                        }
                    }
                }
            }
        }

        helper evidence_kind_owner_key_present(
            evidence_kind: WorkEvidenceKind,
            confirming_owner_key: Option<WorkOwnerKey>
        ) -> bool {
            if evidence_kind == WorkEvidenceKind::SupervisorConfirmation {
                confirming_owner_key != None
            } else {
                if evidence_kind == WorkEvidenceKind::ReviewerConfirmation {
                    confirming_owner_key != None
                } else {
                    true
                }
            }
        }

        invariant supervisor_policy_has_owner {
            self.completion_policy != WorkCompletionPolicy::Supervisor
                || self.completion_supervisor_owner_key != None
        }

        invariant non_supervisor_policy_has_no_owner {
            self.completion_policy == WorkCompletionPolicy::Supervisor
                || self.completion_supervisor_owner_key == None
        }

        invariant reviewer_quorum_policy_has_positive_threshold {
            self.completion_policy != WorkCompletionPolicy::ReviewerQuorum
                || (self.completion_reviewer_quorum_threshold != None
                    && self.completion_reviewer_quorum_threshold.get("value") > 0)
        }

        invariant non_reviewer_quorum_policy_has_no_threshold {
            self.completion_policy == WorkCompletionPolicy::ReviewerQuorum
                || self.completion_reviewer_quorum_threshold == None
        }

        disposition Created => local,
        disposition Updated => local,
        disposition Claimed => local,
        disposition Released => local,
        disposition Blocked => local,
        disposition LinkValidated => local,
        disposition Closed => routed [WorkAttentionLifecycleMachine],
        disposition EvidenceAdded => local,
        disposition WorkGraphPublicErrorClassified => local,
        disposition WorkItemTerminalityClassified => local,
        disposition BlockerSatisfactionClassified => local,

        transition CreateOpen {
            on input CreateOpen { due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, unresolved_blocker_count }
            guard { self.lifecycle_phase == Phase::Absent }
            guard "completion_policy_payload_valid" {
                completion_policy_payload_valid(completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold)
            }
            update {
                self.revision = 1;
                self.unresolved_blocker_count = unresolved_blocker_count;
                self.due_at_utc_ms = due_at_utc_ms;
                self.not_before_utc_ms = not_before_utc_ms;
                self.snoozed_until_utc_ms = snoozed_until_utc_ms;
                self.completion_policy = completion_policy;
                self.completion_supervisor_owner_key = completion_supervisor_owner_key;
                self.completion_reviewer_quorum_threshold = completion_reviewer_quorum_threshold;
            }
            to Open
            emit Created
        }

        transition CreateBlocked {
            on input CreateBlocked { due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, unresolved_blocker_count }
            guard { self.lifecycle_phase == Phase::Absent }
            guard "completion_policy_payload_valid" {
                completion_policy_payload_valid(completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold)
            }
            update {
                self.revision = 1;
                self.unresolved_blocker_count = unresolved_blocker_count;
                self.due_at_utc_ms = due_at_utc_ms;
                self.not_before_utc_ms = not_before_utc_ms;
                self.snoozed_until_utc_ms = snoozed_until_utc_ms;
                self.completion_policy = completion_policy;
                self.completion_supervisor_owner_key = completion_supervisor_owner_key;
                self.completion_reviewer_quorum_threshold = completion_reviewer_quorum_threshold;
            }
            to Blocked
            emit Created
        }

        transition UpdateOpen {
            on input Update { expected_revision, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, unresolved_blocker_count }
            guard { self.lifecycle_phase == Phase::Open && self.revision == expected_revision }
            guard "completion_policy_payload_valid" {
                completion_policy_payload_valid(completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold)
            }
            update {
                self.revision += 1;
                self.unresolved_blocker_count = unresolved_blocker_count;
                self.due_at_utc_ms = due_at_utc_ms;
                self.not_before_utc_ms = not_before_utc_ms;
                self.snoozed_until_utc_ms = snoozed_until_utc_ms;
                self.completion_policy = completion_policy;
                self.completion_supervisor_owner_key = completion_supervisor_owner_key;
                self.completion_reviewer_quorum_threshold = completion_reviewer_quorum_threshold;
            }
            to Open
            emit Updated
        }

        transition UpdateInProgress {
            on input Update { expected_revision, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, unresolved_blocker_count }
            guard { self.lifecycle_phase == Phase::InProgress && self.revision == expected_revision }
            guard "completion_policy_payload_valid" {
                completion_policy_payload_valid(completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold)
            }
            update {
                self.revision += 1;
                self.unresolved_blocker_count = unresolved_blocker_count;
                self.due_at_utc_ms = due_at_utc_ms;
                self.not_before_utc_ms = not_before_utc_ms;
                self.snoozed_until_utc_ms = snoozed_until_utc_ms;
                self.completion_policy = completion_policy;
                self.completion_supervisor_owner_key = completion_supervisor_owner_key;
                self.completion_reviewer_quorum_threshold = completion_reviewer_quorum_threshold;
            }
            to InProgress
            emit Updated
        }

        transition UpdateBlocked {
            on input Update { expected_revision, due_at_utc_ms, not_before_utc_ms, snoozed_until_utc_ms, completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold, unresolved_blocker_count }
            guard { self.lifecycle_phase == Phase::Blocked && self.revision == expected_revision }
            guard "completion_policy_payload_valid" {
                completion_policy_payload_valid(completion_policy, completion_supervisor_owner_key, completion_reviewer_quorum_threshold)
            }
            update {
                self.revision += 1;
                self.unresolved_blocker_count = unresolved_blocker_count;
                self.due_at_utc_ms = due_at_utc_ms;
                self.not_before_utc_ms = not_before_utc_ms;
                self.snoozed_until_utc_ms = snoozed_until_utc_ms;
                self.completion_policy = completion_policy;
                self.completion_supervisor_owner_key = completion_supervisor_owner_key;
                self.completion_reviewer_quorum_threshold = completion_reviewer_quorum_threshold;
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
                self.revision += 1;
                self.unresolved_blocker_count = unresolved_blocker_count;
            }
            to Open
        }

        transition RefreshEligibilityInProgress {
            on input RefreshEligibility { unresolved_blocker_count }
            guard { self.lifecycle_phase == Phase::InProgress }
            update {
                self.revision += 1;
                self.unresolved_blocker_count = unresolved_blocker_count;
            }
            to InProgress
        }

        transition RefreshEligibilityBlocked {
            on input RefreshEligibility { unresolved_blocker_count }
            guard { self.lifecycle_phase == Phase::Blocked }
            update {
                self.revision += 1;
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
            guard "completion_policy_satisfied" {
                completion_policy_is_satisfied(
                    self.completion_policy,
                    self.completion_supervisor_owner_key,
                    self.completion_reviewer_quorum_threshold,
                    self.host_confirmation_count,
                    self.principal_confirmation_count,
                    self.supervisor_confirmation_owner_keys,
                    self.reviewer_confirmation_owner_keys
                )
            }
            update {
                self.revision += 1;
                self.claim_owner_key = None;
                self.claimed_at_utc_ms = None;
                self.lease_expires_at_utc_ms = None;
                self.terminal_at_utc_ms = Some(at_utc_ms);
            }
            to Completed
            emit Closed { terminal_state: WorkLifecycleState::Completed, at_utc_ms: at_utc_ms }
        }

        transition CloseInProgressCompleted {
            on input CloseCompleted { expected_revision, at_utc_ms }
            guard { self.lifecycle_phase == Phase::InProgress && self.revision == expected_revision }
            guard "completion_policy_satisfied" {
                completion_policy_is_satisfied(
                    self.completion_policy,
                    self.completion_supervisor_owner_key,
                    self.completion_reviewer_quorum_threshold,
                    self.host_confirmation_count,
                    self.principal_confirmation_count,
                    self.supervisor_confirmation_owner_keys,
                    self.reviewer_confirmation_owner_keys
                )
            }
            update {
                self.revision += 1;
                self.claim_owner_key = None;
                self.claimed_at_utc_ms = None;
                self.lease_expires_at_utc_ms = None;
                self.terminal_at_utc_ms = Some(at_utc_ms);
            }
            to Completed
            emit Closed { terminal_state: WorkLifecycleState::Completed, at_utc_ms: at_utc_ms }
        }

        transition CloseBlockedCompleted {
            on input CloseCompleted { expected_revision, at_utc_ms }
            guard { self.lifecycle_phase == Phase::Blocked && self.revision == expected_revision }
            guard "completion_policy_satisfied" {
                completion_policy_is_satisfied(
                    self.completion_policy,
                    self.completion_supervisor_owner_key,
                    self.completion_reviewer_quorum_threshold,
                    self.host_confirmation_count,
                    self.principal_confirmation_count,
                    self.supervisor_confirmation_owner_keys,
                    self.reviewer_confirmation_owner_keys
                )
            }
            update {
                self.revision += 1;
                self.claim_owner_key = None;
                self.claimed_at_utc_ms = None;
                self.lease_expires_at_utc_ms = None;
                self.terminal_at_utc_ms = Some(at_utc_ms);
            }
            to Completed
            emit Closed { terminal_state: WorkLifecycleState::Completed, at_utc_ms: at_utc_ms }
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
            emit Closed { terminal_state: WorkLifecycleState::Cancelled, at_utc_ms: at_utc_ms }
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
            emit Closed { terminal_state: WorkLifecycleState::Cancelled, at_utc_ms: at_utc_ms }
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
            emit Closed { terminal_state: WorkLifecycleState::Cancelled, at_utc_ms: at_utc_ms }
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
            emit Closed { terminal_state: WorkLifecycleState::Failed, at_utc_ms: at_utc_ms }
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
            emit Closed { terminal_state: WorkLifecycleState::Failed, at_utc_ms: at_utc_ms }
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
            emit Closed { terminal_state: WorkLifecycleState::Failed, at_utc_ms: at_utc_ms }
        }

        transition AddEvidenceOpen {
            on input AddEvidence { expected_revision, evidence_kind, confirming_owner_key }
            guard { self.lifecycle_phase == Phase::Open && self.revision == expected_revision }
            guard "owner_key_present_for_kind" {
                evidence_kind_owner_key_present(evidence_kind, confirming_owner_key)
            }
            update {
                self.revision += 1;
                self.evidence_count += 1;
                if evidence_kind == WorkEvidenceKind::HostConfirmation {
                    self.host_confirmation_count += 1;
                }
                if evidence_kind == WorkEvidenceKind::PrincipalConfirmation {
                    self.principal_confirmation_count += 1;
                }
                if evidence_kind == WorkEvidenceKind::SupervisorConfirmation {
                    self.supervisor_confirmation_owner_keys.insert(confirming_owner_key.get("value"));
                }
                if evidence_kind == WorkEvidenceKind::ReviewerConfirmation {
                    self.reviewer_confirmation_owner_keys.insert(confirming_owner_key.get("value"));
                }
            }
            to Open
            emit EvidenceAdded
        }

        transition AddEvidenceInProgress {
            on input AddEvidence { expected_revision, evidence_kind, confirming_owner_key }
            guard { self.lifecycle_phase == Phase::InProgress && self.revision == expected_revision }
            guard "owner_key_present_for_kind" {
                evidence_kind_owner_key_present(evidence_kind, confirming_owner_key)
            }
            update {
                self.revision += 1;
                self.evidence_count += 1;
                if evidence_kind == WorkEvidenceKind::HostConfirmation {
                    self.host_confirmation_count += 1;
                }
                if evidence_kind == WorkEvidenceKind::PrincipalConfirmation {
                    self.principal_confirmation_count += 1;
                }
                if evidence_kind == WorkEvidenceKind::SupervisorConfirmation {
                    self.supervisor_confirmation_owner_keys.insert(confirming_owner_key.get("value"));
                }
                if evidence_kind == WorkEvidenceKind::ReviewerConfirmation {
                    self.reviewer_confirmation_owner_keys.insert(confirming_owner_key.get("value"));
                }
            }
            to InProgress
            emit EvidenceAdded
        }

        transition AddEvidenceBlocked {
            on input AddEvidence { expected_revision, evidence_kind, confirming_owner_key }
            guard { self.lifecycle_phase == Phase::Blocked && self.revision == expected_revision }
            guard "owner_key_present_for_kind" {
                evidence_kind_owner_key_present(evidence_kind, confirming_owner_key)
            }
            update {
                self.revision += 1;
                self.evidence_count += 1;
                if evidence_kind == WorkEvidenceKind::HostConfirmation {
                    self.host_confirmation_count += 1;
                }
                if evidence_kind == WorkEvidenceKind::PrincipalConfirmation {
                    self.principal_confirmation_count += 1;
                }
                if evidence_kind == WorkEvidenceKind::SupervisorConfirmation {
                    self.supervisor_confirmation_owner_keys.insert(confirming_owner_key.get("value"));
                }
                if evidence_kind == WorkEvidenceKind::ReviewerConfirmation {
                    self.reviewer_confirmation_owner_keys.insert(confirming_owner_key.get("value"));
                }
            }
            to Blocked
            emit EvidenceAdded
        }

        transition AddEvidenceCompleted {
            on input AddEvidence { expected_revision, evidence_kind, confirming_owner_key }
            guard { self.lifecycle_phase == Phase::Completed && self.revision == expected_revision }
            guard "owner_key_present_for_kind" {
                evidence_kind_owner_key_present(evidence_kind, confirming_owner_key)
            }
            update {
                self.revision += 1;
                self.evidence_count += 1;
                if evidence_kind == WorkEvidenceKind::HostConfirmation {
                    self.host_confirmation_count += 1;
                }
                if evidence_kind == WorkEvidenceKind::PrincipalConfirmation {
                    self.principal_confirmation_count += 1;
                }
                if evidence_kind == WorkEvidenceKind::SupervisorConfirmation {
                    self.supervisor_confirmation_owner_keys.insert(confirming_owner_key.get("value"));
                }
                if evidence_kind == WorkEvidenceKind::ReviewerConfirmation {
                    self.reviewer_confirmation_owner_keys.insert(confirming_owner_key.get("value"));
                }
            }
            to Completed
            emit EvidenceAdded
        }

        transition AddEvidenceCancelled {
            on input AddEvidence { expected_revision, evidence_kind, confirming_owner_key }
            guard { self.lifecycle_phase == Phase::Cancelled && self.revision == expected_revision }
            guard "owner_key_present_for_kind" {
                evidence_kind_owner_key_present(evidence_kind, confirming_owner_key)
            }
            update {
                self.revision += 1;
                self.evidence_count += 1;
                if evidence_kind == WorkEvidenceKind::HostConfirmation {
                    self.host_confirmation_count += 1;
                }
                if evidence_kind == WorkEvidenceKind::PrincipalConfirmation {
                    self.principal_confirmation_count += 1;
                }
                if evidence_kind == WorkEvidenceKind::SupervisorConfirmation {
                    self.supervisor_confirmation_owner_keys.insert(confirming_owner_key.get("value"));
                }
                if evidence_kind == WorkEvidenceKind::ReviewerConfirmation {
                    self.reviewer_confirmation_owner_keys.insert(confirming_owner_key.get("value"));
                }
            }
            to Cancelled
            emit EvidenceAdded
        }

        transition AddEvidenceFailed {
            on input AddEvidence { expected_revision, evidence_kind, confirming_owner_key }
            guard { self.lifecycle_phase == Phase::Failed && self.revision == expected_revision }
            guard "owner_key_present_for_kind" {
                evidence_kind_owner_key_present(evidence_kind, confirming_owner_key)
            }
            update {
                self.revision += 1;
                self.evidence_count += 1;
                if evidence_kind == WorkEvidenceKind::HostConfirmation {
                    self.host_confirmation_count += 1;
                }
                if evidence_kind == WorkEvidenceKind::PrincipalConfirmation {
                    self.principal_confirmation_count += 1;
                }
                if evidence_kind == WorkEvidenceKind::SupervisorConfirmation {
                    self.supervisor_confirmation_owner_keys.insert(confirming_owner_key.get("value"));
                }
                if evidence_kind == WorkEvidenceKind::ReviewerConfirmation {
                    self.reviewer_confirmation_owner_keys.insert(confirming_owner_key.get("value"));
                }
            }
            to Failed
            emit EvidenceAdded
        }

        // --- Public error-class classification ---
        //
        // The public WorkGraph error class surfaced via REST/RPC is a machine
        // fact. The shell extracts a typed WorkGraphErrorKind from the internal
        // WorkGraphError (pure typed extraction, no grouping) and feeds it back
        // here; the transitions below own the many-to-one variant->class POLICY.
        // Each transition self-loops in every lifecycle phase (classification
        // never mutates lifecycle state) and emits exactly one public class.

        transition ClassifyPublicErrorNotFound {
            per_phase [Absent, Open, InProgress, Blocked, Completed, Cancelled, Failed]
            on input ClassifyWorkGraphPublicError { kind }
            guard "not_found_class" {
                kind == WorkGraphErrorKind::NotFound
                || kind == WorkGraphErrorKind::AttentionNotFound
            }
            update {}
            to Absent
            emit WorkGraphPublicErrorClassified {
                kind: kind,
                public_class: WorkGraphPublicErrorClass::NotFound
            }
        }

        transition ClassifyPublicErrorConflict {
            per_phase [Absent, Open, InProgress, Blocked, Completed, Cancelled, Failed]
            on input ClassifyWorkGraphPublicError { kind }
            guard "conflict_class" {
                kind == WorkGraphErrorKind::StaleRevision
                || kind == WorkGraphErrorKind::Conflict
            }
            update {}
            to Absent
            emit WorkGraphPublicErrorClassified {
                kind: kind,
                public_class: WorkGraphPublicErrorClass::Conflict
            }
        }

        transition ClassifyPublicErrorInvalidTransition {
            per_phase [Absent, Open, InProgress, Blocked, Completed, Cancelled, Failed]
            on input ClassifyWorkGraphPublicError { kind }
            guard "invalid_transition_class" {
                kind == WorkGraphErrorKind::InvalidTransition
            }
            update {}
            to Absent
            emit WorkGraphPublicErrorClassified {
                kind: kind,
                public_class: WorkGraphPublicErrorClass::InvalidTransition
            }
        }

        transition ClassifyPublicErrorInvalidArguments {
            per_phase [Absent, Open, InProgress, Blocked, Completed, Cancelled, Failed]
            on input ClassifyWorkGraphPublicError { kind }
            guard "invalid_arguments_class" {
                kind == WorkGraphErrorKind::InvalidInput
                || kind == WorkGraphErrorKind::InvalidTimestampMillis
            }
            update {}
            to Absent
            emit WorkGraphPublicErrorClassified {
                kind: kind,
                public_class: WorkGraphPublicErrorClass::InvalidArguments
            }
        }

        transition ClassifyPublicErrorCapabilityUnavailable {
            per_phase [Absent, Open, InProgress, Blocked, Completed, Cancelled, Failed]
            on input ClassifyWorkGraphPublicError { kind }
            guard "capability_unavailable_class" {
                kind == WorkGraphErrorKind::UnsupportedBackend
            }
            update {}
            to Absent
            emit WorkGraphPublicErrorClassified {
                kind: kind,
                public_class: WorkGraphPublicErrorClass::CapabilityUnavailable
            }
        }

        transition ClassifyPublicErrorStoreError {
            per_phase [Absent, Open, InProgress, Blocked, Completed, Cancelled, Failed]
            on input ClassifyWorkGraphPublicError { kind }
            guard "store_error_class" {
                kind == WorkGraphErrorKind::Store
            }
            update {}
            to Absent
            emit WorkGraphPublicErrorClassified {
                kind: kind,
                public_class: WorkGraphPublicErrorClass::StoreError
            }
        }

        // --- Terminality classification ---
        //
        // The terminality verdict over the machine-owned lifecycle_phase is a
        // machine fact. The shell drives this input over recovered state and
        // mirrors the emitted `terminal`. Each transition self-loops in its phase
        // (classification never mutates lifecycle state). The terminal phase set
        // here is exactly `terminal [Completed, Cancelled, Failed]` above.

        transition ClassifyTerminalityTerminal {
            per_phase [Completed, Cancelled, Failed]
            on input ClassifyTerminality {}
            update {}
            to Absent
            emit WorkItemTerminalityClassified { terminal: true }
        }

        transition ClassifyTerminalityLive {
            per_phase [Absent, Open, InProgress, Blocked]
            on input ClassifyTerminality {}
            update {}
            to Absent
            emit WorkItemTerminalityClassified { terminal: false }
        }

        // --- Per-blocking-edge satisfaction classification ---
        //
        // A blocking edge is satisfied iff its blocker item reached terminal
        // SUCCESS (Completed). The shell extracts only the raw blocker lifecycle
        // phase (projected from the blocker's own machine state) and whether the
        // blocker was present at all; this machine owns the satisfaction POLICY
        // and emits the verdict. The shell mirrors it and mechanically fans-in
        // (counts) the unsatisfied edges. Phase-independent: self-loops in every
        // phase of the item whose blockers are being classified.
        transition ClassifyBlockerSatisfaction {
            per_phase [Absent, Open, InProgress, Blocked, Completed, Cancelled, Failed]
            on input ClassifyBlockerSatisfied { blocker_present, blocker_lifecycle_phase }
            update {}
            to Absent
            emit BlockerSatisfactionClassified {
                satisfied: blocker_present && blocker_lifecycle_phase == WorkLifecycleState::Completed
            }
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
    #[serde(default)]
    completion_policy: WorkCompletionPolicy,
    #[serde(default)]
    completion_supervisor_owner_key: Option<WorkOwnerKey>,
    #[serde(default)]
    completion_reviewer_quorum_threshold: Option<u64>,
    terminal_at_utc_ms: Option<u64>,
    evidence_count: u64,
    #[serde(default)]
    host_confirmation_count: u64,
    #[serde(default)]
    principal_confirmation_count: u64,
    #[serde(default)]
    supervisor_confirmation_owner_keys: std::collections::BTreeSet<WorkOwnerKey>,
    #[serde(default)]
    reviewer_confirmation_owner_keys: std::collections::BTreeSet<WorkOwnerKey>,
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
            completion_policy: state.completion_policy,
            completion_supervisor_owner_key: state.completion_supervisor_owner_key.clone(),
            completion_reviewer_quorum_threshold: state.completion_reviewer_quorum_threshold,
            terminal_at_utc_ms: state.terminal_at_utc_ms,
            evidence_count: state.evidence_count,
            host_confirmation_count: state.host_confirmation_count,
            principal_confirmation_count: state.principal_confirmation_count,
            supervisor_confirmation_owner_keys: state.supervisor_confirmation_owner_keys.clone(),
            reviewer_confirmation_owner_keys: state.reviewer_confirmation_owner_keys.clone(),
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
            completion_policy: wire.completion_policy,
            completion_supervisor_owner_key: wire.completion_supervisor_owner_key,
            completion_reviewer_quorum_threshold: wire.completion_reviewer_quorum_threshold,
            terminal_at_utc_ms: wire.terminal_at_utc_ms,
            evidence_count: wire.evidence_count,
            host_confirmation_count: wire.host_confirmation_count,
            principal_confirmation_count: wire.principal_confirmation_count,
            supervisor_confirmation_owner_keys: wire.supervisor_confirmation_owner_keys,
            reviewer_confirmation_owner_keys: wire.reviewer_confirmation_owner_keys,
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
