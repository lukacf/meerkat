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
//!   kind whose requester may see what it asked for (`tool_grant.issue`,
//!   and a change of an agent: `identity.create`, `identity.update`,
//!   `identity.archive`, `identity.unarchive`, `identity.remove`,
//!   `identity.adopt`); the requester of a child grant (`grant.child`), a
//!   held release (`route.release`), a gating release (`gating.release`) or
//!   any other kind never does. Reading its own request never lets a
//!   person decide it: a person cannot approve their own request, whatever
//!   approver id they hold;
//! - any other caller, an Open Console's anonymous caller included, sees
//!   nothing of the decision: it reads as a decision that is not there.
//!
//! A list pages over what the caller may see, so that it learns of no
//! other decision. An administrator reads the store's own pages. Any other
//! caller reads, through the store's indexed filters, one stream for each
//! approver id it holds and one of its own requests, merged by decision
//! id; its cursor names only a row it was given, and a row the store cannot
//! decode is not shown to it.
//!
//! While access control is enforced, the reads need `decisions.view`. The
//! withdraw is the administrator's, `decisions.admin` while access control
//! is enforced; with access control off, or without an access controller,
//! the Console serves no withdraw (the stdio host withdraws). A withdraw
//! the Console serves is a console change: it passes the caller checks
//! every console change passes (`caller_changed`, `caller_not_current`),
//! and the console audit log records it. The Console decides nothing yet:
//! `meta/decisions/decide` and `present` are not Console methods.

use std::collections::{BTreeMap, BTreeSet};

use meerkat_contracts::{
    DecisionGetParams, DecisionListParams, DecisionRenderingParams, DecisionResult,
    DecisionWithdrawParams,
};
use meerkat_core::approval::decision::{
    ApproverId, DecisionId, DecisionListFilter, DecisionPrincipalKey, StoredDecision,
};
use meerkat_runtime::decision::wire::{wire_decision, wire_unreadable_decision};
use meerkat_runtime::decision::{DecisionError, DecisionWithdrawer, DurableDecisionService};
use serde_json::{Value, json};

use super::owner::RuntimeDecisions;
use super::stdio::{list_filter, page_limit, parse, part_size, read_page, rendering_of};
use super::wire::{
    self, GET_METHOD, LIST_METHOD, MetaDecisionListResult, MetaDecisionStatus, MetaDecisionView,
    RENDERING_METHOD, WITHDRAW_METHOD,
};
use crate::access::{ACTION_DECISIONS_ADMIN, ACTION_DECISIONS_VIEW, AccessView};
use crate::http_console::ConsoleCallerRefusal;

/// The kinds whose requester may read what it asked for: a tool grant
/// request, and a change of an agent (the `identity.*` kinds), whose
/// rendering its requester is shown already when it asks what the change
/// would get.
const REQUESTER_READS_CONTENT: &[&str] = &[
    "tool_grant.issue",
    "identity.create",
    "identity.update",
    "identity.archive",
    "identity.unarchive",
    "identity.remove",
    "identity.adopt",
];

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

    /// Whether access control is enforced for this caller.
    fn enforced(&self) -> bool {
        self.access_view.is_some_and(AccessView::enforced)
    }

    /// Whether the caller reads every decision whole and withdraws:
    /// `decisions.admin` while access control is enforced.
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
    held: BTreeSet<String>,
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
        // A question set is its asking member's: whatever the caller holds
        // or administers, the set of a member hidden from them reads as a
        // decision that is not there.
        if !self.sees_asker(row) {
            return Sight::Nothing;
        }
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

    /// Whether the caller may see the member that asked `row`, when `row`
    /// is a question set (`crate::ask_user`): the rule the gating reads
    /// apply to an entry's origin, so a member private to someone else, or
    /// one the Console does not know while it keeps its people apart, is
    /// hidden. A decision of any other kind has no asker to hide. A
    /// question set whose asker cannot be read is shown only where the
    /// Console keeps nobody apart.
    fn sees_asker(&self, row: &StoredDecision) -> bool {
        match crate::ask_user::question_set_asker(row) {
            crate::ask_user::QuestionSetAsker::NotAQuestionSet => true,
            crate::ask_user::QuestionSetAsker::Asker(asker) => {
                crate::http_console::gating_origin_visible(self.caller.access_view, &asker)
            }
            crate::ask_user::QuestionSetAsker::Unreadable => self.caller.access_view.is_none(),
        }
    }

    /// The listing streams of what this caller may see under `filter`: one
    /// for each approver id it holds, and one of its own requests. A filter
    /// that names an approver id the caller does not hold opens no holder
    /// stream, and one that names another requester opens no request
    /// stream.
    fn streams(&self, filter: &DecisionListFilter) -> Vec<DecisionListFilter> {
        let mut streams = Vec::new();
        for held in &self.held {
            // A held id the store cannot hold names no decision.
            let Ok(approver_id) = ApproverId::new(held.clone()) else {
                continue;
            };
            if filter
                .approver_id
                .as_ref()
                .is_some_and(|wanted| *wanted != approver_id)
            {
                continue;
            }
            streams.push(filter.clone().with_approver_id(approver_id));
        }
        if let Some(requester) = self.caller.requester.as_ref()
            && filter
                .requester
                .as_ref()
                .is_none_or(|wanted| wanted == requester)
        {
            streams.push(filter.clone().with_requester(requester.clone()));
        }
        streams
    }
}

