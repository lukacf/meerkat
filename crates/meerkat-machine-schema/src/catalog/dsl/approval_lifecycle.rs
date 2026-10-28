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
    ReviewRetired,
    ReviewNotSatisfied,
    ReviewPending,
    /// A consent approval accepts only an authenticated human decision.
    AuthenticationRequired,
    /// A generic input on a consent approval or its linked attempt: only the
    /// consent inputs move a linked pair.
    ConsentLinked,
    /// The approval and the attempt are not linked to each other.
    ConsentMismatch,
    /// The consent was already spent.
    AlreadyConsumed,
    /// An unlinked consent settlement applies to consent approvals only.
    NotConsent,
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
    /// The admitting run closed before the consent was spent.
    RunEnded,
}

/// The entered effect of one spent (`Used`) attempt: `Open` while its
/// admitting dispatch runs, `Closing` once that dispatch ended while admitted
/// steps were still in flight. A closing effect admits no step, and its last
/// step's finish releases the attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum EnteredEffectPhase {
    #[default]
    Open,
    Closing,
}

machine! {
    machine ApprovalLifecycleMachine {
        version: 6,
        rust: "self" / "catalog::dsl::approval_lifecycle",

        state {
            lifecycle_phase: ApprovalLifecyclePhase,
            approval_ids: Set<String>,
            approval_statuses: Map<String, Enum<ApprovalLifecycleStatus>>,
            approval_approve_allowed: Map<String, bool>,
            approval_deny_allowed: Map<String, bool>,
            approval_has_expiry: Map<String, bool>,
            // Approval issued for one retained R3 candidate (human consent).
            approval_consent: Map<String, bool>,
            // A consent approval decided by an authenticated human.
            approval_consent_authenticated: Map<String, bool>,
            // The one-use fact: the consent was spent at an entry.
            approval_consent_consumed: Map<String, bool>,
            // The live link between a consent approval and its human attempt,
            // in both directions. Present only while that attempt can still
            // be decided or spent.
            approval_consent_review: Map<String, String>,
            review_consent_approval: Map<String, String>,
            review_ids: Set<String>,
            review_statuses: Map<String, Enum<ReviewAttemptStatus>>,
            review_retirements: Map<String, Enum<ReviewRetirementReason>>,
            // The entered effect of each spent (`Used`) attempt, from its one
            // spend until its release: its phase, and its admitted steps
            // still in flight.
            review_entered_phase: Map<String, Enum<EnteredEffectPhase>>,
            review_entered_steps: Map<String, u64>,
        }

        init(Ready) {
            approval_ids = EmptySet,
            approval_statuses = EmptyMap,
            approval_approve_allowed = EmptyMap,
            approval_deny_allowed = EmptyMap,
            approval_has_expiry = EmptyMap,
            approval_consent = EmptyMap,
            approval_consent_authenticated = EmptyMap,
            approval_consent_consumed = EmptyMap,
            approval_consent_review = EmptyMap,
            review_consent_approval = EmptyMap,
            review_ids = EmptySet,
            review_statuses = EmptyMap,
            review_retirements = EmptyMap,
            review_entered_phase = EmptyMap,
            review_entered_steps = EmptyMap,
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
            CreateConsentApproval {
                approval_id: String,
                review_id: String,
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
                consent: bool,
                authenticated: bool,
                consumed: bool,
            },
            ReconcileApproval {
                approval_id: String,
                status: Enum<ApprovalLifecycleStatus>,
                approve_allowed: bool,
                deny_allowed: bool,
                has_expiry: bool,
                decision: Option<Enum<ApprovalLifecycleDecision>>,
                consent: bool,
                authenticated: bool,
                consumed: bool,
            },
            ObserveApprovalExpiry {
                approval_id: String,
                expired: bool,
            },
            DecideApproval {
                approval_id: String,
                decision: Enum<ApprovalLifecycleDecision>,
            },
            DecideConsent {
                approval_id: String,
                review_id: String,
                decision: Enum<ApprovalLifecycleDecision>,
                authenticated: bool,
            },
            ConsumeConsentForEntry {
                approval_id: String,
                review_id: String,
            },
            WithdrawConsent {
                approval_id: String,
                review_id: String,
                reason: Enum<ReviewRetirementReason>,
            },
            // Settle the status of a consent row that no attempt is linked to
            // any more, without any candidate: after this owner retired its
            // custody and still owes the withdrawal's record against the
            // reconciled row, or after its issuing owner is proven lost. The
            // durable withdrawal cause is recorded with the row, not here.
            SettleUnlinkedConsent {
                approval_id: String,
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
            // One later step of an entered effect begins: admitted only
            // against the attempt its entry spent, while that effect is open.
            AdmitEnteredStep {
                review_id: String,
            },
            // One admitted step of an entered effect finished.
            FinishEnteredStep {
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

        // A stored row's status agrees with its decision audit and allowed
        // decisions, and its consent history is consistent: only a consent
        // row is authenticated; a consent row is authenticated exactly when
        // it was decided; only an approved, authenticated consent is spent.
        helper restored_record_valid(
            status: Enum<ApprovalLifecycleStatus>,
            approve_allowed: bool,
            deny_allowed: bool,
            decision: Option<Enum<ApprovalLifecycleDecision>>,
            consent: bool,
            authenticated: bool,
            consumed: bool
        ) -> bool {
            allowed_non_empty(approve_allowed, deny_allowed)
                && (
                    ((status == ApprovalLifecycleStatus::Pending
                        || status == ApprovalLifecycleStatus::Expired
                        || status == ApprovalLifecycleStatus::Cancelled)
                        && decision == None)
                    || (status == ApprovalLifecycleStatus::Approved
                        && approve_allowed
                        && decision == Some(ApprovalLifecycleDecision::Approve))
                    || (status == ApprovalLifecycleStatus::Denied
                        && deny_allowed
                        && decision == Some(ApprovalLifecycleDecision::Deny))
                )
                && (authenticated == false || consent)
                && (consent == false
                    || ((status == ApprovalLifecycleStatus::Approved
                        || status == ApprovalLifecycleStatus::Denied)
                        && authenticated)
                    || (status != ApprovalLifecycleStatus::Approved
                        && status != ApprovalLifecycleStatus::Denied
                        && authenticated == false))
                && (consumed == false
                    || (authenticated && status == ApprovalLifecycleStatus::Approved))
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

        // Decide guards read the consent maps behind approval_ids.contains(id).
        invariant approval_consent_covers_exactly_the_approval_ids {
            self.approval_consent.keys() == self.approval_ids
                && self.approval_consent_authenticated.keys() == self.approval_ids
                && self.approval_consent_consumed.keys() == self.approval_ids
        }

        invariant review_retirement_only_for_retired_attempts {
            for_all(id in self.review_retirements.keys(),
                self.review_statuses.get_cloned(id) == Some(ReviewAttemptStatus::Retired))
        }

        // The link is a bijection between consent approvals and live
        // (Pending or Allowed) attempts.
        invariant consent_link_is_a_bijection_over_live_attempts {
            for_all(id in self.approval_consent_review.keys(),
                self.approval_consent.get_cloned(id) == Some(true)
                    && self.review_consent_approval.get_cloned(self.approval_consent_review.get_cloned(id).get("value")) == Some(id))
                && for_all(review in self.review_consent_approval.keys(),
                    self.approval_consent_review.get_cloned(self.review_consent_approval.get_cloned(review).get("value")) == Some(review)
                        && (self.review_statuses.get_cloned(review) == Some(ReviewAttemptStatus::Pending)
                            || self.review_statuses.get_cloned(review) == Some(ReviewAttemptStatus::Allowed)))
        }

        // A linked attempt is allowed exactly when its approval was approved;
        // it is never Used (the spend unlinks it).
        invariant linked_attempt_allowed_iff_approved {
            for_all(id in self.approval_consent_review.keys(),
                (self.approval_statuses.get_cloned(id) == Some(ApprovalLifecycleStatus::Approved)
                    && self.review_statuses.get_cloned(self.approval_consent_review.get_cloned(id).get("value")) == Some(ReviewAttemptStatus::Allowed))
                || (self.approval_statuses.get_cloned(id) != Some(ApprovalLifecycleStatus::Approved)
                    && self.review_statuses.get_cloned(self.approval_consent_review.get_cloned(id).get("value")) == Some(ReviewAttemptStatus::Pending)))
        }

        // Spent consent was approved by an authenticated decision, and its
        // attempt is no longer linked.
        invariant consumed_consent_is_approved_authenticated_and_unlinked {
            for_all(id in self.approval_consent_consumed.keys(),
                self.approval_consent_consumed.get_cloned(id) == Some(false)
                    || (self.approval_statuses.get_cloned(id) == Some(ApprovalLifecycleStatus::Approved)
                        && self.approval_consent_authenticated.get_cloned(id) == Some(true)
                        && self.approval_consent_review.contains_key(id) == false))
        }

        // The entered state exists exactly for the spent (`Used`) attempts:
        // a spend opens it, and only the attempt's release ends it.
        invariant entered_state_covers_exactly_the_used_attempts {
            self.review_entered_steps.keys() == self.review_entered_phase.keys()
                && for_all(id in self.review_entered_phase.keys(),
                    self.review_statuses.get_cloned(id) == Some(ReviewAttemptStatus::Used))
                && for_all(id in self.review_statuses.keys(),
                    self.review_statuses.get_cloned(id) != Some(ReviewAttemptStatus::Used)
                        || self.review_entered_phase.contains_key(id))
        }

        // A closing effect still has an admitted step in flight: its last
        // step's finish released it.
        invariant closing_entered_effect_has_a_step_in_flight {
            for_all(id in self.review_entered_phase.keys(),
                self.review_entered_phase.get_cloned(id) != Some(EnteredEffectPhase::Closing)
                    || self.review_entered_steps.get_cloned(id) != Some(0))
        }

        transition CreateRejectedEmptyAllowedDecisions {
            on input CreateApproval { approval_id, approve_allowed, deny_allowed, has_expiry }
            guard {
                self.lifecycle_phase == Phase::Ready
                && allowed_non_empty(approve_allowed, deny_allowed) == false
            }
            update {}
            to Ready
            emit ApprovalLifecycleRejected { approval_id: approval_id, reason: ApprovalLifecycleRejectionReason::EmptyAllowedDecisions }
        }

        transition CreateRejectedAlreadyExists {
            on input CreateApproval { approval_id, approve_allowed, deny_allowed, has_expiry }
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
            on input CreateApproval { approval_id, approve_allowed, deny_allowed, has_expiry }
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
                self.approval_consent.insert(approval_id, false);
                self.approval_consent_authenticated.insert(approval_id, false);
                self.approval_consent_consumed.insert(approval_id, false);
            }
            to Ready
            emit ApprovalStatusResolved { approval_id: approval_id, status: ApprovalLifecycleStatus::Pending }
        }

        transition CreateConsentRejectedEmptyAllowedDecisions {
            on input CreateConsentApproval { approval_id, review_id, approve_allowed, deny_allowed, has_expiry }
            guard {
                self.lifecycle_phase == Phase::Ready
                && allowed_non_empty(approve_allowed, deny_allowed) == false
            }
            update {}
            to Ready
            emit ApprovalLifecycleRejected { approval_id: approval_id, reason: ApprovalLifecycleRejectionReason::EmptyAllowedDecisions }
        }

        transition CreateConsentRejectedAlreadyExists {
            on input CreateConsentApproval { approval_id, review_id, approve_allowed, deny_allowed, has_expiry }
            guard {
                self.lifecycle_phase == Phase::Ready
                && allowed_non_empty(approve_allowed, deny_allowed)
                && (self.approval_ids.contains(approval_id) || self.review_ids.contains(review_id))
            }
            update {}
            to Ready
            emit ApprovalLifecycleRejected { approval_id: approval_id, reason: ApprovalLifecycleRejectionReason::AlreadyExists }
        }

        // One consent approval and its human attempt, created and linked atomically.
        transition CreateConsentPending {
            on input CreateConsentApproval { approval_id, review_id, approve_allowed, deny_allowed, has_expiry }
            guard {
                self.lifecycle_phase == Phase::Ready
                && allowed_non_empty(approve_allowed, deny_allowed)
                && self.approval_ids.contains(approval_id) == false
                && self.review_ids.contains(review_id) == false
            }
            update {
                self.approval_ids.insert(approval_id);
                self.approval_statuses.insert(approval_id, ApprovalLifecycleStatus::Pending);
                self.approval_approve_allowed.insert(approval_id, approve_allowed);
                self.approval_deny_allowed.insert(approval_id, deny_allowed);
                self.approval_has_expiry.insert(approval_id, has_expiry);
                self.approval_consent.insert(approval_id, true);
                self.approval_consent_authenticated.insert(approval_id, false);
                self.approval_consent_consumed.insert(approval_id, false);
                self.review_ids.insert(review_id);
                self.review_statuses.insert(review_id, ReviewAttemptStatus::Pending);
                self.approval_consent_review.insert(approval_id, review_id);
                self.review_consent_approval.insert(review_id, approval_id);
            }
            to Ready
            emit ApprovalStatusResolved { approval_id: approval_id, status: ApprovalLifecycleStatus::Pending }
        }

        transition RestoreRejectedDuplicate {
            on input RestoreApproval { approval_id, status, approve_allowed, deny_allowed, has_expiry, decision, consent, authenticated, consumed }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.approval_ids.contains(approval_id)
            }
            update {}
            to Ready
            emit ApprovalLifecycleRejected { approval_id: approval_id, reason: ApprovalLifecycleRejectionReason::AlreadyExists }
        }

        transition RestoreRejectedEmptyAllowedDecisions {
            on input RestoreApproval { approval_id, status, approve_allowed, deny_allowed, has_expiry, decision, consent, authenticated, consumed }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.approval_ids.contains(approval_id) == false
                && allowed_non_empty(approve_allowed, deny_allowed) == false
            }
            update {}
            to Ready
            emit ApprovalLifecycleRejected { approval_id: approval_id, reason: ApprovalLifecycleRejectionReason::EmptyAllowedDecisions }
        }

        // Restored rows are history, never linked: no live attempt or custody.
        transition RestorePending {
            on input RestoreApproval { approval_id, status, approve_allowed, deny_allowed, has_expiry, decision, consent, authenticated, consumed }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.approval_ids.contains(approval_id) == false
                && allowed_non_empty(approve_allowed, deny_allowed)
                && status == ApprovalLifecycleStatus::Pending
                && restored_record_valid(status, approve_allowed, deny_allowed, decision, consent, authenticated, consumed)
            }
            update {
                self.approval_ids.insert(approval_id);
                self.approval_statuses.insert(approval_id, ApprovalLifecycleStatus::Pending);
                self.approval_approve_allowed.insert(approval_id, approve_allowed);
                self.approval_deny_allowed.insert(approval_id, deny_allowed);
                self.approval_has_expiry.insert(approval_id, has_expiry);
                self.approval_consent.insert(approval_id, consent);
                self.approval_consent_authenticated.insert(approval_id, authenticated);
                self.approval_consent_consumed.insert(approval_id, consumed);
            }
            to Ready
            emit ApprovalStatusResolved { approval_id: approval_id, status: ApprovalLifecycleStatus::Pending }
        }

        transition RestoreExpired {
            on input RestoreApproval { approval_id, status, approve_allowed, deny_allowed, has_expiry, decision, consent, authenticated, consumed }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.approval_ids.contains(approval_id) == false
                && allowed_non_empty(approve_allowed, deny_allowed)
                && status == ApprovalLifecycleStatus::Expired
                && restored_record_valid(status, approve_allowed, deny_allowed, decision, consent, authenticated, consumed)
            }
            update {
                self.approval_ids.insert(approval_id);
                self.approval_statuses.insert(approval_id, ApprovalLifecycleStatus::Expired);
                self.approval_approve_allowed.insert(approval_id, approve_allowed);
                self.approval_deny_allowed.insert(approval_id, deny_allowed);
                self.approval_has_expiry.insert(approval_id, has_expiry);
                self.approval_consent.insert(approval_id, consent);
                self.approval_consent_authenticated.insert(approval_id, authenticated);
                self.approval_consent_consumed.insert(approval_id, consumed);
            }
            to Ready
            emit ApprovalStatusResolved { approval_id: approval_id, status: ApprovalLifecycleStatus::Expired }
        }

        transition RestoreCancelled {
            on input RestoreApproval { approval_id, status, approve_allowed, deny_allowed, has_expiry, decision, consent, authenticated, consumed }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.approval_ids.contains(approval_id) == false
                && allowed_non_empty(approve_allowed, deny_allowed)
                && status == ApprovalLifecycleStatus::Cancelled
                && restored_record_valid(status, approve_allowed, deny_allowed, decision, consent, authenticated, consumed)
            }
            update {
                self.approval_ids.insert(approval_id);
                self.approval_statuses.insert(approval_id, ApprovalLifecycleStatus::Cancelled);
                self.approval_approve_allowed.insert(approval_id, approve_allowed);
                self.approval_deny_allowed.insert(approval_id, deny_allowed);
                self.approval_has_expiry.insert(approval_id, has_expiry);
                self.approval_consent.insert(approval_id, consent);
                self.approval_consent_authenticated.insert(approval_id, authenticated);
                self.approval_consent_consumed.insert(approval_id, consumed);
            }
            to Ready
            emit ApprovalStatusResolved { approval_id: approval_id, status: ApprovalLifecycleStatus::Cancelled }
        }

        transition RestoreApproved {
            on input RestoreApproval { approval_id, status, approve_allowed, deny_allowed, has_expiry, decision, consent, authenticated, consumed }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.approval_ids.contains(approval_id) == false
                && allowed_non_empty(approve_allowed, deny_allowed)
                && status == ApprovalLifecycleStatus::Approved
                && restored_record_valid(status, approve_allowed, deny_allowed, decision, consent, authenticated, consumed)
            }
            update {
                self.approval_ids.insert(approval_id);
                self.approval_statuses.insert(approval_id, ApprovalLifecycleStatus::Approved);
                self.approval_approve_allowed.insert(approval_id, approve_allowed);
                self.approval_deny_allowed.insert(approval_id, deny_allowed);
                self.approval_has_expiry.insert(approval_id, has_expiry);
                self.approval_consent.insert(approval_id, consent);
                self.approval_consent_authenticated.insert(approval_id, authenticated);
                self.approval_consent_consumed.insert(approval_id, consumed);
            }
            to Ready
            emit ApprovalStatusResolved { approval_id: approval_id, status: ApprovalLifecycleStatus::Approved }
        }

        transition RestoreDenied {
            on input RestoreApproval { approval_id, status, approve_allowed, deny_allowed, has_expiry, decision, consent, authenticated, consumed }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.approval_ids.contains(approval_id) == false
                && allowed_non_empty(approve_allowed, deny_allowed)
                && status == ApprovalLifecycleStatus::Denied
                && restored_record_valid(status, approve_allowed, deny_allowed, decision, consent, authenticated, consumed)
            }
            update {
                self.approval_ids.insert(approval_id);
                self.approval_statuses.insert(approval_id, ApprovalLifecycleStatus::Denied);
                self.approval_approve_allowed.insert(approval_id, approve_allowed);
                self.approval_deny_allowed.insert(approval_id, deny_allowed);
                self.approval_has_expiry.insert(approval_id, has_expiry);
                self.approval_consent.insert(approval_id, consent);
                self.approval_consent_authenticated.insert(approval_id, authenticated);
                self.approval_consent_consumed.insert(approval_id, consumed);
            }
            to Ready
            emit ApprovalStatusResolved { approval_id: approval_id, status: ApprovalLifecycleStatus::Denied }
        }

        transition RestoreRejectedInvalidRecord {
            on input RestoreApproval { approval_id, status, approve_allowed, deny_allowed, has_expiry, decision, consent, authenticated, consumed }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.approval_ids.contains(approval_id) == false
                && allowed_non_empty(approve_allowed, deny_allowed)
                && restored_record_valid(status, approve_allowed, deny_allowed, decision, consent, authenticated, consumed) == false
            }
            update {}
            to Ready
            emit ApprovalLifecycleRejected { approval_id: approval_id, reason: ApprovalLifecycleRejectionReason::InvalidRestoredRecord }
        }

        // Replace one existing row's state from its stored row. Never for a row with live consent custody, never back from a spend, and never a link: reconciled history cannot become executable consent.
        transition ReconcileRejectedMissing {
            on input ReconcileApproval { approval_id, status, approve_allowed, deny_allowed, has_expiry, decision, consent, authenticated, consumed }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.approval_ids.contains(approval_id) == false
            }
            update {}
            to Ready
            emit ApprovalLifecycleRejected { approval_id: approval_id, reason: ApprovalLifecycleRejectionReason::NotFound }
        }

        transition ReconcileRejectedLinked {
            on input ReconcileApproval { approval_id, status, approve_allowed, deny_allowed, has_expiry, decision, consent, authenticated, consumed }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.approval_ids.contains(approval_id)
                && self.approval_consent_review.contains_key(approval_id)
            }
            update {}
            to Ready
            emit ApprovalLifecycleRejected { approval_id: approval_id, reason: ApprovalLifecycleRejectionReason::ConsentLinked }
        }

        transition ReconcileRejectedInvalidRecord {
            on input ReconcileApproval { approval_id, status, approve_allowed, deny_allowed, has_expiry, decision, consent, authenticated, consumed }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.approval_ids.contains(approval_id)
                && self.approval_consent_review.contains_key(approval_id) == false
                && (
                    restored_record_valid(status, approve_allowed, deny_allowed, decision, consent, authenticated, consumed) == false
                    || self.approval_consent.get_cloned(approval_id).get("value") != consent
                    || (self.approval_consent_consumed.get_cloned(approval_id).get("value") && consumed == false)
                )
            }
            update {}
            to Ready
            emit ApprovalLifecycleRejected { approval_id: approval_id, reason: ApprovalLifecycleRejectionReason::InvalidRestoredRecord }
        }

        transition ReconcileAppliedPending {
            on input ReconcileApproval { approval_id, status, approve_allowed, deny_allowed, has_expiry, decision, consent, authenticated, consumed }
            guard {
                self.lifecycle_phase == Phase::Ready
                && status == ApprovalLifecycleStatus::Pending
                && self.approval_ids.contains(approval_id)
                && self.approval_consent_review.contains_key(approval_id) == false
                && restored_record_valid(status, approve_allowed, deny_allowed, decision, consent, authenticated, consumed)
                && self.approval_consent.get_cloned(approval_id).get("value") == consent
                && (self.approval_consent_consumed.get_cloned(approval_id).get("value") == false || consumed)
            }
            update {
                self.approval_statuses.insert(approval_id, ApprovalLifecycleStatus::Pending);
                self.approval_approve_allowed.insert(approval_id, approve_allowed);
                self.approval_deny_allowed.insert(approval_id, deny_allowed);
                self.approval_has_expiry.insert(approval_id, has_expiry);
                self.approval_consent_authenticated.insert(approval_id, authenticated);
                self.approval_consent_consumed.insert(approval_id, consumed);
            }
            to Ready
            emit ApprovalStatusResolved { approval_id: approval_id, status: ApprovalLifecycleStatus::Pending }
        }

        transition ReconcileAppliedExpired {
            on input ReconcileApproval { approval_id, status, approve_allowed, deny_allowed, has_expiry, decision, consent, authenticated, consumed }
            guard {
                self.lifecycle_phase == Phase::Ready
                && status == ApprovalLifecycleStatus::Expired
                && self.approval_ids.contains(approval_id)
                && self.approval_consent_review.contains_key(approval_id) == false
                && restored_record_valid(status, approve_allowed, deny_allowed, decision, consent, authenticated, consumed)
                && self.approval_consent.get_cloned(approval_id).get("value") == consent
                && (self.approval_consent_consumed.get_cloned(approval_id).get("value") == false || consumed)
            }
            update {
                self.approval_statuses.insert(approval_id, ApprovalLifecycleStatus::Expired);
                self.approval_approve_allowed.insert(approval_id, approve_allowed);
                self.approval_deny_allowed.insert(approval_id, deny_allowed);
                self.approval_has_expiry.insert(approval_id, has_expiry);
                self.approval_consent_authenticated.insert(approval_id, authenticated);
                self.approval_consent_consumed.insert(approval_id, consumed);
            }
            to Ready
            emit ApprovalStatusResolved { approval_id: approval_id, status: ApprovalLifecycleStatus::Expired }
        }

        transition ReconcileAppliedCancelled {
            on input ReconcileApproval { approval_id, status, approve_allowed, deny_allowed, has_expiry, decision, consent, authenticated, consumed }
            guard {
                self.lifecycle_phase == Phase::Ready
                && status == ApprovalLifecycleStatus::Cancelled
                && self.approval_ids.contains(approval_id)
                && self.approval_consent_review.contains_key(approval_id) == false
                && restored_record_valid(status, approve_allowed, deny_allowed, decision, consent, authenticated, consumed)
                && self.approval_consent.get_cloned(approval_id).get("value") == consent
                && (self.approval_consent_consumed.get_cloned(approval_id).get("value") == false || consumed)
            }
            update {
                self.approval_statuses.insert(approval_id, ApprovalLifecycleStatus::Cancelled);
                self.approval_approve_allowed.insert(approval_id, approve_allowed);
                self.approval_deny_allowed.insert(approval_id, deny_allowed);
                self.approval_has_expiry.insert(approval_id, has_expiry);
                self.approval_consent_authenticated.insert(approval_id, authenticated);
                self.approval_consent_consumed.insert(approval_id, consumed);
            }
            to Ready
            emit ApprovalStatusResolved { approval_id: approval_id, status: ApprovalLifecycleStatus::Cancelled }
        }

        transition ReconcileAppliedApproved {
            on input ReconcileApproval { approval_id, status, approve_allowed, deny_allowed, has_expiry, decision, consent, authenticated, consumed }
            guard {
                self.lifecycle_phase == Phase::Ready
                && status == ApprovalLifecycleStatus::Approved
                && self.approval_ids.contains(approval_id)
                && self.approval_consent_review.contains_key(approval_id) == false
                && restored_record_valid(status, approve_allowed, deny_allowed, decision, consent, authenticated, consumed)
                && self.approval_consent.get_cloned(approval_id).get("value") == consent
                && (self.approval_consent_consumed.get_cloned(approval_id).get("value") == false || consumed)
            }
            update {
                self.approval_statuses.insert(approval_id, ApprovalLifecycleStatus::Approved);
                self.approval_approve_allowed.insert(approval_id, approve_allowed);
                self.approval_deny_allowed.insert(approval_id, deny_allowed);
                self.approval_has_expiry.insert(approval_id, has_expiry);
                self.approval_consent_authenticated.insert(approval_id, authenticated);
                self.approval_consent_consumed.insert(approval_id, consumed);
            }
            to Ready
            emit ApprovalStatusResolved { approval_id: approval_id, status: ApprovalLifecycleStatus::Approved }
        }

        transition ReconcileAppliedDenied {
            on input ReconcileApproval { approval_id, status, approve_allowed, deny_allowed, has_expiry, decision, consent, authenticated, consumed }
            guard {
                self.lifecycle_phase == Phase::Ready
                && status == ApprovalLifecycleStatus::Denied
                && self.approval_ids.contains(approval_id)
                && self.approval_consent_review.contains_key(approval_id) == false
                && restored_record_valid(status, approve_allowed, deny_allowed, decision, consent, authenticated, consumed)
                && self.approval_consent.get_cloned(approval_id).get("value") == consent
                && (self.approval_consent_consumed.get_cloned(approval_id).get("value") == false || consumed)
            }
            update {
                self.approval_statuses.insert(approval_id, ApprovalLifecycleStatus::Denied);
                self.approval_approve_allowed.insert(approval_id, approve_allowed);
                self.approval_deny_allowed.insert(approval_id, deny_allowed);
                self.approval_has_expiry.insert(approval_id, has_expiry);
                self.approval_consent_authenticated.insert(approval_id, authenticated);
                self.approval_consent_consumed.insert(approval_id, consumed);
            }
            to Ready
            emit ApprovalStatusResolved { approval_id: approval_id, status: ApprovalLifecycleStatus::Denied }
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

        // A linked consent approval keeps its link until the owner withdraws the attempt in the same reservation.
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

        // Caller-supplied text is not authentication: a consent approval is decided only through DecideConsent.
        transition DecideRejectedAuthenticationRequired {
            on input DecideApproval { approval_id, decision }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.approval_ids.contains(approval_id)
                && self.approval_statuses.get_cloned(approval_id).get("value") == ApprovalLifecycleStatus::Pending
                && self.approval_consent.get_cloned(approval_id).get("value")
            }
            update {}
            to Ready
            emit ApprovalLifecycleRejected { approval_id: approval_id, reason: ApprovalLifecycleRejectionReason::AuthenticationRequired }
        }

        transition DecideRejectedApproveNotAllowed {
            on input DecideApproval { approval_id, decision }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.approval_ids.contains(approval_id)
                && self.approval_statuses.get_cloned(approval_id).get("value") == ApprovalLifecycleStatus::Pending
                && self.approval_consent.get_cloned(approval_id).get("value") == false
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
                && self.approval_consent.get_cloned(approval_id).get("value") == false
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
                && self.approval_consent.get_cloned(approval_id).get("value") == false
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
                && self.approval_consent.get_cloned(approval_id).get("value") == false
                && decision == ApprovalLifecycleDecision::Deny
                && self.approval_deny_allowed.get_cloned(approval_id).get("value")
            }
            update {
                self.approval_statuses.insert(approval_id, ApprovalLifecycleStatus::Denied);
            }
            to Ready
            emit ApprovalStatusResolved { approval_id: approval_id, status: ApprovalLifecycleStatus::Denied }
        }

        transition DecideConsentRejectedMissing {
            on input DecideConsent { approval_id, review_id, decision, authenticated }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.approval_ids.contains(approval_id) == false
            }
            update {}
            to Ready
            emit ApprovalLifecycleRejected { approval_id: approval_id, reason: ApprovalLifecycleRejectionReason::NotFound }
        }

        // Only the linked pair moves: a copied or mismatched attempt, or a pair whose attempt was withdrawn, is refused.
        transition DecideConsentRejectedMismatch {
            on input DecideConsent { approval_id, review_id, decision, authenticated }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.approval_ids.contains(approval_id)
                && (self.approval_consent_review.contains_key(approval_id) == false || self.approval_consent_review.get_cloned(approval_id).get("value") != review_id)
            }
            update {}
            to Ready
            emit ApprovalLifecycleRejected { approval_id: approval_id, reason: ApprovalLifecycleRejectionReason::ConsentMismatch }
        }

        transition DecideConsentRejectedExpired {
            on input DecideConsent { approval_id, review_id, decision, authenticated }
            guard {
                self.lifecycle_phase == Phase::Ready
                && (self.approval_consent_review.contains_key(approval_id) && self.approval_consent_review.get_cloned(approval_id).get("value") == review_id)
                && self.approval_statuses.get_cloned(approval_id).get("value") == ApprovalLifecycleStatus::Expired
            }
            update {}
            to Ready
            emit ApprovalLifecycleRejected { approval_id: approval_id, reason: ApprovalLifecycleRejectionReason::Expired }
        }

        transition DecideConsentRejectedAlreadyDecided {
            on input DecideConsent { approval_id, review_id, decision, authenticated }
            guard {
                self.lifecycle_phase == Phase::Ready
                && (self.approval_consent_review.contains_key(approval_id) && self.approval_consent_review.get_cloned(approval_id).get("value") == review_id)
                && is_terminal_status(self.approval_statuses.get_cloned(approval_id).get("value"))
            }
            update {}
            to Ready
            emit ApprovalLifecycleRejected { approval_id: approval_id, reason: ApprovalLifecycleRejectionReason::AlreadyDecided }
        }

        transition DecideConsentRejectedAuthenticationRequired {
            on input DecideConsent { approval_id, review_id, decision, authenticated }
            guard {
                self.lifecycle_phase == Phase::Ready
                && (self.approval_consent_review.contains_key(approval_id) && self.approval_consent_review.get_cloned(approval_id).get("value") == review_id)
                && self.approval_statuses.get_cloned(approval_id).get("value") == ApprovalLifecycleStatus::Pending
                && authenticated == false
            }
            update {}
            to Ready
            emit ApprovalLifecycleRejected { approval_id: approval_id, reason: ApprovalLifecycleRejectionReason::AuthenticationRequired }
        }

        transition DecideConsentRejectedNotAllowed {
            on input DecideConsent { approval_id, review_id, decision, authenticated }
            guard {
                self.lifecycle_phase == Phase::Ready
                && (self.approval_consent_review.contains_key(approval_id) && self.approval_consent_review.get_cloned(approval_id).get("value") == review_id)
                && self.approval_statuses.get_cloned(approval_id).get("value") == ApprovalLifecycleStatus::Pending
                && authenticated
                && ((decision == ApprovalLifecycleDecision::Approve && self.approval_approve_allowed.get_cloned(approval_id).get("value") == false)
                    || (decision == ApprovalLifecycleDecision::Deny && self.approval_deny_allowed.get_cloned(approval_id).get("value") == false))
            }
            update {}
            to Ready
            emit ApprovalLifecycleRejected { approval_id: approval_id, reason: ApprovalLifecycleRejectionReason::InvalidDecision }
        }

        // An approval records the decision and allows the linked attempt; it executes nothing.
        transition DecideConsentApprove {
            on input DecideConsent { approval_id, review_id, decision, authenticated }
            guard {
                self.lifecycle_phase == Phase::Ready
                && (self.approval_consent_review.contains_key(approval_id) && self.approval_consent_review.get_cloned(approval_id).get("value") == review_id)
                && self.approval_statuses.get_cloned(approval_id).get("value") == ApprovalLifecycleStatus::Pending
                && authenticated
                && decision == ApprovalLifecycleDecision::Approve
                && self.approval_approve_allowed.get_cloned(approval_id).get("value")
            }
            update {
                self.approval_statuses.insert(approval_id, ApprovalLifecycleStatus::Approved);
                self.approval_consent_authenticated.insert(approval_id, true);
                self.review_statuses.insert(review_id, ReviewAttemptStatus::Allowed);
            }
            to Ready
            emit ApprovalStatusResolved { approval_id: approval_id, status: ApprovalLifecycleStatus::Approved }
        }

        transition DecideConsentDeny {
            on input DecideConsent { approval_id, review_id, decision, authenticated }
            guard {
                self.lifecycle_phase == Phase::Ready
                && (self.approval_consent_review.contains_key(approval_id) && self.approval_consent_review.get_cloned(approval_id).get("value") == review_id)
                && self.approval_statuses.get_cloned(approval_id).get("value") == ApprovalLifecycleStatus::Pending
                && authenticated
                && decision == ApprovalLifecycleDecision::Deny
                && self.approval_deny_allowed.get_cloned(approval_id).get("value")
            }
            update {
                self.approval_statuses.insert(approval_id, ApprovalLifecycleStatus::Denied);
                self.approval_consent_authenticated.insert(approval_id, true);
                self.review_statuses.insert(review_id, ReviewAttemptStatus::Denied);
                self.approval_consent_review.remove(approval_id);
                self.review_consent_approval.remove(review_id);
            }
            to Ready
            emit ApprovalStatusResolved { approval_id: approval_id, status: ApprovalLifecycleStatus::Denied }
        }

        transition ConsumeConsentRejectedMissing {
            on input ConsumeConsentForEntry { approval_id, review_id }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.approval_ids.contains(approval_id) == false
            }
            update {}
            to Ready
            emit ApprovalLifecycleRejected { approval_id: approval_id, reason: ApprovalLifecycleRejectionReason::NotFound }
        }

        transition ConsumeConsentRejectedAlreadyConsumed {
            on input ConsumeConsentForEntry { approval_id, review_id }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.approval_ids.contains(approval_id)
                && self.approval_consent_consumed.get_cloned(approval_id).get("value")
            }
            update {}
            to Ready
            emit ApprovalLifecycleRejected { approval_id: approval_id, reason: ApprovalLifecycleRejectionReason::AlreadyConsumed }
        }

        transition ConsumeConsentRejectedMismatch {
            on input ConsumeConsentForEntry { approval_id, review_id }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.approval_ids.contains(approval_id)
                && self.approval_consent_consumed.get_cloned(approval_id).get("value") == false
                && (self.approval_consent_review.contains_key(approval_id) == false || self.approval_consent_review.get_cloned(approval_id).get("value") != review_id)
            }
            update {}
            to Ready
            emit ApprovalLifecycleRejected { approval_id: approval_id, reason: ApprovalLifecycleRejectionReason::ConsentMismatch }
        }

        transition ConsumeConsentRejectedNotSatisfied {
            on input ConsumeConsentForEntry { approval_id, review_id }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.approval_ids.contains(approval_id)
                && self.approval_consent_consumed.get_cloned(approval_id).get("value") == false
                && (self.approval_consent_review.contains_key(approval_id) && self.approval_consent_review.get_cloned(approval_id).get("value") == review_id)
                && (
                    self.approval_statuses.get_cloned(approval_id).get("value") != ApprovalLifecycleStatus::Approved
                    || self.approval_consent_authenticated.get_cloned(approval_id).get("value") == false
                    || self.review_statuses.get_cloned(review_id).get("value") != ReviewAttemptStatus::Allowed
                )
            }
            update {}
            to Ready
            emit ApprovalLifecycleRejected { approval_id: approval_id, reason: ApprovalLifecycleRejectionReason::ReviewNotSatisfied }
        }

        // The one spend: linked pair, approved by an authenticated decision, unconsumed, attempt allowed.
        transition ConsumeConsentForEntry {
            on input ConsumeConsentForEntry { approval_id, review_id }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.approval_ids.contains(approval_id)
                && self.approval_consent_consumed.get_cloned(approval_id).get("value") == false
                && (self.approval_consent_review.contains_key(approval_id) && self.approval_consent_review.get_cloned(approval_id).get("value") == review_id)
                && self.approval_statuses.get_cloned(approval_id).get("value") == ApprovalLifecycleStatus::Approved
                && self.approval_consent_authenticated.get_cloned(approval_id).get("value")
                && self.review_statuses.get_cloned(review_id).get("value") == ReviewAttemptStatus::Allowed
            }
            update {
                self.approval_consent_consumed.insert(approval_id, true);
                self.review_statuses.insert(review_id, ReviewAttemptStatus::Used);
                self.approval_consent_review.remove(approval_id);
                self.review_consent_approval.remove(review_id);
                self.review_entered_phase.insert(review_id, EnteredEffectPhase::Open);
                self.review_entered_steps.insert(review_id, 0);
            }
            to Ready
            emit ReviewStatusResolved { review_id: review_id, status: ReviewAttemptStatus::Used }
        }

        transition WithdrawConsentRejectedMissing {
            on input WithdrawConsent { approval_id, review_id, reason }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.approval_ids.contains(approval_id) == false
            }
            update {}
            to Ready
            emit ApprovalLifecycleRejected { approval_id: approval_id, reason: ApprovalLifecycleRejectionReason::NotFound }
        }

        transition WithdrawConsentRejectedMismatch {
            on input WithdrawConsent { approval_id, review_id, reason }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.approval_ids.contains(approval_id)
                && (self.approval_consent_review.contains_key(approval_id) == false || self.approval_consent_review.get_cloned(approval_id).get("value") != review_id)
            }
            update {}
            to Ready
            emit ApprovalLifecycleRejected { approval_id: approval_id, reason: ApprovalLifecycleRejectionReason::ConsentMismatch }
        }

        // A withdrawn pending consent approval is Cancelled: never a denial. Its attempt retires with the typed reason.
        transition WithdrawConsentPending {
            on input WithdrawConsent { approval_id, review_id, reason }
            guard {
                self.lifecycle_phase == Phase::Ready
                && (self.approval_consent_review.contains_key(approval_id) && self.approval_consent_review.get_cloned(approval_id).get("value") == review_id)
                && self.approval_statuses.get_cloned(approval_id).get("value") == ApprovalLifecycleStatus::Pending
            }
            update {
                self.approval_statuses.insert(approval_id, ApprovalLifecycleStatus::Cancelled);
                self.review_statuses.insert(review_id, ReviewAttemptStatus::Retired);
                self.review_retirements.insert(review_id, reason);
                self.approval_consent_review.remove(approval_id);
                self.review_consent_approval.remove(review_id);
            }
            to Ready
            emit ApprovalStatusResolved { approval_id: approval_id, status: ApprovalLifecycleStatus::Cancelled }
        }

        // Approved (unspent) or expired history is kept; only the linked attempt retires. No refund.
        transition WithdrawConsentSettled {
            on input WithdrawConsent { approval_id, review_id, reason }
            guard {
                self.lifecycle_phase == Phase::Ready
                && (self.approval_consent_review.contains_key(approval_id) && self.approval_consent_review.get_cloned(approval_id).get("value") == review_id)
                && self.approval_statuses.get_cloned(approval_id).get("value") != ApprovalLifecycleStatus::Pending
            }
            update {
                self.review_statuses.insert(review_id, ReviewAttemptStatus::Retired);
                self.review_retirements.insert(review_id, reason);
                self.approval_consent_review.remove(approval_id);
                self.review_consent_approval.remove(review_id);
            }
            to Ready
            emit ReviewStatusResolved { review_id: review_id, status: ReviewAttemptStatus::Retired }
        }

        // Unlinked consent settlement: no attempt of a consent row is live,
        // either because this owner retired its custody and still owes the
        // withdrawal's record (its write failed or its row was unknown, and
        // the row was reconciled from the store), or because a
        // cross-process fresh acquisition or takeover proved the issuing
        // owner gone. The row's status is settled without any candidate; a
        // row whose attempt is linked here is held by this owner and
        // refused.
        transition SettleUnlinkedRejectedMissing {
            on input SettleUnlinkedConsent { approval_id }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.approval_ids.contains(approval_id) == false
            }
            update {}
            to Ready
            emit ApprovalLifecycleRejected { approval_id: approval_id, reason: ApprovalLifecycleRejectionReason::NotFound }
        }

        transition SettleUnlinkedRejectedNotConsent {
            on input SettleUnlinkedConsent { approval_id }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.approval_ids.contains(approval_id)
                && self.approval_consent.get_cloned(approval_id).get("value") == false
            }
            update {}
            to Ready
            emit ApprovalLifecycleRejected { approval_id: approval_id, reason: ApprovalLifecycleRejectionReason::NotConsent }
        }

        transition SettleUnlinkedRejectedLinked {
            on input SettleUnlinkedConsent { approval_id }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.approval_ids.contains(approval_id)
                && self.approval_consent.get_cloned(approval_id).get("value") == true
                && self.approval_consent_review.contains_key(approval_id)
            }
            update {}
            to Ready
            emit ApprovalLifecycleRejected { approval_id: approval_id, reason: ApprovalLifecycleRejectionReason::ConsentLinked }
        }

        transition SettleUnlinkedRejectedConsumed {
            on input SettleUnlinkedConsent { approval_id }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.approval_ids.contains(approval_id)
                && self.approval_consent.get_cloned(approval_id).get("value") == true
                && self.approval_consent_review.contains_key(approval_id) == false
                && self.approval_consent_consumed.get_cloned(approval_id).get("value") == true
            }
            update {}
            to Ready
            emit ApprovalLifecycleRejected { approval_id: approval_id, reason: ApprovalLifecycleRejectionReason::AlreadyConsumed }
        }

        // Expired history is never rewritten by a settlement.
        transition SettleUnlinkedRejectedExpired {
            on input SettleUnlinkedConsent { approval_id }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.approval_ids.contains(approval_id)
                && self.approval_consent.get_cloned(approval_id).get("value") == true
                && self.approval_consent_review.contains_key(approval_id) == false
                && self.approval_consent_consumed.get_cloned(approval_id).get("value") == false
                && self.approval_statuses.get_cloned(approval_id).get("value") == ApprovalLifecycleStatus::Expired
            }
            update {}
            to Ready
            emit ApprovalLifecycleRejected { approval_id: approval_id, reason: ApprovalLifecycleRejectionReason::Expired }
        }

        // Denied or cancelled history needs no settlement.
        transition SettleUnlinkedRejectedSettled {
            on input SettleUnlinkedConsent { approval_id }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.approval_ids.contains(approval_id)
                && self.approval_consent.get_cloned(approval_id).get("value") == true
                && self.approval_consent_review.contains_key(approval_id) == false
                && self.approval_consent_consumed.get_cloned(approval_id).get("value") == false
                && (self.approval_statuses.get_cloned(approval_id).get("value") == ApprovalLifecycleStatus::Denied
                    || self.approval_statuses.get_cloned(approval_id).get("value") == ApprovalLifecycleStatus::Cancelled)
            }
            update {}
            to Ready
            emit ApprovalLifecycleRejected { approval_id: approval_id, reason: ApprovalLifecycleRejectionReason::AlreadyDecided }
        }

        // A pending unlinked consent is Cancelled, never a denial, whatever
        // its deadline: a pending row past its expiry is settled here, not
        // rewritten as Expired.
        transition SettleUnlinkedPending {
            on input SettleUnlinkedConsent { approval_id }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.approval_ids.contains(approval_id)
                && self.approval_consent.get_cloned(approval_id).get("value") == true
                && self.approval_consent_review.contains_key(approval_id) == false
                && self.approval_consent_consumed.get_cloned(approval_id).get("value") == false
                && self.approval_statuses.get_cloned(approval_id).get("value") == ApprovalLifecycleStatus::Pending
            }
            update {
                self.approval_statuses.insert(approval_id, ApprovalLifecycleStatus::Cancelled);
            }
            to Ready
            emit ApprovalStatusResolved { approval_id: approval_id, status: ApprovalLifecycleStatus::Cancelled }
        }

        // Approved, unspent history is kept: only its withdrawal is
        // recorded with the row. No refund.
        transition SettleUnlinkedApproved {
            on input SettleUnlinkedConsent { approval_id }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.approval_ids.contains(approval_id)
                && self.approval_consent.get_cloned(approval_id).get("value") == true
                && self.approval_consent_review.contains_key(approval_id) == false
                && self.approval_consent_consumed.get_cloned(approval_id).get("value") == false
                && self.approval_statuses.get_cloned(approval_id).get("value") == ApprovalLifecycleStatus::Approved
            }
            update {}
            to Ready
            emit ApprovalStatusResolved { approval_id: approval_id, status: ApprovalLifecycleStatus::Approved }
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

        // A linked human attempt moves only through the consent inputs.
        transition RecordReviewVerdictRejectedConsentLinked {
            on input RecordReviewVerdict { review_id, verdict }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.review_ids.contains(review_id)
                && self.review_consent_approval.contains_key(review_id)
            }
            update {}
            to Ready
            emit ReviewLifecycleRejected { review_id: review_id, reason: ApprovalLifecycleRejectionReason::ConsentLinked }
        }

        transition RecordReviewVerdictRejectedRetired {
            on input RecordReviewVerdict { review_id, verdict }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.review_ids.contains(review_id)
                && self.review_consent_approval.contains_key(review_id) == false
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
                && self.review_consent_approval.contains_key(review_id) == false
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
                && self.review_consent_approval.contains_key(review_id) == false
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
                && self.review_consent_approval.contains_key(review_id) == false
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
                && self.review_consent_approval.contains_key(review_id) == false
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

        transition RecordReviewUnavailableRejectedConsentLinked {
            on input RecordReviewUnavailable { review_id }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.review_ids.contains(review_id)
                && self.review_consent_approval.contains_key(review_id)
            }
            update {}
            to Ready
            emit ReviewLifecycleRejected { review_id: review_id, reason: ApprovalLifecycleRejectionReason::ConsentLinked }
        }

        transition RecordReviewUnavailableRejectedRetired {
            on input RecordReviewUnavailable { review_id }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.review_ids.contains(review_id)
                && self.review_consent_approval.contains_key(review_id) == false
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
                && self.review_consent_approval.contains_key(review_id) == false
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
                && self.review_consent_approval.contains_key(review_id) == false
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

        transition RetireReviewRejectedConsentLinked {
            on input RetireReview { review_id, reason }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.review_ids.contains(review_id)
                && self.review_consent_approval.contains_key(review_id)
            }
            update {}
            to Ready
            emit ReviewLifecycleRejected { review_id: review_id, reason: ApprovalLifecycleRejectionReason::ConsentLinked }
        }

        transition RetireReviewRejectedRetired {
            on input RetireReview { review_id, reason }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.review_ids.contains(review_id)
                && self.review_consent_approval.contains_key(review_id) == false
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
                && self.review_consent_approval.contains_key(review_id) == false
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
                && self.review_consent_approval.contains_key(review_id) == false
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

        transition ConsumeReviewRejectedConsentLinked {
            on input ConsumeReviewForEntry { review_id }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.review_ids.contains(review_id)
                && self.review_consent_approval.contains_key(review_id)
            }
            update {}
            to Ready
            emit ReviewLifecycleRejected { review_id: review_id, reason: ApprovalLifecycleRejectionReason::ConsentLinked }
        }

        transition ConsumeReviewRejectedRetired {
            on input ConsumeReviewForEntry { review_id }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.review_ids.contains(review_id)
                && self.review_consent_approval.contains_key(review_id) == false
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
                && self.review_consent_approval.contains_key(review_id) == false
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
                && self.review_consent_approval.contains_key(review_id) == false
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
                && self.review_consent_approval.contains_key(review_id) == false
                && self.review_statuses.get_cloned(review_id).get("value") == ReviewAttemptStatus::Allowed
            }
            update {
                self.review_statuses.insert(review_id, ReviewAttemptStatus::Used);
                self.review_entered_phase.insert(review_id, EnteredEffectPhase::Open);
                self.review_entered_steps.insert(review_id, 0);
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

        transition ReleaseReviewRejectedConsentLinked {
            on input ReleaseReview { review_id }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.review_ids.contains(review_id)
                && self.review_consent_approval.contains_key(review_id)
            }
            update {}
            to Ready
            emit ReviewLifecycleRejected { review_id: review_id, reason: ApprovalLifecycleRejectionReason::ConsentLinked }
        }

        transition ReleaseReviewRejectedPending {
            on input ReleaseReview { review_id }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.review_ids.contains(review_id)
                && self.review_consent_approval.contains_key(review_id) == false
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
                && self.review_consent_approval.contains_key(review_id) == false
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
                && self.review_consent_approval.contains_key(review_id) == false
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
                && self.review_consent_approval.contains_key(review_id) == false
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
                && self.review_consent_approval.contains_key(review_id) == false
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
                && self.review_consent_approval.contains_key(review_id) == false
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

        // An entered effect ends with no admitted step in flight: the spent
        // attempt is released.
        transition ReleaseReviewUsed {
            on input ReleaseReview { review_id }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.review_ids.contains(review_id)
                && self.review_consent_approval.contains_key(review_id) == false
                && self.review_statuses.get_cloned(review_id).get("value") == ReviewAttemptStatus::Used
                && self.review_entered_steps.get_cloned(review_id).get("value") == 0
            }
            update {
                self.review_ids.remove(review_id);
                self.review_statuses.remove(review_id);
                self.review_retirements.remove(review_id);
                self.review_entered_phase.remove(review_id);
                self.review_entered_steps.remove(review_id);
            }
            to Ready
            emit ReviewStatusResolved { review_id: review_id, status: ReviewAttemptStatus::Used }
        }

        // The entered state of one effect is its spent attempt's `Used`
        // status with its entered phase and in-flight steps. A later step of
        // that effect begins only here (AdmitEnteredStep), against that
        // attempt while its effect is open, and ends with FinishEnteredStep;
        // neither ever spends. The admitting dispatch's end (ReleaseReview)
        // releases the attempt with no step in flight, and otherwise closes
        // the effect: no new step, and the last step's finish releases it.
        // An unknown or released attempt, a consent-linked one (its consent
        // is unspent), a retired one and one never spent admit no step.
        transition AdmitEnteredStepRejectedMissing {
            on input AdmitEnteredStep { review_id }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.review_ids.contains(review_id) == false
            }
            update {}
            to Ready
            emit ReviewLifecycleRejected { review_id: review_id, reason: ApprovalLifecycleRejectionReason::NotFound }
        }

        transition AdmitEnteredStepRejectedConsentLinked {
            on input AdmitEnteredStep { review_id }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.review_ids.contains(review_id)
                && self.review_consent_approval.contains_key(review_id)
            }
            update {}
            to Ready
            emit ReviewLifecycleRejected { review_id: review_id, reason: ApprovalLifecycleRejectionReason::ConsentLinked }
        }

        transition AdmitEnteredStepRejectedRetired {
            on input AdmitEnteredStep { review_id }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.review_ids.contains(review_id)
                && self.review_consent_approval.contains_key(review_id) == false
                && self.review_statuses.get_cloned(review_id).get("value") == ReviewAttemptStatus::Retired
            }
            update {}
            to Ready
            emit ReviewLifecycleRejected { review_id: review_id, reason: ApprovalLifecycleRejectionReason::ReviewRetired }
        }

        transition AdmitEnteredStepRejectedNotEntered {
            on input AdmitEnteredStep { review_id }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.review_ids.contains(review_id)
                && self.review_consent_approval.contains_key(review_id) == false
                && (self.review_statuses.get_cloned(review_id).get("value") == ReviewAttemptStatus::Pending
                    || self.review_statuses.get_cloned(review_id).get("value") == ReviewAttemptStatus::Allowed
                    || self.review_statuses.get_cloned(review_id).get("value") == ReviewAttemptStatus::Denied
                    || self.review_statuses.get_cloned(review_id).get("value") == ReviewAttemptStatus::Escalated
                    || self.review_statuses.get_cloned(review_id).get("value") == ReviewAttemptStatus::Unavailable)
            }
            update {}
            to Ready
            emit ReviewLifecycleRejected { review_id: review_id, reason: ApprovalLifecycleRejectionReason::ReviewNotSatisfied }
        }

        transition AdmitEnteredStep {
            on input AdmitEnteredStep { review_id }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.review_ids.contains(review_id)
                && self.review_consent_approval.contains_key(review_id) == false
                && self.review_statuses.get_cloned(review_id).get("value") == ReviewAttemptStatus::Used
                && self.review_entered_phase.get_cloned(review_id).get("value") == EnteredEffectPhase::Open
            }
            update {
                self.review_entered_steps.insert(review_id, self.review_entered_steps.get_cloned(review_id).get("value") + 1);
            }
            to Ready
            emit ReviewStatusResolved { review_id: review_id, status: ReviewAttemptStatus::Used }
        }

        // The admitting dispatch ended while admitted steps are still in
        // flight (closing): no new step of its effect begins. The existing
        // reason `Expired` stands for that ended dispatch, so no public
        // enum changes; once the attempt is released, a step answers
        // `NotFound` instead. Core maps both to the same ended refusal.
        transition AdmitEnteredStepRejectedClosing {
            on input AdmitEnteredStep { review_id }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.review_ids.contains(review_id)
                && self.review_consent_approval.contains_key(review_id) == false
                && self.review_statuses.get_cloned(review_id).get("value") == ReviewAttemptStatus::Used
                && self.review_entered_phase.get_cloned(review_id).get("value") == EnteredEffectPhase::Closing
            }
            update {}
            to Ready
            emit ReviewLifecycleRejected { review_id: review_id, reason: ApprovalLifecycleRejectionReason::Expired }
        }

        // The admitting dispatch ended while admitted steps are still in
        // flight: the attempt stays registered as Used, closing, until the
        // last of them finishes.
        transition ReleaseReviewUsedClosing {
            on input ReleaseReview { review_id }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.review_ids.contains(review_id)
                && self.review_consent_approval.contains_key(review_id) == false
                && self.review_statuses.get_cloned(review_id).get("value") == ReviewAttemptStatus::Used
                && self.review_entered_steps.get_cloned(review_id).get("value") != 0
                && self.review_entered_phase.get_cloned(review_id).get("value") == EnteredEffectPhase::Open
            }
            update {
                self.review_entered_phase.insert(review_id, EnteredEffectPhase::Closing);
            }
            to Ready
            emit ReviewStatusResolved { review_id: review_id, status: ReviewAttemptStatus::Used }
        }

        transition ReleaseReviewRejectedClosing {
            on input ReleaseReview { review_id }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.review_ids.contains(review_id)
                && self.review_consent_approval.contains_key(review_id) == false
                && self.review_statuses.get_cloned(review_id).get("value") == ReviewAttemptStatus::Used
                && self.review_entered_steps.get_cloned(review_id).get("value") != 0
                && self.review_entered_phase.get_cloned(review_id).get("value") == EnteredEffectPhase::Closing
            }
            update {}
            to Ready
            emit ReviewLifecycleRejected { review_id: review_id, reason: ApprovalLifecycleRejectionReason::AlreadyDecided }
        }

        transition FinishEnteredStepRejectedMissing {
            on input FinishEnteredStep { review_id }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.review_ids.contains(review_id) == false
            }
            update {}
            to Ready
            emit ReviewLifecycleRejected { review_id: review_id, reason: ApprovalLifecycleRejectionReason::NotFound }
        }

        transition FinishEnteredStepRejectedNotInFlight {
            on input FinishEnteredStep { review_id }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.review_ids.contains(review_id)
                && (self.review_consent_approval.contains_key(review_id)
                    || self.review_statuses.get_cloned(review_id).get("value") != ReviewAttemptStatus::Used
                    || self.review_entered_steps.get_cloned(review_id).get("value") == 0)
            }
            update {}
            to Ready
            emit ReviewLifecycleRejected { review_id: review_id, reason: ApprovalLifecycleRejectionReason::AlreadyDecided }
        }

        transition FinishEnteredStepOpen {
            on input FinishEnteredStep { review_id }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.review_ids.contains(review_id)
                && self.review_consent_approval.contains_key(review_id) == false
                && self.review_statuses.get_cloned(review_id).get("value") == ReviewAttemptStatus::Used
                && self.review_entered_steps.get_cloned(review_id).get("value") != 0
                && self.review_entered_phase.get_cloned(review_id).get("value") == EnteredEffectPhase::Open
            }
            update {
                self.review_entered_steps.insert(review_id, self.review_entered_steps.get_cloned(review_id).get("value") - 1);
            }
            to Ready
            emit ReviewStatusResolved { review_id: review_id, status: ReviewAttemptStatus::Used }
        }

        transition FinishEnteredStepClosing {
            on input FinishEnteredStep { review_id }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.review_ids.contains(review_id)
                && self.review_consent_approval.contains_key(review_id) == false
                && self.review_statuses.get_cloned(review_id).get("value") == ReviewAttemptStatus::Used
                && self.review_entered_steps.get_cloned(review_id).get("value") != 0
                && self.review_entered_steps.get_cloned(review_id).get("value") != 1
                && self.review_entered_phase.get_cloned(review_id).get("value") == EnteredEffectPhase::Closing
            }
            update {
                self.review_entered_steps.insert(review_id, self.review_entered_steps.get_cloned(review_id).get("value") - 1);
            }
            to Ready
            emit ReviewStatusResolved { review_id: review_id, status: ReviewAttemptStatus::Used }
        }

        // The last admitted step of a closing effect finished: the spent
        // attempt is released now, after the work it admitted.
        transition FinishEnteredStepReleases {
            on input FinishEnteredStep { review_id }
            guard {
                self.lifecycle_phase == Phase::Ready
                && self.review_ids.contains(review_id)
                && self.review_consent_approval.contains_key(review_id) == false
                && self.review_statuses.get_cloned(review_id).get("value") == ReviewAttemptStatus::Used
                && self.review_entered_steps.get_cloned(review_id).get("value") == 1
                && self.review_entered_phase.get_cloned(review_id).get("value") == EnteredEffectPhase::Closing
            }
            update {
                self.review_ids.remove(review_id);
                self.review_statuses.remove(review_id);
                self.review_retirements.remove(review_id);
                self.review_entered_phase.remove(review_id);
                self.review_entered_steps.remove(review_id);
            }
            to Ready
            emit ReviewStatusResolved { review_id: review_id, status: ReviewAttemptStatus::Used }
        }
    }
}
