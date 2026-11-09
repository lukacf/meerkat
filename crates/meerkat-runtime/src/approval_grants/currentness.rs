//! The decider's currentness at a use's spend.

use std::sync::Arc;

use meerkat_authorization::grants::{ApprovalUseAnswer, ApprovalUseCurrentness, GrantApproval};
use meerkat_core::approval::consent::ConsentEligibilityAnswer;

use crate::decision::{DecisionApproverCurrentness, DurableDecisionService};

/// The host's [`DecisionApproverCurrentness`] answer, asked again at each use
/// of an approval-carried grant: the decision the grant
/// carries is read from its owner, and the host answers for the person who
/// decided it, as it answers when the decision's execution is authorized.
/// A decision that cannot be read now, or an answer the host cannot give
/// now, is unsettled: nothing is spent.
///
/// Compose the grant owner with it
/// ([`meerkat_authorization::grants::LocalGrantAuthority::with_approval_use_currentness`]).
pub struct DecisionApprovalUseCurrentness {
    decisions: DurableDecisionService,
    approver: Arc<dyn DecisionApproverCurrentness>,
}

impl DecisionApprovalUseCurrentness {
    /// The currentness of deciders of decisions in `decisions`, as
    /// `approver` answers it.
    #[must_use]
    pub fn new(
        decisions: DurableDecisionService,
        approver: Arc<dyn DecisionApproverCurrentness>,
    ) -> Self {
        Self {
            decisions,
            approver,
        }
    }
}

impl std::fmt::Debug for DecisionApprovalUseCurrentness {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DecisionApprovalUseCurrentness")
            .finish_non_exhaustive()
    }
}

impl ApprovalUseCurrentness for DecisionApprovalUseCurrentness {
    fn approver_answer(&self, approval: &GrantApproval) -> ApprovalUseAnswer {
        let Ok(decision) = self.decisions.get(&approval.decision_id) else {
            return ApprovalUseAnswer::Unsettled;
        };
        if decision.descriptor.approver_id != approval.approver_id {
            return ApprovalUseAnswer::NotCurrent;
        }
        match self.approver.approver_answer(&decision) {
            ConsentEligibilityAnswer::Eligible(evidence) => {
                if evidence.check_current().is_ok() {
                    ApprovalUseAnswer::Current
                } else {
                    ApprovalUseAnswer::Unsettled
                }
            }
            ConsentEligibilityAnswer::Refused => ApprovalUseAnswer::NotCurrent,
            _ => ApprovalUseAnswer::Unsettled,
        }
    }
}