/// The checks a Console change passes, as every console change does: one
/// made for another caller than its token (`expected_caller_scope`) is
/// refused `caller_changed`, and one whose caller's login ended before it
/// is made is refused `caller_not_current`.
pub(crate) struct ConsoleChangeChecks<'a> {
    /// The refusal of a change made for another caller, read with the
    /// request.
    pub(crate) other_caller: Option<ConsoleCallerRefusal>,
    /// Checks again that the caller's login is current: called right before
    /// the change is made.
    pub(crate) caller_current: &'a dyn Fn() -> Result<(), ConsoleCallerRefusal>,
}

/// Whether the Console serves `caller` the decision change `method`: the
/// withdraw, while access control is enforced. With access control off, or
/// without an access controller, nobody administers decisions on the
/// Console and it serves no withdraw. The console audit log records the
/// changes the Console serves.
pub(crate) fn is_served_decision_change(method: &str, caller: &ConsoleDecisionCaller<'_>) -> bool {
    method == WITHDRAW_METHOD && caller.enforced()
}

/// The decision methods the Console offers `caller`: the reads where it may
/// call them, and the withdraw to an administrator on a writable Console.
pub(crate) fn advertised_methods(
    caller: &ConsoleDecisionCaller<'_>,
    read_only: bool,
) -> Vec<&'static str> {
    let mut methods = Vec::new();
    if caller.may(ACTION_DECISIONS_VIEW) {
        methods.extend_from_slice(&[LIST_METHOD, GET_METHOD, RENDERING_METHOD]);
    }
    if !read_only && caller.administers() {
        methods.push(WITHDRAW_METHOD);
    }
    methods
}

/// The answer to a Console decision request, or `None` when the Console
/// serves no such method (it then answers it as any other method: not
/// found). The withdraw is a console change, checked by `change`; its
/// refusals are decided before any store read, and are the same for every
/// decision id.
pub(crate) fn handle_console_decision_rpc(
    decisions: &RuntimeDecisions,
    caller: &ConsoleDecisionCaller<'_>,
    change: &ConsoleChangeChecks<'_>,
    read_only: bool,
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
        // With access control off, or without an access controller, nobody
        // administers decisions on the Console: it serves no withdraw.
        WITHDRAW_METHOD if !is_served_decision_change(method, caller) => return None,
        WITHDRAW_METHOD => withdraw_change(decisions, caller, change, read_only, id, params),
        _ => return None,
    };
    Some(answer)
}

/// The Console withdraw, a console change, refused in this order: made for
/// another caller (`caller_changed`), on a read-only Console (`-32010`), by
/// a caller without `decisions.admin` (`access_denied`), and, checked again
/// right before the withdraw is made, by a caller whose login ended
/// (`caller_not_current`).
fn withdraw_change(
    decisions: &RuntimeDecisions,
    caller: &ConsoleDecisionCaller<'_>,
    change: &ConsoleChangeChecks<'_>,
    read_only: bool,
    id: &Value,
    params: Value,
) -> Value {
    if let Some(refusal) = change.other_caller {
        return caller_refused(id, refusal);
    }
    if read_only {
        return crate::http_console::console_read_only_rpc_error(id.clone());
    }
    if !caller.administers() {
        return access_denied(id, ACTION_DECISIONS_ADMIN);
    }
    if let Err(refusal) = (change.caller_current)() {
        return caller_refused(id, refusal);
    }
    withdraw(decisions, id, params)
}

