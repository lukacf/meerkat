//! The console's side of the console audit log ([`crate::console_audit`]):
//! which console requests write a row, the row a request writes, and the
//! `meta/console/audit` read.

use std::collections::BTreeSet;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use meerkat_mob::MobId;
use serde_json::Value;

use super::{
    CONSOLE_RPC_MEMBER_TARGET_FIELDS, ConsoleHttpAuthContext, ConsoleJsonState, JsonRpcError,
    JsonRpcRequest, MobsSelector, console_method_member_fields_or_subject, invalid_params,
    is_console_mutating_rpc_method, response_value,
};
use crate::access::{AccessController, AccessView};
use crate::console_audit::{
    CONSOLE_AUDIT_DEFAULT_PAGE, CONSOLE_AUDIT_MAX_PAGE, ConsoleAuditActor, ConsoleAuditEntry,
    ConsoleAuditLog, ConsoleAuditRow, ConsoleAuditSet, Identifiers, TARGET_MOB_FIELD, audit_target,
    bounded,
};
use crate::rpc::mob_addressing::{AddressedMob, NamedMob, judge_named_mob};
use crate::runtime::AuthenticatedConsoleHuman;

/// How long a console change's answer waits for its audit row; past it the
/// console answers, and the row still lands.
const ROW_WAIT: Duration = Duration::from_secs(2);

/// The route a console MCP app tool call takes, the `method` its rows name.
pub(crate) const MCP_APP_CALL_TOOL_ROUTE: &str = "/console/mcp-apps/call-tool";

/// Whether `method` is a console change the audit log records: every
/// mutating console method but conversation traffic (sends and their
/// recovery, voice and live media), whose record is the transcript. A
/// durable decision withdraw is a console change too, recorded where
/// `POST /console/rpc` serves it ([`ConsoleAuditIntent::of_console_rpc`]).
pub(super) fn is_audited_console_method(method: &str) -> bool {
    is_console_mutating_rpc_method(method)
        && !matches!(
            method,
            "meta/console/send"
                | "meta/console/recover_send"
                | crate::console_voice::VOICE_OPEN_METHOD
                | "live/webrtc/answer"
        )
        && !method.starts_with("meta/live/")
}

/// A console change the audit log records once its answer is known.
pub(crate) struct ConsoleAuditIntent {
    log: Arc<ConsoleAuditLog>,
    set: ConsoleAuditSet,
    actor: ConsoleAuditActor,
    method: String,
    target: Identifiers,
}

impl ConsoleAuditIntent {
    /// The intent to record `request` by the caller `auth`, when the console
    /// keeps an audit log and the request is a change it records
    /// ([`is_audited_console_method`]). Read before the request is handled:
    /// the target is the request's own.
    pub(super) fn of(
        state: &ConsoleJsonState,
        auth: &ConsoleHttpAuthContext,
        request: &JsonRpcRequest,
    ) -> Option<Self> {
        let log = state.console_audit.as_ref()?;
        is_audited_console_method(&request.method).then(|| {
            Self::new(
                Arc::clone(log),
                auth,
                request.method.clone(),
                target_of_a_request(request),
            )
        })
    }

    /// The intent to record a `POST /console/rpc` request, as [`Self::of`],
    /// and a durable decision change the Console serves the caller there
    /// (the withdraw, while access control is enforced).
    pub(super) fn of_console_rpc(
        state: &ConsoleJsonState,
        auth: &ConsoleHttpAuthContext,
        request: &JsonRpcRequest,
    ) -> Option<Self> {
        Self::of(state, auth, request).or_else(|| {
            let log = state.console_audit.as_ref()?;
            crate::durable_decisions::console::is_served_decision_change(
                &request.method,
                &super::console_decision_caller(state, auth),
            )
            .then(|| {
                Self::new(
                    Arc::clone(log),
                    auth,
                    request.method.clone(),
                    audit_target(&request.params),
                )
            })
        })
    }

