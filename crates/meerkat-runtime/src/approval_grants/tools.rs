//! The `decisions` tool family: `access_offer` and `reply_to_requester`.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use meerkat_core::approval::decision::{DecisionPhase, Sha256Digest};
use meerkat_core::error::ToolError;
use meerkat_core::types::{ToolCallView, ToolDef, ToolProvenance, ToolResult, ToolSourceKind};
use meerkat_core::{
    AgentToolDispatcher, ToolDispatchContext, ToolDispatchOutcome, ToolOperationTarget,
};
use serde::Deserialize;

use super::body::{GrantChildRequest, decision_of_approved_work};
use super::offer::{AccessOfferArguments, AccessOfferError, AccessOfferRefusal, CallingWork};
use super::seams::ReplyRouteError;
use super::{ACCESS_OFFER_TOOL, ApprovalGrants, GRANT_CHILD_KIND, REPLY_TO_REQUESTER_TOOL};
use crate::decision::{DecisionError, DecisionRefusal};

static NEXT_DECISIONS_EXECUTION_AUTHORITY_ID: AtomicU64 = AtomicU64::new(1);

/// The arguments of `reply_to_requester`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReplyArguments {
    text: String,
}

/// The `decisions` tools of a host's agents: `access_offer`, which holds a
/// `grant.child` decision for the calling work, and `reply_to_requester`,
/// the one route of an approved work to its requester. Both run only in
/// governed work. Every plan this owner resolves carries its witness with
/// the call's typed operation target: a hold for the offering grant's
/// approver id, or the requester reply.
///
/// A call refused for good fails as `execution_failed` with typed data, the
/// same payload whether it is refused when its plan resolves, when its plan
/// dispatches or at its effect: `{"refusal": "not_governed_work"}` outside
/// governed work of one original, `{"refusal": "not_an_approved_work"}` for
/// a reply outside an approved work, `{"refusal": "reply_already_settled"}`
/// for a reply with another text than the settled one, and an access
/// offer's refusal kind with its reason ([`AccessOfferRefusal`], for example
/// `{"refusal": "no_child_approval_rule", "reason": "no_parent_grant"}`). A
/// reply is settled before the host's route delivers it, so a delivery the
/// route cannot make now fails as `execution_failed` with
/// `{"refusal": "reply_route_unavailable", "retry": "same_text"}`: the
/// settled text is the one to send again. A grant owner that cannot answer
/// now fails the call as the retryable `tool_unavailable`
/// ([`meerkat_core::ToolUnavailableReason::TemporarilyUnavailable`]).
pub struct DecisionsToolDispatcher {
    grants: ApprovalGrants,
    tool_defs: Arc<[Arc<ToolDef>]>,
    key: String,
}

impl std::fmt::Debug for DecisionsToolDispatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DecisionsToolDispatcher")
            .finish_non_exhaustive()
    }
}

fn tool_def(name: &str, description: &str, input_schema: serde_json::Value) -> Arc<ToolDef> {
    let mut def = ToolDef::new(name, description, input_schema);
    def.provenance = Some(ToolProvenance {
        kind: ToolSourceKind::Builtin,
        source_id: "decisions".into(),
        artifact: None,
    });
    Arc::new(def)
}

fn decisions_tool_defs() -> Arc<[Arc<ToolDef>]> {
    Arc::from(vec![
        tool_def(
            ACCESS_OFFER_TOOL,
            "Offer the agent that asked you, for its requester, one exact operation of a governed \
             MCP source you can reach, under the source owner's decision. The offer waits for a \
             person's approval; once approved, the asking agent runs exactly that operation once \
             in a private branch and answers the requester directly. Nothing runs before the \
             approval.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "operation": {
                        "type": "object",
                        "properties": {
                            "server": {"type": "string", "description": "The MCP server."},
                            "tool": {"type": "string", "description": "The server's tool."},
                            "arguments": {
                                "type": "object",
                                "description": "The exact arguments of the call."
                            }
                        },
                        "required": ["server", "tool", "arguments"],
                        "additionalProperties": false
                    },
                    "uses": {"type": "integer", "minimum": 1},
                    "duration_ms": {"type": "integer", "minimum": 1},
                    "summary": {
                        "type": "string",
                        "description": "What the asking agent asked for, shown to the approver as data."
                    }
                },
                "required": ["operation", "summary"],
                "additionalProperties": false
            }),
        ),
        tool_def(
            REPLY_TO_REQUESTER_TOOL,
            "Send the answer of this approved work to the requester who asked, once. Only an \
             approved work can reply.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "text": {"type": "string", "description": "The answer to the requester."}
                },
                "required": ["text"],
                "additionalProperties": false
            }),
        ),
    ])
}

