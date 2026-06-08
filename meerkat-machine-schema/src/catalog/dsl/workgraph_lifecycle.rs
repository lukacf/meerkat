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

/// Machine-owned admission verdict for the requested INITIAL lifecycle status of
/// a newly created work item. This machine — not the shell — owns the creation
/// policy "a new work item may only start open or blocked". The shell extracts
/// the requested status as a pure typed observation (a `WorkLifecycleState`),
/// drives `ClassifyCreateStatusAdmission`, and mirrors the verdict:
/// `AdmittedOpen` -> drive `CreateOpen`, `AdmittedBlocked` -> drive
/// `CreateBlocked`, `Denied` -> reject with `InvalidTransition`.
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
pub enum WorkCreateStatusAdmissionKind {
    #[default]
    Denied,
    AdmittedOpen,
    AdmittedBlocked,
}

/// Machine-owned admission verdict for the requested `completion_policy` of a
/// newly created NON-GOAL work item. This machine — not the shell — owns the
/// creation policy "non-goal work items must use the self-attest completion
/// policy". The shell extracts the requested completion policy as a pure typed
/// observation (a `WorkCompletionPolicy`), drives
/// `ClassifyCreateCompletionPolicyAdmission`, and mirrors the verdict:
/// `Admitted` -> proceed, `DeniedNonSelfAttest` -> reject with the same
/// `InvalidInput` rejection. Fails closed.
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
pub enum WorkCreateCompletionPolicyAdmissionKind {
    #[default]
    DeniedNonSelfAttest,
    Admitted,
}

/// Machine-owned admission verdict for the requested target lifecycle status of
/// a `close` operation. This machine — not the shell — owns the lifecycle-class
/// fact "close requires a terminal status". The shell extracts the requested
/// target status as a pure typed observation (a `WorkLifecycleState`), drives
/// `ClassifyCloseStatusAdmission`, and mirrors the verdict:
/// `AdmittedCompleted` -> drive `CloseCompleted`, `AdmittedCancelled` -> drive
/// `CloseCancelled`, `AdmittedFailed` -> drive `CloseFailed`,
/// `DeniedNonTerminal` -> reject with the same `InvalidTransition` rejection.
/// Fails closed.
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
pub enum WorkCloseStatusAdmissionKind {
    #[default]
    DeniedNonTerminal,
    AdmittedCompleted,
    AdmittedCancelled,
    AdmittedFailed,
}

/// Machine-owned admission verdict for a PUBLIC (untrusted) goal-confirmation
/// over a work item's machine-owned `completion_policy`. This machine — not the
/// shell — owns the trust-scoped eligibility "only a self-attested completion
/// policy may be confirmed by an untrusted public caller; every other policy
/// requires trusted in-process host authority". The public-confirm surface
/// extracts the typed `completion_policy` as a pure observation, drives
/// `ClassifyPublicConfirmationAdmission`, and mirrors the verdict: `Admitted`
/// -> proceed, `DeniedRequiresTrustedHost` -> reject with `InvalidInput`.
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
pub enum WorkPublicConfirmationAdmissionKind {
    #[default]
    DeniedRequiresTrustedHost,
    Admitted,
}

/// Machine-owned admission verdict for a requested mutation of a work item's
/// `completion_policy`. This machine — not the shell — owns the immutability
/// invariant "a work item's completion policy is fixed at creation and cannot be
/// changed by an update". The shell extracts the requested completion policy as a
/// pure typed observation (variant plus supervisor owner key plus reviewer quorum
/// threshold), drives `ClassifyCompletionPolicyMutationAdmission` over the
/// recovered item state, and mirrors the verdict: `Admitted` (the requested
/// policy is identical to the current machine-owned policy, so the update is a
/// no-op on policy) -> proceed, `Denied` (the requested policy differs) -> reject
/// with the same `InvalidInput` rejection. Fails closed.
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
pub enum WorkCompletionPolicyMutationAdmissionKind {
    #[default]
    Denied,
    Admitted,
}

/// Typed observation the trusted goal-confirm shell extracts from the OPAQUE
/// confirmation `evidence.kind` provenance string. This is a pure typed
/// extraction (the recognized reserved confirmation literals map 1:1 onto a
/// confirmation-evidence variant; an empty trimmed string is `Empty`; everything
/// else is `Other`); the shell performs NO admission decision — the
/// per-policy required-evidence-kind POLICY lives in the
/// `ClassifyConfirmationAdmission` transitions, owned by this canonical machine.
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
pub enum WorkConfirmationEvidenceObservation {
    #[default]
    Empty,
    Other,
    HostConfirmation,
    PrincipalConfirmation,
    SupervisorConfirmation,
    ReviewerConfirmation,
}

