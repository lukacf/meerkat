//! `tool_grants/*` JSON-RPC handlers: the scoped tool grants of a session or
//! of a mob member identity.
//!
//! Every method reads and writes grant rows through the runtime's grant
//! service only: it never loads, registers or wakes the subject's session or
//! member. A session grant reads the gate the session records (its staged
//! build before or during its first turn, its persisted metadata after). An
//! identity grant reads the gate the member's next build resolves, its own
//! gate joined with its role's current gate, and needs the identity in the
//! mob's roster. List and audit resolve the mob only, so a retired
//! identity's history stays readable. Every refusal of a request these
//! methods check is the typed `tool_grant_request_refused` refusal, a
//! `current_run` grant to a subject that runs no turn included
//! (`run_not_active`).
//!
//! A `current_run` grant takes the run the runtime records now for the
//! session serving the subject: a session's own, or the session an
//! identity's member holds here. `tool_grants/request` checks its subject,
//! target and scope the same
//! way and holds a durable decision of kind `tool_grant.issue` (this host is
//! its requester) instead of issuing anything: the runtime's executor of the
//! kind issues the grant once a person approved it. `tool_grants/list` lists
//! the subject's open and approved requests beside its grants, as
//! `requested`, and with `include_ended` its recently declined ones too.

use std::collections::{BTreeSet, HashSet};
use std::sync::Arc;
use std::time::Duration;

use meerkat_contracts::{
    GrantToolParams, GrantToolResult, ListToolGrantsParams, ListToolGrantsResult,
    RequestToolGrantParams, RequestToolGrantResult, RevokeToolGrantParams, RevokeToolGrantResult,
    ToolGrantAuditParams, ToolGrantAuditResult, ToolGrantScopeRequest, ToolGrantSubjectRequest,
    ToolGrantTargetRequest, WireMemberToolGrantGate, WireToolGrantApplies,
    WireToolGrantGateEntries, WireToolGrantGateView,
};
use meerkat_core::approval::decision::DecisionReference;
use meerkat_core::tool_grant::{
    GrantGatedTools, RunningToolGrantGate, ToolGrantId, ToolGrantIssuer, ToolGrantRequestRefusal,
    ToolGrantSubject, ToolGrantTarget,
};
use meerkat_core::{RunId, SessionId};
use meerkat_runtime::RuntimeStoreError;
use meerkat_runtime::decision::wire::wire_decision;
use meerkat_runtime::tool_grant::wire::{
    wire_tool_grant, wire_tool_grant_audit_row, wire_tool_grant_request,
    wire_unreadable_tool_grant, wire_unreadable_tool_grant_audit_row,
};
use meerkat_runtime::tool_grant::{
    ToolGrantIssueRequest, ToolGrantRequest, ToolGrantRequestError, ToolGrantRequestOrigin,
    ToolGrantRequestPolicy, ToolGrantRevocation, ToolGrantServiceError, ToolGrantStatus,
    ToolGrantWindow,
};

use crate::error;
use crate::handlers::decision::{decision_error_to_response, decision_owner};
use crate::handlers::{RpcResponseExt, parse_params, parse_session_id_for_runtime};
use crate::protocol::{RpcId, RpcResponse};
use crate::session_runtime::SessionRuntime;

/// The audit page size when a request names none.
const DEFAULT_AUDIT_PAGE: usize = 100;

/// The largest audit page: a request asking for more reads this many.
const MAX_AUDIT_PAGE: usize = 1_000;

/// The typed refusal of a grant request: invalid params carrying
/// `{"kind": "tool_grant_request_refused", "tool_grant_request_refusal": ...}`.
fn refused(
    id: Option<RpcId>,
    refusal: &ToolGrantRequestRefusal,
    message: impl Into<String>,
) -> RpcResponse {
    RpcResponse::error_with_data(id, error::INVALID_PARAMS, message, refusal.data())
}

fn grant_error_to_response(id: Option<RpcId>, error_value: ToolGrantServiceError) -> RpcResponse {
    let refusal = match &error_value {
        ToolGrantServiceError::InvalidWindow { .. } => Some(ToolGrantRequestRefusal::InvalidWindow),
        ToolGrantServiceError::EmptyTarget => Some(ToolGrantRequestRefusal::EmptyTarget),
        ToolGrantServiceError::NotFound { grant_id } => Some(ToolGrantRequestRefusal::NotFound {
            grant_id: grant_id.clone(),
        }),
        _ => None,
    };
    if let Some(refusal) = refusal {
        return refused(id, &refusal, error_value.to_string());
    }
    let code = match &error_value {
        // Concurrent writers changed the row under this one twice: the
        // busy class, which a retry resolves by reading the row afresh.
        ToolGrantServiceError::Conflict { .. } => error::SESSION_BUSY,
        ToolGrantServiceError::Store(RuntimeStoreError::Unsupported(_)) => {
            meerkat_contracts::ErrorCode::CapabilityUnavailable.jsonrpc_code()
        }
        // The clock, a row this binary cannot read, the machine or the
        // store: the server's fault.
        _ => error::INTERNAL_ERROR,
    };
    RpcResponse::error(id, code, error_value.to_string())
}