/// A `decisions` tool's typed refusal, the one value a call refused for good
/// is answered with wherever it is refused: when its plan resolves, when its
/// resolved plan dispatches, and at its effect. It surfaces as
/// `execution_failed` with the refusal's data.
struct Refusal {
    message: String,
    data: serde_json::Value,
}

impl Refusal {
    /// The refusal as the call's tool error.
    fn tool_error(self) -> ToolError {
        ToolError::ExecutionFailedWithData {
            message: self.message,
            data: self.data,
        }
    }

    /// The refusal as the call's plan resolution error, which becomes the
    /// same tool error.
    fn resolution_error(self, tool_name: &str) -> meerkat_core::ToolExecutionResolutionError {
        meerkat_core::ToolExecutionResolutionError::RefusedWithData {
            tool_name: tool_name.to_owned(),
            message: self.message,
            data: self.data,
        }
    }
}

/// The call is not in governed work of one original.
fn not_governed(name: &str) -> Refusal {
    Refusal {
        message: format!("{name} runs only in governed work of one original"),
        data: serde_json::json!({"refusal": "not_governed_work"}),
    }
}

/// An access offer refused at hold, with its kind and reason.
fn offer_refused(name: &str, refusal: &AccessOfferRefusal) -> Refusal {
    Refusal {
        message: format!("{name} refused: {refusal}"),
        data: serde_json::to_value(refusal).unwrap_or(serde_json::Value::Null),
    }
}

/// The work is not an approved work.
fn not_approved() -> Refusal {
    Refusal {
        message: "reply_to_requester runs only in an approved work".to_owned(),
        data: serde_json::json!({"refusal": "not_an_approved_work"}),
    }
}

/// Another reply is settled for the approved work, delivered or not: the
/// work replies once, with that text.
fn another_reply_settled() -> Refusal {
    Refusal {
        message: "another reply is settled for this approved work: it replies once, with that \
                  text, and a delivery of it that failed is retried with exactly that text"
            .to_owned(),
        data: serde_json::json!({"refusal": "reply_already_settled"}),
    }
}

/// The reply is settled, and the host's route could not deliver it now:
/// only the same text is ever delivered.
fn not_delivered_now(reason: &str) -> Refusal {
    Refusal {
        message: format!(
            "the reply is settled and was not delivered now ({reason}): retry with exactly the \
             same text"
        ),
        data: serde_json::json!({"refusal": "reply_route_unavailable", "retry": "same_text"}),
    }
}

/// Why the typed operation target of a `decisions` tool call cannot be
/// resolved.
enum TargetRefusal {
    /// The call's arguments do not parse.
    InvalidArguments(String),
    /// The call is refused for good, typed.
    Refused(Refusal),
    /// The grant owner cannot say now: the call may be made again.
    Unavailable,
}

impl TargetRefusal {
    fn tool_error(self, name: &str) -> ToolError {
        match self {
            Self::InvalidArguments(reason) => ToolError::invalid_arguments(name, reason),
            Self::Refused(refusal) => refusal.tool_error(),
            Self::Unavailable => ToolError::unavailable(
                name,
                meerkat_core::ToolUnavailableReason::TemporarilyUnavailable,
            ),
        }
    }

    fn resolution_error(self, name: &str) -> meerkat_core::ToolExecutionResolutionError {
        match self {
            Self::InvalidArguments(reason) => {
                meerkat_core::ToolExecutionResolutionError::InvalidArguments {
                    tool_name: name.to_owned(),
                    reason,
                }
            }
            Self::Refused(refusal) => refusal.resolution_error(name),
            Self::Unavailable => meerkat_core::ToolExecutionResolutionError::Unavailable {
                tool_name: name.to_owned(),
                reason: meerkat_core::ToolUnavailableReason::TemporarilyUnavailable,
            },
        }
    }
}

impl DecisionsToolDispatcher {
    /// The `decisions` tools of `grants`.
    #[must_use]
    pub fn new(grants: ApprovalGrants) -> Self {
        Self {
            grants,
            tool_defs: decisions_tool_defs(),
            key: format!(
                "decisions-tools:{}",
                NEXT_DECISIONS_EXECUTION_AUTHORITY_ID.fetch_add(1, Ordering::Relaxed)
            ),
        }
    }