    /// The intent to record the change `method` on `target` by the caller
    /// `auth` in `log`.
    pub(crate) fn new(
        log: Arc<ConsoleAuditLog>,
        auth: &ConsoleHttpAuthContext,
        method: String,
        target: Identifiers,
    ) -> Self {
        Self {
            log,
            set: ConsoleAuditSet::Changes,
            actor: audit_actor(auth),
            method,
            target,
        }
    }

    /// The intent to record an MCP app tool call on `target` by the caller
    /// `auth` in `log`, among its tool calls.
    pub(super) fn tool_call(
        log: Arc<ConsoleAuditLog>,
        auth: &ConsoleHttpAuthContext,
        target: Identifiers,
    ) -> Self {
        Self {
            log,
            set: ConsoleAuditSet::ToolCalls,
            actor: audit_actor(auth),
            method: MCP_APP_CALL_TOOL_ROUTE.to_string(),
            target,
        }
    }

    /// Make the change `change` and write its row, both on the server: a
    /// caller that goes away cuts neither short, and the row records the
    /// outcome the change reached ([`ConsoleAuditEntry`] from `row_of`). The
    /// change runs on a task of its own, so when it panics its row is the
    /// row of `lost`, the answer the caller then gets. The answer comes back
    /// once the row is written, or once [`ROW_WAIT`] passed, whichever is
    /// first; the row still lands. `None` only when the task writing the row
    /// ended without an answer.
    pub(crate) async fn run<T, F>(
        self,
        change: F,
        lost: T,
        row_of: fn(ConsoleAuditActor, String, Identifiers, &T) -> ConsoleAuditEntry,
    ) -> Option<T>
    where
        T: Send + 'static,
        F: Future<Output = T> + Send + 'static,
    {
        let (answer_sender, answer) = tokio::sync::oneshot::channel();
        let (written_sender, written) = tokio::sync::oneshot::channel::<()>();
        let Self {
            log,
            set,
            actor,
            method,
            target,
        } = self;
        tokio::spawn(async move {
            let answered = tokio::spawn(change).await.unwrap_or(lost);
            let entry = row_of(actor, method, target, &answered);
            let _ = answer_sender.send(answered);
            log.record_in(set, entry).await;
            let _ = written_sender.send(());
        });
        let answered = answer.await.ok()?;
        let _ = tokio::time::timeout(ROW_WAIT, written).await;
        Some(answered)
    }
}

/// The target of a request's row: the identifiers among its params
/// ([`audit_target`]), and for a create over the mobs of the host the mob
/// the create is of. A create names its mob in the definition it carries,
/// which is content a row does not keep, so its row holds the definition's
/// id as its mob ([`TARGET_MOB_FIELD`]): the id the family's door judges
/// with the caller ([`crate::rpc::mob_hosting::mob_a_request_names`], asked
/// of the request without the addressing `mob_id` the door takes off it).
/// The row is then shown as a row of that mob is
/// ([`rows_mob_as_shown_to`]). A create whose definition does not read, and
/// one whose id a row cannot keep whole, names a blank mob: such a row is
/// an administrator's alone.
fn target_of_a_request(request: &JsonRpcRequest) -> Identifiers {
    let mut target = audit_target(&request.params);
    if request.method != crate::rpc::mob_hosting::MOBS_CREATE {
        return target;
    }
    let mut params = request.params.clone();
    if let Some(members) = params.as_object_mut() {
        members.remove(TARGET_MOB_FIELD);
    }
    let named = match crate::rpc::mob_hosting::mob_a_request_names(&request.method, &params) {
        Some(crate::rpc::mob_hosting::RequestsMob::Names(mob_id)) => {
            let kept = bounded(mob_id.as_str());
            if kept == mob_id.as_str() {
                kept
            } else {
                target.truncated = true;
                String::new()
            }
        }
        _ => String::new(),
    };
    target
        .fields
        .insert(TARGET_MOB_FIELD.to_string(), Value::String(named));
    target
}