/// The session a session subject names. An id that names no session is the
/// typed `subject_unknown`.
#[allow(clippy::result_large_err)]
fn session_of(
    id: Option<RpcId>,
    raw: &str,
    runtime: &SessionRuntime,
) -> Result<SessionId, RpcResponse> {
    parse_session_id_for_runtime(id.clone(), raw, runtime).map_err(|response| {
        let message = response
            .error
            .map(|error| error.message)
            .unwrap_or_else(|| format!("no session '{raw}'"));
        refused(id, &ToolGrantRequestRefusal::SubjectUnknown, message)
    })
}

/// A member identity's gate as its next build resolves it: what that build
/// gates, the role entries it ignores as not mounted, and the MCP servers it
/// configures.
#[cfg_attr(not(feature = "mob"), allow(dead_code))]
struct NextBuildGate {
    gated: GrantGatedTools,
    not_mounted: GrantGatedTools,
    configured_servers: BTreeSet<String>,
}

/// A write's subject, resolved: whose grants, the gate a target must be in
/// (with the servers its build configures ungated), the session serving the
/// subject here when there is one, for a member identity the gate its next
/// build resolves, and who commits the grant.
struct GrantSubject {
    subject: ToolGrantSubject,
    gate: GrantGatedTools,
    ungated_servers: BTreeSet<String>,
    serving: Option<SessionId>,
    next_build: Option<NextBuildGate>,
    commit: GrantCommit,
}

/// Who commits a grant: the runtime's grant service for a session subject;
/// for a member identity, the mob's actor, which decides it against its live
/// roster in order with the retirements that end an identity's grants.
enum GrantCommit {
    Session,
    #[cfg(feature = "mob")]
    Member {
        handle: meerkat_mob::MobHandle,
        mob_id: String,
        identity: meerkat_mob::AgentIdentity,
    },
}

async fn subject_for_grant(
    id: Option<RpcId>,
    request: &ToolGrantSubjectRequest,
    runtime: &SessionRuntime,
) -> Result<GrantSubject, RpcResponse> {
    match request {
        ToolGrantSubjectRequest::Session { session_id } => {
            let session_id = session_of(id.clone(), session_id, runtime)?;
            let gate = match runtime.session_grant_gate(&session_id).await {
                Ok(Some(gate)) => gate,
                Ok(None) => {
                    return Err(refused(
                        id,
                        &ToolGrantRequestRefusal::SubjectUnknown,
                        format!("Session not found: {session_id}"),
                    ));
                }
                Err(error_value) => return Err(RpcResponse::from_error(id, error_value)),
            };
            // A session's recorded gate names no ungated server: with its
            // unconfigured-servers mode on, a grant of a server the session
            // configured ungated is issued and opens nothing.
            Ok(GrantSubject {
                subject: ToolGrantSubject::session(session_id.clone()),
                gate,
                ungated_servers: BTreeSet::new(),
                serving: Some(session_id),
                next_build: None,
                commit: GrantCommit::Session,
            })
        }
        ToolGrantSubjectRequest::Identity { mob_id, identity } => {
            members::identity_for_grant(id, mob_id, identity, runtime).await
        }
    }
}

/// A read's subject, resolved: whose grants, the session serving the subject
/// here when there is one, and, for a member identity in the mob's roster,
/// the gate its next build resolves. A session subject is not looked up, and
/// an identity subject resolves its mob only, so a retired identity's history
/// stays readable.
struct ReadSubject {
    subject: ToolGrantSubject,
    serving: Option<SessionId>,
    next_build: Option<NextBuildGate>,
}

async fn subject_for_read(
    id: Option<RpcId>,
    request: &ToolGrantSubjectRequest,
    runtime: &SessionRuntime,
) -> Result<ReadSubject, RpcResponse> {
    match request {
        ToolGrantSubjectRequest::Session { session_id } => {
            let session_id = session_of(id, session_id, runtime)?;
            Ok(ReadSubject {
                subject: ToolGrantSubject::session(session_id.clone()),
                serving: Some(session_id),
                next_build: None,
            })
        }
        ToolGrantSubjectRequest::Identity { mob_id, identity } => {
            members::identity_for_read(id, mob_id, identity, runtime).await
        }
    }
}

/// The gate the member identity's live build runs, read from the machine of
/// the session serving it here: `None` when no build of it is live here.
async fn running_gate(
    runtime: &SessionRuntime,
    serving: Option<&SessionId>,
) -> Option<RunningToolGrantGate> {
    runtime.runtime_adapter().tool_grant_gate(serving?).await
}

