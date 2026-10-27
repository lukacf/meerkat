//! Actual selected helper result dispatch into the same acquisition owner.
//! Resource preparation MCP output is never interpreted as native readiness.
use super::*;
use meerkat_core::{
    ToolCallView, ToolCatalogCapabilities, ToolDef, ToolError, ToolMutationClass, ToolProvenance,
    ToolResult, ToolSourceKind, ops::ToolDispatchOutcome,
};
use meerkat_toolkit::{
    continuation::ContinuationStep,
    setup::{SETUP_REPORT, SETUP_RESULTS_BUNDLE, SetupHelperFailure, SetupHelperResultAccess},
};
use serde::Serialize;
use toolkit_store::EffectKey;

pub(crate) struct SetupResultsTool {
    owner: Arc<AcquisitionToolBinding>,
    tools: Arc<[Arc<ToolDef>]>,
}
impl SetupResultsTool {
    pub(super) fn new(owner: Arc<AcquisitionToolBinding>) -> Self {
        Self {
            owner,
            tools: vec![Arc::new(setup_report_definition())].into(),
        }
    }
}
#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum ContinuationObservation {
    Acknowledged,
    Pending,
    RetrySafe,
    Rejected,
    BudgetExhausted,
    Unavailable,
}
#[derive(Serialize)]
struct ResultReply {
    operation: Id,
    revision: u64,
    phase: toolkit_catalog::AcquisitionPhase,
    continuation: ContinuationObservation,
}

