//! The Console's decision verbs: `meta/decisions/list`, `get` and
//! `rendering`, and the administrator's `withdraw`.
//!
//! Who sees what of a decision is the decision kind's rule, applied on the
//! Console whatever the access switch, and no access rule widens it:
//!
//! - a holder of the decision's approver id in the approver map reads it
//!   whole, and so, while access control is enforced, does a caller granted
//!   `decisions.admin`;
//! - the requester, a Console human matched by its verified principal,
//!   reads the status of its own request, and reads it whole only for a
//!   kind whose requester may see what it asked for (`tool_grant.issue`);
//!   the requester of a child grant (`grant.child`), a held release
//!   (`route.release`), a gating release (`gating.release`) or any other
//!   kind never does;
//! - any other caller, an Open Console's anonymous caller included, sees
//!   nothing of the decision: it reads as a decision that is not there.
//!
//! While access control is enforced, the reads need `decisions.view` and
//! the withdraw needs `decisions.admin`. The Console decides nothing yet:
//! `meta/decisions/decide` and `present` are not Console methods.

use meerkat_contracts::{
    DecisionGetParams, DecisionListParams, DecisionRenderingParams, DecisionResult,
    DecisionWithdrawParams,
};
use meerkat_core::approval::decision::{DecisionId, DecisionPrincipalKey, StoredDecision};
use meerkat_runtime::decision::DecisionWithdrawer;
use meerkat_runtime::decision::wire::{wire_decision, wire_unreadable_decision};
use serde_json::{Value, json};

use super::owner::RuntimeDecisions;
use super::stdio::{parse, part_size, read_page, rendering_of};
use super::wire::{
    self, GET_METHOD, LIST_METHOD, MetaDecisionListResult, MetaDecisionStatus, MetaDecisionView,
    RENDERING_METHOD, WITHDRAW_METHOD,
};
use crate::access::{ACTION_DECISIONS_ADMIN, ACTION_DECISIONS_VIEW, AccessView};

/// The kinds whose requester may read what it asked for.
const REQUESTER_READS_CONTENT: &[&str] = &["tool_grant.issue"];

/// The Console caller of one decision request.
pub(crate) struct ConsoleDecisionCaller<'a> {
    /// The caller's access snapshot, when an access controller is installed.
    pub(crate) access_view: Option<&'a AccessView>,
    /// The verified Console human's qualified principal, as a requester key.
    pub(crate) requester: Option<DecisionPrincipalKey>,
}

/// What a caller may see of one decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Sight {
    Whole,
    Status,
    Nothing,
}

impl ConsoleDecisionCaller<'_> {
    fn may(&self, action: &str) -> bool {
        self.access_view
            .is_none_or(|view| !view.enforced() || view.allows(action))
    }

    /// Whether the caller reads every decision whole: `decisions.admin`
    /// while access control is enforced.
    fn administers(&self) -> bool {
        self.access_view
            .is_some_and(|view| view.enforced() && view.allows(ACTION_DECISIONS_ADMIN))
    }
}

/// The caller's sight of decisions, fixed for one request: the approver ids
/// it holds in one snapshot of the map.
struct Viewer<'a> {
    caller: &'a ConsoleDecisionCaller<'a>,
    administers: bool,
    held: std::collections::BTreeSet<String>,
}

impl<'a> Viewer<'a> {
    fn of(caller: &'a ConsoleDecisionCaller<'a>) -> Self {
        Self {
            caller,
            administers: caller.administers(),
            held: caller
                .access_view
                .map(AccessView::held_approver_ids)
                .unwrap_or_default(),
        }
    }

    fn sight(&self, row: &StoredDecision) -> Sight {
        if self.administers || self.held.contains(row.descriptor.approver_id.as_str()) {
            return Sight::Whole;
        }
        match self.caller.requester.as_ref() {
            Some(requester) if *requester == row.descriptor.requester => {
                if REQUESTER_READS_CONTENT.contains(&row.descriptor.kind.as_str()) {
                    Sight::Whole
                } else {
                    Sight::Status
                }
            }
            _ => Sight::Nothing,
        }
    }
}

/// The decision methods the Console offers `caller`: the reads where it may
/// call them, and the withdraw on a writable Console.
pub(crate) fn advertised_methods(
    caller: &ConsoleDecisionCaller<'_>,
    read_only: bool,
) -> Vec<&'static str> {
    let mut methods = Vec::new();
    if caller.may(ACTION_DECISIONS_VIEW) {
        methods.extend_from_slice(&[LIST_METHOD, GET_METHOD, RENDERING_METHOD]);
    }
    if !read_only && caller.may(ACTION_DECISIONS_ADMIN) {
        methods.push(WITHDRAW_METHOD);
    }
    methods
}

/// The answer to a Console decision request, or `None` when `method` is no
/// Console decision method (the Console then answers it as any other
/// method: not found).
pub(crate) fn handle_console_decision_rpc(
    decisions: &RuntimeDecisions,
    caller: &ConsoleDecisionCaller<'_>,
    method: &str,
    id: &Value,
    params: Value,
) -> Option<Value> {
    let answer = match method {
        LIST_METHOD | GET_METHOD | RENDERING_METHOD if !caller.may(ACTION_DECISIONS_VIEW) => {
            access_denied(id, ACTION_DECISIONS_VIEW)
        }
        LIST_METHOD => list(decisions, caller, id, params),
        GET_METHOD => get(decisions, caller, id, params),
        RENDERING_METHOD => rendering(decisions, caller, id, params),
        WITHDRAW_METHOD if !caller.may(ACTION_DECISIONS_ADMIN) => {
            access_denied(id, ACTION_DECISIONS_ADMIN)
        }
        WITHDRAW_METHOD => withdraw(decisions, id, params),
        _ => return None,
    };
    Some(answer)
}