/// Whether an identity grant of `target` applies to the member's running
/// build now: `now` when the running gate covers the target, unless the
/// target is a gated MCP server the member's build configures and the
/// running build's surface does not hold (the build left it unattached, so
/// the next build connects it); `not_mounted` when that build ignored it,
/// or, with no live build, when the next build leaves its family off;
/// `at_next_build` otherwise.
fn grant_applies(
    running: Option<&RunningToolGrantGate>,
    next_build: &NextBuildGate,
    target: &ToolGrantTarget,
) -> WireToolGrantApplies {
    match running {
        Some(running) if running.gate.gates_target(target) => match target {
            ToolGrantTarget::McpServer { server, .. }
                if !running.attached_servers.contains(server)
                    && next_build.configured_servers.contains(server) =>
            {
                WireToolGrantApplies::AtNextBuild
            }
            _ => WireToolGrantApplies::Now,
        },
        Some(running) if running.gate.ignores_target(target) => WireToolGrantApplies::NotMounted,
        Some(_) => WireToolGrantApplies::AtNextBuild,
        None => match target {
            ToolGrantTarget::Family { family, .. }
                if next_build.not_mounted.families.contains(family) =>
            {
                WireToolGrantApplies::NotMounted
            }
            _ => WireToolGrantApplies::AtNextBuild,
        },
    }
}

/// A running gate as `tool_grants/list` shows it: its gated names under
/// `tools` with its gated servers and its unconfigured-servers mode, and the
/// names it ignored as not mounted.
fn wire_running_gate(running: &RunningToolGrantGate) -> WireToolGrantGateView {
    WireToolGrantGateView {
        gated: WireToolGrantGateEntries {
            families: Vec::new(),
            tools: running.gate.gated.iter().map(ToString::to_string).collect(),
            mcp_servers: running.gate.gated_servers.iter().cloned().collect(),
            unconfigured_mcp_servers: running.gate.gates_unconfigured_servers,
        },
        not_mounted: WireToolGrantGateEntries {
            tools: running
                .gate
                .not_mounted_names
                .iter()
                .map(ToString::to_string)
                .collect(),
            ..WireToolGrantGateEntries::default()
        },
    }
}

/// A member identity's gate as `tool_grants/list` answers it.
async fn wire_member_gate(
    runtime: &SessionRuntime,
    serving: Option<&SessionId>,
    next_build: &NextBuildGate,
) -> WireMemberToolGrantGate {
    WireMemberToolGrantGate {
        running: running_gate(runtime, serving)
            .await
            .as_ref()
            .map(wire_running_gate),
        next_build: WireToolGrantGateView {
            gated: WireToolGrantGateEntries::from(&next_build.gated),
            not_mounted: WireToolGrantGateEntries::from(&next_build.not_mounted),
        },
    }
}

/// Identity subjects: the members of the mobs this server runs.
#[cfg(feature = "mob")]
mod members {
    use super::*;

    /// The handle of the mob `mob_id`; a mob this server does not run is
    /// the typed `subject_unknown`.
    async fn mob_handle(
        id: Option<RpcId>,
        mob_id: &str,
        runtime: &SessionRuntime,
    ) -> Result<meerkat_mob::MobHandle, RpcResponse> {
        let unknown = || {
            refused(
                id.clone(),
                &ToolGrantRequestRefusal::SubjectUnknown,
                format!("no mob '{mob_id}'"),
            )
        };
        let Some(state) = runtime.mob_state() else {
            return Err(unknown());
        };
        match state.handle_for(&meerkat_mob::MobId::from(mob_id)).await {
            Ok(handle) => Ok(handle),
            Err(meerkat_mob::MobError::MobNotFound(_)) => Err(unknown()),
            Err(error_value) => Err(crate::handlers::mob::mob_call_error(id, &error_value)),
        }
    }

    pub(super) async fn identity_for_grant(
        id: Option<RpcId>,
        mob_id: &str,
        identity: &str,
        runtime: &SessionRuntime,
    ) -> Result<GrantSubject, RpcResponse> {
        let handle = mob_handle(id.clone(), mob_id, runtime).await?;
        let member = meerkat_mob::AgentIdentity::from(identity);
        let gate = match handle.member_grant_gate(&member).await {
            Ok(gate) => gate,
            Err(meerkat_mob::MobError::MemberNotFound(_)) => {
                return Err(unknown_member(id, mob_id, identity));
            }
            Err(error_value) => return Err(crate::handlers::mob::mob_call_error(id, &error_value)),
        };
        Ok(GrantSubject {
            subject: ToolGrantSubject::member(mob_id, &handle.live_incarnation(), identity),
            gate: gate.named(),
            ungated_servers: gate.ungated_servers.clone(),
            serving: handle.resolve_bridge_session_id(&member).await,
            next_build: Some(NextBuildGate {
                gated: gate.gated,
                not_mounted: gate.not_mounted,
                configured_servers: gate.configured_servers,
            }),
            commit: GrantCommit::Member {
                handle,
                mob_id: mob_id.to_string(),
                identity: member,
            },
        })
    }