    /// The typed operation target of `call` in `context`, derived from the
    /// same typed input and calling work its handler acts on: a hold for the
    /// approver id of the offering grant's child rule, or the requester
    /// reply. A refusal is the one the call's effect would answer.
    fn target(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
    ) -> Result<ToolOperationTarget, TargetRefusal> {
        match call.name {
            REPLY_TO_REQUESTER_TOOL => Ok(ToolOperationTarget::RequesterReply),
            ACCESS_OFFER_TOOL => {
                let arguments: AccessOfferArguments = serde_json::from_str(call.args.get())
                    .map_err(|error| TargetRefusal::InvalidArguments(error.to_string()))?;
                let calling = CallingWork::of_dispatch(context)
                    .ok_or_else(|| TargetRefusal::Refused(not_governed(call.name)))?;
                match self
                    .grants
                    .offer_approver(&calling, &arguments.operation.server)
                {
                    Ok(approver_id) => Ok(ToolOperationTarget::DecisionHold { approver_id }),
                    Err(AccessOfferError::Refused(refusal)) => {
                        Err(TargetRefusal::Refused(offer_refused(call.name, &refusal)))
                    }
                    Err(AccessOfferError::Unavailable(_)) => Err(TargetRefusal::Unavailable),
                    Err(error) => Err(TargetRefusal::InvalidArguments(error.to_string())),
                }
            }
            other => Err(TargetRefusal::InvalidArguments(format!(
                "unknown decisions tool {other}"
            ))),
        }
    }

    async fn access_offer(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
    ) -> Result<ToolDispatchOutcome, ToolError> {
        let arguments: AccessOfferArguments = serde_json::from_str(call.args.get())
            .map_err(|error| ToolError::invalid_arguments(call.name, error.to_string()))?;
        let calling = CallingWork::of_dispatch(context)
            .ok_or_else(|| not_governed(call.name).tool_error())?;
        match self.grants.offer(&calling, arguments) {
            Ok(decision) => {
                let result = serde_json::json!({
                    "decision_id": decision.decision_id,
                    "phase": decision.state.phase.as_str(),
                });
                Ok(ToolResult::new(call.id.to_string(), result.to_string(), false).into())
            }
            Err(AccessOfferError::Refused(refusal)) => {
                Err(offer_refused(call.name, &refusal).tool_error())
            }
            Err(AccessOfferError::InvalidArguments(reason)) => {
                Err(ToolError::invalid_arguments(call.name, reason))
            }
            Err(AccessOfferError::Unavailable(_)) => {
                Err(TargetRefusal::Unavailable.tool_error(call.name))
            }
            Err(error) => Err(ToolError::ExecutionFailed {
                message: error.to_string(),
            }),
        }
    }

    async fn reply_to_requester(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
    ) -> Result<ToolDispatchOutcome, ToolError> {
        let arguments: ReplyArguments = serde_json::from_str(call.args.get())
            .map_err(|error| ToolError::invalid_arguments(call.name, error.to_string()))?;
        let calling = CallingWork::of_dispatch(context)
            .ok_or_else(|| not_governed(call.name).tool_error())?;
        let decision_id = decision_of_approved_work(&calling.original_work)
            .ok_or_else(|| not_approved().tool_error())?;
        let inner = &self.grants.0;
        let decision =
            inner
                .decisions
                .get(&decision_id)
                .map_err(|error| ToolError::ExecutionFailed {
                    message: error.to_string(),
                })?;
        let request = GrantChildRequest::of(&decision).map_err(|_| not_approved().tool_error())?;
        if decision.descriptor.kind.as_str() != GRANT_CHILD_KIND
            || decision.state.phase != DecisionPhase::Executed
            || request.terms.work != calling.original_work
            || request.terms.requester != calling.requester
            || request.terms.grantee != calling.executor
            || request.terms.represented_subject != calling.represented_subject
        {
            return Err(not_approved().tool_error());
        }
        // The decision settles its work's one reply before it is handed to
        // the route: another text is refused here (the decision answers
        // `not_owed`), whatever the route does; the same text again is
        // handed to the route, which answers the first delivery.
        let digest = Sha256Digest::of(arguments.text.as_bytes());
        match inner.decisions.settle_reply(&decision_id, &digest) {
            Ok(_) => {}
            Err(DecisionError::Refused {
                refusal: DecisionRefusal::NotOwed,
                ..
            }) => return Err(another_reply_settled().tool_error()),
            Err(error) => {
                return Err(ToolError::ExecutionFailed {
                    message: error.to_string(),
                });
            }
        }
        match inner
            .replies
            .deliver(
                &request.origin,
                &calling.original_work.work,
                &arguments.text,
            )
            .await
        {
            Ok(receipt) => {
                let result = serde_json::json!({
                    "delivered": true,
                    "delivery": receipt.delivery,
                });
                Ok(ToolResult::new(call.id.to_string(), result.to_string(), false).into())
            }
            // The route holds another text for this reply id: its own guard
            // answers as the decision does.
            Err(ReplyRouteError::AlreadyDelivered) => Err(another_reply_settled().tool_error()),
            Err(ReplyRouteError::Unavailable(reason)) => {
                Err(not_delivered_now(&reason).tool_error())
            }
        }
    }
}