/// The refusal of a console change for its caller, as the Console's other
/// changes answer it (`-32056`).
fn caller_refused(id: &Value, refusal: ConsoleCallerRefusal) -> Value {
    let error = refusal.json_rpc_error();
    wire::failure(id, error.code, error.message, error.data)
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
    let viewer = Viewer::of(caller);
    if viewer.administers {
        // The administrator reads the store's own page and cursor, and is
        // told of the rows the server cannot read. A question set of a
        // member hidden from them is left out, as every read leaves it out.
        let (_, listing) = match read_page(id, &params, decisions) {
            Ok(page) => page,
            Err(answer) => return answer,
        };
        return wire::success(
            id,
            MetaDecisionListResult {
                decisions: listing
                    .decisions
                    .iter()
                    .filter(|row| viewer.sight(row) != Sight::Nothing)
                    .map(|row| wire_decision(row, false))
                    .collect(),
                statuses: Vec::new(),
                unreadable: listing
                    .unreadable
                    .iter()
                    .map(wire_unreadable_decision)
                    .collect(),
                next_after: listing.next_after.clone(),
            },
        );
    }
    let filter = match list_filter(id, &params) {
        Ok(filter) => filter,
        Err(answer) => return answer,
    };
    let limit = match page_limit(id, &params) {
        Ok(limit) => limit,
        Err(answer) => return answer,
    };
    if !decisions.is_available() {
        return wire::no_decision_store(id);
    }
    let streams = viewer.streams(&filter);
    let (rows, more) =
        match visible_page(decisions.owner(), &streams, params.after.as_deref(), limit) {
            Ok(page) => page,
            Err(error) => return wire::decision_error(id, error),
        };
    let mut page = MetaDecisionListResult {
        decisions: Vec::new(),
        statuses: Vec::new(),
        // A row the server cannot read cannot be shown to be the caller's.
        unreadable: Vec::new(),
        next_after: if more {
            rows.last().map(|row| row.decision_id.to_string())
        } else {
            None
        },
    };
    for row in &rows {
        match viewer.sight(row) {
            Sight::Whole => page.decisions.push(wire_decision(row, false)),
            Sight::Status => page.statuses.push(MetaDecisionStatus::of(row)),
            Sight::Nothing => {}
        }
    }
    wire::success(id, page)
}

/// The first `limit` rows after `after` of the union of `streams`, in
/// decision id order, and whether more may follow.
///
/// Each stream is read through the store's indexed filter, page by page,
/// until it gave `limit` rows the server can read or it ended. The first
/// `limit` rows of the union are then all read: a row of a stream that may
/// hold more lies within that stream's rows read, or after `limit` rows
/// already read.
fn visible_page(
    owner: &DurableDecisionService,
    streams: &[DecisionListFilter],
    after: Option<&str>,
    limit: usize,
) -> Result<(Vec<StoredDecision>, bool), DecisionError> {
    let mut rows: BTreeMap<String, StoredDecision> = BTreeMap::new();
    let mut more = false;
    for stream in streams {
        let mut cursor = after.map(str::to_owned);
        let mut readable = 0_usize;
        loop {
            let listing = owner.list(stream, cursor.as_deref(), limit)?;
            readable += listing.decisions.len();
            for row in listing.decisions {
                rows.entry(row.decision_id.to_string()).or_insert(row);
            }
            match listing.next_after {
                Some(next) if readable < limit => cursor = Some(next),
                Some(_) => {
                    more = true;
                    break;
                }
                None => break,
            }
        }
    }
    let mut page: Vec<StoredDecision> = rows.into_values().collect();
    if page.len() > limit {
        page.truncate(limit);
        more = true;
    }
    Ok((page, more))
}

/// The decision `decision_id` names, as the caller may see it; a decision
/// it may not see at all reads as one that is not there, and so does a row
/// that does not decode, to a caller that does not administer.
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
    let row = match decisions.owner().get(&decision_id) {
        Ok(row) => row,
        // Only an administrator, who reads every decision whole, learns
        // that an unreadable row is there (and its decode error); to any
        // other caller it answers as an id that names no decision.
        Err(DecisionError::Unreadable { .. }) if !caller.administers() => {
            return Err(wire::not_found(id));
        }
        Err(error) => return Err(wire::decision_error(id, error)),
    };
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
        Ok(row) => {
            decisions.notify_work();
            wire::success(
                id,
                DecisionResult {
                    decision: wire_decision(&row, false),
                },
            )
        }
        Err(error) => wire::decision_error(id, error),
    }
}