/// Machine-owned admission verdict for a TRUSTED-path goal confirmation over a
/// work item's machine-owned `completion_policy`. This machine — not the
/// goal-confirm shell — owns the eligibility "is this confirming principal +
/// supplied evidence kind admissible for this completion policy". The
/// goal-confirm shell extracts only pure typed observations (the machine-owned
/// completion policy + its supervisor owner key, the requested confirming
/// principal owner key + kind, and the typed evidence-kind observation), drives
/// `ClassifyConfirmationAdmission`, and mirrors the verdict: `Admitted` ->
/// proceed to stamp the canonicalized evidence; each `Denied*` -> the exact same
/// `InvalidInput` rejection the shell previously produced. Fails closed.
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
pub enum WorkConfirmationAdmissionKind {
    /// A confirming principal is required by the policy but none was supplied.
    #[default]
    DeniedPrincipalRequired,
    /// `PrincipalConfirmed` requires a principal-kind owner key.
    DeniedPrincipalKindMismatch,
    /// `Supervisor` requires confirmation from the policy's owner key.
    DeniedSupervisorMismatch,
    /// The supplied evidence kind does not match the policy's required kind.
    DeniedEvidenceKind,
    /// `SelfAttest` requires a non-empty evidence kind.
    DeniedSelfAttestEmptyEvidenceKind,
    /// The confirmation is admissible; the shell proceeds to stamp evidence.
    Admitted,
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
            // Create-status admission classification. This machine owns the
            // creation policy "a new work item may only start open or blocked".
            // The shell extracts the requested INITIAL status as a pure typed
            // observation (a WorkLifecycleState) and feeds it here; this machine
            // decides whether that status is an admissible creation state and
            // emits CreateStatusAdmissionClassified. The shell mirrors the
            // verdict (AdmittedOpen -> CreateOpen, AdmittedBlocked ->
            // CreateBlocked, Denied -> InvalidTransition), failing closed.
            ClassifyCreateStatusAdmission { requested_status: Enum<WorkLifecycleState> },
            // Create-time completion-policy admission classification. This
            // machine owns the creation policy "non-goal work items must use the
            // self-attest completion policy". The shell extracts the requested
            // completion policy as a pure typed observation (a
            // WorkCompletionPolicy) and feeds it here; this machine decides
            // whether that policy is admissible at create and emits
            // CreateCompletionPolicyAdmissionClassified. The shell mirrors the
            // verdict (Admitted -> proceed, DeniedNonSelfAttest -> InvalidInput),
            // failing closed. Phase-independent: self-loops over every phase so
            // the classification is total regardless of the authority phase.
            ClassifyCreateCompletionPolicyAdmission { completion_policy: Enum<WorkCompletionPolicy> },
            // Close-status admission classification. This machine owns the
            // lifecycle-class fact "close requires a terminal status" — i.e.
            // which statuses are admissible as a CLOSE target. The shell extracts
            // the requested target status as a pure typed observation (a
            // WorkLifecycleState) and feeds it here; this machine decides
            // admissibility and emits CloseStatusAdmissionClassified. The shell
            // mirrors the verdict (AdmittedCompleted -> CloseCompleted,
            // AdmittedCancelled -> CloseCancelled, AdmittedFailed -> CloseFailed,
            // DeniedNonTerminal -> InvalidTransition), failing closed.
            // Phase-independent: self-loops over every phase so the
            // classification is total regardless of the authority phase.
            ClassifyCloseStatusAdmission { requested_status: Enum<WorkLifecycleState> },
            // Public-confirmation admission classification. This machine owns
            // the trust-scoped eligibility "only a self-attested completion
            // policy may be confirmed by an untrusted public caller". The
            // public-confirm surface extracts the typed completion_policy as a
            // pure observation and feeds it here; this machine decides
            // admissibility and emits PublicConfirmationAdmissionClassified. The
            // surface mirrors the verdict (Admitted -> proceed,
            // DeniedRequiresTrustedHost -> InvalidInput), failing closed.
            ClassifyPublicConfirmationAdmission { completion_policy: Enum<WorkCompletionPolicy> },
            // Completion-policy mutation admission classification. This machine
            // owns the immutability invariant "a work item's completion policy is
            // fixed at creation and cannot be changed by an update". The shell
            // extracts the REQUESTED completion policy as a pure typed observation
            // (variant plus supervisor owner key plus reviewer quorum threshold)
            // and drives this input over the item's recovered machine state; this
            // machine compares the requested policy against its own
            // machine-owned completion policy and emits
            // CompletionPolicyMutationAdmissionClassified. The shell mirrors the
            // verdict (Admitted -> the policy is unchanged, proceed; Denied -> the
            // policy would change, reject with InvalidInput), failing closed.
            ClassifyCompletionPolicyMutationAdmission {
                requested_completion_policy: Enum<WorkCompletionPolicy>,
                requested_completion_supervisor_owner_key: Option<WorkOwnerKey>,
                requested_completion_reviewer_quorum_threshold: Option<u64>,
            },
            // Trusted-path confirmation admission classification. This machine
            // owns the eligibility "is this confirming principal + supplied
            // evidence kind admissible for this completion policy". The
            // goal-confirm shell extracts only pure typed observations (the
            // machine-owned completion policy + its supervisor owner key, the
            // requested confirming principal owner key + kind, and the typed
            // evidence-kind observation parsed from the opaque evidence.kind
            // string) and drives this input; this machine decides admissibility
            // and emits ConfirmationAdmissionClassified. The shell mirrors the
            // verdict (Admitted -> stamp the canonicalized evidence, each
            // Denied* -> the exact same InvalidInput rejection), failing closed.
            // Phase-independent: self-loops over every phase so the
            // classification is total regardless of the authority phase.
            ClassifyConfirmationAdmission {
                completion_policy: Enum<WorkCompletionPolicy>,
                completion_supervisor_owner_key: Option<WorkOwnerKey>,
                requested_principal_owner_key: Option<WorkOwnerKey>,
                requested_principal_kind: Option<Enum<WorkOwnerKind>>,
                supplied_evidence_kind: Enum<WorkConfirmationEvidenceObservation>,
            },
            // Readiness classification. An item is "ready" iff it is claimable
            // right now — exactly the condition the `Claim` transition guards
            // accept (`ClaimOpen` from Open, or `ClaimExpiredInProgress`
            // reclaiming an expired in-progress lease). This machine owns that
            // readiness POLICY; the shell extracts only the raw wall-clock
            // `now_utc_ms` (a pure observation) and drives this input over the
            // recovered item state, mirroring the emitted
            // WorkItemReadinessClassified.ready, failing closed. Each transition
            // self-loops in its phase (classification never mutates state).
            ClassifyReadiness { now_utc_ms: u64 },
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
            CreateStatusAdmissionClassified { admission: Enum<WorkCreateStatusAdmissionKind> },
            CreateCompletionPolicyAdmissionClassified {
                admission: Enum<WorkCreateCompletionPolicyAdmissionKind>,
            },
            CloseStatusAdmissionClassified { admission: Enum<WorkCloseStatusAdmissionKind> },
            PublicConfirmationAdmissionClassified {
                admission: Enum<WorkPublicConfirmationAdmissionKind>,
            },
            CompletionPolicyMutationAdmissionClassified {
                admission: Enum<WorkCompletionPolicyMutationAdmissionKind>,
            },
            ConfirmationAdmissionClassified {
                admission: Enum<WorkConfirmationAdmissionKind>,
            },
            WorkItemReadinessClassified { ready: bool },
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