    /// The refusal of an identity the mob's roster does not hold, at the
    /// read and at the commit alike.
    fn unknown_member(id: Option<RpcId>, mob_id: &str, identity: &str) -> RpcResponse {
        refused(
            id,
            &ToolGrantRequestRefusal::SubjectUnknown,
            format!("no member '{identity}' in mob '{mob_id}'"),
        )
    }

    /// Commit a member identity's grant through the mob's actor, which
    /// decides it against its live roster: an identity retired since the
    /// gate read is refused as the read refuses an unknown one.
    pub(super) async fn commit(
        id: Option<RpcId>,
        handle: &meerkat_mob::MobHandle,
        mob_id: &str,
        identity: &meerkat_mob::AgentIdentity,
        request: ToolGrantRequest,
        serving: Option<&SessionId>,
    ) -> Result<meerkat_runtime::tool_grant::StoredToolGrant, RpcResponse> {
        use meerkat_mob::MemberToolGrantError;
        let grant = meerkat_mob::MemberToolGrant {
            target: request.target,
            issuer: request.issuer,
            reason: request.reason,
            window: request.window,
            run_scope: request.run_scope,
        };
        match handle.grant_member_tool(identity, grant, serving).await {
            Ok(grant) => Ok(grant),
            Err(MemberToolGrantError::Mob(meerkat_mob::MobError::MemberNotFound(_))) => {
                Err(unknown_member(id, mob_id, identity.as_str()))
            }
            Err(MemberToolGrantError::Mob(error_value)) => {
                Err(crate::handlers::mob::mob_call_error(id, &error_value))
            }
            Err(MemberToolGrantError::Grant(error_value)) => {
                Err(grant_error_to_response(id, error_value))
            }
            Err(MemberToolGrantError::NoRuntime) => Err(RpcResponse::error(
                id,
                meerkat_contracts::ErrorCode::CapabilityUnavailable.jsonrpc_code(),
                "this mob runs without a runtime: it holds no scoped tool grants",
            )),
        }
    }

    pub(super) async fn identity_for_read(
        id: Option<RpcId>,
        mob_id: &str,
        identity: &str,
        runtime: &SessionRuntime,
    ) -> Result<ReadSubject, RpcResponse> {
        let handle = mob_handle(id, mob_id, runtime).await?;
        let member = meerkat_mob::AgentIdentity::from(identity);
        let serving = handle.resolve_bridge_session_id(&member).await;
        // An identity outside the roster (a retired one) has no next build:
        // its history stays readable, without a gate.
        let next_build = handle
            .member_grant_gate(&member)
            .await
            .ok()
            .map(|gate| NextBuildGate {
                gated: gate.gated,
                not_mounted: gate.not_mounted,
                configured_servers: gate.configured_servers,
            });
        Ok(ReadSubject {
            subject: ToolGrantSubject::member(mob_id, &handle.live_incarnation(), identity),
            serving,
            next_build,
        })
    }
}

/// Identity subjects: a server without mobs has none.
#[cfg(not(feature = "mob"))]
mod members {
    use super::*;

    fn unsupported(id: Option<RpcId>) -> RpcResponse {
        RpcResponse::error(
            id,
            meerkat_contracts::ErrorCode::CapabilityUnavailable.jsonrpc_code(),
            "this server runs no mobs: identity subjects are unsupported",
        )
    }

    pub(super) async fn identity_for_grant(
        id: Option<RpcId>,
        _mob_id: &str,
        _identity: &str,
        _runtime: &SessionRuntime,
    ) -> Result<GrantSubject, RpcResponse> {
        Err(unsupported(id))
    }

    pub(super) async fn identity_for_read(
        id: Option<RpcId>,
        _mob_id: &str,
        _identity: &str,
        _runtime: &SessionRuntime,
    ) -> Result<ReadSubject, RpcResponse> {
        Err(unsupported(id))
    }
}