/// The row of a change a JSON-RPC answer ended.
pub(crate) fn json_rpc_row(
    actor: ConsoleAuditActor,
    method: String,
    target: Identifiers,
    answer: &Value,
) -> ConsoleAuditEntry {
    ConsoleAuditEntry::answered(actor, method, target, answer)
}

/// The row of a change a JSON-RPC answer with an HTTP status ended.
pub(crate) fn status_json_rpc_row(
    actor: ConsoleAuditActor,
    method: String,
    target: Identifiers,
    answer: &(axum::http::StatusCode, Value),
) -> ConsoleAuditEntry {
    json_rpc_row(actor, method, target, &answer.1)
}

/// An MCP app tool call's answer, as the route gives it, and whether the
/// call reached the member's tool application: from then on its tool may
/// have run, whatever the route answers.
pub(super) struct ToolCallAnswer {
    pub(super) parts: axum::http::response::Parts,
    pub(super) body: axum::body::Bytes,
    pub(super) dispatched: bool,
}

/// The row of an MCP app tool call: its status, and the `error` of its
/// body when it is no success (a success's body is the tool's result, which
/// the row does not read).
pub(super) fn tool_call_row(
    actor: ConsoleAuditActor,
    method: String,
    target: Identifiers,
    answer: &ToolCallAnswer,
) -> ConsoleAuditEntry {
    let status = answer.parts.status;
    let body = if status.is_success() {
        Value::Null
    } else {
        serde_json::from_slice::<Value>(&answer.body).unwrap_or(Value::Null)
    };
    ConsoleAuditEntry::answered_http(
        actor,
        method,
        target,
        status.as_u16(),
        &body,
        answer.dispatched,
    )
}

/// The actor a row names for the caller `auth`, as `meta/console/whoami`
/// reports it, with a person's verified issuer and subject and a digest of
/// the person's login.
fn audit_actor(auth: &ConsoleHttpAuthContext) -> ConsoleAuditActor {
    let kind = auth.actor_kind();
    let human = auth.human.as_ref();
    ConsoleAuditActor {
        kind: kind.as_str(),
        principal: auth.principal.as_deref().map(bounded),
        issuer: human.map(|human| bounded(human.issuer())),
        subject: human.map(|human| bounded(human.subject())),
        assurance: kind.assurance(),
        login_scope_digest: human.map(login_scope_digest),
    }
}

/// What a row keeps of `human`'s login, in the form decision records keep it
/// (design answer A5): `sha256:` and the lowercase hex SHA-256 of the signed
/// login association (`sid`), or, for a token that carries none, of its
/// unpadded base64url digest (one login per token). It names neither the
/// login nor the token.
fn login_scope_digest(human: &AuthenticatedConsoleHuman) -> String {
    use base64::Engine as _;
    use sha2::{Digest, Sha256};
    let scope = match human.login_scope() {
        Some(sid) => sid.to_string(),
        None => base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(human.token_digest()),
    };
    format!("sha256:{:x}", Sha256::digest(scope.as_bytes()))
}

/// The actor fields that name a person: what the audit read leaves out of a
/// side chat's `meta/fork_member` row for a reader who may not see its
/// creator.
const PERSON_FIELDS: [&str; 4] = ["principal", "issuer", "subject", "login_scope_digest"];

/// The mob of the change `row` records: the mob its target names, none
/// for a change that named no mob. A value that is no id of a mob (a blank
/// one, or one that is no string: the row of a request refused for it)
/// names no mob here; who is shown such a row is
/// [`rows_mob_as_shown_to`]'s to say.
fn row_mob(row: &ConsoleAuditRow) -> Option<MobId> {
    row.fields
        .get("target")
        .and_then(|target| target.get(TARGET_MOB_FIELD))
        .and_then(Value::as_str)
        .map(MobId::from)
        .filter(crate::member_key::names_a_mob)
}

