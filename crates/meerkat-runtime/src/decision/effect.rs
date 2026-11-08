//! What an executed decision did, as its kind names it: the effect a
//! decision's wire form carries ([`super::wire::wire_decision`]).
//!
//! A kind that names an effect has one row in [`KIND_EFFECTS`]: the
//! function that derives the effect from the decision's own id, the one
//! that minted what the kind's executor committed. A kind has a row only
//! when its executor records a decision executed after that commit landed,
//! so an effect names only what exists. A decision that is not executed,
//! and a decision of a kind without a row, names none.
//!
//! The table reads nothing but the decision's row and is compiled on every
//! target and feature set, so every reader of a decision names the same
//! effect, whether or not it composes the kind's owner.

use meerkat_authorization_contracts::evidence::EvidenceId;
use meerkat_core::approval::decision::{DecisionId, DecisionPhase, StoredDecision};

/// What an executed decision did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DecisionEffect {
    /// The execution issued the grant with this id.
    Grant { grant_id: EvidenceId },
}

/// The effect of one kind's executed decisions, derived from a decision's
/// id.
struct KindEffect {
    kind: &'static str,
    effect: fn(&DecisionId) -> Option<DecisionEffect>,
}

/// The kinds that name an effect.
const KIND_EFFECTS: &[KindEffect] = &[KindEffect {
    kind: GRANT_CHILD_KIND,
    effect: approval_grant_effect,
}];

/// The effect `decision` names: set only for an executed decision of a kind
/// that names one.
pub(crate) fn decision_effect(decision: &StoredDecision) -> Option<DecisionEffect> {
    if decision.state.phase != DecisionPhase::Executed {
        return None;
    }
    let row = KIND_EFFECTS
        .iter()
        .find(|row| row.kind == decision.descriptor.kind.as_str())?;
    (row.effect)(&decision.decision_id)
}

// The `grant.child` kind's row. Its owner, `approval_grants` (with local
// authorization, on native targets), re-exports the kind token and the id
// below and mints the approval-carried grant's id with them; they live here
// so that every reader derives the effect, whatever its features.

/// The decision kind of access offers.
pub const GRANT_CHILD_KIND: &str = "grant.child";

/// The id `decision` derives for `what`.
pub(crate) fn derived_id(decision: &DecisionId, what: &str) -> Option<EvidenceId> {
    EvidenceId::new(format!("{decision}/{what}")).ok()
}

/// The id of the approval-carried grant `decision` issues.
#[must_use]
pub fn approval_grant_id(decision: &DecisionId) -> Option<EvidenceId> {
    derived_id(decision, "approval-grant")
}

/// An executed `grant.child` decision issued its approval-carried grant:
/// its executor records the decision executed only once that grant exists.
fn approval_grant_effect(decision: &DecisionId) -> Option<DecisionEffect> {
    approval_grant_id(decision).map(|grant_id| DecisionEffect::Grant { grant_id })
}