/// The target a request names, resolved against its subject's gate (and the
/// MCP servers its build configures ungated): the typed refusal when the gate
/// does not name it.
#[allow(clippy::result_large_err)]
fn target_of(
    id: Option<RpcId>,
    requested: ToolGrantTargetRequest,
    subject: &GrantSubject,
) -> Result<ToolGrantTarget, RpcResponse> {
    let named = match &requested {
        ToolGrantTargetRequest::Tool { name } => format!("the tool '{name}'"),
        ToolGrantTargetRequest::Family { family } => format!("the tool family '{family}'"),
        ToolGrantTargetRequest::McpServer { server, .. } => format!("the MCP server '{server}'"),
    };
    let names_a_tool = matches!(requested, ToolGrantTargetRequest::Tool { .. });
    meerkat::resolve_grant_target(requested, &subject.gate, &subject.ungated_servers).map_err(
        |refusal| {
            let message = match refusal {
                ToolGrantRequestRefusal::TargetNotGated if names_a_tool => format!(
                    "the subject's grant gate does not name {named}: a grant opens only names \
                     the subject's build gates; a gated MCP server's tools are granted with \
                     {{\"kind\": \"mcp_server\", \"server\": ..., \"tools\": [...]}}"
                ),
                ToolGrantRequestRefusal::TargetNotGated => format!(
                    "the subject's grant gate does not name {named}: a grant opens only what \
                     the subject's build gates"
                ),
                _ => refusal.to_string(),
            };
            refused(id, &refusal, message)
        },
    )
}

/// The run a requested scope binds: none for a window, and for
/// `current_run` the run the runtime records now for the session serving the
/// subject here (a session's own, or the session an identity's member
/// holds), which a subject that runs none refuses as `run_not_active`.
#[allow(clippy::result_large_err)]
async fn run_scope_of(
    id: Option<RpcId>,
    scope: Option<ToolGrantScopeRequest>,
    subject: &GrantSubject,
    runtime: &SessionRuntime,
) -> Result<Option<RunId>, RpcResponse> {
    match scope {
        None | Some(ToolGrantScopeRequest::Window {}) => Ok(None),
        Some(ToolGrantScopeRequest::CurrentRun {}) => {
            let current_run = match &subject.serving {
                Some(session_id) => runtime.runtime_adapter().current_run(session_id).await,
                None => None,
            };
            current_run.map(Some).ok_or_else(|| {
                let idle = match &subject.subject {
                    ToolGrantSubject::Session { session_id } => {
                        format!("the session {session_id} runs no turn now")
                    }
                    ToolGrantSubject::Identity {
                        mob_id, identity, ..
                    } => format!(
                        "no member of the identity {identity} in mob {mob_id} runs a turn here now"
                    ),
                };
                refused(
                    id,
                    &ToolGrantRequestRefusal::RunNotActive,
                    format!("{idle}: a current_run grant needs a run"),
                )
            })
        }
    }
}

/// Handle `tool_grants/grant`.
pub async fn handle_grant(
    id: Option<RpcId>,
    params: Option<&serde_json::value::RawValue>,
    runtime: Arc<SessionRuntime>,
) -> RpcResponse {
    let params: GrantToolParams = match parse_params(params) {
        Ok(params) => params,
        Err(response) => return response.with_id(id),
    };
    let window = match (params.expires_at_ms, params.duration_ms) {
        (Some(expires_at_ms), None) => ToolGrantWindow::Until { expires_at_ms },
        (None, Some(duration_ms)) => ToolGrantWindow::For(Duration::from_millis(duration_ms)),
        _ => {
            return refused(
                id,
                &ToolGrantRequestRefusal::InvalidWindow,
                "a tool grant is bounded by exactly one of expires_at_ms and duration_ms",
            );
        }
    };
    let decision_ref = match params.decision_ref.map(DecisionReference::new).transpose() {
        Ok(decision_ref) => decision_ref,
        Err(error_value) => {
            return refused(
                id,
                &ToolGrantRequestRefusal::InvalidDecisionRef,
                error_value.to_string(),
            );
        }
    };
    let subject = match subject_for_grant(id.clone(), &params.subject, &runtime).await {
        Ok(subject) => subject,
        Err(response) => return response,
    };
    let target = match target_of(id.clone(), params.target, &subject) {
        Ok(target) => target,
        Err(response) => return response,
    };
    let run_scope = match run_scope_of(id.clone(), params.scope, &subject, &runtime).await {
        Ok(run_scope) => run_scope,
        Err(response) => return response,
    };
    let request = ToolGrantRequest {
        subject: subject.subject,
        target,
        issuer: ToolGrantIssuer::Operator {
            caller: None,
            decision_ref,
        },
        reason: params.reason,
        window,
        run_scope,
    };
    let issued = match &subject.commit {
        GrantCommit::Session => runtime
            .runtime_adapter()
            .grant_tool(request, subject.serving.as_ref())
            .await
            .map_err(|error_value| grant_error_to_response(id.clone(), error_value)),
        #[cfg(feature = "mob")]
        GrantCommit::Member {
            handle,
            mob_id,
            identity,
        } => {
            members::commit(
                id.clone(),
                handle,
                mob_id,
                identity,
                request,
                subject.serving.as_ref(),
            )
            .await
        }
    };
    match issued {
        Ok(grant) => {
            let applies = match &subject.next_build {
                Some(next_build) => Some(grant_applies(
                    running_gate(&runtime, subject.serving.as_ref())
                        .await
                        .as_ref(),
                    next_build,
                    &grant.record.target,
                )),
                None => None,
            };
            RpcResponse::success(
                id,
                GrantToolResult {
                    grant: wire_tool_grant(&grant),
                    applies,
                },
            )
        }
        Err(response) => response,
    }
}

