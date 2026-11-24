//! When a gating request was made, as the Console reads it from the audit:
//! the time of the row that opens the request's action.

use serde_json::json;

use super::GatingRequestTimes;
use crate::runtime::{GatingAuditEntry, GatingOutcome, GatingRiskTier};

fn row(
    event_type: &str,
    action_id: &str,
    timestamp_ms: u64,
    detail: serde_json::Value,
) -> GatingAuditEntry {
    GatingAuditEntry {
        audit_id: format!("audit-{action_id}-{timestamp_ms}"),
        timestamp_ms,
        event_type: event_type.to_string(),
        action_id: action_id.to_string(),
        pending_id: None,
        actor_id: "agent:writer".to_string(),
        risk_tier: GatingRiskTier::R3,
        outcome: GatingOutcome::PendingApproval,
        detail,
    }
}

/// A request's time is the time of the first row its evaluation wrote, not
/// of a later row of the action: the pending row of an escalation
/// successor and a decision are written later, perhaps under another holder
/// of the origin's name. Where the audit no longer retains the opening row,
/// the request's time is not known.
#[test]
fn a_requests_time_is_the_time_of_the_row_that_opens_its_action() {
    let successor = json!({ "escalated_from_pending_id": "gate-pending-1" });
    let audit = [
        // An action evaluated, held, escalated and decided.
        row("evaluated", "action-whole", 10, json!({})),
        row("pending_created", "action-whole", 11, json!({})),
        row("escalation_decided", "action-whole", 50, json!({})),
        row("pending_created", "action-whole", 50, successor.clone()),
        row("approval_decided", "action-whole", 60, json!({})),
        // An action whose opening rows the audit no longer retains.
        row("pending_created", "action-cut", 70, successor),
        row("approval_decided", "action-cut", 75, json!({})),
        // An action that opens with its pending row.
        row("pending_created", "action-held", 80, json!({})),
        // An action a conflict blocked.
        row("conflict_blocked", "action-blocked", 90, json!({})),
        // An action of which only a decision is retained.
        row("rejection_decided", "action-decided", 95, json!({})),
    ];
    let times = GatingRequestTimes::of(&audit, crate::unified_runtime::OtherMobsServed::NotKnown);
    assert_eq!(times.made_at("action-whole"), Some(10));
    assert_eq!(times.made_at("action-cut"), None);
    assert_eq!(times.made_at("action-held"), Some(80));
    assert_eq!(times.made_at("action-blocked"), Some(90));
    assert_eq!(times.made_at("action-decided"), None);
    assert_eq!(times.made_at("action-the-audit-never-held"), None);
}