        // --- Trusted-path confirmation-admission verdict helpers ---
        //
        // These encode the EXACT per-policy check precedence the retired
        // `confirmation_evidence_for_policy` shell reducer applied:
        //   SelfAttest         : Empty evidence -> empty-denial; else Admitted.
        //   HostConfirmed      : evidence != HostConfirmation -> evidence-denial;
        //                        else Admitted. (no principal check)
        //   PrincipalConfirmed : principal absent -> principal-required;
        //                        principal kind != Principal -> kind-mismatch;
        //                        evidence != PrincipalConfirmation -> evidence;
        //                        else Admitted.
        //   Supervisor         : principal absent -> principal-required;
        //                        principal != owner_key -> supervisor-mismatch;
        //                        evidence != SupervisorConfirmation -> evidence;
        //                        else Admitted.
        //   ReviewerQuorum     : principal absent -> principal-required;
        //                        evidence != ReviewerConfirmation -> evidence;
        //                        else Admitted.
        // Each helper returns true iff its verdict is the one that fires; the
        // helpers are mutually exclusive and total.

        // A confirming principal is required by the policy but none was supplied.
        helper confirmation_denies_principal_required(
            completion_policy: WorkCompletionPolicy,
            requested_principal_owner_key: Option<WorkOwnerKey>
        ) -> bool {
            (completion_policy == WorkCompletionPolicy::PrincipalConfirmed
                || completion_policy == WorkCompletionPolicy::Supervisor
                || completion_policy == WorkCompletionPolicy::ReviewerQuorum)
                && requested_principal_owner_key == None
        }

        // PrincipalConfirmed: principal present but its kind is not Principal.
        helper confirmation_denies_principal_kind_mismatch(
            completion_policy: WorkCompletionPolicy,
            requested_principal_owner_key: Option<WorkOwnerKey>,
            requested_principal_kind: Option<WorkOwnerKind>
        ) -> bool {
            completion_policy == WorkCompletionPolicy::PrincipalConfirmed
                && requested_principal_owner_key != None
                && (requested_principal_kind == None
                    || requested_principal_kind.get("value") != WorkOwnerKind::Principal)
        }

        // Supervisor: principal present but does not equal the policy owner key.
        helper confirmation_denies_supervisor_mismatch(
            completion_policy: WorkCompletionPolicy,
            completion_supervisor_owner_key: Option<WorkOwnerKey>,
            requested_principal_owner_key: Option<WorkOwnerKey>
        ) -> bool {
            completion_policy == WorkCompletionPolicy::Supervisor
                && requested_principal_owner_key != None
                && (completion_supervisor_owner_key == None
                    || requested_principal_owner_key.get("value")
                        != completion_supervisor_owner_key.get("value"))
        }

        // SelfAttest: the evidence kind string is empty.
        helper confirmation_denies_self_attest_empty(
            completion_policy: WorkCompletionPolicy,
            supplied_evidence_kind: WorkConfirmationEvidenceObservation
        ) -> bool {
            completion_policy == WorkCompletionPolicy::SelfAttest
                && supplied_evidence_kind == WorkConfirmationEvidenceObservation::Empty
        }