fn access_denied(id: &Value, action: &str) -> Value {
    wire::failure(
        id,
        crate::http_console::ACCESS_DENIED_RPC_CODE,
        format!("access denied: {action}"),
        Some(json!({"kind": "access_denied", "action": action})),
    )
}

fn list(
    decisions: &RuntimeDecisions,
    caller: &ConsoleDecisionCaller<'_>,
    id: &Value,
    params: Value,
) -> Value {
    let params: DecisionListParams = match params {
        Value::Null => DecisionListParams::default(),
        params => match parse(id, params) {
            Ok(params) => params,
            Err(answer) => return answer,
        },
    };
    let (_, listing) = match read_page(id, &params, decisions) {
        Ok(page) => page,
        Err(answer) => return answer,
    };
    let viewer = Viewer::of(caller);
    let mut page = MetaDecisionListResult {
        decisions: Vec::new(),
        statuses: Vec::new(),
        // A row the server cannot read shows nothing of whom it concerns:
        // only a caller who reads every decision is told of it.
        unreadable: if viewer.administers {
            listing
                .unreadable
                .iter()
                .map(wire_unreadable_decision)
                .collect()
        } else {
            Vec::new()
        },
        next_after: listing.next_after.clone(),
    };
    for row in &listing.decisions {
        match viewer.sight(row) {
            Sight::Whole => page.decisions.push(wire_decision(row, false)),
            Sight::Status => page.statuses.push(MetaDecisionStatus::of(row)),
            Sight::Nothing => {}
        }
    }
    wire::success(id, page)
}

/// The decision `decision_id` names, as the caller may see it; a decision
/// it may not see at all reads as one that is not there.
fn visible(
    decisions: &RuntimeDecisions,
    caller: &ConsoleDecisionCaller<'_>,
    id: &Value,
    decision_id: String,
) -> Result<(StoredDecision, Sight), Value> {
    let decision_id = DecisionId::new(decision_id)
        .map_err(|error| wire::invalid_params(id, format!("invalid decision request: {error}")))?;
    if !decisions.is_available() {
        return Err(wire::no_decision_store(id));
    }
    let row = decisions
        .owner()
        .get(&decision_id)
        .map_err(|error| wire::decision_error(id, error))?;
    match Viewer::of(caller).sight(&row) {
        Sight::Nothing => Err(wire::not_found(id)),
        sight => Ok((row, sight)),
    }
}

fn get(
    decisions: &RuntimeDecisions,
    caller: &ConsoleDecisionCaller<'_>,
    id: &Value,
    params: Value,
) -> Value {
    let params: DecisionGetParams = match parse(id, params) {
        Ok(params) => params,
        Err(answer) => return answer,
    };
    match visible(decisions, caller, id, params.decision_id) {
        Ok((row, Sight::Whole)) => wire::success(
            id,
            MetaDecisionView::Decision(Box::new(wire_decision(&row, params.include_request))),
        ),
        Ok((row, _)) => wire::success(id, MetaDecisionView::Status(MetaDecisionStatus::of(&row))),
        Err(answer) => answer,
    }
}

fn rendering(
    decisions: &RuntimeDecisions,
    caller: &ConsoleDecisionCaller<'_>,
    id: &Value,
    params: Value,
) -> Value {
    let params: DecisionRenderingParams = match parse(id, params) {
        Ok(params) => params,
        Err(answer) => return answer,
    };
    let max_part_bytes = match part_size(id, params.max_part_bytes) {
        Ok(size) => size,
        Err(answer) => return answer,
    };
    match visible(decisions, caller, id, params.decision_id) {
        Ok((row, Sight::Whole)) => wire::success(id, rendering_of(&row, max_part_bytes)),
        Ok(_) => wire::failure(
            id,
            crate::http_console::ACCESS_DENIED_RPC_CODE,
            "the caller may see this decision's status, not what it holds",
            Some(json!({"kind": "content_not_visible"})),
        ),
        Err(answer) => answer,
    }
}

/// The administrator withdraws an open decision as the host.
fn withdraw(decisions: &RuntimeDecisions, id: &Value, params: Value) -> Value {
    let params: DecisionWithdrawParams = match parse(id, params) {
        Ok(params) => params,
        Err(answer) => return answer,
    };
    let decision_id = match DecisionId::new(params.decision_id) {
        Ok(decision_id) => decision_id,
        Err(error) => {
            return wire::invalid_params(id, format!("invalid decision request: {error}"));
        }
    };
    if !decisions.is_available() {
        return wire::no_decision_store(id);
    }
    match decisions
        .owner()
        .withdraw(&decision_id, &DecisionWithdrawer::Host)
    {
        Ok(row) => wire::success(
            id,
            DecisionResult {
                decision: wire_decision(&row, false),
            },
        ),
        Err(error) => wire::decision_error(id, error),
    }
}