/// Handle `tool_grants/request`: hold a durable decision of kind
/// `tool_grant.issue` for a grant a person approves. The subject, target and
/// scope are checked as `tool_grants/grant` checks them, and the duration
/// against the host's maximum lifetime. This host is the requester, and the
/// decision is held for the approver id `operator` until the request's
/// deadline. Nothing is issued here: the runtime's executor of the kind
/// issues the grant, under the id the answer names, once a person approved
/// it. A request names a session or a mob member identity, as a grant does;
/// an identity's notice goes through the mob's member input.
pub async fn handle_request(
    id: Option<RpcId>,
    params: Option<&serde_json::value::RawValue>,
    runtime: Arc<SessionRuntime>,
) -> RpcResponse {
    let params: RequestToolGrantParams = match parse_params(params) {
        Ok(params) => params,
        Err(response) => return response.with_id(id),
    };
    let decisions = match decision_owner(id.clone(), &runtime) {
        Ok(decisions) => decisions,
        Err(response) => return response,
    };
    let subject = match subject_for_grant(id.clone(), &params.subject, &runtime).await {
        Ok(subject) => subject,
        Err(response) => return response,
    };
    let target = match target_of(id.clone(), params.target, &subject) {
        Ok(target) => target,
        Err(response) => return response,
    };
    let run_scope = match run_scope_of(id.clone(), params.scope, &subject, &runtime).await {
        Ok(run_scope) => run_scope,
        Err(response) => return response,
    };
    let requester = match runtime.tool_grant_host_requester() {
        Ok(requester) => requester,
        Err(message) => return RpcResponse::error(id, error::INTERNAL_ERROR, message),
    };
    // The life of the subject the request is made in (a member identity's
    // life marker): an approval executed in another life issues nothing.
    let subject_life = runtime.tool_grant_subject_life(&subject.subject).await;
    let request = ToolGrantIssueRequest {
        subject: subject.subject,
        target,
        duration: Duration::from_millis(params.duration_ms),
        run_scope,
        reason: params.reason,
        origin: ToolGrantRequestOrigin::Host {
            request_id: uuid::Uuid::new_v4().to_string(),
        },
        requester,
        subject_life,
    };
    match runtime.runtime_adapter().request_tool_grant(
        &decisions,
        &ToolGrantRequestPolicy::default(),
        request,
    ) {
        Ok((decision, body)) => RpcResponse::success(
            id,
            RequestToolGrantResult {
                grant: wire_tool_grant_request(&decision, &body),
                decision: wire_decision(&decision, false),
            },
        ),
        Err(ToolGrantRequestError::Decision(error_value)) => {
            decision_error_to_response(id, error_value)
        }
        // The grant's window and target, refused as a grant's own are.
        Err(error_value @ ToolGrantRequestError::InvalidDuration { .. }) => refused(
            id,
            &ToolGrantRequestRefusal::InvalidWindow,
            error_value.to_string(),
        ),
        Err(error_value @ ToolGrantRequestError::EmptyTarget) => refused(
            id,
            &ToolGrantRequestRefusal::EmptyTarget,
            error_value.to_string(),
        ),
        // The clock, or a held body this binary cannot read back: the
        // server's fault.
        Err(
            error_value @ (ToolGrantRequestError::Clock(_)
            | ToolGrantRequestError::UnreadableRequest(_)),
        ) => RpcResponse::error(id, error::INTERNAL_ERROR, error_value.to_string()),
        // A request no decision can hold (a reason past a decision's size
        // bounds): plain invalid params, the decision contract's refusal.
        Err(error_value) => RpcResponse::error(id, error::INVALID_PARAMS, error_value.to_string()),
    }
}

/// Handle `tool_grants/revoke`. Idempotent: revoking a grant that already
/// ended answers it as it stands, with `changed` false.
pub async fn handle_revoke(
    id: Option<RpcId>,
    params: Option<&serde_json::value::RawValue>,
    runtime: Arc<SessionRuntime>,
) -> RpcResponse {
    let params: RevokeToolGrantParams = match parse_params(params) {
        Ok(params) => params,
        Err(response) => return response.with_id(id),
    };
    let grant_id = match ToolGrantId::parse(&params.grant_id) {
        Ok(grant_id) => grant_id,
        Err(error_value) => {
            return refused(
                id,
                &ToolGrantRequestRefusal::NotFound {
                    grant_id: params.grant_id,
                },
                error_value.to_string(),
            );
        }
    };
    let revocation = ToolGrantRevocation {
        actor: None,
        reason: params.reason,
    };
    match runtime
        .runtime_adapter()
        .revoke_tool_grant(&grant_id, revocation)
        .await
    {
        Ok(revoked) => RpcResponse::success(
            id,
            RevokeToolGrantResult {
                grant: wire_tool_grant(&revoked.grant),
                changed: revoked.changed,
            },
        ),
        Err(error_value) => grant_error_to_response(id, error_value),
    }
}

