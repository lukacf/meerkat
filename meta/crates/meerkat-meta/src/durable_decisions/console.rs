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
//! A held change of an agent (the `identity.*` kinds) is its mob's. One
//! whose agent is of a mob other than the Console's launch mob is read, by
//! whichever of the rules above, only by a caller who may address that
//! mob, as the Console's request doors judge it; to every other caller it
//! reads as a decision that is not there. A caller who addresses the mob
//! by an access rule, and may not address every mob, reads it only while
//! the mob it is held for stands under the id: the incarnation its body
//! names is the incarnation of the mob that stands there, which the door
//! reads without loading the mob. Where no mob stands under the id, where
//! the standing mob's incarnation is not read, and where the body names
//! none, it reads to that caller as a decision that is not there.
//!
//! A list pages over what the caller may see, so that it learns of no
//! other decision: a row it may not see fills no place in a page and is
//! never a cursor, and the request's filters choose among the rows it may
//! see. An administrator reads every decision the filters name. Any other
//! caller reads, through the store's indexed filters, one stream for each
//! approver id it holds and one of its own requests, merged by decision
//! id. A cursor names only a row its caller was given. A row the store
//! cannot decode is shown only to an administrator who is not kept apart
//! from other people (an access administrator): it cannot be shown to be
//! one a caller may see.
//!
//! While access control is enforced, the reads need `decisions.view`. The
//! withdraw is the administrator's, `decisions.admin` while access control
//! is enforced; with access control off, or without an access controller,
//! the Console serves no withdraw (the stdio host withdraws). A withdraw
//! the Console serves is a console change: it passes the caller checks
//! every console change passes (`caller_changed`, `caller_not_current`),
//! and the console audit log records it. The Console decides nothing yet:
//! `meta/decisions/decide` and `present` are not Console methods.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};

use meerkat_contracts::{
    DecisionGetParams, DecisionListParams, DecisionRenderingParams, DecisionRenderingResult,
    DecisionResult, DecisionWithdrawParams,
};
use meerkat_core::approval::decision::{
    ApproverId, DecisionId, DecisionListFilter, DecisionPhase, DecisionPrincipalKey,
    StoredDecision, UnreadableDecision,
};
use meerkat_runtime::ask_user::ASK_USER_QUESTIONS_KIND;
use meerkat_runtime::decision::wire::{wire_decision, wire_unreadable_decision};
use meerkat_runtime::decision::{DecisionError, DecisionWithdrawer, DurableDecisionService};
use serde::Serialize;
use serde_json::{Value, json};

use super::owner::RuntimeDecisions;
use super::stdio::{list_filter, page_limit, parse, part_size, rendering_of};
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
    /// The mob this Console serves to every caller it lets in (its launch
    /// mob), when it serves one: a held change of an agent of any other
    /// mob is read only by a caller who may address that mob.
    pub(crate) launch_mob: Option<meerkat_mob::MobId>,
}

/// The incarnation of the mob that stands under each mob id the door read
/// for one decision request, by mob id: `None` where no mob stands under
/// the id or its incarnation is not read. The door reads an id when the
/// request's answer needs it ([`ConsoleDecisionAnswer::StandingMobsToRead`])
/// and no other.
pub(crate) type StandingMobs = BTreeMap<String, Option<String>>;

/// What the Console's decision methods answer one pass over a request.
pub(crate) enum ConsoleDecisionAnswer {
    /// The answer to the request.
    Answer(Value),
    /// No answer yet: what the caller may see depends on the mobs that
    /// stand under these ids, which the door has not read for this
    /// request. The door reads each once and asks again with them. Nothing
    /// was changed, and nothing of the pass is given to the caller.
    StandingMobsToRead(BTreeSet<String>),
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

    /// Whether the caller is kept apart from other people on this Console
    /// (an access administrator is not, and nobody is where the Console
    /// identifies no person).
    fn kept_apart(&self) -> bool {
        self.access_view
            .is_some_and(AccessView::kept_apart_from_other_people)
    }

