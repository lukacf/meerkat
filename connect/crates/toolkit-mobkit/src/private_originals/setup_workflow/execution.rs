//! Exact public/setup call selection from the existing enrolled workflow.
//! These handles retain source custody, never permission or a result lifecycle.
use super::*;
use crate::installed_host::acquisition::ConfiguredHelperReportCapability;
use crate::private_originals::setup_results::hex;
use meerkat_core::{
    ResolvedToolExecutionPlan, ToolDispatchContext, ToolOperationTarget,
    exact_operation::OperationExecutionScope,
};
use toolkit_store::RecordCondition;

#[derive(Clone)]
pub(crate) struct WorkflowTaskCustody {
    pub(super) waiter: Arc<HeldWorkflowWaiter>,
    pub(crate) entry: Arc<WorkflowCallCustody>,
}

/// A refusal of a call's workflow custody.
#[derive(Debug)]
pub(crate) enum WorkflowCustodyRefusal {
    /// The source owner's own answer.
    Refused(PrivateOriginalError),
    /// The call's current authorization refused to rebuild after its bounded
    /// waits: Core's typed answer as Core gave it.
    Current(meerkat_core::ToolError),
}
impl From<PrivateOriginalError> for WorkflowCustodyRefusal {
    fn from(error: PrivateOriginalError) -> Self {
        Self::Refused(error)
    }
}

/// Validate the service-task helper correlation. A current workflow owns G's
/// selection; the request's fixed helper is only eligible for optional warm
/// setup metadata when it is that same member. This grants no operation access.
pub(crate) fn fixed_service_helper_matches(
    fixed: &MobMemberBinding,
    captured: &MobMemberBinding,
    workflow: Option<&WorkflowTaskCustody>,
) -> Result<bool, PrivateOriginalError> {
    if let Some(custody) = workflow {
        custody.check_helper(captured)?;
    } else if fixed != captured {
        return Err(PrivateOriginalError::Denied);
    }
    Ok(fixed == captured)
}

impl WorkflowTaskCustody {
    pub(crate) fn check_helper(
        &self,
        captured: &MobMemberBinding,
    ) -> Result<(), PrivateOriginalError> {
        self.current()?;
        let held = self
            .waiter
            .workflow
            .upgrade()
            .ok_or(PrivateOriginalError::Unavailable)?;
        if &held.member != captured {
            return Err(PrivateOriginalError::Denied);
        }
        Ok(())
    }
    pub(crate) fn matches_task(
        &self,
        original: &InputAuthorityAssociation,
        acquisition: &toolkit_store::StoreKey,
    ) -> bool {
        self.waiter.authority.original() == original
            && self.waiter.membership.acquisition() == acquisition
    }
    /// Preparation membership can outlive one settled call. New effect entry
    /// still requires its own exact call's native binding and source gate.
    pub(crate) fn current(&self) -> Result<(), PrivateOriginalError> {
        self.waiter
            .workflow
            .upgrade()
            .ok_or(PrivateOriginalError::Unavailable)?
            .current_operation_source()?;
        self.entry.current_relationship().map_err(custody_error)?;
        Ok(())
    }
    pub(crate) fn effect_current(&self) -> Result<(), PrivateOriginalError> {
        self.entry.check_source_current().map_err(custody_error)
    }
    pub(crate) async fn conditions(
        &self,
        source: &PrivateOriginals,
    ) -> Result<Vec<RecordCondition>, PrivateOriginalError> {
        self.conditions_from(source, || source.workflow_store())
            .await
    }
    // The supplied read view is always this source's store in production. Tests
    // may wrap that same store to fail an exact canonical read without inventing
    // a workflow, acquisition, source association or successful observation.
    pub(super) async fn conditions_from<'a>(
        &self,
        source: &'a PrivateOriginals,
        read_store: impl FnOnce() -> Result<SetupStore<'a>, PrivateOriginalError>,
    ) -> Result<Vec<RecordCondition>, PrivateOriginalError> {
        self.current()?;
        let workflow = self
            .waiter
            .workflow
            .upgrade()
            .ok_or(PrivateOriginalError::Unavailable)?;
        let store = read_store()?;
        let (snapshot, _) = store
            .workflow_waiter_recovery_snapshot(
                &self.waiter.membership,
                self.waiter.authority.original(),
            )
            .await
            .map_err(workflow_read_error)?;
        let row = source
            .store
            .get(&snapshot.terms().planning_source)
            .await
            .map_err(store_error)?
            .ok_or(PrivateOriginalError::Unavailable)?;
        let reference = source
            .retained
            .lock()
            .map_err(|_| PrivateOriginalError::Unavailable)?
            .iter()
            .find(|held| Arc::ptr_eq(&held.original, &workflow.planning))
            .map(|held| held.reference.clone())
            .ok_or(PrivateOriginalError::Unavailable)?;
        if source.key(&reference)? != row.key {
            return Err(PrivateOriginalError::Conflict);
        }
        let record = source.decode_record(&row, &reference)?;
        if record.operation_cancelled
            || record.final_association.as_ref() != Some(workflow.planning.association())
        {
            return Err(PrivateOriginalError::Denied);
        }
        self.current()?;
        Ok(vec![
            snapshot.condition(),
            RecordCondition {
                key: row.key,
                revision: row.revision,
                content_digest: row.content_digest,
                claim: None,
                dispatch_claim: None,
            },
        ])
    }
}