/// Handle `tool_grants/list`. Each due grant is settled as expired first;
/// ended grants (expired, revoked, or ended with their run) are listed only
/// when the request asks for them. On a runtime that keeps durable
/// decisions the subject's grant requests follow its grants: the open and
/// approved ones as `requested` and, when ended ones are asked for, the
/// rejected, expired, withdrawn and refused ones whose deadline is within
/// the grant policy's maximum lifetime of now as `declined`. A request whose
/// grant was issued is listed as that grant only.
pub async fn handle_list(
    id: Option<RpcId>,
    params: Option<&serde_json::value::RawValue>,
    runtime: Arc<SessionRuntime>,
) -> RpcResponse {
    let params: ListToolGrantsParams = match parse_params(params) {
        Ok(params) => params,
        Err(response) => return response.with_id(id),
    };
    let ReadSubject {
        subject,
        serving,
        next_build,
    } = match subject_for_read(id.clone(), &params.subject, &runtime).await {
        Ok(resolved) => resolved,
        Err(response) => return response,
    };
    let gate = match &next_build {
        Some(next_build) => Some(wire_member_gate(&runtime, serving.as_ref(), next_build).await),
        None => None,
    };
    let listing = match runtime
        .runtime_adapter()
        .list_tool_grants(&subject, serving.as_ref())
        .await
    {
        Ok(listing) => listing,
        Err(error_value) => return grant_error_to_response(id, error_value),
    };
    let mut grants: Vec<_> = listing
        .grants
        .iter()
        .filter(|grant| params.include_ended || matches!(grant.status, ToolGrantStatus::Active))
        .map(wire_tool_grant)
        .collect();
    let decisions = runtime.decision_service();
    if decisions.is_available() {
        let requests = match runtime.runtime_adapter().list_tool_grant_requests(
            &decisions,
            &subject,
            params.include_ended,
        ) {
            Ok(requests) => requests,
            Err(ToolGrantRequestError::Decision(error_value)) => {
                return decision_error_to_response(id, error_value);
            }
            Err(error_value) => {
                return RpcResponse::error(id, error::INTERNAL_ERROR, error_value.to_string());
            }
        };
        let issued: HashSet<&ToolGrantId> = listing
            .grants
            .iter()
            .map(|grant| &grant.record.grant_id)
            .collect();
        grants.extend(
            requests
                .iter()
                .filter(|(decision, _)| {
                    !issued.contains(&ToolGrantId::approved(&decision.decision_id))
                })
                .map(|(decision, body)| wire_tool_grant_request(decision, body)),
        );
    }
    RpcResponse::success(
        id,
        ListToolGrantsResult {
            grants,
            unreadable: listing
                .unreadable
                .iter()
                .map(wire_unreadable_tool_grant)
                .collect(),
            gate,
        },
    )
}