/// The members `row` names: a member field of the row's method in its
/// target (the console's one member field table, whose top-level fields a
/// row keeps; a route's row, such as an MCP app tool call's, names its
/// member as a subject does), and a member its owner's receipt names (the
/// member a change created).
fn named_members(row: &ConsoleAuditRow) -> impl Iterator<Item = &str> {
    let named_in = |part: &'static str, key: &'static str| {
        row.fields
            .get(part)
            .and_then(|fields| fields.get(key))
            .and_then(Value::as_str)
    };
    let method = row
        .fields
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or_default();
    console_method_member_fields_or_subject(method)
        .iter()
        .filter_map(|field| match field.path {
            [key] => Some(*key),
            _ => None,
        })
        .filter_map(move |key| named_in("target", key))
        .chain(
            CONSOLE_RPC_MEMBER_TARGET_FIELDS
                .into_iter()
                .filter_map(move |key| named_in("receipt", key)),
        )
}

/// Whether `row` names a member `audience` hides (one private to another
/// person, [`AccessView::hides_agent`]), among the members it names
/// ([`named_members`]).
fn names_hidden_member(audience: &AccessView, row: &ConsoleAuditRow) -> bool {
    named_members(row).any(|member| audience.hides_agent(member))
}

/// Whether `row` names a member at all ([`named_members`]).
fn names_a_member(row: &ConsoleAuditRow) -> bool {
    named_members(row).next().is_some()
}

/// The mob the member a shown row names is judged in.
enum RowsMob {
    /// The console's own mob: a row of the launch mob, and a row that names
    /// no mob at all.
    OfTheConsole,
    /// Another mob, which the reader may address.
    Other(MobId),
}

/// Whether the reader `reader` is shown the row `row`, by the mob the row's
/// target names, and the mob its member is judged in. The
/// judgement is the one a door makes of a request's mob
/// ([`judge_named_mob`]): a row that names no mob, or the launch mob by its
/// id, is the launch mob's and every reader's; a row of another mob is
/// shown to a reader who may address that mob and to no other, as a row of
/// the mob that stands under that mob's id ([`row_is_withheld`]). A row whose
/// mob is no id of a mob (a blank one, or a value that is no string: the
/// row of a request refused for it) is an administrator's alone.
///
/// `None` for a row the reader is not shown.
fn rows_mob_as_shown_to(
    reader: &AccessView,
    launch_mob: Option<&MobId>,
    row: &ConsoleAuditRow,
) -> Option<RowsMob> {
    let named = match row
        .fields
        .get("target")
        .and_then(|target| target.get(TARGET_MOB_FIELD))
    {
        None | Some(Value::Null) => return Some(RowsMob::OfTheConsole),
        Some(Value::String(mob_id))
            if crate::member_key::names_a_mob(&MobId::from(mob_id.as_str())) =>
        {
            NamedMob::from_query(Some(mob_id.clone()))
        }
        Some(_) => {
            return (!reader.enforced() || reader.is_admin()).then_some(RowsMob::OfTheConsole);
        }
    };
    match judge_named_mob(&named, launch_mob, reader.clone()).ok()? {
        (AddressedMob::Launch, _) => Some(RowsMob::OfTheConsole),
        (AddressedMob::Other(mob_id), _) => Some(RowsMob::Other(mob_id)),
    }
}

/// How the host's resolver knows the mob a shown row names.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ServedAs {
    /// A mob the host serves beside its default mob, loaded: its members
    /// are recorded and the row's member is judged in that mob.
    Loaded,
    /// A mob nothing of whose members is recorded for this read: one the
    /// host serves that is not loaded, or one it does not serve and that
    /// no member of the console's mob is found to have created (a mob the
    /// host served and ended, an id no mob has, a mob whose creator cannot
    /// be placed). The read loads no mob to learn its members.
    NotLoaded,
    /// A mob the host's resolver does not serve whose members this console
    /// knows as its own: a mob that runs now and that a member of the
    /// console's mob created, or a member of such a mob
    /// ([`MemberCreatedMobs`]). Also the mob a row names on a console that
    /// carries no resolver of the host, and the host's default mob where a
    /// row names it by an id this door did not read as the launch mob's.
    /// Its members are judged as this console knows them.
    NotServed,
}

