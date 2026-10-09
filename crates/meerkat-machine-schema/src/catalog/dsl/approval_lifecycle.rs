use meerkat_machine_dsl::machine;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum ApprovalLifecycleStatus {
    #[default]
    Pending,
    Approved,
    Denied,
    Expired,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum ApprovalLifecycleDecision {
    #[default]
    Approve,
    Deny,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum ApprovalLifecycleRejectionReason {
    #[default]
    NotFound,
    AlreadyExists,
    AlreadyDecided,
    Expired,
    InvalidDecision,
    EmptyAllowedDecisions,
    InvalidRestoredRecord,
    ReviewRetired,
    ReviewNotSatisfied,
    ReviewPending,
}

/// Process-local review attempt for one retained native operation candidate.
/// Attempts are never persisted or restored: a restart invalidates them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum ReviewAttemptStatus {
    #[default]
    Pending,
    Allowed,
    Denied,
    Escalated,
    Unavailable,
    Retired,
    Used,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum ReviewVerdict {
    #[default]
    Allow,
    Deny,
    Escalate,
}

/// Why the owner retired a retained review before its allow was spent.
/// `Abandoned` covers every dropped dispatch (caller interrupt, enclosing
/// deadline, other drops); the review owner never claims caller cancellation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum ReviewRetirementReason {
    #[default]
    ContextChanged,
    AuthorityChanged,
    DeadlineExpired,
    Abandoned,
}

machine! {
    machine ApprovalLifecycleMachine {
        version: 2,
        rust: "self" / "catalog::dsl::approval_lifecycle",

        state {
            lifecycle_phase: ApprovalLifecyclePhase,
            approval_ids: Set<String>,
            approval_statuses: Map<String, Enum<ApprovalLifecycleStatus>>,
            approval_approve_allowed: Map<String, bool>,
            approval_deny_allowed: Map<String, bool>,
            approval_has_expiry: Map<String, bool>,
            review_ids: Set<String>,
            review_statuses: Map<String, Enum<ReviewAttemptStatus>>,
            review_retirements: Map<String, Enum<ReviewRetirementReason>>,
        }

        init(Ready) {
            approval_ids = EmptySet,
            approval_statuses = EmptyMap,
            approval_approve_allowed = EmptyMap,
            approval_deny_allowed = EmptyMap,
            approval_has_expiry = EmptyMap,
            review_ids = EmptySet,
            review_statuses = EmptyMap,
            review_retirements = EmptyMap,
        }

        terminal []

        phase ApprovalLifecyclePhase {
            Ready,
        }

        input ApprovalLifecycleInput {
            CreateApproval {
                approval_id: String,
                approve_allowed: bool,
                deny_allowed: bool,
                has_expiry: bool,
            },
            RestoreApproval {
                approval_id: String,
                status: Enum<ApprovalLifecycleStatus>,
                approve_allowed: bool,
                deny_allowed: bool,
                has_expiry: bool,
                decision: Option<Enum<ApprovalLifecycleDecision>>,
            },
            ObserveApprovalExpiry {
                approval_id: String,
                expired: bool,
            },
            DecideApproval {
                approval_id: String,
                decision: Enum<ApprovalLifecycleDecision>,
            },
            BeginReview {
                review_id: String,
            },
            RecordReviewVerdict {
                review_id: String,
                verdict: Enum<ReviewVerdict>,
            },
            RecordReviewUnavailable {
                review_id: String,
            },
            RetireReview {
                review_id: String,
                reason: Enum<ReviewRetirementReason>,
            },
            ConsumeReviewForEntry {
                review_id: String,
            },
            ReleaseReview {
                review_id: String,
            },
        }

        effect ApprovalLifecycleEffect {
            ApprovalStatusResolved { approval_id: String, status: Enum<ApprovalLifecycleStatus> },
            ApprovalLifecycleRejected { approval_id: String, reason: Enum<ApprovalLifecycleRejectionReason> },
            ReviewStatusResolved { review_id: String, status: Enum<ReviewAttemptStatus> },
            ReviewLifecycleRejected { review_id: String, reason: Enum<ApprovalLifecycleRejectionReason> },
        }

        helper allowed_non_empty(approve_allowed: bool, deny_allowed: bool) -> bool {
            approve_allowed || deny_allowed
        }

        helper is_terminal_status(status: Enum<ApprovalLifecycleStatus>) -> bool {
            status == ApprovalLifecycleStatus::Approved
                || status == ApprovalLifecycleStatus::Denied
                || status == ApprovalLifecycleStatus::Cancelled
        }

        // Every guard reads the approval maps behind
        // approval_ids.contains(id); this pins the key sets those strict
        // reads rely on (#1811).
        invariant approval_maps_cover_exactly_the_registered_ids {
            self.approval_statuses.keys() == self.approval_ids
                && self.approval_approve_allowed.keys() == self.approval_ids
                && self.approval_deny_allowed.keys() == self.approval_ids
                && self.approval_has_expiry.keys() == self.approval_ids
        }

        disposition ApprovalStatusResolved => local seam SurfaceResultAlignment,
        disposition ApprovalLifecycleRejected => local seam SurfaceResultAlignment,
        disposition ReviewStatusResolved => local seam SurfaceResultAlignment,
        disposition ReviewLifecycleRejected => local seam SurfaceResultAlignment,

        // Review guards read review_statuses behind review_ids.contains(id),
        // so the key set must equal the registered attempts (#1811).
        invariant review_statuses_cover_exactly_the_review_ids {
            self.review_statuses.keys() == self.review_ids
        }

        invariant review_retirement_only_for_retired_attempts {
            for_all(id in self.review_retirements.keys(),
                self.review_statuses.get_cloned(id) == Some(ReviewAttemptStatus::Retired))
        }

        transition CreateRejectedEmptyAllowedDecisions {
            on input CreateApproval {
                approval_id,
                approve_allowed,
                deny_allowed,
                has_expiry
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && allowed_non_empty(approve_allowed, deny_allowed) == false
            }
            update {}
            to Ready
            emit ApprovalLifecycleRejected { approval_id: approval_id, reason: ApprovalLifecycleRejectionReason::EmptyAllowedDecisions }
        }

        transition CreateRejectedAlreadyExists {
            on input CreateApproval {
                approval_id,
                approve_allowed,
                deny_allowed,
                has_expiry
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && allowed_non_empty(approve_allowed, deny_allowed)
                && self.approval_ids.contains(approval_id)
            }
            update {}
            to Ready
            emit ApprovalLifecycleRejected { approval_id: approval_id, reason: ApprovalLifecycleRejectionReason::AlreadyExists }
        }

        transition CreatePending {
            on input CreateApproval {
                approval_id,
                approve_allowed,
                deny_allowed,
                has_expiry
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && allowed_non_empty(approve_allowed, deny_allowed)
                && self.approval_ids.contains(approval_id) == false
            }
            update {
                self.approval_ids.insert(approval_id);
                self.approval_statuses.insert(approval_id, ApprovalLifecycleStatus::Pending);
                self.approval_approve_allowed.insert(approval_id, approve_allowed);
                self.approval_deny_allowed.insert(approval_id, deny_allowed);
                self.approval_has_expiry.insert(approval_id, has_expiry);
            }
            to Ready
            emit ApprovalStatusResolved { approval_id: approval_id, status: ApprovalLifecycleStatus::Pending }
        }

        transition RestoreRejectedDuplicate {
            on input RestoreApproval {
                approval_id,
                status,
                approve_allowed,
                deny_allowed,
                has_expiry,
                decision
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.approval_ids.contains(approval_id)
            }
            update {}
            to Ready
            emit ApprovalLifecycleRejected { approval_id: approval_id, reason: ApprovalLifecycleRejectionReason::AlreadyExists }
        }

        transition RestoreRejectedEmptyAllowedDecisions {
            on input RestoreApproval {
                approval_id,
                status,
                approve_allowed,
                deny_allowed,
                has_expiry,
                decision
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.approval_ids.contains(approval_id) == false
                && allowed_non_empty(approve_allowed, deny_allowed) == false
            }
            update {}
            to Ready
            emit ApprovalLifecycleRejected { approval_id: approval_id, reason: ApprovalLifecycleRejectionReason::EmptyAllowedDecisions }
        }

        transition RestorePending {
            on input RestoreApproval {
                approval_id,
                status,
                approve_allowed,
                deny_allowed,
                has_expiry,
                decision
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.approval_ids.contains(approval_id) == false
                && allowed_non_empty(approve_allowed, deny_allowed)
                && status == ApprovalLifecycleStatus::Pending
                && decision == None
            }
            update {
                self.approval_ids.insert(approval_id);
                self.approval_statuses.insert(approval_id, ApprovalLifecycleStatus::Pending);
                self.approval_approve_allowed.insert(approval_id, approve_allowed);
                self.approval_deny_allowed.insert(approval_id, deny_allowed);
                self.approval_has_expiry.insert(approval_id, has_expiry);
            }
            to Ready
            emit ApprovalStatusResolved { approval_id: approval_id, status: ApprovalLifecycleStatus::Pending }
        }

        transition RestoreExpired {
            on input RestoreApproval {
                approval_id,
                status,
                approve_allowed,
                deny_allowed,
                has_expiry,
                decision
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.approval_ids.contains(approval_id) == false
                && allowed_non_empty(approve_allowed, deny_allowed)
                && status == ApprovalLifecycleStatus::Expired
                && decision == None
            }
            update {
                self.approval_ids.insert(approval_id);
                self.approval_statuses.insert(approval_id, ApprovalLifecycleStatus::Expired);
                self.approval_approve_allowed.insert(approval_id, approve_allowed);
                self.approval_deny_allowed.insert(approval_id, deny_allowed);
                self.approval_has_expiry.insert(approval_id, has_expiry);
            }
            to Ready
            emit ApprovalStatusResolved { approval_id: approval_id, status: ApprovalLifecycleStatus::Expired }
        }

        transition RestoreCancelled {
            on input RestoreApproval {
                approval_id,
                status,
                approve_allowed,
                deny_allowed,
                has_expiry,
                decision
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.approval_ids.contains(approval_id) == false
                && allowed_non_empty(approve_allowed, deny_allowed)
                && status == ApprovalLifecycleStatus::Cancelled
                && decision == None
            }
            update {
                self.approval_ids.insert(approval_id);
                self.approval_statuses.insert(approval_id, ApprovalLifecycleStatus::Cancelled);
                self.approval_approve_allowed.insert(approval_id, approve_allowed);
                self.approval_deny_allowed.insert(approval_id, deny_allowed);
                self.approval_has_expiry.insert(approval_id, has_expiry);
            }
            to Ready
            emit ApprovalStatusResolved { approval_id: approval_id, status: ApprovalLifecycleStatus::Cancelled }
        }

        transition RestoreApproved {
            on input RestoreApproval {
                approval_id,
                status,
                approve_allowed,
                deny_allowed,
                has_expiry,
                decision
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.approval_ids.contains(approval_id) == false
                && allowed_non_empty(approve_allowed, deny_allowed)
                && approve_allowed
                && status == ApprovalLifecycleStatus::Approved
                && decision == Some(ApprovalLifecycleDecision::Approve)
            }
            update {
                self.approval_ids.insert(approval_id);
                self.approval_statuses.insert(approval_id, ApprovalLifecycleStatus::Approved);
                self.approval_approve_allowed.insert(approval_id, approve_allowed);
                self.approval_deny_allowed.insert(approval_id, deny_allowed);
                self.approval_has_expiry.insert(approval_id, has_expiry);
            }
            to Ready
            emit ApprovalStatusResolved { approval_id: approval_id, status: ApprovalLifecycleStatus::Approved }
        }

        transition RestoreDenied {
            on input RestoreApproval {
                approval_id,
                status,
                approve_allowed,
                deny_allowed,
                has_expiry,
                decision
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.approval_ids.contains(approval_id) == false
                && allowed_non_empty(approve_allowed, deny_allowed)
                && deny_allowed
                && status == ApprovalLifecycleStatus::Denied
                && decision == Some(ApprovalLifecycleDecision::Deny)
            }
            update {
                self.approval_ids.insert(approval_id);
                self.approval_statuses.insert(approval_id, ApprovalLifecycleStatus::Denied);
                self.approval_approve_allowed.insert(approval_id, approve_allowed);
                self.approval_deny_allowed.insert(approval_id, deny_allowed);
                self.approval_has_expiry.insert(approval_id, has_expiry);
            }
            to Ready
            emit ApprovalStatusResolved { approval_id: approval_id, status: ApprovalLifecycleStatus::Denied }
        }

        transition RestoreRejectedInvalidRecord {
            on input RestoreApproval {
                approval_id,
                status,
                approve_allowed,
                deny_allowed,
                has_expiry,
                decision
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.approval_ids.contains(approval_id) == false
                && allowed_non_empty(approve_allowed, deny_allowed)
                && (
                    (status == ApprovalLifecycleStatus::Pending && decision != None)
                    || (status == ApprovalLifecycleStatus::Expired && decision != None)
                    || (status == ApprovalLifecycleStatus::Cancelled && decision != None)
                    || (status == ApprovalLifecycleStatus::Approved && (approve_allowed == false || decision != Some(ApprovalLifecycleDecision::Approve)))
                    || (status == ApprovalLifecycleStatus::Denied && (deny_allowed == false || decision != Some(ApprovalLifecycleDecision::Deny)))
                )
            }
            update {}
            to Ready
            emit ApprovalLifecycleRejected { approval_id: approval_id, reason: ApprovalLifecycleRejectionReason::InvalidRestoredRecord }
        }

        transition ObserveExpiryRejectedMissing {
            on input ObserveApprovalExpiry { approval_id, expired }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.approval_ids.contains(approval_id) == false
            }
            update {}
            to Ready
            emit ApprovalLifecycleRejected { approval_id: approval_id, reason: ApprovalLifecycleRejectionReason::NotFound }
        }

        transition ObserveExpiryExpiresPending {
            on input ObserveApprovalExpiry { approval_id, expired }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.approval_ids.contains(approval_id)
                && self.approval_statuses.get_cloned(approval_id).get("value") == ApprovalLifecycleStatus::Pending
                && self.approval_has_expiry.get_cloned(approval_id).get("value")
                && expired
            }
            update {
                self.approval_statuses.insert(approval_id, ApprovalLifecycleStatus::Expired);
            }
            to Ready
            emit ApprovalStatusResolved { approval_id: approval_id, status: ApprovalLifecycleStatus::Expired }
        }

        transition ObserveExpiryPendingNoop {
            on input ObserveApprovalExpiry { approval_id, expired }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.approval_ids.contains(approval_id)
                && self.approval_statuses.get_cloned(approval_id).get("value") == ApprovalLifecycleStatus::Pending
                && (self.approval_has_expiry.get_cloned(approval_id).get("value") == false || expired == false)
            }
            update {}
            to Ready
            emit ApprovalStatusResolved { approval_id: approval_id, status: ApprovalLifecycleStatus::Pending }
        }

        transition ObserveExpiryApprovedNoop {
            on input ObserveApprovalExpiry { approval_id, expired }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.approval_ids.contains(approval_id)
                && self.approval_statuses.get_cloned(approval_id).get("value") == ApprovalLifecycleStatus::Approved
            }
            update {}
            to Ready
            emit ApprovalStatusResolved { approval_id: approval_id, status: ApprovalLifecycleStatus::Approved }
        }

        transition ObserveExpiryDeniedNoop {
            on input ObserveApprovalExpiry { approval_id, expired }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.approval_ids.contains(approval_id)
                && self.approval_statuses.get_cloned(approval_id).get("value") == ApprovalLifecycleStatus::Denied
            }
            update {}
            to Ready
            emit ApprovalStatusResolved { approval_id: approval_id, status: ApprovalLifecycleStatus::Denied }
        }

        transition ObserveExpiryExpiredNoop {
            on input ObserveApprovalExpiry { approval_id, expired }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.approval_ids.contains(approval_id)
                && self.approval_statuses.get_cloned(approval_id).get("value") == ApprovalLifecycleStatus::Expired
            }
            update {}
            to Ready
            emit ApprovalStatusResolved { approval_id: approval_id, status: ApprovalLifecycleStatus::Expired }
        }

        transition ObserveExpiryCancelledNoop {
            on input ObserveApprovalExpiry { approval_id, expired }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.approval_ids.contains(approval_id)
                && self.approval_statuses.get_cloned(approval_id).get("value") == ApprovalLifecycleStatus::Cancelled
            }
            update {}
            to Ready
            emit ApprovalStatusResolved { approval_id: approval_id, status: ApprovalLifecycleStatus::Cancelled }
        }

        transition DecideRejectedMissing {
            on input DecideApproval { approval_id, decision }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.approval_ids.contains(approval_id) == false
            }
            update {}
            to Ready
            emit ApprovalLifecycleRejected { approval_id: approval_id, reason: ApprovalLifecycleRejectionReason::NotFound }
        }

        transition DecideRejectedExpired {
            on input DecideApproval { approval_id, decision }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.approval_ids.contains(approval_id)
                && self.approval_statuses.get_cloned(approval_id).get("value") == ApprovalLifecycleStatus::Expired
            }
            update {}
            to Ready
            emit ApprovalLifecycleRejected { approval_id: approval_id, reason: ApprovalLifecycleRejectionReason::Expired }
        }

        transition DecideRejectedAlreadyDecided {
            on input DecideApproval { approval_id, decision }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.approval_ids.contains(approval_id)
                && is_terminal_status(self.approval_statuses.get_cloned(approval_id).get("value"))
            }
            update {}
            to Ready
            emit ApprovalLifecycleRejected { approval_id: approval_id, reason: ApprovalLifecycleRejectionReason::AlreadyDecided }
        }

        transition DecideRejectedApproveNotAllowed {
            on input DecideApproval { approval_id, decision }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.approval_ids.contains(approval_id)
                && self.approval_statuses.get_cloned(approval_id).get("value") == ApprovalLifecycleStatus::Pending
                && decision == ApprovalLifecycleDecision::Approve
                && self.approval_approve_allowed.get_cloned(approval_id).get("value") == false
            }
            update {}
            to Ready
            emit ApprovalLifecycleRejected { approval_id: approval_id, reason: ApprovalLifecycleRejectionReason::InvalidDecision }
        }

        transition DecideRejectedDenyNotAllowed {
            on input DecideApproval { approval_id, decision }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.approval_ids.contains(approval_id)
                && self.approval_statuses.get_cloned(approval_id).get("value") == ApprovalLifecycleStatus::Pending
                && decision == ApprovalLifecycleDecision::Deny
                && self.approval_deny_allowed.get_cloned(approval_id).get("value") == false
            }
            update {}
            to Ready
            emit ApprovalLifecycleRejected { approval_id: approval_id, reason: ApprovalLifecycleRejectionReason::InvalidDecision }
        }

        transition DecideApprove {
            on input DecideApproval { approval_id, decision }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.approval_ids.contains(approval_id)
                && self.approval_statuses.get_cloned(approval_id).get("value") == ApprovalLifecycleStatus::Pending
                && decision == ApprovalLifecycleDecision::Approve
                && self.approval_approve_allowed.get_cloned(approval_id).get("value")
            }
            update {
                self.approval_statuses.insert(approval_id, ApprovalLifecycleStatus::Approved);
            }
            to Ready
            emit ApprovalStatusResolved { approval_id: approval_id, status: ApprovalLifecycleStatus::Approved }
        }

        transition DecideDeny {
            on input DecideApproval { approval_id, decision }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.approval_ids.contains(approval_id)
                && self.approval_statuses.get_cloned(approval_id).get("value") == ApprovalLifecycleStatus::Pending
                && decision == ApprovalLifecycleDecision::Deny
                && self.approval_deny_allowed.get_cloned(approval_id).get("value")
            }
            update {
                self.approval_statuses.insert(approval_id, ApprovalLifecycleStatus::Denied);
            }
            to Ready
            emit ApprovalStatusResolved { approval_id: approval_id, status: ApprovalLifecycleStatus::Denied }
        }

        transition BeginReviewRejectedDuplicate {
            on input BeginReview { review_id }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.review_ids.contains(review_id)
            }
            update {}
            to Ready
            emit ReviewLifecycleRejected { review_id: review_id, reason: ApprovalLifecycleRejectionReason::AlreadyExists }
        }

        transition BeginReviewPending {
            on input BeginReview { review_id }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.review_ids.contains(review_id) == false
            }
            update {
                self.review_ids.insert(review_id);
                self.review_statuses.insert(review_id, ReviewAttemptStatus::Pending);
            }
            to Ready
            emit ReviewStatusResolved { review_id: review_id, status: ReviewAttemptStatus::Pending }
        }

        transition RecordReviewVerdictRejectedMissing {
            on input RecordReviewVerdict { review_id, verdict }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.review_ids.contains(review_id) == false
            }
            update {}
            to Ready
            emit ReviewLifecycleRejected { review_id: review_id, reason: ApprovalLifecycleRejectionReason::NotFound }
        }

        transition RecordReviewVerdictRejectedRetired {
            on input RecordReviewVerdict { review_id, verdict }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.review_ids.contains(review_id)
                && self.review_statuses.get_cloned(review_id).get("value") == ReviewAttemptStatus::Retired
            }
            update {}
            to Ready
            emit ReviewLifecycleRejected { review_id: review_id, reason: ApprovalLifecycleRejectionReason::ReviewRetired }
        }

        transition RecordReviewVerdictRejectedSettled {
            on input RecordReviewVerdict { review_id, verdict }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.review_ids.contains(review_id)
                && self.review_statuses.get_cloned(review_id).get("value") != ReviewAttemptStatus::Pending
                && self.review_statuses.get_cloned(review_id).get("value") != ReviewAttemptStatus::Retired
            }
            update {}
            to Ready
            emit ReviewLifecycleRejected { review_id: review_id, reason: ApprovalLifecycleRejectionReason::AlreadyDecided }
        }

        transition RecordReviewVerdictAllowed {
            on input RecordReviewVerdict { review_id, verdict }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.review_ids.contains(review_id)
                && self.review_statuses.get_cloned(review_id).get("value") == ReviewAttemptStatus::Pending
                && verdict == ReviewVerdict::Allow
            }
            update {
                self.review_statuses.insert(review_id, ReviewAttemptStatus::Allowed);
            }
            to Ready
            emit ReviewStatusResolved { review_id: review_id, status: ReviewAttemptStatus::Allowed }
        }

        transition RecordReviewVerdictDenied {
            on input RecordReviewVerdict { review_id, verdict }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.review_ids.contains(review_id)
                && self.review_statuses.get_cloned(review_id).get("value") == ReviewAttemptStatus::Pending
                && verdict == ReviewVerdict::Deny
            }
            update {
                self.review_statuses.insert(review_id, ReviewAttemptStatus::Denied);
            }
            to Ready
            emit ReviewStatusResolved { review_id: review_id, status: ReviewAttemptStatus::Denied }
        }

        transition RecordReviewVerdictEscalated {
            on input RecordReviewVerdict { review_id, verdict }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.review_ids.contains(review_id)
                && self.review_statuses.get_cloned(review_id).get("value") == ReviewAttemptStatus::Pending
                && verdict == ReviewVerdict::Escalate
            }
            update {
                self.review_statuses.insert(review_id, ReviewAttemptStatus::Escalated);
            }
            to Ready
            emit ReviewStatusResolved { review_id: review_id, status: ReviewAttemptStatus::Escalated }
        }

        transition RecordReviewUnavailableRejectedMissing {
            on input RecordReviewUnavailable { review_id }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.review_ids.contains(review_id) == false
            }
            update {}
            to Ready
            emit ReviewLifecycleRejected { review_id: review_id, reason: ApprovalLifecycleRejectionReason::NotFound }
        }

        transition RecordReviewUnavailableRejectedRetired {
            on input RecordReviewUnavailable { review_id }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.review_ids.contains(review_id)
                && self.review_statuses.get_cloned(review_id).get("value") == ReviewAttemptStatus::Retired
            }
            update {}
            to Ready
            emit ReviewLifecycleRejected { review_id: review_id, reason: ApprovalLifecycleRejectionReason::ReviewRetired }
        }

        transition RecordReviewUnavailableRejectedSettled {
            on input RecordReviewUnavailable { review_id }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.review_ids.contains(review_id)
                && self.review_statuses.get_cloned(review_id).get("value") != ReviewAttemptStatus::Pending
                && self.review_statuses.get_cloned(review_id).get("value") != ReviewAttemptStatus::Retired
            }
            update {}
            to Ready
            emit ReviewLifecycleRejected { review_id: review_id, reason: ApprovalLifecycleRejectionReason::AlreadyDecided }
        }

        transition RecordReviewUnavailable {
            on input RecordReviewUnavailable { review_id }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.review_ids.contains(review_id)
                && self.review_statuses.get_cloned(review_id).get("value") == ReviewAttemptStatus::Pending
            }
            update {
                self.review_statuses.insert(review_id, ReviewAttemptStatus::Unavailable);
            }
            to Ready
            emit ReviewStatusResolved { review_id: review_id, status: ReviewAttemptStatus::Unavailable }
        }

        transition RetireReviewRejectedMissing {
            on input RetireReview { review_id, reason }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.review_ids.contains(review_id) == false
            }
            update {}
            to Ready
            emit ReviewLifecycleRejected { review_id: review_id, reason: ApprovalLifecycleRejectionReason::NotFound }
        }

        transition RetireReviewRejectedRetired {
            on input RetireReview { review_id, reason }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.review_ids.contains(review_id)
                && self.review_statuses.get_cloned(review_id).get("value") == ReviewAttemptStatus::Retired
            }
            update {}
            to Ready
            emit ReviewLifecycleRejected { review_id: review_id, reason: ApprovalLifecycleRejectionReason::ReviewRetired }
        }

        transition RetireReviewRejectedSettled {
            on input RetireReview { review_id, reason }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.review_ids.contains(review_id)
                && self.review_statuses.get_cloned(review_id).get("value") != ReviewAttemptStatus::Pending
                && self.review_statuses.get_cloned(review_id).get("value") != ReviewAttemptStatus::Allowed
                && self.review_statuses.get_cloned(review_id).get("value") != ReviewAttemptStatus::Retired
            }
            update {}
            to Ready
            emit ReviewLifecycleRejected { review_id: review_id, reason: ApprovalLifecycleRejectionReason::AlreadyDecided }
        }

        transition RetireReview {
            on input RetireReview { review_id, reason }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.review_ids.contains(review_id)
                && (self.review_statuses.get_cloned(review_id).get("value") == ReviewAttemptStatus::Pending
                    || self.review_statuses.get_cloned(review_id).get("value") == ReviewAttemptStatus::Allowed)
            }
            update {
                self.review_statuses.insert(review_id, ReviewAttemptStatus::Retired);
                self.review_retirements.insert(review_id, reason);
            }
            to Ready
            emit ReviewStatusResolved { review_id: review_id, status: ReviewAttemptStatus::Retired }
        }

        transition ConsumeReviewRejectedMissing {
            on input ConsumeReviewForEntry { review_id }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.review_ids.contains(review_id) == false
            }
            update {}
            to Ready
            emit ReviewLifecycleRejected { review_id: review_id, reason: ApprovalLifecycleRejectionReason::NotFound }
        }

        transition ConsumeReviewRejectedRetired {
            on input ConsumeReviewForEntry { review_id }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.review_ids.contains(review_id)
                && self.review_statuses.get_cloned(review_id).get("value") == ReviewAttemptStatus::Retired
            }
            update {}
            to Ready
            emit ReviewLifecycleRejected { review_id: review_id, reason: ApprovalLifecycleRejectionReason::ReviewRetired }
        }

        transition ConsumeReviewRejectedUsed {
            on input ConsumeReviewForEntry { review_id }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.review_ids.contains(review_id)
                && self.review_statuses.get_cloned(review_id).get("value") == ReviewAttemptStatus::Used
            }
            update {}
            to Ready
            emit ReviewLifecycleRejected { review_id: review_id, reason: ApprovalLifecycleRejectionReason::AlreadyDecided }
        }

        transition ConsumeReviewRejectedNotSatisfied {
            on input ConsumeReviewForEntry { review_id }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.review_ids.contains(review_id)
                && (self.review_statuses.get_cloned(review_id).get("value") == ReviewAttemptStatus::Pending
                    || self.review_statuses.get_cloned(review_id).get("value") == ReviewAttemptStatus::Denied
                    || self.review_statuses.get_cloned(review_id).get("value") == ReviewAttemptStatus::Escalated
                    || self.review_statuses.get_cloned(review_id).get("value") == ReviewAttemptStatus::Unavailable)
            }
            update {}
            to Ready
            emit ReviewLifecycleRejected { review_id: review_id, reason: ApprovalLifecycleRejectionReason::ReviewNotSatisfied }
        }

        transition ConsumeReviewForEntry {
            on input ConsumeReviewForEntry { review_id }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.review_ids.contains(review_id)
                && self.review_statuses.get_cloned(review_id).get("value") == ReviewAttemptStatus::Allowed
            }
            update {
                self.review_statuses.insert(review_id, ReviewAttemptStatus::Used);
            }
            to Ready
            emit ReviewStatusResolved { review_id: review_id, status: ReviewAttemptStatus::Used }
        }

        transition ReleaseReviewRejectedMissing {
            on input ReleaseReview { review_id }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.review_ids.contains(review_id) == false
            }
            update {}
            to Ready
            emit ReviewLifecycleRejected { review_id: review_id, reason: ApprovalLifecycleRejectionReason::NotFound }
        }

        transition ReleaseReviewRejectedPending {
            on input ReleaseReview { review_id }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.review_ids.contains(review_id)
                && self.review_statuses.get_cloned(review_id).get("value") == ReviewAttemptStatus::Pending
            }
            update {}
            to Ready
            emit ReviewLifecycleRejected { review_id: review_id, reason: ApprovalLifecycleRejectionReason::ReviewPending }
        }

        transition ReleaseReviewAllowed {
            on input ReleaseReview { review_id }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.review_ids.contains(review_id)
                && self.review_statuses.get_cloned(review_id).get("value") == ReviewAttemptStatus::Allowed
            }
            update {
                self.review_ids.remove(review_id);
                self.review_statuses.remove(review_id);
                self.review_retirements.remove(review_id);
            }
            to Ready
            emit ReviewStatusResolved { review_id: review_id, status: ReviewAttemptStatus::Allowed }
        }

        transition ReleaseReviewDenied {
            on input ReleaseReview { review_id }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.review_ids.contains(review_id)
                && self.review_statuses.get_cloned(review_id).get("value") == ReviewAttemptStatus::Denied
            }
            update {
                self.review_ids.remove(review_id);
                self.review_statuses.remove(review_id);
                self.review_retirements.remove(review_id);
            }
            to Ready
            emit ReviewStatusResolved { review_id: review_id, status: ReviewAttemptStatus::Denied }
        }

        transition ReleaseReviewEscalated {
            on input ReleaseReview { review_id }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.review_ids.contains(review_id)
                && self.review_statuses.get_cloned(review_id).get("value") == ReviewAttemptStatus::Escalated
            }
            update {
                self.review_ids.remove(review_id);
                self.review_statuses.remove(review_id);
                self.review_retirements.remove(review_id);
            }
            to Ready
            emit ReviewStatusResolved { review_id: review_id, status: ReviewAttemptStatus::Escalated }
        }

        transition ReleaseReviewUnavailable {
            on input ReleaseReview { review_id }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.review_ids.contains(review_id)
                && self.review_statuses.get_cloned(review_id).get("value") == ReviewAttemptStatus::Unavailable
            }
            update {
                self.review_ids.remove(review_id);
                self.review_statuses.remove(review_id);
                self.review_retirements.remove(review_id);
            }
            to Ready
            emit ReviewStatusResolved { review_id: review_id, status: ReviewAttemptStatus::Unavailable }
        }

        transition ReleaseReviewRetired {
            on input ReleaseReview { review_id }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.review_ids.contains(review_id)
                && self.review_statuses.get_cloned(review_id).get("value") == ReviewAttemptStatus::Retired
            }
            update {
                self.review_ids.remove(review_id);
                self.review_statuses.remove(review_id);
                self.review_retirements.remove(review_id);
            }
            to Ready
            emit ReviewStatusResolved { review_id: review_id, status: ReviewAttemptStatus::Retired }
        }

        transition ReleaseReviewUsed {
            on input ReleaseReview { review_id }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.review_ids.contains(review_id)
                && self.review_statuses.get_cloned(review_id).get("value") == ReviewAttemptStatus::Used
            }
            update {
                self.review_ids.remove(review_id);
                self.review_statuses.remove(review_id);
                self.review_retirements.remove(review_id);
            }
            to Ready
            emit ReviewStatusResolved { review_id: review_id, status: ReviewAttemptStatus::Used }
        }
    }
}
