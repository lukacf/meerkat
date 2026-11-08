//! The console's side of the console audit log ([`crate::console_audit`]):
//! which console requests write a row, the row a request writes, and the
//! `meta/console/audit` read.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;

use super::{
    ConsoleHttpAuthContext, ConsoleJsonState, JsonRpcError, JsonRpcRequest, invalid_params,
    is_console_mutating_rpc_method, response_value,
};
use crate::console_audit::{
    CONSOLE_AUDIT_DEFAULT_PAGE, CONSOLE_AUDIT_MAX_PAGE, ConsoleAuditActor, ConsoleAuditEntry,
    ConsoleAuditLog, ConsoleAuditSet, Identifiers, audit_target, bounded,
};
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
                audit_target(&request.params),
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
                &super::console_decision_caller(auth),
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

/// `meta/console/audit` on the console route: one page of a record set of
/// the console audit log (`log`: `changes` by default, or `tool_calls`),
/// oldest first, after `after_seq` (from the oldest row kept without it),
/// at most `limit` rows (100 by default, 500 at most). It needs
/// `audit.view`; a console that keeps no log answers that the method is not
/// found. A `meta/fork_member` row names its actor, a side chat's creator,
/// only to that person and to admins of an enforced policy, as every other
/// console answer shows the creator.
pub(super) async fn console_audit_rpc(
    state: &ConsoleJsonState,
    auth: &ConsoleHttpAuthContext,
    request: &JsonRpcRequest,
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