fn workflow_read_error(error: meerkat_toolkit::setup::SetupError) -> PrivateOriginalError {
    match error {
        meerkat_toolkit::setup::SetupError::Store(error) => store_error(error),
        _ => PrivateOriginalError::Denied,
    }
}

fn selector(call: ToolCallView<'_>) -> Result<Option<Id>, PrivateOriginalError> {
    #[derive(Deserialize)]
    struct Selection {
        #[serde(default, deserialize_with = "selected")]
        setup_acquisition: Option<Id>,
    }
    fn selected<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Option<Id>, D::Error> {
        let value = String::deserialize(deserializer)?;
        Id::new(value).map(Some).map_err(serde::de::Error::custom)
    }
    serde_json::from_str::<Selection>(call.args.get())
        .map(|value| value.setup_acquisition)
        .map_err(|_| PrivateOriginalError::Conflict)
}
fn target(plan: &ResolvedToolExecutionPlan) -> Result<Option<GrantTarget>, PrivateOriginalError> {
    Ok(
        match plan
            .single_operation_target()
            .map_err(|_| PrivateOriginalError::Unavailable)?
        {
            Some(ToolOperationTarget::McpServerOperation { server, .. }) => {
                Some(GrantTarget::McpServer {
                    server: server.clone(),
                })
            }
            Some(ToolOperationTarget::RegisteredToolBundleOperation {
                bundle, operation, ..
            }) => {
                let target = GrantTarget::RegisteredToolBundleOperation {
                    bundle: bundle.clone(),
                    operation: operation.clone(),
                };
                (target == ConfiguredHelperReportCapability::public_target()).then_some(target)
            }
            _ => None,
        },
    )
}
impl PrivateOriginals {
    fn workflow_for_dispatch(
        &self,
        context: &ToolDispatchContext,
    ) -> Result<Option<Arc<HeldSetupWorkflow>>, PrivateOriginalError> {
        let authorization = context
            .work_authorization()
            .ok_or(PrivateOriginalError::Unavailable)?;
        let retained = context
            .retained_work()
            .ok_or(PrivateOriginalError::Unavailable)?;
        let session = context
            .origin_session_id()
            .ok_or(PrivateOriginalError::Unavailable)?;
        let OperationExecutionScope::RuntimeInput {
            owner_session_id,
            canonical_input_id,
            ..
        } = authorization.execution_scope()
        else {
            return Err(PrivateOriginalError::Denied);
        };
        if session != owner_session_id
            || context.run_id().is_some_and(|run| run != retained.run_id())
            || retained.runtime_id()
                != meerkat_runtime::LogicalRuntimeId::for_session(session).to_string()
        {
            return Err(PrivateOriginalError::Denied);
        }
        let selected = retained
            .selected_input_bindings()
            .get(&canonical_input_id.to_string())
            .ok_or(PrivateOriginalError::Denied)?;
        let mut matched = None;
        let held = self
            .retained
            .lock()
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        for source in held.iter() {
            let association = source.original.association();
            let binding = hex(&association
                .canonical_bytes()
                .map_err(|_| PrivateOriginalError::Conflict)?);
            let batch = hex(&association
                .batch_identity_bytes()
                .map_err(|_| PrivateOriginalError::Conflict)?);
            if selected.binding != binding || selected.batch_key != batch {
                continue;
            }
            let digest = ContentDigest::of_bytes(
                &serde_json::to_vec(association).map_err(|_| PrivateOriginalError::Conflict)?,
            );
            if !retained
                .contributors()
                .iter()
                .any(|row| row.association_digest == digest.as_str())
                || association.candidate().target.context.as_str() != session.to_string()
            {
                return Err(PrivateOriginalError::Denied);
            }
            if matched.is_some() {
                return Err(PrivateOriginalError::Conflict);
            }
            matched = Some(source.setup_workflow.get().cloned());
        }
        Ok(matched.flatten())
    }
    fn workflow_call_selection(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
        plan: &ResolvedToolExecutionPlan,
    ) -> Result<Option<Arc<HeldWorkflowWaiter>>, PrivateOriginalError> {
        let selected = selector(call)?;
        if selected.is_none()
            && (context.work_authorization().is_none()
                || context.retained_work().is_none()
                || context.origin_session_id().is_none())
        {
            return Ok(None);
        }
        let Some(workflow) = self.workflow_for_dispatch(context).inspect_err(|_| {
            eprintln!(
                r#"{{"event":"toolkit_shared_dispatch_refused","stage":"workflow.select.context"}}"#
            )
        })?
        else {
            return if selected.is_none() {
                Ok(None)
            } else {
                Err(PrivateOriginalError::Denied)
            };
        };
        let Some(target) = target(plan).inspect_err(|_| {
            eprintln!(
                r#"{{"event":"toolkit_shared_dispatch_refused","stage":"workflow.select.target"}}"#
            )
        })?
        else {
            return if selected.is_none() {
                Ok(None)
            } else {
                Err(PrivateOriginalError::Denied)
            };
        };
        workflow.current_operation_source()
            .inspect_err(|_| eprintln!(r#"{{"event":"toolkit_shared_dispatch_refused","stage":"workflow.select.current_source"}}"#))?;
        let selected = selected.ok_or(PrivateOriginalError::Denied)?;
        workflow.selected_waiter(&selected, &target)
            .inspect_err(|_| eprintln!(r#"{{"event":"toolkit_shared_dispatch_refused","stage":"workflow.select.waiter"}}"#))
            .map(Some)
    }
    pub(crate) fn stage_workflow_call(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
        plan: &ResolvedToolExecutionPlan,
    ) -> Result<(), PrivateOriginalError> {
        if let Some(waiter) = self.workflow_call_selection(call, context, plan)? {
            waiter.stage_call(call).inspect_err(|_| {
                eprintln!(
                    r#"{{"event":"toolkit_shared_dispatch_refused","stage":"workflow.stage.call"}}"#
                )
            })?;
        }
        Ok(())
    }
    pub(crate) fn settle_unentered_workflow_call(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
        plan: &ResolvedToolExecutionPlan,
    ) -> Result<(), PrivateOriginalError> {
        let Some(selected) = selector(call)? else {
            return Ok(());
        };
        let Some(workflow) = self.workflow_for_dispatch(context)? else {
            return Ok(());
        };
        let Some(target) = target(plan)? else {
            return Ok(());
        };
        workflow
            .selected_waiter(&selected, &target)?
            .authority
            .settle_unentered_call(call)
            .map_err(custody_error)
    }
    /// The workflow custody of this exact call: its selected waiter, the
    /// call's current authorization, then the selected call's entry. The
    /// check holds only the shared waiter selection, so an owner change of
    /// the work's own publication in progress is waited out with `churn`,
    /// the waiting state of the call's entry point, and the check made
    /// again. A refusal to rebuild that outlasts the bounds stays Core's
    /// typed answer ([`WorkflowCustodyRefusal::Current`]), never a denial;
    /// every other refusal keeps its source projection.
    pub(crate) async fn workflow_task_custody(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
        plan: &ResolvedToolExecutionPlan,
        churn: &mut meerkat_core::authorization::OwnerChangeChurn,
    ) -> Result<Option<WorkflowTaskCustody>, WorkflowCustodyRefusal> {
        let Some(waiter) = self.workflow_call_selection(call, context, plan)? else {
            return Ok(None);
        };
        if let Err(error) = crate::current_check::check_before_effect_after_owner_change(
            context,
            call,
            Some(plan),
            churn,
            "toolkit.workflow.custody",
        )
        .await
        {
            return Err(if crate::current_check::refuses_to_rebuild(&error) {
                WorkflowCustodyRefusal::Current(error)
            } else {
                authorization_error(error).into()
            });
        }
        let entry = waiter
            .authority
            .selected_call(call)
            .map_err(custody_error)?;
        entry.current().map_err(custody_error)?;
        Ok(Some(WorkflowTaskCustody { waiter, entry }))
    }
    pub(crate) async fn workflow_service_task_snapshot(
        &self,
        custody: &WorkflowTaskCustody,
        backend: &meerkat_toolkit::setup::SetupWorkBackend,
    ) -> Result<(meerkat_toolkit::setup::SetupSnapshot, MobMemberBinding), PrivateOriginalError>
    {
        self.workflow_service_task_snapshot_from(custody, backend, || self.workflow_store())
            .await
    }
    pub(super) async fn workflow_service_task_snapshot_from<'a>(
        &'a self,
        custody: &WorkflowTaskCustody,
        backend: &meerkat_toolkit::setup::SetupWorkBackend,
        read_store: impl Fn() -> Result<SetupStore<'a>, PrivateOriginalError>,
    ) -> Result<(meerkat_toolkit::setup::SetupSnapshot, MobMemberBinding), PrivateOriginalError>
    {
        custody.conditions_from(self, &read_store).await?;
        let workflow = custody
            .waiter
            .workflow
            .upgrade()
            .ok_or(PrivateOriginalError::Unavailable)?;
        let store = read_store()?;
        let snapshot = store
            .workflow_service_task_snapshot(
                &custody.waiter.membership,
                custody.waiter.authority.original(),
                backend,
            )
            .await
            .map_err(workflow_read_error)?;
        custody.current()?;
        Ok((snapshot, workflow.member.clone()))
    }
}
impl HeldSetupWorkflow {
    fn selected_waiter(
        &self,
        selection: &Id,
        target: &GrantTarget,
    ) -> Result<Arc<HeldWorkflowWaiter>, PrivateOriginalError> {
        let waiters = self
            .waiters
            .try_lock()
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        let mut matched = waiters.iter().filter(|waiter| {
            waiter.authority.selection() == selection && waiter.authority.target() == target
        });
        let selected = matched
            .next()
            .cloned()
            .ok_or(PrivateOriginalError::Denied)?;
        if matched.next().is_some() {
            return Err(PrivateOriginalError::Conflict);
        }
        Ok(selected)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::private_originals::governed_submit::tests::first_input_tests::FirstFixture;
    use futures::FutureExt;
    use std::{panic::AssertUnwindSafe, sync::mpsc, time::Duration};

    const GUARD: Duration = Duration::from_secs(30);

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn workflow_lookup_waits_for_retained_source_holder_without_changing_native_binding() {
        let fixture = FirstFixture::for_service_input().await;
        let checked = AssertUnwindSafe(async {
            let admitted = fixture.submit().await;
            fixture.finish(&admitted).await.unwrap();
            let context = fixture.probe.as_ref().unwrap().captured();
            let before = fixture.binding().await.unwrap().unwrap();
            let inputs = fixture.inputs().await.unwrap();
            let requests = fixture.model_requests();
            // A is an actual admitted original, with no workflow selected.
            // This checks identity lookup, not a permit after its run ended.
            assert!(context.work_authorization().is_some());
            assert!(context.retained_work().is_some());
            assert_eq!(context.origin_session_id(), Some(&fixture.session));
            assert!(matches!(fixture.source.workflow_for_dispatch(&context), Ok(None)));
            let (started, entered) = mpsc::sync_channel(0);
            let (returned, result) = mpsc::channel();
            let (worker, entered, early) = {
                let held = fixture.source.retained.lock().unwrap();
                assert!(!held.is_empty());
                let source = fixture.source.clone();
                let worker = std::thread::spawn(move || {
                    started.send(()).unwrap();
                    let selected = source.workflow_for_dispatch(&context).map(|value| value.is_none());
                    returned.send(selected).unwrap();
                });
                let entered = entered.recv_timeout(GUARD);
                // A bounded no-result observation while the actual mutex is held.
                // Release and join before asserting, including on early refusal.
                let early = result.recv_timeout(Duration::from_millis(100));
                (worker, entered, early)
            };
            let selected = match &early {
                Ok(selected) => Ok(*selected),
                Err(mpsc::RecvTimeoutError::Timeout) => result.recv_timeout(GUARD),
                Err(mpsc::RecvTimeoutError::Disconnected) => Err(mpsc::RecvTimeoutError::Disconnected),
            };
            let joined = worker.join();
            assert!(entered.is_ok(), "selector thread did not enter");
            assert!(joined.is_ok(), "selector thread did not join");
            assert!(matches!(early, Err(mpsc::RecvTimeoutError::Timeout)),
                "retained-source contention must wait, not return an authorization refusal: {early:?}");
            assert_eq!(selected, Ok(Ok(true)));
            assert!(fixture.binding().await.unwrap().as_ref() == Some(&before),
                "the exact canonical native source association must remain unchanged");
            assert_eq!(fixture.inputs().await.unwrap(), inputs);
            assert_eq!(fixture.model_requests(), requests);
        }).catch_unwind().await;
        let cleanup = fixture.shutdown().await;
        assert!(cleanup.is_ok(), "{cleanup:?}");
        if let Err(panic) = checked {
            std::panic::resume_unwind(panic);
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn workflow_lookup_preserves_unavailable_for_poisoned_retained_source() {
        let fixture = FirstFixture::for_service_input().await;
        let prepared = AssertUnwindSafe(async {
            let admitted = fixture.submit().await;
            fixture.finish(&admitted).await.unwrap();
            let context = fixture.probe.as_ref().unwrap().captured();
            assert!(matches!(
                fixture.source.workflow_for_dispatch(&context),
                Ok(None)
            ));
            context
        })
        .catch_unwind()
        .await;
        // Join real native cleanup before deliberate test-only poisoning.
        // No clear_poison or poisoned-source authority recovery is used.
        let cleanup = fixture.shutdown().await;
        assert!(cleanup.is_ok(), "{cleanup:?}");
        let context = match prepared {
            Ok(context) => context,
            Err(panic) => std::panic::resume_unwind(panic),
        };
        assert!(matches!(
            fixture.source.workflow_for_dispatch(&context),
            Ok(None)
        ));
        let source = fixture.source.clone();
        let poisoned = std::thread::spawn(move || {
            let _held = source.retained.lock().unwrap();
            panic!("poison retained-source mutex for refusal control");
        })
        .join();
        assert!(poisoned.is_err());
        assert!(fixture.source.retained.is_poisoned());
        assert!(matches!(
            fixture.source.workflow_for_dispatch(&context),
            Err(PrivateOriginalError::Unavailable)
        ));
    }
}
