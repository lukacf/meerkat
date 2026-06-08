use meerkat_machine_dsl::machine;

use super::OptionValueExt;

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
}

machine! {
    machine ApprovalLifecycleMachine {
        version: 1,
        rust: "self" / "catalog::dsl::approval_lifecycle",

        state {
            lifecycle_phase: ApprovalLifecyclePhase,
            approval_ids: Set<String>,
            approval_statuses: Map<String, Enum<ApprovalLifecycleStatus>>,
            approval_approve_allowed: Map<String, bool>,
            approval_deny_allowed: Map<String, bool>,
            approval_has_expiry: Map<String, bool>,
        }

        init(Ready) {
            approval_ids = EmptySet,
            approval_statuses = EmptyMap,
            approval_approve_allowed = EmptyMap,
            approval_deny_allowed = EmptyMap,
            approval_has_expiry = EmptyMap,
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
        }

        effect ApprovalLifecycleEffect {
            ApprovalStatusResolved { approval_id: String, status: Enum<ApprovalLifecycleStatus> },
            ApprovalLifecycleRejected { approval_id: String, reason: Enum<ApprovalLifecycleRejectionReason> },
        }

        helper allowed_non_empty(approve_allowed: bool, deny_allowed: bool) -> bool {
            approve_allowed || deny_allowed
        }

        helper is_terminal_status(status: Enum<ApprovalLifecycleStatus>) -> bool {
            status == ApprovalLifecycleStatus::Approved
                || status == ApprovalLifecycleStatus::Denied
                || status == ApprovalLifecycleStatus::Cancelled
        }

        disposition ApprovalStatusResolved => local seam SurfaceResultAlignment,
        disposition ApprovalLifecycleRejected => local seam SurfaceResultAlignment,

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
    }
}
