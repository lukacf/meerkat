//! Fresh local service instructions selected through the real helper task.
//! The source and acquisition owners remain the existing installed instances.
use super::*;
use crate::private_originals::setup_workflow::execution::WorkflowCustodyRefusal;
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

/// A refusal of a service task entry point. `Refused` is the source owner's
/// own answer. `ReprepareRequired` is this exact call's current authorization
/// check refusing to rebuild after its bounded waits for owner changes on the
/// work's own publication: Core's typed answer as Core gave it, no verdict,
/// and never a denial or unavailability.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ServiceTaskError {
    #[error(transparent)]
    Refused(#[from] PrivateOriginalError),
    #[error("service task authorization must be prepared again")]
    ReprepareRequired(meerkat_core::authorization::OperationRefused),
}

impl ServiceTaskError {
    /// The tool projection of [`Self::ReprepareRequired`]: Core's typed
    /// refusal to rebuild as Core gave it. It claims nothing about whether
    /// the call dispatched unless Core certified that it did not. A
    /// `Refused` answer keeps its caller's own projection.
    pub fn reprepare_required(
        refusal: meerkat_core::authorization::OperationRefused,
    ) -> meerkat_core::ToolError {
        meerkat_core::ToolError::AuthorizationRefused { refusal }
    }
}

// Diagnostic labels describe only this failed boundary, never authority or retry permission.
pub(super) fn shared_task_refusal(
    stage: &'static str,
    error: PrivateOriginalError,
) -> PrivateOriginalError {
    emit_shared_task_refusal(
        stage,
        match error {
            PrivateOriginalError::Denied => "denied",
            PrivateOriginalError::Unavailable => "unavailable",
            PrivateOriginalError::Conflict => "conflict",
        },
    );
    error
}
fn service_task_refusal(stage: &'static str, error: ServiceTaskError) -> ServiceTaskError {
    match error {
        ServiceTaskError::Refused(error) => shared_task_refusal(stage, error).into(),
        ServiceTaskError::ReprepareRequired(_) => {
            emit_shared_task_refusal(stage, "reprepare_required");
            error
        }
    }
}
fn emit_shared_task_refusal(stage: &'static str, kind: &'static str) {
    eprintln!(
        "{{\"event\":\"toolkit_shared_dispatch_refused\",\"stage\":\"{stage}\",\"kind\":\"{kind}\"}}"
    );
    crate::original_ingress::shared_entry_diagnostics::emit(stage, None, Some(kind));
}

/// Core's refusal to rebuild beneath `error`, when it is one: a stale read of
/// the work's own publication that ran nothing.
fn rebuild_refusal(
    error: &meerkat_core::ToolError,
) -> Option<meerkat_core::authorization::OperationRefused> {
    match error.primary_error() {
        meerkat_core::ToolError::AuthorizationRefused { refusal }
            if refusal.kind()
                == meerkat_core::authorization::OperationRefusalKind::ReprepareRequired =>
        {
            Some(*refusal)
        }
        _ => None,
    }
}

/// This exact call's current authorization answer as a service task answer.
/// A refusal to rebuild stays typed, as Core gave it. Every other refusal
/// keeps its source projection: a denial stays Denied and unavailability
/// stays Unavailable.
fn service_task_check(
    checked: Result<Option<ToolDispatchContext>, meerkat_core::ToolError>,
) -> Result<Option<ToolDispatchContext>, ServiceTaskError> {
    checked.map_err(service_task_error)
}

/// [`service_task_check`]'s answer for a refused check.
fn service_task_error(error: meerkat_core::ToolError) -> ServiceTaskError {
    match rebuild_refusal(&error) {
        Some(refusal) => ServiceTaskError::ReprepareRequired(refusal),
        None => authorization_error(error).into(),
    }
}

/// This exact call's pure current authorization check, checked again after a
/// refusal to rebuild: an owner change of the work's own publication was in
/// progress, or completed during the read. The refused check ran nothing, so
/// it takes Core's wait after a stale read with `churn`, the one waiting
/// state that every check of this entry point shares: one deadline, set at
/// the first wait within the call's remaining tool time, and one budget of
/// free rebuilds, neither renewed by a later check. Only the check repeats,
/// never the preparation around it. The caller holds no lock, store
/// transaction or source reservation across this wait, and cancelling it
/// releases nothing. Any other answer is final at once, and a refusal to
/// rebuild that outlasts the bounds stays typed. The check is Core's check
/// before a tool's effect: every task entry point's own steps follow either
/// nothing or the transport's effect entry, so its refusal is certified not
/// dispatched exactly while nothing of the call ran.
async fn check_current_after_owner_change(
    context: &ToolDispatchContext,
    call: ToolCallView<'_>,
    plan: Option<&meerkat_core::ResolvedToolExecutionPlan>,
    churn: &mut meerkat_core::authorization::OwnerChangeChurn,
    site: &'static str,
) -> Result<Option<ToolDispatchContext>, ServiceTaskError> {
    service_task_check(
        crate::current_check::check_before_effect_after_owner_change(
            context, call, plan, churn, site,
        )
        .await,
    )
}

