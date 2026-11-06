//! The console's side of the console audit log ([`crate::console_audit`]):
//! which console requests write a row, the row a request writes, and the
//! `meta/console/audit` read.

use std::sync::Arc;

use serde_json::{Map, Value, json};

use super::{
    ConsoleHttpAuthContext, ConsoleJsonState, JsonRpcError, JsonRpcRequest, invalid_params,
    is_console_mutating_rpc_method, response_value,
};
use crate::console_audit::{
    CONSOLE_AUDIT_DEFAULT_PAGE, CONSOLE_AUDIT_MAX_PAGE, ConsoleAuditActor, ConsoleAuditEntry,
    ConsoleAuditLog, audit_target,
};
use crate::runtime::AuthenticatedConsoleHuman;

/// Whether `method` is a console change the audit log records: every
/// mutating console method but conversation traffic (sends and their
/// recovery, voice and live media), whose record is the transcript.
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
pub(super) struct ConsoleAuditIntent {
    log: Arc<ConsoleAuditLog>,
    method: String,
    target: Map<String, Value>,
}

impl ConsoleAuditIntent {
    /// The intent to record `request`, when the console keeps an audit log
    /// and the request is a change it records
    /// ([`is_audited_console_method`]). Read before the request is handled:
    /// the target is the request's own.
    pub(super) fn of(state: &ConsoleJsonState, request: &JsonRpcRequest) -> Option<Self> {
        let log = state.console_audit.as_ref()?;
        is_audited_console_method(&request.method).then(|| Self {
            log: Arc::clone(log),
            method: request.method.clone(),
            target: audit_target(&request.params),
        })
    }

    /// Record the change, made by the caller `auth`, as the JSON-RPC answer
    /// `answer` ended it. The row is written on a task of its own, so a
    /// caller that goes away meanwhile does not cut it short.
    pub(super) async fn record(self, auth: &ConsoleHttpAuthContext, answer: &Value) {
        let entry =
            ConsoleAuditEntry::answered(audit_actor(auth), self.method, self.target, answer);
        let log = self.log;
        let written = tokio::spawn(async move { log.record(entry).await });
        let _ = written.await;
    }
}

/// The actor a row names for the caller `auth`, as `meta/console/whoami`
/// reports it, with a person's verified issuer and subject and a digest of
/// the person's login.
fn audit_actor(auth: &ConsoleHttpAuthContext) -> ConsoleAuditActor {
    let kind = auth.actor_kind();
    let human = auth.human.as_ref();
    ConsoleAuditActor {
        kind: kind.as_str(),
        principal: auth.principal.clone(),
        issuer: human.map(|human| human.issuer().to_string()),
        subject: human.map(|human| human.subject().to_string()),
        assurance: kind.assurance(),
        login_scope_digest: human.map(login_scope_digest),
    }
}

/// `sha256:` and a hex SHA-256 naming `human`'s login: the issuer's login
/// association (`sid`) when the token carries one, which every token of the
/// login shares, else the token itself. Neither can be read back from it.
fn login_scope_digest(human: &AuthenticatedConsoleHuman) -> String {
    use base64::Engine as _;
    use sha2::{Digest, Sha256};
    let login = match human.login_scope() {
        Some(sid) => json!(["meta-console-login-v1", "sid", human.issuer(), sid]),
        None => json!([
            "meta-console-login-v1",
            "token",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(human.token_digest()),
        ]),
    };
    format!("sha256:{:x}", Sha256::digest(login.to_string().as_bytes()))
}

/// `meta/console/audit` on the console route: one page of the console audit
/// log, oldest first, after `after_seq` (from the oldest row kept without
/// it), at most `limit` rows (100 by default, 500 at most). It needs
/// `audit.view`; a console that keeps no log answers that the method is not
/// found.
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
    let page = match log.page(after_seq, limit).await {
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