#[async_trait]
impl AgentToolDispatcher for DecisionsToolDispatcher {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        Arc::clone(&self.tool_defs)
    }

    fn tool_mutation_class(&self, tool_name: &str) -> meerkat_core::ToolMutationClass {
        match tool_name {
            ACCESS_OFFER_TOOL | REPLY_TO_REQUESTER_TOOL => {
                meerkat_core::ToolMutationClass::Mutating
            }
            _ => meerkat_core::ToolMutationClass::Unknown,
        }
    }

    fn review_entry_support(
        &self,
        tool_name: &str,
    ) -> meerkat_core::approval::review::ReviewEntrySupport {
        match tool_name {
            ACCESS_OFFER_TOOL | REPLY_TO_REQUESTER_TOOL => {
                meerkat_core::approval::review::ReviewEntrySupport::ConsumesAtEntry
            }
            _ => meerkat_core::approval::review::ReviewEntrySupport::Unsupported,
        }
    }

    /// The plan carries this owner's witness with the call's typed target.
    fn resolve_execution_plan(
        &self,
        call: ToolCallView<'_>,
        dispatch_context: &ToolDispatchContext,
        resolution_context: &meerkat_core::ToolExecutionResolutionContext,
    ) -> Result<meerkat_core::ResolvedToolExecutionPlan, meerkat_core::ToolExecutionResolutionError>
    {
        if !matches!(call.name, ACCESS_OFFER_TOOL | REPLY_TO_REQUESTER_TOOL) {
            return Err(meerkat_core::ToolExecutionResolutionError::NotFound {
                tool_name: call.name.to_string(),
            });
        }
        let target = self
            .target(call, dispatch_context)
            .map_err(|refusal| refusal.resolution_error(call.name))?;
        let witness = meerkat_core::ToolExecutionOwnerWitness::new(
            self.key.clone(),
            call.name,
            self.execution_binding_fingerprint(call.name)?,
        )?
        .with_operation_target(target);
        meerkat_core::ToolExecutionContract::default()
            .resolve_default(resolution_context.deadlines().clone())?
            .with_owner_witness(witness)
    }

    /// A resolved plan runs only when this owner resolved it for exactly
    /// the call's current target and binding; its leaf enters the effect
    /// once, immediately before the hold or the delivery.
    async fn dispatch_resolved_with_context(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
        plan: &meerkat_core::ResolvedToolExecutionPlan,
    ) -> Result<ToolDispatchOutcome, ToolError> {
        if !matches!(call.name, ACCESS_OFFER_TOOL | REPLY_TO_REQUESTER_TOOL) {
            return Err(ToolError::not_found(call.name));
        }
        if plan.mode() != meerkat_core::ToolExecutionMode::Fast {
            return Err(ToolError::unavailable(
                call.name,
                meerkat_core::ToolUnavailableReason::ExecutionModeOwnerUnavailable,
            ));
        }
        let binding = self
            .execution_binding_fingerprint(call.name)
            .map_err(ToolError::from)?;
        let target = self
            .target(call, context)
            .map_err(|refusal| refusal.tool_error(call.name))?;
        let resolved_here = plan.owner_witness(&self.key).is_some_and(|witness| {
            witness.owner_key() == call.name
                && witness.binding_fingerprint() == &binding
                && witness.operation_target() == Some(&target)
        });
        if !resolved_here {
            return Err(ToolError::unavailable(
                call.name,
                meerkat_core::ToolUnavailableReason::ExecutionOwnerChanged,
            ));
        }
        let (entering, _custody) = context.enter_reviewed_effect_held(call, Some(plan))?;
        let context = entering.as_ref().unwrap_or(context);
        match call.name {
            ACCESS_OFFER_TOOL => self.access_offer(call, context).await,
            _ => self.reply_to_requester(call, context).await,
        }
    }

    /// Outside a resolved governed dispatch these tools run nothing.
    async fn dispatch_with_context(
        &self,
        call: ToolCallView<'_>,
        _context: &ToolDispatchContext,
    ) -> Result<ToolDispatchOutcome, ToolError> {
        if !matches!(call.name, ACCESS_OFFER_TOOL | REPLY_TO_REQUESTER_TOOL) {
            return Err(ToolError::not_found(call.name));
        }
        Err(not_governed(call.name).tool_error())
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        self.dispatch_with_context(call, &ToolDispatchContext::default())
            .await
    }
}