/// Handle `tool_grants/audit`: one page of the subject's audit log, in
/// sequence order. A row the server cannot read is reported in the page's
/// `unreadable`, and the rest of the page is served. A full page names its
/// last sequence, readable or not, as the next page's `after_sequence`.
pub async fn handle_audit(
    id: Option<RpcId>,
    params: Option<&serde_json::value::RawValue>,
    runtime: Arc<SessionRuntime>,
) -> RpcResponse {
    let params: ToolGrantAuditParams = match parse_params(params) {
        Ok(params) => params,
        Err(response) => return response.with_id(id),
    };
    let limit = match params.limit {
        None => DEFAULT_AUDIT_PAGE,
        Some(0) => {
            return refused(
                id,
                &ToolGrantRequestRefusal::InvalidPageLimit,
                "an audit page limit must be at least 1",
            );
        }
        Some(limit) => usize::try_from(limit)
            .unwrap_or(MAX_AUDIT_PAGE)
            .min(MAX_AUDIT_PAGE),
    };
    let subject = match subject_for_read(id.clone(), &params.subject, &runtime).await {
        Ok(resolved) => resolved.subject,
        Err(response) => return response,
    };
    match runtime
        .runtime_adapter()
        .read_tool_grant_audit(&subject, params.after_sequence, limit)
        .await
    {
        Ok(page) => {
            let next_sequence = if page.len() == limit {
                page.last_sequence()
            } else {
                None
            };
            RpcResponse::success(
                id,
                ToolGrantAuditResult {
                    rows: page.rows.iter().map(wire_tool_grant_audit_row).collect(),
                    unreadable: page
                        .unreadable
                        .iter()
                        .map(wire_unreadable_tool_grant_audit_row)
                        .collect(),
                    next_sequence,
                },
            )
        }
        Err(error_value) => grant_error_to_response(id, error_value),
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    /// A grant row that concurrent writers kept changing answers the busy
    /// class, which a retry resolves; the other service faults stay
    /// internal; the request's own faults are typed refusals.
    #[test]
    fn grant_service_errors_answer_their_classes() {
        let response = |error_value| grant_error_to_response(None, error_value);
        let code = |error_value| response(error_value).error.map(|error| error.code);
        assert_eq!(
            code(ToolGrantServiceError::Conflict {
                grant_id: "g1".to_string()
            }),
            Some(error::SESSION_BUSY)
        );
        assert_eq!(
            code(ToolGrantServiceError::Clock),
            Some(error::INTERNAL_ERROR)
        );
        let not_found = response(ToolGrantServiceError::NotFound {
            grant_id: "g1".to_string(),
        })
        .error
        .expect("refused");
        assert_eq!(not_found.code, error::INVALID_PARAMS);
        assert_eq!(
            not_found.data,
            Some(serde_json::json!({
                "kind": "tool_grant_request_refused",
                "tool_grant_request_refusal": {"kind": "not_found", "grant_id": "g1"},
            }))
        );
    }

    /// An identity grant applies now when the member's running build gates
    /// its target, except a gated MCP server the member's build configures
    /// and the running build's surface does not hold, which applies at the
    /// next build; it is not mounted when that build ignored every name it
    /// names, or, with no live build, when the next build leaves its family
    /// off; otherwise it applies at the next build. The running gate lists
    /// its gated names under `tools`, with its servers and its mode.
    #[test]
    fn an_identity_grant_answers_whether_it_applies_to_the_running_build() {
        use meerkat_core::tool_grant::InstalledToolGrantGate;
        use meerkat_core::types::ToolName;
        let running = RunningToolGrantGate {
            gate: InstalledToolGrantGate {
                gated: BTreeSet::from([ToolName::from("deploy")]),
                gated_servers: BTreeSet::from([
                    "drive".to_string(),
                    "mail".to_string(),
                    "tickets".to_string(),
                ]),
                gates_unconfigured_servers: false,
                ungated_servers: BTreeSet::new(),
                not_mounted_names: BTreeSet::from([ToolName::from("shell")]),
            },
            open: BTreeSet::new(),
            attached_servers: BTreeSet::from(["tickets".to_string()]),
        };
        let next_build = NextBuildGate {
            gated: GrantGatedTools::default()
                .with_tool("deploy")
                .with_tool("web_search")
                .with_mcp_server("drive")
                .with_mcp_server("mail")
                .with_mcp_server("tickets"),
            not_mounted: GrantGatedTools::default().with_family("shell"),
            configured_servers: BTreeSet::from(["mail".to_string(), "tickets".to_string()]),
        };
        let tool = |name: &str| ToolGrantTarget::Tool {
            name: ToolName::from(name),
        };
        let shell = ToolGrantTarget::Family {
            family: "shell".to_string(),
            names: BTreeSet::from([ToolName::from("shell")]),
        };
        let server = |server: &str| ToolGrantTarget::McpServer {
            server: server.to_string(),
            tools: None,
        };
        for (target, live, idle) in [
            (
                tool("deploy"),
                WireToolGrantApplies::Now,
                WireToolGrantApplies::AtNextBuild,
            ),
            // Configured and attached.
            (
                server("tickets"),
                WireToolGrantApplies::Now,
                WireToolGrantApplies::AtNextBuild,
            ),
            // Configured, left unattached by the running build.
            (
                server("mail"),
                WireToolGrantApplies::AtNextBuild,
                WireToolGrantApplies::AtNextBuild,
            ),
            // Gated, configured by no build: gated wherever it is attached.
            (
                server("drive"),
                WireToolGrantApplies::Now,
                WireToolGrantApplies::AtNextBuild,
            ),
            (
                shell,
                WireToolGrantApplies::NotMounted,
                WireToolGrantApplies::NotMounted,
            ),
            (
                tool("web_search"),
                WireToolGrantApplies::AtNextBuild,
                WireToolGrantApplies::AtNextBuild,
            ),
        ] {
            assert_eq!(
                grant_applies(Some(&running), &next_build, &target),
                live,
                "{target:?}"
            );
            assert_eq!(
                grant_applies(None, &next_build, &target),
                idle,
                "{target:?}"
            );
        }

        let listed = wire_running_gate(&running);
        assert_eq!(listed.gated.tools, ["deploy"]);
        assert_eq!(listed.gated.mcp_servers, ["drive", "mail", "tickets"]);
        assert!(!listed.gated.unconfigured_mcp_servers);
        assert_eq!(listed.not_mounted.tools, ["shell"]);
    }
}