/// The mobs that run now and that a member of the console's own mob
/// created, or a member of such a mob: the console's own walk of them
/// ([`super::delegate_mobs_with_hosts`]), made once for a read and only
/// when a row asks. A mob no member created, or one whose creator the
/// walk cannot place, is not among them. A console without a runtime of
/// its own lists none.
struct MemberCreatedMobs<'a> {
    console: Option<&'a super::MobRuntime>,
    listed: Option<BTreeSet<String>>,
}

impl<'a> MemberCreatedMobs<'a> {
    fn of(console: Option<&'a super::MobRuntime>) -> Self {
        Self {
            console,
            listed: None,
        }
    }

    /// Whether the walk lists the mob `mob_id`.
    async fn lists(&mut self, mob_id: &MobId) -> bool {
        if self.listed.is_none() {
            let listed = match self.console {
                Some(console) => Box::pin(super::delegate_mobs_with_hosts(console))
                    .await
                    .into_iter()
                    .map(|(mob_id, _, _)| mob_id)
                    .collect(),
                None => BTreeSet::new(),
            };
            self.listed = Some(listed);
        }
        self.listed
            .as_ref()
            .is_some_and(|listed| listed.contains(mob_id.as_str()))
    }
}

/// Record the members of the mob `mob_id` as that mob's agents in `access`,
/// when the host's resolver serves it as a mob other than its default mob
/// and the mob is loaded, and say how the resolver knows it. A row of such
/// a mob names that mob's members: they are judged in the reader's view of
/// that mob, which reads what is recorded here. The read asks the resolver
/// for the mob "if loaded", never for a resolve: a resolve is what loads a
/// mob, and a page of the audit log loads none. A mob the resolver does
/// not serve is one of the console's own when `created_by_members` lists
/// it: the read's priming recorded its members with the console's own.
/// Of any other mob a row names nothing is recorded.
async fn prime_served_mob(
    resolver: Option<&crate::unified_runtime::MobResolver>,
    created_by_members: &mut MemberCreatedMobs<'_>,
    mob_id: &MobId,
    access: &AccessController,
) -> ServedAs {
    let Some(resolver) = resolver else {
        return ServedAs::NotServed;
    };
    match resolver.resolve_if_loaded(mob_id) {
        Some(mob) if !mob.is_default() => {
            super::prime_access_cache_from_runtime_of(mob.mob_runtime(), access, Some(mob_id))
                .await;
            ServedAs::Loaded
        }
        Some(_) => ServedAs::NotServed,
        None if resolver.served_mob_id(Some(mob_id)).await.is_ok() => ServedAs::NotLoaded,
        // A mob the host does not serve. One that runs now and that a
        // member of the console's mob created is read as before: its
        // members are the console's own. Any other id, a mob the host
        // served and ended among them, is placed nowhere, and nothing
        // says who a member of it was private to.
        None => {
            if created_by_members.lists(mob_id).await {
                ServedAs::NotServed
            } else {
                ServedAs::NotLoaded
            }
        }
    }
}