impl InstalledAcquisition {
    async fn report_result(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
    ) -> Result<ToolDispatchOutcome, ToolError> {
        // The dispatcher retained the installed native owner. This report
        // must still match the actual helper input and canonical source below.
        if call.id.is_empty() || call.args.get().len() > 128 * 1024 {
            return Err(invalid());
        }
        // Installed results never use the ungoverned native dispatch case.
        context.work_authorization().ok_or_else(denied)?;
        let report: SetupHelperFailure = call.parse_args().map_err(|_| invalid())?;
        let source = self
            .host
            .originals
            .setup_result_source_from(context, &self.helper_selections())
            .await;
        let initial_current = context.check_tool_authorization(call, None)?;
        let context = initial_current.as_ref().unwrap_or(context);
        let source = source.map_err(source_error)?;
        let operation = Id::new(source.operation.acquisition.id.clone()).map_err(|_| denied())?;
        let effect = EffectKey {
            record: source.operation.acquisition.clone(),
            id: source.operation.effect.clone(),
        };
        let authorization = context.work_authorization().ok_or_else(denied)?;
        let current = source.context.borrowed(authorization);
        let store = SetupStore::new_conditional(self.store.as_ref(), self.scope.clone())
            .map_err(|_| unavailable())?;
        let recorded = store
            .resolve_helper_failure(
                &operation,
                report,
                SetupHelperResultAccess {
                    helper: &source.helper,
                    source: &source.source,
                    contributors: &source.contributors,
                    contributor_sources: &source.contributor_sources,
                    ancestor_sources: &source.ancestor_sources,
                    effect: &effect,
                    input_digest: &source.operation.input_digest,
                    context: &current,
                },
                &self.work_backend,
                self.work_step.worker.as_str(),
            )
            .await;
        // All outcomes remain behind the same actual source/current host,
        // including a conflict or a lost storage result. No parent context is used.
        let after = self
            .host
            .originals
            .setup_result_source_from(context, &self.helper_selections())
            .await;
        let recorded_current = context.check_tool_authorization(call, None)?;
        let context = recorded_current.as_ref().unwrap_or(context);
        let after = after.map_err(source_error)?;
        if after.helper != source.helper
            || after.source != source.source
            || after.contributors != source.contributors
            || after.contributor_sources != source.contributor_sources
            || after.ancestor_sources != source.ancestor_sources
            || after.operation != source.operation
            || after.context.policy_revision != source.context.policy_revision
        {
            return Err(denied());
        }
        let recorded = recorded.map_err(|error| match error {
            SetupError::Denied | SetupError::WrongScope => denied(),
            SetupError::Store(toolkit_store::StoreError::AdmissionConflict) => invalid(),
            _ => unavailable(),
        })?;
        // The terminal producer is the canonical SetupStore result above. Its
        // existing outbox/continuation owner now gets an actual bounded step.
        // A pending/error result retains that same obligation for reconciliation.
        let continuation = match self
            .continue_original(
                &operation,
                &ContinuationStepOptions {
                    worker: self.work_step.worker.clone(),
                    lease_ms: self.work_step.lease_ms,
                    owner_timeout_ms: self.work_step.owner_timeout_ms,
                    settlement_budget_ms: self.work_step.settlement_budget_ms,
                },
            )
            .await
        {
            Ok(ContinuationStep::Acknowledged(_)) => ContinuationObservation::Acknowledged,
            Ok(ContinuationStep::Pending) => ContinuationObservation::Pending,
            Ok(ContinuationStep::RetryAuthorized) => ContinuationObservation::RetrySafe,
            Ok(ContinuationStep::OwnerRejected) => ContinuationObservation::Rejected,
            Ok(ContinuationStep::DispatchBudgetExhausted) => {
                ContinuationObservation::BudgetExhausted
            }
            Err(_) => ContinuationObservation::Unavailable,
        };
        // Continuation may await native admission or reconciliation. Reopen
        // the same current helper source after every outcome before disclosure.
        let after = self
            .host
            .originals
            .setup_result_source_from(context, &self.helper_selections())
            .await;
        // Recheck this exact borrowed call through the native owner after the
        // last wait. Its policy checks current host/component and full native
        // authority. Do not nest this under the host recovery custody lock:
        // that policy enters the same activation owner.
        let final_current = context.check_tool_authorization(call, None)?;
        let _current_context = final_current.as_ref().unwrap_or(context);
        let after = after.map_err(source_error)?;
        if after.helper != source.helper
            || after.source != source.source
            || after.contributors != source.contributors
            || after.contributor_sources != source.contributor_sources
            || after.ancestor_sources != source.ancestor_sources
            || after.operation != source.operation
            || after.context.policy_revision != source.context.policy_revision
        {
            return Err(denied());
        }
        // Keep the refreshed native context alive through the synchronous
        // projection. No later await can separate this check from the reply.
        let reply = ResultReply {
            operation: recorded.operation,
            revision: recorded.revision,
            phase: recorded.phase,
            continuation,
        };
        serde_json::to_string(&reply)
            .map(|body| ToolResult::new(call.id.into(), body, false).into())
            .map_err(|_| unavailable())
    }
}
#[async_trait]
impl AgentToolDispatcher for SetupResultsTool {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        self.tools.clone()
    }
    fn tool_catalog_capabilities(&self) -> ToolCatalogCapabilities {
        ToolCatalogCapabilities {
            exact_catalog: true,
            may_require_catalog_control_plane: false,
        }
    }
    fn tool_mutation_class(&self, name: &str) -> ToolMutationClass {
        if name == SETUP_REPORT {
            ToolMutationClass::Mutating
        } else {
            ToolMutationClass::Unknown
        }
    }
    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        if call.name != SETUP_REPORT {
            return Err(ToolError::NotFound {
                name: call.name.into(),
            });
        }
        Err(denied())
    }
    async fn dispatch_with_context(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
    ) -> Result<ToolDispatchOutcome, ToolError> {
        if call.name != SETUP_REPORT {
            return Err(ToolError::NotFound {
                name: call.name.into(),
            });
        }
        let owner = self.owner.execution_owner().ok_or_else(unavailable)?;
        // Existing owner/settlement bounds govern entered continuation work.
        // An unrelated outer timeout must not drop that entered step.
        owner.report_result(call, context).await
    }
}
fn denied() -> ToolError {
    ToolError::AccessDenied {
        name: SETUP_REPORT.into(),
    }
}
fn unavailable() -> ToolError {
    ToolError::unavailable(
        SETUP_REPORT,
        meerkat_core::ToolUnavailableReason::TemporarilyUnavailable,
    )
}
fn invalid() -> ToolError {
    ToolError::InvalidArguments { name: SETUP_REPORT.into(), reason: "Provide kind failed or unavailable and a nonempty human-facing explanation within 4096 bytes. Operation, authority and routing are supplied by the installed host.".into() }
}
fn source_error(error: PrivateOriginalError) -> ToolError {
    match error {
        PrivateOriginalError::Unavailable => unavailable(),
        _ => denied(),
    }
}
/// Exact native tool definition for the application's initial-surface compiler.
/// Its schema accepts report content only and cannot assert readiness.
pub fn setup_report_definition() -> ToolDef {
    ToolDef::new(
        SETUP_REPORT,
        "Record why this setup cannot finish. Use unavailable for a missing capability and failed for a known unsuccessful setup. Never report pending human authentication as failure. Successful setup must be established by the grant and runtime owners. Supply only a concise human-facing explanation, never raw tool logs, authentication payloads or credentials.",
        serde_json::json!({"type":"object","additionalProperties":false,"required":["kind","detail"],"properties":{"kind":{"type":"string","enum":["failed","unavailable"]},"detail":{"type":"string","minLength":1,"maxLength":4096}}}),
    ).with_provenance(ToolProvenance::new(
        ToolSourceKind::RustBundle,
        SETUP_RESULTS_BUNDLE,
    ))
}