        // The supplied evidence kind does not match the policy's required kind.
        // Only reached for the policies that perform an evidence-kind check,
        // AFTER the principal/kind/supervisor checks above have passed.
        helper confirmation_denies_evidence_kind(
            completion_policy: WorkCompletionPolicy,
            completion_supervisor_owner_key: Option<WorkOwnerKey>,
            requested_principal_owner_key: Option<WorkOwnerKey>,
            requested_principal_kind: Option<WorkOwnerKind>,
            supplied_evidence_kind: WorkConfirmationEvidenceObservation
        ) -> bool {
            if completion_policy == WorkCompletionPolicy::HostConfirmed {
                supplied_evidence_kind != WorkConfirmationEvidenceObservation::HostConfirmation
            } else {
                if completion_policy == WorkCompletionPolicy::PrincipalConfirmed {
                    confirmation_denies_principal_required(completion_policy, requested_principal_owner_key) == false
                        && confirmation_denies_principal_kind_mismatch(completion_policy, requested_principal_owner_key, requested_principal_kind) == false
                        && supplied_evidence_kind != WorkConfirmationEvidenceObservation::PrincipalConfirmation
                } else {
                    if completion_policy == WorkCompletionPolicy::Supervisor {
                        confirmation_denies_principal_required(completion_policy, requested_principal_owner_key) == false
                            && confirmation_denies_supervisor_mismatch(completion_policy, completion_supervisor_owner_key, requested_principal_owner_key) == false
                            && supplied_evidence_kind != WorkConfirmationEvidenceObservation::SupervisorConfirmation
                    } else {
                        if completion_policy == WorkCompletionPolicy::ReviewerQuorum {
                            confirmation_denies_principal_required(completion_policy, requested_principal_owner_key) == false
                                && supplied_evidence_kind != WorkConfirmationEvidenceObservation::ReviewerConfirmation
                        } else {
                            false
                        }
                    }
                }
            }
        }