/// Whether the read of `auth` with the selector `mobs` does not show the
/// row `row` of the console audit log. Three questions, in this order. The
/// request's selection first ([`MobsSelector::answers_host_entry`]): a
/// read that selects no mob answers the launch mob's rows alone, whoever
/// reads, and with every mob selected a row of another mob is answered to
/// a reader who may address that mob. This is the one place that decides
/// a row of another mob for a reader. Then the row's own mob
/// ([`rows_mob_as_shown_to`]), which places the row in the launch mob or in
/// another and decides a row whose mob is blank. Then the member the row
/// names, judged in the row's mob. `served` keeps how the host serves each
/// mob a row of this read named, so each is asked once.
///
/// A row that names a mob beside the launch mob is a row of the mob that
/// stands under that mob's id: one written at or after that mob's creation
/// ([`crate::unified_runtime::MobResolver::standing_mob_created_at_ms`],
/// kept in `served` beside how the mob is served). A reader who addresses
/// the mob by an access rule is shown the standing mob's rows, and none
/// of an id under which no mob stands or whose journal cannot be read; a
/// reader who may address every mob is shown every row, and nothing is
/// read for one.
async fn row_is_withheld(
    auth: &ConsoleHttpAuthContext,
    mobs: MobsSelector,
    launch_mob: Option<&MobId>,
    resolver: Option<&crate::unified_runtime::MobResolver>,
    served: &mut Vec<(MobId, ServedAs, Option<u64>)>,
    created_by_members: &mut MemberCreatedMobs<'_>,
    row: &ConsoleAuditRow,
) -> bool {
    if !mobs.answers_host_entry(Some(&auth.audience), launch_mob, row_mob(row).as_ref()) {
        return true;
    }
    let Some(rows_mob) = rows_mob_as_shown_to(&auth.audience, launch_mob, row) else {
        return true;
    };
    match rows_mob {
        RowsMob::OfTheConsole => names_hidden_member(&auth.audience, row),
        RowsMob::Other(mob_id) => {
            let of_every_mob = auth.audience.may_address_every_mob();
            let known = served
                .iter()
                .find(|(known, _, _)| *known == mob_id)
                .map(|(_, served_as, created_at_ms)| (*served_as, *created_at_ms));
            let (served_as, created_at_ms) = match known {
                Some(known) => known,
                None => {
                    let served_as = prime_served_mob(
                        resolver,
                        created_by_members,
                        &mob_id,
                        &auth.audience.controller(),
                    )
                    .await;
                    let created_at_ms = match resolver {
                        Some(resolver) if !of_every_mob => {
                            Box::pin(resolver.standing_mob_created_at_ms(&mob_id)).await
                        }
                        _ => None,
                    };
                    served.push((mob_id.clone(), served_as, created_at_ms));
                    (served_as, created_at_ms)
                }
            };
            // A record that names the mob the caller's access controller
            // was told is its default mob is the launch mob's, and the
            // launch mob stands for as long as its host does.
            if !of_every_mob
                && !auth.audience.stored_mob_is_the_default_mob(&mob_id)
                && created_at_ms.is_none_or(|created| row.at_ms < created)
            {
                return true;
            }
            match served_as {
                ServedAs::Loaded => names_hidden_member(&auth.audience.in_mob(&mob_id), row),
                // Nothing says who a member of a mob that is not loaded,
                // or of one the host does not serve and no member of the
                // console's mob is found to have created, is private
                // to: a row that names one is shown to no
                // caller the console keeps apart from other people,
                // enforced access or not (the rule for a member the
                // console does not know), and where access is enforced
                // to an administrator alone; it is never judged by the
                // launch mob's member of the same name.
                ServedAs::NotLoaded => {
                    names_a_member(row)
                        && (auth.audience.kept_apart_from_other_people()
                            || (auth.audience.enforced() && !auth.audience.is_admin()))
                }
                ServedAs::NotServed => names_hidden_member(&auth.audience, row),
            }
        }
    }
}