    /// Whether the caller is told of the rows the store cannot decode: an
    /// administrator nobody is kept from. Such a row cannot be shown to be
    /// one the caller may see.
    fn reads_undecodable(&self) -> bool {
        self.administers() && !self.kept_apart()
    }

    /// Whether the caller cannot be told yet whose a question set is: who
    /// held its asker's name when it was asked is still being read
    /// (`crate::http_console::gating_origins_not_read`). Every request of
    /// such a caller is answered that it must ask again, whatever the store
    /// holds: an answer that depended on the store would tell the caller
    /// of what it may not see.
    fn awaits_askers(&self) -> bool {
        crate::http_console::gating_origins_not_read(self.access_view)
    }
}

/// The answer to a caller who cannot be told yet whose a question set is:
/// the Console's retryable refusal
/// (`crate::http_console::gating_origins_not_read_error`).
fn askers_not_read(id: &Value) -> Value {
    let error = crate::http_console::gating_origins_not_read_error();
    wire::failure(id, error.code, error.message, error.data)
}

/// The caller's sight of decisions, fixed for one request: the approver ids
/// it holds in one snapshot of the map.
struct Viewer<'a> {
    caller: &'a ConsoleDecisionCaller<'a>,
    administers: bool,
    held: BTreeSet<String>,
    /// The standing mobs the door read for this request.
    standing_mobs: &'a StandingMobs,
    /// The mob ids whose standing mob this pass needed and the door has
    /// not read: while it holds one, the pass gives no answer
    /// ([`Self::standing_mobs_to_read`]).
    unread_mobs: RefCell<BTreeSet<String>>,
}

impl<'a> Viewer<'a> {
    fn of(caller: &'a ConsoleDecisionCaller<'a>, standing_mobs: &'a StandingMobs) -> Self {
        Self {
            caller,
            administers: caller.administers(),
            held: caller
                .access_view
                .map(AccessView::held_approver_ids)
                .unwrap_or_default(),
            standing_mobs,
            unread_mobs: RefCell::new(BTreeSet::new()),
        }
    }

    /// Whether this pass needed a standing mob the door has not read: it
    /// then gives no answer and changes nothing.
    fn awaits_standing_mobs(&self) -> bool {
        !self.unread_mobs.borrow().is_empty()
    }

    /// The mob ids whose standing mob this pass needed and the door has
    /// not read.
    fn standing_mobs_to_read(self) -> BTreeSet<String> {
        self.unread_mobs.into_inner()
    }

    fn sight(&self, row: &StoredDecision) -> Sight {
        // A question set is its asking member's: whatever the caller holds
        // or administers, the set of a member hidden from them reads as a
        // decision that is not there.
        if !self.sees_asker(row) {
            return Sight::Nothing;
        }
        let by_the_kinds_rule =
            if self.administers || self.held.contains(row.descriptor.approver_id.as_str()) {
                Sight::Whole
            } else {
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
            };
        // A held change of an agent is its mob's: whatever the caller
        // holds or administers, the change of an agent of a mob the caller
        // may not address reads as a decision that is not there. The mob is
        // asked last, of a decision the caller would see: a standing mob is
        // read for no other.
        if by_the_kinds_rule == Sight::Nothing || !self.addresses_mob_of(row) {
            return Sight::Nothing;
        }
        by_the_kinds_rule
    }

    /// Whether the caller may see the member that asked `row`, when `row`
    /// is a question set (`crate::ask_user`): the rule the gating reads
    /// apply to an entry's origin, so a member private to someone else, or
    /// one the Console does not know while it keeps its people apart, is
    /// hidden. A decision of any other kind has no asker to hide. A
    /// question set whose asker cannot be read is shown only to a caller
    /// who is not kept apart from other people.
    fn sees_asker(&self, row: &StoredDecision) -> bool {
        match crate::ask_user::question_set_asker(row) {
            crate::ask_user::QuestionSetAsker::NotAQuestionSet => true,
            crate::ask_user::QuestionSetAsker::Asker(asker) => {
                // A set is its asking member's: decided by who held the
                // asker's name when the set was asked.
                crate::http_console::gating_origin_visible(
                    self.caller.access_view,
                    &asker,
                    Some(row.descriptor.created_at_ms),
                )
            }
            crate::ask_user::QuestionSetAsker::Unreadable => !self.caller.kept_apart(),
        }
    }