        // The confirmation is admissible: no denial helper fires.
        helper confirmation_admits(
            completion_policy: WorkCompletionPolicy,
            completion_supervisor_owner_key: Option<WorkOwnerKey>,
            requested_principal_owner_key: Option<WorkOwnerKey>,
            requested_principal_kind: Option<WorkOwnerKind>,
            supplied_evidence_kind: WorkConfirmationEvidenceObservation
        ) -> bool {
            confirmation_denies_principal_required(completion_policy, requested_principal_owner_key) == false
                && confirmation_denies_principal_kind_mismatch(completion_policy, requested_principal_owner_key, requested_principal_kind) == false
                && confirmation_denies_supervisor_mismatch(completion_policy, completion_supervisor_owner_key, requested_principal_owner_key) == false
                && confirmation_denies_self_attest_empty(completion_policy, supplied_evidence_kind) == false
                && confirmation_denies_evidence_kind(completion_policy, completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind) == false
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

        disposition Created => local seam NoOwnerRealization,
        disposition Updated => local seam NoOwnerRealization,
        disposition Claimed => local seam NoOwnerRealization,
        disposition Released => local seam NoOwnerRealization,
        disposition Blocked => local seam NoOwnerRealization,
        disposition LinkValidated => local seam NoOwnerRealization,
        disposition Closed => routed [WorkAttentionLifecycleMachine] seam NoOwnerRealization,
        disposition EvidenceAdded => local seam NoOwnerRealization,
        disposition WorkGraphPublicErrorClassified => local seam SurfaceResultAlignment,
        disposition WorkItemTerminalityClassified => local seam SurfaceResultAlignment,
        disposition BlockerSatisfactionClassified => local seam SurfaceResultAlignment,
        disposition CreateStatusAdmissionClassified => local seam SurfaceResultAlignment,
        disposition CreateCompletionPolicyAdmissionClassified => local seam SurfaceResultAlignment,
        disposition CloseStatusAdmissionClassified => local seam SurfaceResultAlignment,
        disposition PublicConfirmationAdmissionClassified => local seam SurfaceResultAlignment,
        disposition CompletionPolicyMutationAdmissionClassified => local seam SurfaceResultAlignment,
        disposition ConfirmationAdmissionClassified => local seam SurfaceResultAlignment,
        disposition WorkItemReadinessClassified => local seam SurfaceResultAlignment,

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

        // --- Readiness classification ---
        //
        // An item is "ready" iff it is claimable right now. The readiness
        // condition reproduces EXACTLY the `Claim` transition guards: an Open
        // item is ready when its blockers are resolved and its due / not-before
        // / snooze windows are all eligible (`ClaimOpen`); an InProgress item is
        // ready only when its prior claim's lease has expired and the same
        // eligibility holds (`ClaimExpiredInProgress`); every other phase is not
        // ready. The shell extracts only `now_utc_ms` and mirrors the verdict.
        // Each transition self-loops in its phase.

        helper claim_time_window_eligible(
            due_at_utc_ms: Option<u64>,
            not_before_utc_ms: Option<u64>,
            snoozed_until_utc_ms: Option<u64>,
            now_utc_ms: u64
        ) -> bool {
            (if due_at_utc_ms == None { true } else { due_at_utc_ms.get("value") <= now_utc_ms })
            && (if not_before_utc_ms == None { true } else { not_before_utc_ms.get("value") <= now_utc_ms })
            && (if snoozed_until_utc_ms == None { true } else { snoozed_until_utc_ms.get("value") <= now_utc_ms })
        }

        transition ClassifyReadinessOpen {
            per_phase [Open]
            on input ClassifyReadiness { now_utc_ms }
            update {}
            to Open
            emit WorkItemReadinessClassified {
                ready: self.unresolved_blocker_count == 0
                    && claim_time_window_eligible(
                        self.due_at_utc_ms,
                        self.not_before_utc_ms,
                        self.snoozed_until_utc_ms,
                        now_utc_ms
                    )
            }
        }

        transition ClassifyReadinessInProgress {
            per_phase [InProgress]
            on input ClassifyReadiness { now_utc_ms }
            update {}
            to InProgress
            emit WorkItemReadinessClassified {
                ready: self.claim_owner_key != None
                    && self.lease_expires_at_utc_ms != None
                    && self.lease_expires_at_utc_ms.get("value") <= now_utc_ms
                    && self.unresolved_blocker_count == 0
                    && claim_time_window_eligible(
                        self.due_at_utc_ms,
                        self.not_before_utc_ms,
                        self.snoozed_until_utc_ms,
                        now_utc_ms
                    )
            }
        }

        transition ClassifyReadinessNotClaimable {
            per_phase [Absent, Blocked, Completed, Cancelled, Failed]
            on input ClassifyReadiness { now_utc_ms }
            update {}
            to Absent
            emit WorkItemReadinessClassified { ready: false }
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

        // --- Create-status admission classification ---
        //
        // This machine owns the creation policy "a new work item may only start
        // open or blocked". The shell extracts the requested INITIAL status as a
        // pure typed WorkLifecycleState observation and drives this input over a
        // fresh (Absent) authority; this machine decides admissibility and emits
        // the verdict. Open -> AdmittedOpen, Blocked -> AdmittedBlocked, every
        // other requested status (Absent / InProgress / terminal) -> Denied.
        // Phase-independent: self-loops over every phase so the classification is
        // total regardless of the authority's phase.
        transition ClassifyCreateStatusAdmissionOpen {
            per_phase [Absent, Open, InProgress, Blocked, Completed, Cancelled, Failed]
            on input ClassifyCreateStatusAdmission { requested_status }
            guard "requested_open" { requested_status == WorkLifecycleState::Open }
            update {}
            to Absent
            emit CreateStatusAdmissionClassified { admission: WorkCreateStatusAdmissionKind::AdmittedOpen }
        }

        transition ClassifyCreateStatusAdmissionBlocked {
            per_phase [Absent, Open, InProgress, Blocked, Completed, Cancelled, Failed]
            on input ClassifyCreateStatusAdmission { requested_status }
            guard "requested_blocked" { requested_status == WorkLifecycleState::Blocked }
            update {}
            to Absent
            emit CreateStatusAdmissionClassified { admission: WorkCreateStatusAdmissionKind::AdmittedBlocked }
        }

        transition ClassifyCreateStatusAdmissionDeniedAbsent {
            per_phase [Absent, Open, InProgress, Blocked, Completed, Cancelled, Failed]
            on input ClassifyCreateStatusAdmission { requested_status }
            guard "requested_absent" { requested_status == WorkLifecycleState::Absent }
            update {}
            to Absent
            emit CreateStatusAdmissionClassified { admission: WorkCreateStatusAdmissionKind::Denied }
        }

        transition ClassifyCreateStatusAdmissionDeniedInProgress {
            per_phase [Absent, Open, InProgress, Blocked, Completed, Cancelled, Failed]
            on input ClassifyCreateStatusAdmission { requested_status }
            guard "requested_in_progress" { requested_status == WorkLifecycleState::InProgress }
            update {}
            to Absent
            emit CreateStatusAdmissionClassified { admission: WorkCreateStatusAdmissionKind::Denied }
        }

        transition ClassifyCreateStatusAdmissionDeniedCompleted {
            per_phase [Absent, Open, InProgress, Blocked, Completed, Cancelled, Failed]
            on input ClassifyCreateStatusAdmission { requested_status }
            guard "requested_completed" { requested_status == WorkLifecycleState::Completed }
            update {}
            to Absent
            emit CreateStatusAdmissionClassified { admission: WorkCreateStatusAdmissionKind::Denied }
        }

        transition ClassifyCreateStatusAdmissionDeniedCancelled {
            per_phase [Absent, Open, InProgress, Blocked, Completed, Cancelled, Failed]
            on input ClassifyCreateStatusAdmission { requested_status }
            guard "requested_cancelled" { requested_status == WorkLifecycleState::Cancelled }
            update {}
            to Absent
            emit CreateStatusAdmissionClassified { admission: WorkCreateStatusAdmissionKind::Denied }
        }

        transition ClassifyCreateStatusAdmissionDeniedFailed {
            per_phase [Absent, Open, InProgress, Blocked, Completed, Cancelled, Failed]
            on input ClassifyCreateStatusAdmission { requested_status }
            guard "requested_failed" { requested_status == WorkLifecycleState::Failed }
            update {}
            to Absent
            emit CreateStatusAdmissionClassified { admission: WorkCreateStatusAdmissionKind::Denied }
        }

        // --- Create-time completion-policy admission classification ---
        //
        // This machine owns the creation policy "non-goal work items must use the
        // self-attest completion policy". The shell extracts the requested
        // completion policy as a pure typed WorkCompletionPolicy observation and
        // drives this input; this machine decides admissibility and emits the
        // verdict. SelfAttest -> Admitted, every other policy ->
        // DeniedNonSelfAttest. Phase-independent: self-loops over every phase so
        // the classification is total regardless of the authority phase.
        transition ClassifyCreateCompletionPolicyAdmissionSelfAttest {
            per_phase [Absent, Open, InProgress, Blocked, Completed, Cancelled, Failed]
            on input ClassifyCreateCompletionPolicyAdmission { completion_policy }
            guard "self_attest_admissible_at_create" { completion_policy == WorkCompletionPolicy::SelfAttest }
            update {}
            to Absent
            emit CreateCompletionPolicyAdmissionClassified { admission: WorkCreateCompletionPolicyAdmissionKind::Admitted }
        }

        transition ClassifyCreateCompletionPolicyAdmissionHostConfirmed {
            per_phase [Absent, Open, InProgress, Blocked, Completed, Cancelled, Failed]
            on input ClassifyCreateCompletionPolicyAdmission { completion_policy }
            guard "host_confirmed_denied_at_create" { completion_policy == WorkCompletionPolicy::HostConfirmed }
            update {}
            to Absent
            emit CreateCompletionPolicyAdmissionClassified { admission: WorkCreateCompletionPolicyAdmissionKind::DeniedNonSelfAttest }
        }

        transition ClassifyCreateCompletionPolicyAdmissionPrincipalConfirmed {
            per_phase [Absent, Open, InProgress, Blocked, Completed, Cancelled, Failed]
            on input ClassifyCreateCompletionPolicyAdmission { completion_policy }
            guard "principal_confirmed_denied_at_create" { completion_policy == WorkCompletionPolicy::PrincipalConfirmed }
            update {}
            to Absent
            emit CreateCompletionPolicyAdmissionClassified { admission: WorkCreateCompletionPolicyAdmissionKind::DeniedNonSelfAttest }
        }

        transition ClassifyCreateCompletionPolicyAdmissionSupervisor {
            per_phase [Absent, Open, InProgress, Blocked, Completed, Cancelled, Failed]
            on input ClassifyCreateCompletionPolicyAdmission { completion_policy }
            guard "supervisor_denied_at_create" { completion_policy == WorkCompletionPolicy::Supervisor }
            update {}
            to Absent
            emit CreateCompletionPolicyAdmissionClassified { admission: WorkCreateCompletionPolicyAdmissionKind::DeniedNonSelfAttest }
        }

        transition ClassifyCreateCompletionPolicyAdmissionReviewerQuorum {
            per_phase [Absent, Open, InProgress, Blocked, Completed, Cancelled, Failed]
            on input ClassifyCreateCompletionPolicyAdmission { completion_policy }
            guard "reviewer_quorum_denied_at_create" { completion_policy == WorkCompletionPolicy::ReviewerQuorum }
            update {}
            to Absent
            emit CreateCompletionPolicyAdmissionClassified { admission: WorkCreateCompletionPolicyAdmissionKind::DeniedNonSelfAttest }
        }

        // --- Close-status admission classification ---
        //
        // This machine owns the lifecycle-class fact "close requires a terminal
        // status". The shell extracts the requested target status as a pure typed
        // WorkLifecycleState observation and drives this input; this machine
        // decides which statuses are admissible as a CLOSE target and emits the
        // verdict. Completed -> AdmittedCompleted, Cancelled -> AdmittedCancelled,
        // Failed -> AdmittedFailed, every non-terminal status (Absent / Open /
        // InProgress / Blocked) -> DeniedNonTerminal. Phase-independent:
        // self-loops over every phase so the classification is total regardless
        // of the authority's phase.
        transition ClassifyCloseStatusAdmissionCompleted {
            per_phase [Absent, Open, InProgress, Blocked, Completed, Cancelled, Failed]
            on input ClassifyCloseStatusAdmission { requested_status }
            guard "requested_completed" { requested_status == WorkLifecycleState::Completed }
            update {}
            to Absent
            emit CloseStatusAdmissionClassified { admission: WorkCloseStatusAdmissionKind::AdmittedCompleted }
        }

        transition ClassifyCloseStatusAdmissionCancelled {
            per_phase [Absent, Open, InProgress, Blocked, Completed, Cancelled, Failed]
            on input ClassifyCloseStatusAdmission { requested_status }
            guard "requested_cancelled" { requested_status == WorkLifecycleState::Cancelled }
            update {}
            to Absent
            emit CloseStatusAdmissionClassified { admission: WorkCloseStatusAdmissionKind::AdmittedCancelled }
        }

        transition ClassifyCloseStatusAdmissionFailed {
            per_phase [Absent, Open, InProgress, Blocked, Completed, Cancelled, Failed]
            on input ClassifyCloseStatusAdmission { requested_status }
            guard "requested_failed" { requested_status == WorkLifecycleState::Failed }
            update {}
            to Absent
            emit CloseStatusAdmissionClassified { admission: WorkCloseStatusAdmissionKind::AdmittedFailed }
        }

        transition ClassifyCloseStatusAdmissionDeniedAbsent {
            per_phase [Absent, Open, InProgress, Blocked, Completed, Cancelled, Failed]
            on input ClassifyCloseStatusAdmission { requested_status }
            guard "requested_absent" { requested_status == WorkLifecycleState::Absent }
            update {}
            to Absent
            emit CloseStatusAdmissionClassified { admission: WorkCloseStatusAdmissionKind::DeniedNonTerminal }
        }

        transition ClassifyCloseStatusAdmissionDeniedOpen {
            per_phase [Absent, Open, InProgress, Blocked, Completed, Cancelled, Failed]
            on input ClassifyCloseStatusAdmission { requested_status }
            guard "requested_open" { requested_status == WorkLifecycleState::Open }
            update {}
            to Absent
            emit CloseStatusAdmissionClassified { admission: WorkCloseStatusAdmissionKind::DeniedNonTerminal }
        }

        transition ClassifyCloseStatusAdmissionDeniedInProgress {
            per_phase [Absent, Open, InProgress, Blocked, Completed, Cancelled, Failed]
            on input ClassifyCloseStatusAdmission { requested_status }
            guard "requested_in_progress" { requested_status == WorkLifecycleState::InProgress }
            update {}
            to Absent
            emit CloseStatusAdmissionClassified { admission: WorkCloseStatusAdmissionKind::DeniedNonTerminal }
        }

        transition ClassifyCloseStatusAdmissionDeniedBlocked {
            per_phase [Absent, Open, InProgress, Blocked, Completed, Cancelled, Failed]
            on input ClassifyCloseStatusAdmission { requested_status }
            guard "requested_blocked" { requested_status == WorkLifecycleState::Blocked }
            update {}
            to Absent
            emit CloseStatusAdmissionClassified { admission: WorkCloseStatusAdmissionKind::DeniedNonTerminal }
        }

        // --- Public-confirmation admission classification ---
        //
        // This machine owns the trust-scoped eligibility "only a self-attested
        // completion policy may be confirmed by an untrusted public caller; every
        // other policy requires trusted in-process host authority". The
        // public-confirm surface extracts the typed completion_policy as a pure
        // observation and drives this input; this machine decides admissibility
        // and emits the verdict. SelfAttest -> Admitted, every other policy ->
        // DeniedRequiresTrustedHost. Phase-independent: self-loops over every
        // phase so the classification is total regardless of the authority phase.
        transition ClassifyPublicConfirmationAdmissionSelfAttest {
            per_phase [Absent, Open, InProgress, Blocked, Completed, Cancelled, Failed]
            on input ClassifyPublicConfirmationAdmission { completion_policy }
            guard "self_attest_public_confirmable" { completion_policy == WorkCompletionPolicy::SelfAttest }
            update {}
            to Absent
            emit PublicConfirmationAdmissionClassified { admission: WorkPublicConfirmationAdmissionKind::Admitted }
        }

        transition ClassifyPublicConfirmationAdmissionHostConfirmed {
            per_phase [Absent, Open, InProgress, Blocked, Completed, Cancelled, Failed]
            on input ClassifyPublicConfirmationAdmission { completion_policy }
            guard "host_confirmed_requires_trusted_host" { completion_policy == WorkCompletionPolicy::HostConfirmed }
            update {}
            to Absent
            emit PublicConfirmationAdmissionClassified { admission: WorkPublicConfirmationAdmissionKind::DeniedRequiresTrustedHost }
        }

        transition ClassifyPublicConfirmationAdmissionPrincipalConfirmed {
            per_phase [Absent, Open, InProgress, Blocked, Completed, Cancelled, Failed]
            on input ClassifyPublicConfirmationAdmission { completion_policy }
            guard "principal_confirmed_requires_trusted_host" { completion_policy == WorkCompletionPolicy::PrincipalConfirmed }
            update {}
            to Absent
            emit PublicConfirmationAdmissionClassified { admission: WorkPublicConfirmationAdmissionKind::DeniedRequiresTrustedHost }
        }

        transition ClassifyPublicConfirmationAdmissionSupervisor {
            per_phase [Absent, Open, InProgress, Blocked, Completed, Cancelled, Failed]
            on input ClassifyPublicConfirmationAdmission { completion_policy }
            guard "supervisor_requires_trusted_host" { completion_policy == WorkCompletionPolicy::Supervisor }
            update {}
            to Absent
            emit PublicConfirmationAdmissionClassified { admission: WorkPublicConfirmationAdmissionKind::DeniedRequiresTrustedHost }
        }

        transition ClassifyPublicConfirmationAdmissionReviewerQuorum {
            per_phase [Absent, Open, InProgress, Blocked, Completed, Cancelled, Failed]
            on input ClassifyPublicConfirmationAdmission { completion_policy }
            guard "reviewer_quorum_requires_trusted_host" { completion_policy == WorkCompletionPolicy::ReviewerQuorum }
            update {}
            to Absent
            emit PublicConfirmationAdmissionClassified { admission: WorkPublicConfirmationAdmissionKind::DeniedRequiresTrustedHost }
        }

        // --- Completion-policy mutation admission classification ---
        //
        // This machine owns the immutability invariant "a work item's completion
        // policy is fixed at creation and cannot be changed by an update". The
        // shell extracts the requested completion policy as a pure typed
        // observation (variant plus supervisor owner key plus reviewer quorum
        // threshold) and drives this input over the item's recovered state; this
        // machine compares the requested policy — in full — against its own
        // machine-owned completion policy and emits the verdict. Admitted iff the
        // requested policy is identical to the current policy (the update is a
        // no-op on policy); Denied otherwise. The shell mirrors the verdict and
        // never decides. Phase-independent: self-loops over every phase so the
        // classification is total regardless of the authority's phase, and never
        // mutates lifecycle state.
        transition ClassifyCompletionPolicyMutationAdmissionUnchanged {
            per_phase [Absent, Open, InProgress, Blocked, Completed, Cancelled, Failed]
            on input ClassifyCompletionPolicyMutationAdmission {
                requested_completion_policy,
                requested_completion_supervisor_owner_key,
                requested_completion_reviewer_quorum_threshold
            }
            guard "completion_policy_unchanged" {
                requested_completion_policy == self.completion_policy
                    && requested_completion_supervisor_owner_key == self.completion_supervisor_owner_key
                    && requested_completion_reviewer_quorum_threshold == self.completion_reviewer_quorum_threshold
            }
            update {}
            to Absent
            emit CompletionPolicyMutationAdmissionClassified { admission: WorkCompletionPolicyMutationAdmissionKind::Admitted }
        }

        transition ClassifyCompletionPolicyMutationAdmissionChanged {
            per_phase [Absent, Open, InProgress, Blocked, Completed, Cancelled, Failed]
            on input ClassifyCompletionPolicyMutationAdmission {
                requested_completion_policy,
                requested_completion_supervisor_owner_key,
                requested_completion_reviewer_quorum_threshold
            }
            guard "completion_policy_changed" {
                requested_completion_policy != self.completion_policy
                    || requested_completion_supervisor_owner_key != self.completion_supervisor_owner_key
                    || requested_completion_reviewer_quorum_threshold != self.completion_reviewer_quorum_threshold
            }
            update {}
            to Absent
            emit CompletionPolicyMutationAdmissionClassified { admission: WorkCompletionPolicyMutationAdmissionKind::Denied }
        }

        // --- Trusted-path confirmation-admission classification ---
        //
        // This machine owns the eligibility "is this confirming principal +
        // supplied evidence kind admissible for this completion policy". The
        // goal-confirm shell extracts only pure typed observations and drives
        // this input; this machine decides the verdict and emits
        // ConfirmationAdmissionClassified. The shell mirrors the verdict
        // (Admitted -> stamp evidence, each Denied* -> the exact same InvalidInput
        // rejection), failing closed. The guards are mutually exclusive and total
        // via the confirmation_* helpers (which encode the EXACT per-policy check
        // precedence). Phase-independent: self-loops over every phase.

        transition ClassifyConfirmationAdmissionPrincipalRequired {
            per_phase [Absent, Open, InProgress, Blocked, Completed, Cancelled, Failed]
            on input ClassifyConfirmationAdmission {
                completion_policy,
                completion_supervisor_owner_key,
                requested_principal_owner_key,
                requested_principal_kind,
                supplied_evidence_kind
            }
            guard "principal_required" {
                confirmation_denies_principal_required(completion_policy, requested_principal_owner_key)
            }
            update {}
            to Absent
            emit ConfirmationAdmissionClassified { admission: WorkConfirmationAdmissionKind::DeniedPrincipalRequired }
        }

        transition ClassifyConfirmationAdmissionPrincipalKindMismatch {
            per_phase [Absent, Open, InProgress, Blocked, Completed, Cancelled, Failed]
            on input ClassifyConfirmationAdmission {
                completion_policy,
                completion_supervisor_owner_key,
                requested_principal_owner_key,
                requested_principal_kind,
                supplied_evidence_kind
            }
            guard "principal_kind_mismatch" {
                confirmation_denies_principal_kind_mismatch(completion_policy, requested_principal_owner_key, requested_principal_kind)
            }
            update {}
            to Absent
            emit ConfirmationAdmissionClassified { admission: WorkConfirmationAdmissionKind::DeniedPrincipalKindMismatch }
        }

        transition ClassifyConfirmationAdmissionSupervisorMismatch {
            per_phase [Absent, Open, InProgress, Blocked, Completed, Cancelled, Failed]
            on input ClassifyConfirmationAdmission {
                completion_policy,
                completion_supervisor_owner_key,
                requested_principal_owner_key,
                requested_principal_kind,
                supplied_evidence_kind
            }
            guard "supervisor_mismatch" {
                confirmation_denies_supervisor_mismatch(completion_policy, completion_supervisor_owner_key, requested_principal_owner_key)
            }
            update {}
            to Absent
            emit ConfirmationAdmissionClassified { admission: WorkConfirmationAdmissionKind::DeniedSupervisorMismatch }
        }

        transition ClassifyConfirmationAdmissionSelfAttestEmpty {
            per_phase [Absent, Open, InProgress, Blocked, Completed, Cancelled, Failed]
            on input ClassifyConfirmationAdmission {
                completion_policy,
                completion_supervisor_owner_key,
                requested_principal_owner_key,
                requested_principal_kind,
                supplied_evidence_kind
            }
            guard "self_attest_empty" {
                confirmation_denies_self_attest_empty(completion_policy, supplied_evidence_kind)
            }
            update {}
            to Absent
            emit ConfirmationAdmissionClassified { admission: WorkConfirmationAdmissionKind::DeniedSelfAttestEmptyEvidenceKind }
        }

        transition ClassifyConfirmationAdmissionEvidenceKind {
            per_phase [Absent, Open, InProgress, Blocked, Completed, Cancelled, Failed]
            on input ClassifyConfirmationAdmission {
                completion_policy,
                completion_supervisor_owner_key,
                requested_principal_owner_key,
                requested_principal_kind,
                supplied_evidence_kind
            }
            guard "evidence_kind_mismatch" {
                confirmation_denies_evidence_kind(completion_policy, completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind)
            }
            update {}
            to Absent
            emit ConfirmationAdmissionClassified { admission: WorkConfirmationAdmissionKind::DeniedEvidenceKind }
        }

        transition ClassifyConfirmationAdmissionAdmitted {
            per_phase [Absent, Open, InProgress, Blocked, Completed, Cancelled, Failed]
            on input ClassifyConfirmationAdmission {
                completion_policy,
                completion_supervisor_owner_key,
                requested_principal_owner_key,
                requested_principal_kind,
                supplied_evidence_kind
            }
            guard "confirmation_admissible" {
                confirmation_admits(completion_policy, completion_supervisor_owner_key, requested_principal_owner_key, requested_principal_kind, supplied_evidence_kind)
            }
            update {}
            to Absent
            emit ConfirmationAdmissionClassified { admission: WorkConfirmationAdmissionKind::Admitted }
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