/// `meta/console/audit` on the console route: one page of a record set of
/// the console audit log (`log`: `changes` by default, or `tool_calls`),
/// oldest first, after `after_seq` (from the oldest row kept without it),
/// at most `limit` rows (100 by default, 500 at most). It needs
/// `audit.view`; a console that keeps no log answers that the method is not
/// found. A row that names a member the reader's audience hides (a member
/// private to another person) is left out, as every other console read
/// leaves that member out. A row
/// that names a mob other than the launch mob is shown only to a reader who
/// may address that mob ([`rows_mob_as_shown_to`]), as a row of the mob
/// that stands under that mob's id ([`row_is_withheld`]), and the member it
/// names is judged in that mob. A
/// `meta/fork_member` row names its actor, a side chat's creator, only to
/// that person and to admins of an enforced policy, as every other console
/// answer shows the creator.
///
/// The log is the host's, one log whatever mob a request names, and the
/// read is under the rule of the host's lists: a request that does not
/// select every mob (`mobs`) is answered the rows of the launch mob (a row
/// that names no mob, or the launch mob by its id) and no row
/// of a change that named another mob; with every mob selected, the rows
/// of each mob the caller may address.
///
/// The page is cut to `limit` after the rows left out
/// ([`row_is_withheld`]): it holds up to `limit` rows the reader is shown,
/// its cursor names the last of them (the cursor the read started from
/// when it holds none), and `has_more` says that another row the reader is
/// shown follows. To fill it the log is read on past its first `limit`
/// rows, to the end of the set at most; a set keeps a bounded number of
/// rows ([`crate::console_audit::ConsoleAuditLimits`], 20000 by default).
pub(super) async fn console_audit_rpc(
    state: &ConsoleJsonState,
    resolver: Option<&crate::unified_runtime::MobResolver>,
    auth: &ConsoleHttpAuthContext,
    request: &JsonRpcRequest,
    mobs: MobsSelector,
) -> Value {
    let response_id = request.id.clone().unwrap_or(Value::Null);
    let Some(log) = state.console_audit.as_ref() else {
        return response_value(
            response_id,
            None,
            Some(JsonRpcError::new(-32601, "Method not found")),
        );
    };
    if let Some(violation) = super::console_rpc_access_violation(
        auth.access_view.as_ref(),
        crate::rpc::CONSOLE_AUDIT_METHOD,
        &request.params,
    ) {
        return response_value(response_id, None, Some(violation));
    }
    let set = match request.params.get("log") {
        None | Some(Value::Null) => ConsoleAuditSet::Changes,
        Some(value) => match serde_json::from_value::<ConsoleAuditSet>(value.clone()) {
            Ok(set) => set,
            Err(_) => {
                return invalid_params(response_id, "log must be \"changes\" or \"tool_calls\"");
            }
        },
    };
    let after_seq = match request.params.get("after_seq") {
        None | Some(Value::Null) => None,
        Some(value) => match value.as_u64() {
            Some(seq) => Some(seq),
            None => {
                return invalid_params(response_id, "after_seq must be a non-negative integer");
            }
        },
    };
    let limit = match request.params.get("limit") {
        None | Some(Value::Null) => CONSOLE_AUDIT_DEFAULT_PAGE,
        Some(value) => match value
            .as_u64()
            .and_then(|limit| usize::try_from(limit).ok())
            .filter(|limit| (1..=CONSOLE_AUDIT_MAX_PAGE).contains(limit))
        {
            Some(limit) => limit,
            None => {
                return invalid_params(
                    response_id,
                    format!("limit must be an integer from 1 to {CONSOLE_AUDIT_MAX_PAGE}"),
                );
            }
        },
    };
    let mut page = match log.page_in(set, after_seq, limit).await {
        Ok(page) => page,
        Err(error) => {
            return response_value(
                response_id,
                None,
                Some(JsonRpcError::new(
                    -32603,
                    format!("the console audit log could not be read: {error}"),
                )),
            );
        }
    };
    // The members the rows name are judged with their owners: record the
    // console's runtimes' members first, ended ones included, as every
    // console read does before it decides.
    let controller = auth.audience.controller();
    match (&state.runtime, &state.console_aggregator) {
        (Some(runtime), _) => super::prime_access_cache_from_runtime(runtime, &controller).await,
        (None, Some(aggregator)) => {
            super::prime_access_cache_from_aggregator(aggregator, &controller).await;
        }
        (None, None) => {}
    }
    // The rows the request selects: the launch mob's, or with every mob
    // selected those of each mob the reader may address. Then a row is
    // shown by its own mob: to a reader who may address that mob, as a row
    // of the mob that stands under that mob's id, with the member it names
    // judged in that mob.
    let launch_mob = super::console_launch_mob(state, resolver).await;
    let mut served: Vec<(MobId, ServedAs, Option<u64>)> = Vec::new();
    let mut created_by_members = MemberCreatedMobs::of(state.runtime.as_ref());
    // The cursor this read started from: the one given, or, with none and
    // with one the set does not keep, the cursor of the oldest row kept.
    let started_from = match after_seq {
        Some(after) if !page.gap => after,
        _ => page
            .oldest_available_seq
            .map_or(page.latest_seq, |oldest| oldest.saturating_sub(1)),
    };
    // The page holds `limit` rows the reader is shown. The log is read on
    // until that many are found and it is known whether another follows,
    // or the log ends: so the size of the page, its cursor and `has_more`
    // tell nothing of a row the reader is not shown. The set keeps a
    // bounded number of rows ([`crate::console_audit::ConsoleAuditLimits`]),
    // which bounds the read.
    let mut shown = Vec::with_capacity(limit);
    let mut another_follows = false;
    let mut read = std::mem::take(&mut page.records);
    let mut read_to = page.next_after_seq;
    let mut more_to_read = page.has_more;
    'log: loop {
        for row in read {
            if row_is_withheld(
                auth,
                mobs,
                launch_mob.as_ref(),
                resolver,
                &mut served,
                &mut created_by_members,
                &row,
            )
            .await
            {
                continue;
            }
            if shown.len() == limit {
                another_follows = true;
                break 'log;
            }
            shown.push(row);
        }
        if !more_to_read {
            break;
        }
        let next = match log
            .page_in(set, Some(read_to), CONSOLE_AUDIT_MAX_PAGE)
            .await
        {
            Ok(next) => next,
            Err(error) => {
                return response_value(
                    response_id,
                    None,
                    Some(JsonRpcError::new(
                        -32603,
                        format!("the console audit log could not be read: {error}"),
                    )),
                );
            }
        };
        // The set stopped keeping the rows after this cursor while the read
        // went on: the reader has the rows read so far and reads on.
        if next.gap {
            another_follows = true;
            break;
        }
        read = next.records;
        read_to = next.next_after_seq;
        more_to_read = next.has_more;
    }
    // The cursor of the next read names the last row the reader is shown,
    // or the cursor this read started from when it is shown none.
    page.next_after_seq = shown.last().map_or(started_from, |row| row.seq);
    page.has_more = another_follows;
    page.records = shown;
    let viewer = auth.side_chat_creator_viewer();
    for row in &mut page.records {
        if row.fields.get("method").and_then(Value::as_str) != Some("meta/fork_member") {
            continue;
        }
        let Some(actor) = row.fields.get_mut("actor").and_then(Value::as_object_mut) else {
            continue;
        };
        let creator = actor.get("principal").cloned().unwrap_or(Value::Null);
        if !viewer.sees(&creator) {
            for field in PERSON_FIELDS {
                actor.remove(field);
            }
        }
    }
    match serde_json::to_value(&page) {
        Ok(result) => response_value(response_id, Some(result), None),
        Err(error) => response_value(
            response_id,
            None,
            Some(JsonRpcError::new(
                -32603,
                format!("the console audit page could not be encoded: {error}"),
            )),
        ),
    }
}

/// Whether the caller `auth` may read the console audit log of `state`: the
/// console keeps one, and the caller holds `audit.view` where access is
/// enforced. The console's capabilities list the read only then.
pub(super) fn audit_read_offered(state: &ConsoleJsonState, auth: &ConsoleHttpAuthContext) -> bool {
    state.console_audit.is_some()
        && super::console_rpc_access_violation(
            auth.access_view.as_ref(),
            crate::rpc::CONSOLE_AUDIT_METHOD,
            &Value::Null,
        )
        .is_none()
}