    /// Whether the caller may address the mob whose agent `row` changes,
    /// when `row` is a held change of an agent (the `identity.*` kinds):
    /// the mob the host's family wrote into the held body. The Console's
    /// launch mob is every caller's; any other mob of the host is the
    /// caller's only when its access view lets it address that mob
    /// ([`AccessView::may_address_other_mob`]: access is not enforced, the
    /// caller is an administrator, or an allow rule that applies to it
    /// selects the mob), which is what the Console's request doors ask
    /// before they serve a request for that mob. A decision of any other
    /// kind names no mob here. The mob is the body's own `mob_id`, read
    /// whether or not this version reads the rest of the body. A Console
    /// that serves no mob has no other mob to keep.
    ///
    /// A caller who may address every mob (access is not enforced, or the
    /// caller is an administrator) is asked nothing more. A caller an allow
    /// rule lets address the mob is served only the changes held for the
    /// mob that stands under the id: the body's own `mob_incarnation` is
    /// the standing mob's ([`StandingMobs`]). A body that names no
    /// incarnation, an id no mob stands under and a standing mob whose
    /// incarnation is not read serve nothing. Where the door has not read
    /// the id for this request, the id is noted for it to read, and the
    /// pass, which gives no answer, goes on as if the mob were the
    /// caller's: a list then reads no further than its answer will.
    fn addresses_mob_of(&self, row: &StoredDecision) -> bool {
        if !crate::identity_creation::IDENTITY_DECISION_KINDS
            .contains(&row.descriptor.kind.as_str())
        {
            return true;
        }
        let Some(launch_mob) = self.caller.launch_mob.as_ref() else {
            return true;
        };
        // The mob is read from the body's own `mob_id` and from nothing
        // else of it: a body this version does not read whole still names
        // its mob, and is judged by it. A body that names no mob is no
        // held change any version of the family wrote: it has no mob to
        // keep, and is read by the rules above alone.
        let body = serde_json::from_str::<Value>(row.descriptor.request.get()).ok();
        let named = |member: &str| {
            body.as_ref()
                .and_then(|body| body.get(member))
                .and_then(Value::as_str)
        };
        let Some(mob_id) = named("mob_id") else {
            return true;
        };
        if mob_id == launch_mob.as_str() {
            return true;
        }
        let Some(view) = self.caller.access_view else {
            return false;
        };
        if view.may_address_every_mob() {
            return true;
        }
        if !view.may_address_other_mob(&meerkat_mob::MobId::from(mob_id)) {
            return false;
        }
        let Some(held_for) = named("mob_incarnation") else {
            return false;
        };
        match self.standing_mobs.get(mob_id) {
            Some(standing) => standing.as_deref() == Some(held_for),
            None => {
                self.unread_mobs.borrow_mut().insert(mob_id.to_string());
                true
            }
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
/// refusals of the caller are decided before any store read, and are the
/// same for every decision id. A decision the caller may not see at all is
/// then answered as an id that names no decision.
///
/// `standing_mobs` holds the standing mobs the door read for this request,
/// none at first. A pass whose answer depends on a mob id that is not among
/// them answers [`ConsoleDecisionAnswer::StandingMobsToRead`] in place of
/// the answer, having changed nothing; the door reads those ids and asks
/// again.
#[allow(clippy::too_many_arguments)]
pub(crate) fn handle_console_decision_rpc(
    decisions: &RuntimeDecisions,
    caller: &ConsoleDecisionCaller<'_>,
    standing_mobs: &StandingMobs,
    change: &ConsoleChangeChecks<'_>,
    read_only: bool,
    method: &str,
    id: &Value,
    params: Value,
) -> Option<ConsoleDecisionAnswer> {
    let viewer = Viewer::of(caller, standing_mobs);
    let answer = match method {
        LIST_METHOD | GET_METHOD | RENDERING_METHOD if !caller.may(ACTION_DECISIONS_VIEW) => {
            access_denied(id, ACTION_DECISIONS_VIEW)
        }
        LIST_METHOD | GET_METHOD | RENDERING_METHOD if caller.awaits_askers() => {
            askers_not_read(id)
        }
        LIST_METHOD => list(decisions, &viewer, id, params),
        GET_METHOD => get(decisions, &viewer, id, params),
        RENDERING_METHOD => rendering(decisions, &viewer, id, params),
        // With access control off, or without an access controller, nobody
        // administers decisions on the Console: it serves no withdraw.
        WITHDRAW_METHOD if !is_served_decision_change(method, caller) => return None,
        WITHDRAW_METHOD => withdraw_change(decisions, &viewer, change, read_only, id, params),
        _ => return None,
    };
    // A pass that needed a standing mob the door has not read gives its
    // caller nothing: what it built judged that mob's decisions as the
    // caller's.
    let to_read = viewer.standing_mobs_to_read();
    Some(if to_read.is_empty() {
        ConsoleDecisionAnswer::Answer(answer)
    } else {
        ConsoleDecisionAnswer::StandingMobsToRead(to_read)
    })
}

/// The Console withdraw, a console change, refused in this order: made for
/// another caller (`caller_changed`), on a read-only Console (`-32010`), by
/// a caller without `decisions.admin` (`access_denied`), and, checked again
/// right before the withdraw is made, by a caller whose login ended
/// (`caller_not_current`).
fn withdraw_change(
    decisions: &RuntimeDecisions,
    viewer: &Viewer<'_>,
    change: &ConsoleChangeChecks<'_>,
    read_only: bool,
    id: &Value,
    params: Value,
) -> Value {
    let caller = viewer.caller;
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
    withdraw(decisions, viewer, id, params)
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

fn list(decisions: &RuntimeDecisions, viewer: &Viewer<'_>, id: &Value, params: Value) -> Value {
    let caller = viewer.caller;
    let params: DecisionListParams = match params {
        Value::Null => DecisionListParams::default(),
        params => match parse(id, params) {
            Ok(params) => params,
            Err(answer) => return answer,
        },
    };
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
    // An administrator reads every decision the request's filter names:
    // one stream. Any other caller reads the streams of what it holds and
    // of what it asked for. Either way the page is of rows the caller may
    // see: a row it may not see fills no place in it and is no cursor.
    let streams = if viewer.administers {
        vec![filter]
    } else {
        viewer.streams(&filter)
    };
    let read = match visible_page(
        decisions.owner(),
        &streams,
        params.after.as_deref(),
        limit,
        |row| viewer.sight(row) != Sight::Nothing,
    ) {
        Ok(read) => read,
        Err(error) => return wire::decision_error(id, error),
    };
    let mut page = MetaDecisionListResult {
        decisions: Vec::new(),
        statuses: Vec::new(),
        // A row the server cannot read cannot be shown to be the caller's:
        // only an administrator nobody is kept from is told of it.
        unreadable: if caller.reads_undecodable() {
            read.undecodable
                .iter()
                .map(wire_unreadable_decision)
                .collect()
        } else {
            Vec::new()
        },
        next_after: if read.more {
            read.rows.last().map(|row| row.decision_id.to_string())
        } else {
            None
        },
    };
    for row in &read.rows {
        match viewer.sight(row) {
            Sight::Whole => page.decisions.push(wire_decision(row, false)),
            Sight::Status => page.statuses.push(MetaDecisionStatus::of(row)),
            Sight::Nothing => {}
        }
    }
    wire::success(id, page)
}

/// A page of the rows a caller may see.
struct VisiblePage {
    /// The rows, in decision id order.
    rows: Vec<StoredDecision>,
    /// The rows of the page's stretch the store cannot decode.
    undecodable: Vec<UnreadableDecision>,
    /// Whether a further row the caller may see follows the last one.
    more: bool,
}

/// The first `limit` rows after `after` of the union of `streams` that
/// `shown` says the caller may see, in decision id order, and whether a
/// further such row follows.
///
/// Each stream is read through the store's indexed filter, page by page,
/// until it gave one row more than `limit` that the caller may see, or it
/// ended. The first `limit + 1` such rows of the union are then all read: a
/// row of a stream that may hold more lies within that stream's rows read,
/// or after `limit + 1` rows already read. A row the caller may not see
/// fills no place and is never the page's last row, so the cursor a caller
/// is given names a row it was given, and a page tells it nothing of the
/// rows between. A row the store cannot decode belongs to the page whose
/// stretch it lies in: up to the last row given when more follow.
fn visible_page(
    owner: &DurableDecisionService,
    streams: &[DecisionListFilter],
    after: Option<&str>,
    limit: usize,
    shown: impl Fn(&StoredDecision) -> bool,
) -> Result<VisiblePage, DecisionError> {
    let mut rows: BTreeMap<String, StoredDecision> = BTreeMap::new();
    let mut undecodable: BTreeMap<String, UnreadableDecision> = BTreeMap::new();
    for stream in streams {
        let mut cursor = after.map(str::to_owned);
        let mut given = 0_usize;
        loop {
            let listing = owner.list(stream, cursor.as_deref(), limit.saturating_add(1))?;
            for row in listing.unreadable {
                undecodable.entry(row.decision_id.clone()).or_insert(row);
            }
            for row in listing.decisions {
                if shown(&row) {
                    given += 1;
                    rows.entry(row.decision_id.to_string()).or_insert(row);
                }
            }
            match listing.next_after {
                Some(next) if given <= limit => cursor = Some(next),
                _ => break,
            }
        }
    }
    let mut page: Vec<StoredDecision> = rows.into_values().collect();
    let more = page.len() > limit;
    page.truncate(limit);
    let last = page
        .last()
        .filter(|_| more)
        .map(|row| row.decision_id.to_string());
    let undecodable = undecodable
        .into_values()
        .filter(|row| last.as_ref().is_none_or(|last| row.decision_id <= *last))
        .collect();
    Ok(VisiblePage {
        rows: page,
        undecodable,
        more,
    })
}

/// The decision `decision_id` names, as the caller may see it; a decision
/// it may not see at all reads as one that is not there, and so does a row
/// that does not decode, to a caller that does not administer.
fn visible(
    decisions: &RuntimeDecisions,
    viewer: &Viewer<'_>,
    id: &Value,
    decision_id: String,
) -> Result<(StoredDecision, Sight), Value> {
    let caller = viewer.caller;
    let decision_id = DecisionId::new(decision_id)
        .map_err(|error| wire::invalid_params(id, format!("invalid decision request: {error}")))?;
    if !decisions.is_available() {
        return Err(wire::no_decision_store(id));
    }
    let row = match decisions.owner().get(&decision_id) {
        Ok(row) => row,
        // Only an administrator nobody is kept from, who is told of the
        // rows the store cannot decode, learns that such a row is there
        // (and its decode error); to any other caller it answers as an id
        // that names no decision.
        Err(DecisionError::Unreadable { .. }) if !caller.reads_undecodable() => {
            return Err(wire::not_found(id));
        }
        Err(error) => return Err(wire::decision_error(id, error)),
    };
    match viewer.sight(&row) {
        Sight::Nothing => Err(wire::not_found(id)),
        sight => Ok((row, sight)),
    }
}

fn get(decisions: &RuntimeDecisions, viewer: &Viewer<'_>, id: &Value, params: Value) -> Value {
    let params: DecisionGetParams = match parse(id, params) {
        Ok(params) => params,
        Err(answer) => return answer,
    };
    match visible(decisions, viewer, id, params.decision_id) {
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
    viewer: &Viewer<'_>,
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
    match visible(decisions, viewer, id, params.decision_id) {
        Ok((row, Sight::Whole)) => wire::success(id, console_rendering(&row, max_part_bytes)),
        Ok(_) => wire::failure(
            id,
            crate::http_console::ACCESS_DENIED_RPC_CODE,
            "the caller may see this decision's status, not what it holds",
            Some(json!({"kind": "content_not_visible"})),
        ),
        Err(answer) => answer,
    }
}

/// What the Console adds to the end of the rendering of an open question
/// set: a blank line and a closing note. A Console page of an earlier
/// release draws a question set, where it shows one at all, as the text of
/// a decision with no way to answer; the note says where the questions are
/// answered and what to do on such a page.
const QUESTION_SET_CLOSING_NOTE: &str = "\n\nAnswer these questions in the Console, in the agent's chat or under Approvals. If this page offers no way to answer them, it runs an earlier Console: reload the page, or, in an application that builds the Console in, update its Console.";

/// The Console's `meta/decisions/rendering` answer: the held rendering as
/// every door answers it and, for an open question set, the closing note
/// after it.
#[derive(Serialize)]
struct ConsoleDecisionRendering {
    #[serde(flatten)]
    held: DecisionRenderingResult,
    /// Exactly what was added to the end of `rendering`, the blank line
    /// included, so that a reader takes it off again: a current Console
    /// page does, and says in its own words where a set is answered.
    #[serde(skip_serializing_if = "Option::is_none")]
    rendering_suffix: Option<&'static str>,
}

/// The rendering of `row` as the Console answers it. The note is added
/// here, after the answer is built from the held text, and nowhere else:
/// the decision's rendering, its digest (the one a decide compares) and its
/// parts stay those of the held text, and the stdio host's answer never
/// carries the note. Only an open decision of kind `ask_user.questions`
/// gets it; a set that ended, a set past its deadline (which the read finds
/// ended) and every other kind are answered as they are held.
fn console_rendering(
    row: &StoredDecision,
    max_part_bytes: Option<std::num::NonZeroU32>,
) -> ConsoleDecisionRendering {
    let mut held = rendering_of(row, max_part_bytes);
    let open_question_set = row.descriptor.kind.as_str() == ASK_USER_QUESTIONS_KIND
        && row.state.phase == DecisionPhase::Open;
    let rendering_suffix = open_question_set.then_some(QUESTION_SET_CLOSING_NOTE);
    if let Some(note) = rendering_suffix {
        held.rendering.push_str(note);
    }
    ConsoleDecisionRendering {
        held,
        rendering_suffix,
    }
}

/// The administrator withdraws an open decision as the host. A decision
/// the caller may not see at all (for an administrator: the question set of
/// a member hidden from them, and, unless the caller is told of such rows,
/// a row the store cannot decode) is not there to withdraw: its id is
/// answered as an id that names no decision, as the reads answer it.
fn withdraw(decisions: &RuntimeDecisions, viewer: &Viewer<'_>, id: &Value, params: Value) -> Value {
    let caller = viewer.caller;
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
    // A caller who cannot be told yet whose a question set is asks again,
    // before any store is read.
    if caller.awaits_askers() {
        return askers_not_read(id);
    }
    if !decisions.is_available() {
        return wire::no_decision_store(id);
    }
    // The read records nothing: the withdraw's own touch still answers
    // what it records (the expiry of a decision past its deadline).
    let read = decisions.owner().for_candidate_launch().get(&decision_id);
    let hidden = match read {
        Ok(row) => viewer.sight(&row) == Sight::Nothing,
        Err(DecisionError::Unreadable { .. }) => !caller.reads_undecodable(),
        Err(_) => false,
    };
    if hidden {
        return wire::not_found(id);
    }
    // Whether the decision is the caller's to withdraw depends on a
    // standing mob the door has not read: nothing is withdrawn in this
    // pass, which gives no answer (`handle_console_decision_rpc`).
    if viewer.awaits_standing_mobs() {
        return Value::Null;
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

#[cfg(test)]
#[path = "console_standing_mob_tests.rs"]
mod standing_mob_tests;
