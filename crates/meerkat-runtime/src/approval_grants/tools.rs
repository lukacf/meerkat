//! The `decisions` tool family: `access_offer` and `reply_to_requester`.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use meerkat_core::error::ToolError;
use meerkat_core::types::{ToolCallView, ToolDef, ToolProvenance, ToolResult, ToolSourceKind};
use meerkat_core::{
    AgentToolDispatcher, ToolDispatchContext, ToolDispatchOutcome, ToolOperationTarget,
};
use serde::Deserialize;

use super::body::{GrantChildRequest, decision_of_approved_work};
use super::offer::{AccessOfferArguments, AccessOfferError, CallingWork};
use super::seams::ReplyRouteError;
use super::{ACCESS_OFFER_TOOL, ApprovalGrants, GRANT_CHILD_KIND, REPLY_TO_REQUESTER_TOOL};

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

fn not_governed(name: &str) -> ToolError {
    ToolError::ExecutionFailedWithData {
        message: format!("{name} runs only in governed work"),
        data: serde_json::json!({"refusal": "not_governed_work"}),
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
    /// reply.
    fn target(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
    ) -> Result<ToolOperationTarget, String> {
        match call.name {
            REPLY_TO_REQUESTER_TOOL => Ok(ToolOperationTarget::RequesterReply),
            ACCESS_OFFER_TOOL => {
                let arguments: AccessOfferArguments =
                    serde_json::from_str(call.args.get()).map_err(|error| error.to_string())?;
                let calling = CallingWork::of_dispatch(context)
                    .ok_or_else(|| "access_offer runs only in governed work".to_owned())?;
                let approver_id = self
                    .grants
                    .offer_approver(&calling, &arguments.operation.server)
                    .map_err(|refusal| refusal.to_string())?;
                Ok(ToolOperationTarget::DecisionHold { approver_id })
            }
            other => Err(format!("unknown decisions tool {other}")),
        }
    }

    async fn access_offer(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
    ) -> Result<ToolDispatchOutcome, ToolError> {
        let arguments: AccessOfferArguments = serde_json::from_str(call.args.get())
            .map_err(|error| ToolError::invalid_arguments(call.name, error.to_string()))?;
        let calling = CallingWork::of_dispatch(context).ok_or_else(|| not_governed(call.name))?;
        match self.grants.offer(&calling, arguments) {
            Ok(decision) => {
                let result = serde_json::json!({
                    "decision_id": decision.decision_id,
                    "phase": decision.state.phase.as_str(),
                });
                Ok(ToolResult::new(call.id.to_string(), result.to_string(), false).into())
            }
            Err(AccessOfferError::Refused(refusal)) => Err(ToolError::ExecutionFailedWithData {
                message: format!("access offer refused: {}", refusal.refusal.as_str()),
                data: serde_json::to_value(refusal).unwrap_or(serde_json::Value::Null),
            }),
            Err(AccessOfferError::InvalidArguments(reason)) => {
                Err(ToolError::invalid_arguments(call.name, reason))
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
        let calling = CallingWork::of_dispatch(context).ok_or_else(|| not_governed(call.name))?;
        let not_approved = || ToolError::ExecutionFailedWithData {
            message: "reply_to_requester runs only in an approved work".to_owned(),
            data: serde_json::json!({"refusal": "not_an_approved_work"}),
        };
        let decision_id =
            decision_of_approved_work(&calling.original_work).ok_or_else(not_approved)?;
        let inner = &self.grants.0;
        let decision =
            inner
                .decisions
                .get(&decision_id)
                .map_err(|error| ToolError::ExecutionFailed {
                    message: error.to_string(),
                })?;
        let request = GrantChildRequest::of(&decision).map_err(|_| not_approved())?;
        if decision.descriptor.kind.as_str() != GRANT_CHILD_KIND
            || request.terms.work != calling.original_work
            || request.terms.requester != calling.requester
            || request.terms.grantee != calling.executor
            || request.terms.represented_subject != calling.represented_subject
        {
            return Err(not_approved());
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
            Err(ReplyRouteError::AlreadyDelivered) => Err(ToolError::ExecutionFailedWithData {
                message: "this approved work already replied with another text".to_owned(),
                data: serde_json::json!({"refusal": "reply_already_delivered"}),
            }),
            Err(error) => Err(ToolError::ExecutionFailed {
                message: error.to_string(),
            }),
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
        let target = self.target(call, dispatch_context).map_err(|reason| {
            meerkat_core::ToolExecutionResolutionError::InvalidArguments {
                tool_name: call.name.to_string(),
                reason,
            }
        })?;
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
            .map_err(|reason| ToolError::invalid_arguments(call.name, reason))?;
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
        Err(not_governed(call.name))
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        self.dispatch_with_context(call, &ToolDispatchContext::default())
            .await
    }
}
