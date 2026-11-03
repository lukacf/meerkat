//! Fresh local service instructions selected through the real helper task.
//! The source and acquisition owners remain the existing installed instances.
use super::*;
use crate::private_originals::{InstalledServiceTask, authorization_error};
use meerkat_core::ToolCallView;
use std::sync::{Mutex, Weak};
use toolkit_store::EffectKey;

/// A weak view of this host's existing optional acquisition selection. Factory
/// construction precedes bootstrap, so resolve that same selection at dispatch.
#[derive(Clone)]
pub struct InstalledServiceTaskSource {
    pub(super) selected: Weak<Mutex<AcquisitionBootstrap>>,
}
// Diagnostic labels describe only this failed boundary, never authority or retry permission.
pub(super) fn shared_task_refusal(
    stage: &'static str,
    error: PrivateOriginalError,
) -> PrivateOriginalError {
    let kind = match error {
        PrivateOriginalError::Denied => "denied",
        PrivateOriginalError::Unavailable => "unavailable",
        PrivateOriginalError::Conflict => "conflict",
    };
    eprintln!(
        "{{\"event\":\"toolkit_shared_dispatch_refused\",\"stage\":\"{stage}\",\"kind\":\"{kind}\"}}"
    );
    crate::original_ingress::shared_entry_diagnostics::emit(stage, None, Some(kind));
    error
}
impl InstalledAcquisition {
    /// Current workflow custody supplies G independently of the static helper
    /// inventory. Only a real matching static selection can supply warm setup
    /// metadata; absence is allowed only for this exact requested workflow G.
    pub(crate) fn service_helper_selection(
        &self,
        requested: Option<&MobMemberBinding>,
        captured: &MobMemberBinding,
        workflow: Option<&crate::private_originals::setup_workflow::execution::WorkflowTaskCustody>,
    ) -> Result<
        Option<crate::original_ingress::helper_originals::HelperAdmissionSelection>,
        PrivateOriginalError,
    > {
        if let Some(custody) = workflow {
            custody.check_helper(captured)?;
            if requested == Some(captured)
                && !self
                    .helpers
                    .all()
                    .any(|(helper, _)| &helper.member == captured)
            {
                return Ok(None);
            }
        }
        let (helper, member) = self
            .helpers
            .get(requested)
            .map_err(|_| PrivateOriginalError::Denied)?;
        if !crate::private_originals::setup_workflow::execution::fixed_service_helper_matches(
            &helper.member,
            captured,
            workflow,
        )? {
            return Ok(None);
        }
        Ok(Some(
            crate::original_ingress::helper_originals::HelperAdmissionSelection {
                helper: helper.clone(),
                member: member.clone(),
                grant_caller: self.grant_caller.clone(),
            },
        ))
    }
}
impl InstalledServiceTaskSource {
    pub(crate) fn new(selected: &Arc<Mutex<AcquisitionBootstrap>>) -> Self {
        Self {
            selected: Arc::downgrade(selected),
        }
    }
    /// Return only this source owner's existing bootstrap-selected backend.
    /// A caller cannot substitute an equal serialized realm or storage path.
    pub fn work_backend(
        &self,
        source: &Arc<PrivateOriginals>,
    ) -> Result<SetupWorkBackend, PrivateOriginalError> {
        let selected = self
            .selected
            .upgrade()
            .ok_or(PrivateOriginalError::Unavailable)?;
        let owner = selected
            .lock()
            .map_err(|_| PrivateOriginalError::Unavailable)?
            .configured()
            .ok_or(PrivateOriginalError::Unavailable)?;
        if !Arc::ptr_eq(&owner.host.originals, source) {
            return Err(PrivateOriginalError::Conflict);
        }
        source
            .status_lifetime()?
            .with_current_setup(|| Ok(owner.work_backend.clone()))
            .map_err(|error| match error {
                HostLifetimeError::Unavailable => PrivateOriginalError::Unavailable,
                _ => PrivateOriginalError::Denied,
            })
    }
    pub fn stage_workflow_call(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
        plan: &meerkat_core::ResolvedToolExecutionPlan,
    ) -> Result<(), PrivateOriginalError> {
        let selected = self
            .selected
            .upgrade()
            .ok_or(PrivateOriginalError::Unavailable)?;
        let owner = selected
            .lock()
            .map_err(|_| PrivateOriginalError::Unavailable)?
            .configured();
        match owner {
            Some(owner) => owner
                .host
                .originals
                .stage_workflow_call(call, context, plan),
            None => {
                if serde_json::from_str::<serde_json::Value>(call.args.get())
                    .ok()
                    .is_some_and(|value| value.get("setup_acquisition").is_some())
                {
                    Err(PrivateOriginalError::Denied)
                } else {
                    Ok(())
                }
            }
        }
    }
    pub fn settle_unentered_workflow_call(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
        plan: Option<&meerkat_core::ResolvedToolExecutionPlan>,
    ) -> Result<(), PrivateOriginalError> {
        let Some(plan) = plan else {
            return Ok(());
        };
        let selected = self
            .selected
            .upgrade()
            .ok_or(PrivateOriginalError::Unavailable)?;
        let owner = selected
            .lock()
            .map_err(|_| PrivateOriginalError::Unavailable)?
            .configured();
        match owner {
            Some(owner) => owner
                .host
                .originals
                .settle_unentered_workflow_call(call, context, plan),
            None => Ok(()),
        }
    }
    /// Join only the physical selection returned on this helper's private
    /// Controller channel. Ordinary helper/source capture supplies acquisition
    /// and WorkGraph identities; none are accepted from tool arguments.
    pub async fn join_shared_preparation(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
        plan: Option<&meerkat_core::ResolvedToolExecutionPlan>,
        selection: &meerkat_toolkit::setup::SharedPreparationSelection,
        resource: Arc<dyn crate::private_originals::SharedPreparationResource>,
    ) -> Result<crate::private_originals::InstalledSharedPreparation, PrivateOriginalError> {
        let task = self
            .prepare_selected(call, context, plan, None)
            .await?
            .ok_or(PrivateOriginalError::Unavailable)?;
        let selected = self
            .selected
            .upgrade()
            .ok_or(PrivateOriginalError::Unavailable)?;
        let owner = selected
            .lock()
            .map_err(|_| PrivateOriginalError::Unavailable)?
            .configured()
            .ok_or(PrivateOriginalError::Unavailable)?;
        context
            .check_tool_authorization(call, plan)
            .map_err(authorization_error)?;
        let member = task
            .join_shared_preparation(selection, owner.work_backend.clone(), resource)
            .await?;
        context
            .check_tool_authorization(call, plan)
            .map_err(authorization_error)?;
        member.check_current().await?;
        Ok(member)
    }
    /// Reserve the captured source for this exact native dispatch. A joined
    /// member is required for selected-install effects; initial observations
    /// and selected-check use the same source fence before P exists.
    pub async fn reserve_shared_entry(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
        plan: Option<&meerkat_core::ResolvedToolExecutionPlan>,
        member: Option<&crate::private_originals::InstalledSharedPreparation>,
    ) -> Result<crate::private_originals::InstalledSharedEntry, PrivateOriginalError> {
        let task = self
            .prepare_selected(call, context, plan, None)
            .await
            .map_err(|error| shared_task_refusal("reserve.prepare", error))?
            .ok_or_else(|| {
                shared_task_refusal("reserve.task_missing", PrivateOriginalError::Unavailable)
            })?;
        let entry = crate::original_ingress::shared_entry_diagnostics::scope(
            task.shared_entry_diagnostic_context(call),
            async {
                task.reserve_shared_entry(member)
                    .await
                    .map_err(|error| shared_task_refusal("reserve.source_entry", error))
            },
        )
        .await?;
        if let Err(error) = context.check_tool_authorization(call, plan) {
            // No private transport command has been prepared on this path.
            entry.settle_unentered()?;
            return Err(shared_task_refusal(
                "reserve.authorization",
                authorization_error(error),
            ));
        }
        Ok(entry)
    }
    /// Consume a shared-stop handoff only for this actual native helper's
    /// current captured acquisition. Historical job visibility is insufficient.
    pub async fn leave_shared_preparation(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
        plan: Option<&meerkat_core::ResolvedToolExecutionPlan>,
        member: &crate::private_originals::InstalledSharedPreparation,
    ) -> Result<serde_json::Value, PrivateOriginalError> {
        if call.name != "setup_stop" {
            return Err(PrivateOriginalError::Denied);
        }
        let task = self
            .prepare_selected(call, context, plan, None)
            .await?
            .ok_or(PrivateOriginalError::Unavailable)?;
        context
            .check_tool_authorization(call, plan)
            .map_err(authorization_error)?;
        task.leave_shared_preparation(member).await
    }
    pub async fn complete_shared_preparation(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
        plan: Option<&meerkat_core::ResolvedToolExecutionPlan>,
        member: &crate::private_originals::InstalledSharedPreparation,
    ) -> Result<(), PrivateOriginalError> {
        if call.name != "setup_selected_install" {
            return Err(PrivateOriginalError::Denied);
        }
        let task = self
            .prepare_selected(call, context, plan, None)
            .await?
            .ok_or(PrivateOriginalError::Unavailable)?;
        context
            .check_tool_authorization(call, plan)
            .map_err(authorization_error)?;
        task.complete_shared_preparation(member).await
    }
    pub async fn prepare(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
        plan: Option<&meerkat_core::ResolvedToolExecutionPlan>,
        agent: &str,
    ) -> Result<Option<InstalledServiceTask>, PrivateOriginalError> {
        self.prepare_selected(call, context, plan, Some(agent))
            .await
    }
    /// Prepare only this helper's captured waiting consumer through the same
    /// source, audience and currentness checks. This performs no review or
    /// effect and does not accept or resolve a proposed member name.
    pub async fn prepare_captured_consumer(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
        plan: Option<&meerkat_core::ResolvedToolExecutionPlan>,
    ) -> Result<Option<InstalledServiceTask>, PrivateOriginalError> {
        self.prepare_selected(call, context, plan, None).await
    }
    /// Public service review may select this member's already configured setup
    /// capability for the fresh task. Other preparation surfaces keep their
    /// existing capture contract and do not validate this optional selection.
    pub async fn prepare_public_service(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
        plan: Option<&meerkat_core::ResolvedToolExecutionPlan>,
    ) -> Result<Option<InstalledServiceTask>, PrivateOriginalError> {
        self.prepare_selected_with_setup(call, context, plan, None, true)
            .await
    }
    /// Software review has no proposed target principal. Select the waiting
    /// member from this exact helper's existing canonical acquisition custody.
    pub async fn prepare_software(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
        plan: Option<&meerkat_core::ResolvedToolExecutionPlan>,
    ) -> Result<Option<InstalledServiceTask>, PrivateOriginalError> {
        self.prepare_selected(call, context, plan, None).await
    }
    async fn prepare_selected(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
        plan: Option<&meerkat_core::ResolvedToolExecutionPlan>,
        proposed_agent: Option<&str>,
    ) -> Result<Option<InstalledServiceTask>, PrivateOriginalError> {
        self.prepare_selected_with_setup(call, context, plan, proposed_agent, false)
            .await
    }
    async fn prepare_selected_with_setup(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
        plan: Option<&meerkat_core::ResolvedToolExecutionPlan>,
        proposed_agent: Option<&str>,
        select_setup: bool,
    ) -> Result<Option<InstalledServiceTask>, PrivateOriginalError> {
        let selected = self.selected.upgrade().ok_or_else(|| {
            shared_task_refusal(
                "prepare.selected_missing",
                PrivateOriginalError::Unavailable,
            )
        })?;
        // Carry only a fixed label across this lock boundary. Emit after the
        // same selection guard (including a poisoned guard) has been dropped.
        let owner = selected
            .lock()
            .map_err(|_| "prepare.selected_poisoned")
            .and_then(|selected| selected.configured().ok_or("prepare.owner_unconfigured"))
            .map_err(|stage| shared_task_refusal(stage, PrivateOriginalError::Unavailable))?;
        let store = SetupStore::new_conditional(owner.store.as_ref(), owner.scope.clone())
            .map_err(|error| {
                crate::aa1_setup_failure("prepare.setup_store", &error);
                PrivateOriginalError::Unavailable
            })?;
        let workflow = match plan {
            Some(plan) => owner
                .host
                .originals
                .workflow_task_custody(call, context, plan)
                .map_err(|error| shared_task_refusal("prepare.workflow_custody", error))?,
            None => None,
        };
        let (snapshot, selected_helper) = if let Some(custody) = &workflow {
            owner
                .host
                .originals
                .workflow_service_task_snapshot(custody, &owner.work_backend)
                .await
                .map_err(|error| shared_task_refusal("prepare.workflow_snapshot", error))?
        } else {
            let source = owner
                .host
                .originals
                .setup_result_source_from(context, &owner.helper_selections())
                .await
                .map_err(|error| shared_task_refusal("prepare.result_source", error))?;
            let operation = Id::new(source.operation.acquisition.id.clone())
                .map_err(|_| PrivateOriginalError::Conflict)?;
            let effect = EffectKey {
                record: source.operation.acquisition.clone(),
                id: source.operation.effect.clone(),
            };
            let authorization = context
                .work_authorization()
                .ok_or(PrivateOriginalError::Denied)?;
            let current = source.context.borrowed(authorization);
            let (snapshot, _) = store
                .helper_result_snapshot(
                    &operation,
                    &meerkat_toolkit::setup::SetupHelperResultAccess {
                        helper: &source.helper,
                        source: &source.source,
                        contributors: &source.contributors,
                        contributor_sources: &source.contributor_sources,
                        ancestor_sources: &source.ancestor_sources,
                        effect: &effect,
                        input_digest: &source.operation.input_digest,
                        context: &current,
                    },
                    &owner.work_backend,
                )
                .await
                .map_err(|error| {
                    crate::aa1_setup_failure("prepare.helper_result_snapshot", &error);
                    PrivateOriginalError::Unavailable
                })?;
            (snapshot, source.selected_helper)
        };
        let selected = owner
            .service_helper_selection(
                snapshot.request().host().setup_helper.as_ref(),
                &selected_helper,
                workflow.as_ref(),
            )
            .map_err(|error| shared_task_refusal("prepare.helper_selection", error))?;
        // This exact call owns disclosure. No historical request is a permit.
        context.check_tool_authorization(call, plan)
            .inspect_err(|_| eprintln!(r#"{{"event":"toolkit_shared_dispatch_refused","stage":"prepare.authorization"}}"#))
            .map_err(authorization_error)?;
        let agent = match proposed_agent {
            Some(agent) => agent.to_owned(),
            None => snapshot
                .request()
                .host()
                .continuation
                .native_original
                .as_ref()
                .and_then(|task| task.member.as_ref())
                .ok_or_else(|| {
                    shared_task_refusal(
                        "prepare.continuation_member_missing",
                        PrivateOriginalError::Unavailable,
                    )
                })?
                .member
                .clone(),
        };
        let task = owner
            .host
            .originals
            .prepare_service_task(owner.store.clone(), snapshot, call.id, &agent)
            .await
            .map_err(|error| shared_task_refusal("prepare.service_task", error))?;
        let task = task
            .map(|task| task.with_workflow(workflow))
            .transpose()
            .map_err(|error| shared_task_refusal("prepare.workflow_binding", error))?;
        let task = if let Some(selected) = selected.filter(|_| select_setup) {
            task.map(|task| {
                task.select_configured_setup(
                    &selected.helper,
                    &selected.member,
                    context
                        .origin_session_id()
                        .ok_or(PrivateOriginalError::Denied)?,
                )
            })
            .transpose()
            .map_err(|error| shared_task_refusal("prepare.configured_setup", error))?
        } else {
            task
        };
        context.check_tool_authorization(call, plan)
            .inspect_err(|_| eprintln!(r#"{{"event":"toolkit_shared_dispatch_refused","stage":"prepare.final_authorization"}}"#))
            .map_err(authorization_error)?;
        Ok(task)
    }
}