impl InstalledServiceTaskSource {
    /// This exact call's current authorization check after its effect ran:
    /// a shared selected check that joined, then released, its physical
    /// selection. The caller holds no lock, publication guard or owner
    /// change across it. A refusal to rebuild (an owner change in progress
    /// on the work's own publication, or one completed during the read)
    /// takes Core's wait after a stale read with `churn`, the waiting state
    /// the call's checks after its effect share (one deadline within the
    /// call's remaining tool time, never renewed), and only the check
    /// repeats: never the dispatch, the join or the release. A change that
    /// outlasts the bounds stays Core's typed refusal, which claims nothing
    /// about dispatch, and every other answer is final at once.
    pub async fn check_after_shared_effect(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
        plan: Option<&meerkat_core::ResolvedToolExecutionPlan>,
        churn: &mut meerkat_core::authorization::OwnerChangeChurn,
        site: &'static str,
    ) -> Result<Option<ToolDispatchContext>, meerkat_core::ToolError> {
        crate::current_check::check_after_owner_change(context, call, plan, churn, site).await
    }
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
    ) -> Result<crate::private_originals::InstalledSharedPreparation, ServiceTaskError> {
        let mut churn = context.owner_change_churn();
        let task = self
            .prepare_selected(call, context, plan, None, &mut churn)
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
        check_current_after_owner_change(
            context,
            call,
            plan,
            &mut churn,
            "toolkit.service_task.join",
        )
        .await?;
        let member = task
            .join_shared_preparation(selection, owner.work_backend.clone(), resource)
            .await?;
        check_current_after_owner_change(
            context,
            call,
            plan,
            &mut churn,
            "toolkit.service_task.joined",
        )
        .await?;
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
    ) -> Result<crate::private_originals::InstalledSharedEntry, ServiceTaskError> {
        let mut churn = context.owner_change_churn();
        let task = self
            .prepare_selected(call, context, plan, None, &mut churn)
            .await
            .map_err(|error| service_task_refusal("reserve.prepare", error))?
            .ok_or_else(|| {
                shared_task_refusal("reserve.task_missing", PrivateOriginalError::Unavailable)
            })?;
        // The reservation is source custody that a cancellation drains, so
        // its check never waits while holding it. A refusal to rebuild
        // settles the unused reservation (no private transport command has
        // been prepared on this path), waits out the change holding nothing,
        // with this entry point's one churn, and reserves again: the task's
        // currentness, the member and the source entry are each checked
        // afresh, and the call checked again. A change that outlasts the
        // bounds, and every other refusal, settles the reservation and stays
        // typed.
        loop {
            let entry = crate::original_ingress::shared_entry_diagnostics::scope(
                task.shared_entry_diagnostic_context(call),
                async {
                    task.reserve_shared_entry(member)
                        .await
                        .map_err(|error| shared_task_refusal("reserve.source_entry", error))
                },
            )
            .await?;
            #[cfg(test)]
            task.observe_stage_for_test("reserve.authorization");
            let Err(error) = crate::current_check::check_before_effect(context, call, plan) else {
                return Ok(entry);
            };
            entry.settle_unentered()?;
            drop(entry);
            if crate::current_check::refuses_to_rebuild(&error)
                && crate::current_check::wait_after_refusal(
                    context,
                    &mut churn,
                    "toolkit.service_task.reserve",
                )
                .await
            {
                continue;
            }
            return Err(service_task_refusal(
                "reserve.authorization",
                service_task_error(error),
            ));
        }
    }
    /// Consume a shared-stop handoff only for this actual native helper's
    /// current captured acquisition. Historical job visibility is insufficient.
    pub async fn leave_shared_preparation(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
        plan: Option<&meerkat_core::ResolvedToolExecutionPlan>,
        member: &crate::private_originals::InstalledSharedPreparation,
    ) -> Result<serde_json::Value, ServiceTaskError> {
        if call.name != "setup_stop" {
            return Err(PrivateOriginalError::Denied.into());
        }
        let mut churn = context.owner_change_churn();
        let task = self
            .prepare_selected(call, context, plan, None, &mut churn)
            .await?
            .ok_or(PrivateOriginalError::Unavailable)?;
        check_current_after_owner_change(
            context,
            call,
            plan,
            &mut churn,
            "toolkit.service_task.leave",
        )
        .await?;
        Ok(task.leave_shared_preparation(member).await?)
    }
    pub async fn complete_shared_preparation(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
        plan: Option<&meerkat_core::ResolvedToolExecutionPlan>,
        member: &crate::private_originals::InstalledSharedPreparation,
    ) -> Result<(), ServiceTaskError> {
        if call.name != "setup_selected_install" {
            return Err(PrivateOriginalError::Denied.into());
        }
        let mut churn = context.owner_change_churn();
        let task = self
            .prepare_selected(call, context, plan, None, &mut churn)
            .await?
            .ok_or(PrivateOriginalError::Unavailable)?;
        check_current_after_owner_change(
            context,
            call,
            plan,
            &mut churn,
            "toolkit.service_task.complete",
        )
        .await?;
        Ok(task.complete_shared_preparation(member).await?)
    }
    pub async fn prepare(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
        plan: Option<&meerkat_core::ResolvedToolExecutionPlan>,
        agent: &str,
    ) -> Result<Option<InstalledServiceTask>, ServiceTaskError> {
        let mut churn = context.owner_change_churn();
        self.prepare_selected(call, context, plan, Some(agent), &mut churn)
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
    ) -> Result<Option<InstalledServiceTask>, ServiceTaskError> {
        let mut churn = context.owner_change_churn();
        self.prepare_selected(call, context, plan, None, &mut churn)
            .await
    }
    /// Public service review may select this member's already configured setup
    /// capability for the fresh task. Other preparation surfaces keep their
    /// existing capture contract and do not validate this optional selection.
    pub async fn prepare_public_service(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
        plan: Option<&meerkat_core::ResolvedToolExecutionPlan>,
    ) -> Result<Option<InstalledServiceTask>, ServiceTaskError> {
        let mut churn = context.owner_change_churn();
        self.prepare_selected_with_setup(call, context, plan, None, true, &mut churn)
            .await
    }
    /// Software review has no proposed target principal. Select the waiting
    /// member from this exact helper's existing canonical acquisition custody.
    pub async fn prepare_software(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
        plan: Option<&meerkat_core::ResolvedToolExecutionPlan>,
    ) -> Result<Option<InstalledServiceTask>, ServiceTaskError> {
        let mut churn = context.owner_change_churn();
        self.prepare_selected(call, context, plan, None, &mut churn)
            .await
    }
    async fn prepare_selected(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
        plan: Option<&meerkat_core::ResolvedToolExecutionPlan>,
        proposed_agent: Option<&str>,
        churn: &mut meerkat_core::authorization::OwnerChangeChurn,
    ) -> Result<Option<InstalledServiceTask>, ServiceTaskError> {
        self.prepare_selected_with_setup(call, context, plan, proposed_agent, false, churn)
            .await
    }
    async fn prepare_selected_with_setup(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
        plan: Option<&meerkat_core::ResolvedToolExecutionPlan>,
        proposed_agent: Option<&str>,
        select_setup: bool,
        churn: &mut meerkat_core::authorization::OwnerChangeChurn,
    ) -> Result<Option<InstalledServiceTask>, ServiceTaskError> {
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
                .workflow_task_custody(call, context, plan, churn)
                .await
                .map_err(|refusal| match refusal {
                    WorkflowCustodyRefusal::Refused(error) => {
                        shared_task_refusal("prepare.workflow_custody", error).into()
                    }
                    WorkflowCustodyRefusal::Current(error) => {
                        service_task_refusal("prepare.workflow_custody", service_task_error(error))
                    }
                })?,
            None => None,
        };
        #[cfg(test)]
        owner
            .host
            .originals
            .observe_service_task_stage_for_test("prepare.snapshot");
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
        #[cfg(test)]
        owner
            .host
            .originals
            .observe_service_task_stage_for_test("prepare.authorization");
        check_current_after_owner_change(
            context,
            call,
            plan,
            churn,
            "toolkit.service_task.authorization",
        )
        .await
        .inspect_err(|_| {
            eprintln!(
                r#"{{"event":"toolkit_shared_dispatch_refused","stage":"prepare.authorization"}}"#
            )
        })?;
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
        #[cfg(test)]
        owner
            .host
            .originals
            .observe_service_task_stage_for_test("prepare.service_task");
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
        #[cfg(test)]
        owner
            .host
            .originals
            .observe_service_task_stage_for_test("prepare.final_authorization");
        // The preparation above ran once. Only this check repeats after a
        // refusal to rebuild, with the same churn as the first check.
        check_current_after_owner_change(
            context,
            call,
            plan,
            churn,
            "toolkit.service_task.final_authorization",
        )
        .await
        .inspect_err(|_| eprintln!(r#"{{"event":"toolkit_shared_dispatch_refused","stage":"prepare.final_authorization"}}"#))?;
        Ok(task)
    }
}
