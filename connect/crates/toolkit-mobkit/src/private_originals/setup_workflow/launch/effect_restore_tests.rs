//! Real persistent native inputs and grants, reopened without their old owners.
//! These are custody controls, not a new MCP dispatch or continuation run.
use super::idle_restore_tests::close;
use super::waiter_restore_tests::{
    PersistentDedicated, all_ids, histories, join_recovery_for_idle_restore,
    selected_profile_then_close,
};
use super::*;
use meerkat_authorization_contracts::work_association::{GrantTarget, WorkAuthorityBasis};

async fn restored_targets(revoke_first: bool) {
    let saved = selected_profile_then_close(true).await;
    let c = PersistentDedicated::reopen(saved.durable.clone()).await;
    let body = AssertUnwindSafe(Box::pin(async {
        join_recovery_for_idle_restore(&c).await;
        let source = &c.f.source;
        let WorkAuthorityBasis::TargetedGrantLineages {
            base: None,
            targets,
        } = &saved.association.candidate().authority_basis
        else {
            panic!("the actual input must retain its selected target basis");
        };
        assert_eq!(
            targets.len(),
            3,
            "Find and two separately reviewed MCP targets"
        );
        let selected = ["restore-read-a", "restore-read-b"].map(|server| GrantTarget::McpServer {
            server: server.into(),
        });
        let mut leaves = Vec::new();
        for target in &selected {
            let id = super::super::effects::child_id(
                &c.lead.original,
                &saved.membership,
                c.f.original_reference(),
                target,
            )
            .unwrap();
            leaves.push(
                source
                    .owner
                    .grants()
                    .issued_reference(&id)
                    .unwrap()
                    .expect("already issued before shutdown"),
            );
        }
        if revoke_first {
            let mut custody = c.f.machine.try_controller_grant_mutation().unwrap();
            source
                .owner
                .grants()
                .revoke(
                    &saved.association.candidate().logical_executor,
                    &leaves[0],
                    &mut custody,
                )
                .unwrap();
        }
        let path = c.f.source_path.parent().unwrap().join("grants.jsonl");
        let grants = std::fs::read(&path).unwrap();
        let workflow = source
            .workflow_store()
            .unwrap()
            .load_setup_workflow(saved.membership.workflow())
            .await
            .unwrap();
        let delivery =
            c.f.store
                .get_effect(&saved.membership.delivery_effect().unwrap())
                .await
                .unwrap()
                .unwrap();
        // Exercise the production canonical recovery consumer, not just the
        // component restoration methods. No new native input is submitted.
        assert_eq!(
            source
                .with_synthetic_admission_diagnostics(
                    source.recover_workflow_interest_source(&saved.row, || true)
                )
                .await
                .unwrap(),
            WorkflowInterestRecovery::Restored { memberships: 1 }
        );
        assert_eq!(c.member.session().unwrap(), c.lead_session);
        let held = source
            .retained
            .lock()
            .unwrap()
            .iter()
            .find(|held| held.reference == c.lead.original)
            .unwrap()
            .clone();
        let attached = held.setup_workflow.get().unwrap();
        let waiters = attached.waiters.lock().unwrap().clone();
        assert_eq!(waiters.len(), 2);
        let publication = source.owner.publication();
        let (_, replay) = publication.observe(|| ()).unwrap();
        assert_eq!(
            source
                .with_synthetic_admission_diagnostics(
                    source.recover_workflow_interest_source(&saved.row, || true)
                )
                .await
                .unwrap(),
            WorkflowInterestRecovery::Restored { memberships: 1 }
        );
        replay
            .check_current()
            .expect("exact replay does not publish replacement relationships");
        {
            let again = attached.waiters.lock().unwrap();
            assert_eq!(again.len(), 2);
            assert!(
                again
                    .iter()
                    .zip(&waiters)
                    .all(|(left, right)| Arc::ptr_eq(left, right))
            );
        }
        for (index, target) in selected.iter().enumerate() {
            let waiter = waiters
                .iter()
                .find(|waiter| waiter.authority.target() == target)
                .unwrap();
            assert!(waiter.membership == saved.membership);
            let args = serde_json::value::RawValue::from_string("{}".into()).unwrap();
            let call_id = format!("cold-target-{index}");
            let call = ToolCallView {
                id: &call_id,
                name: "read",
                args: &args,
            };
            let staged = waiter.stage_call(call);
            if revoke_first && index == 0 {
                assert!(
                    staged.is_err(),
                    "historical restore cannot revive revoked target permission"
                );
            } else {
                staged.unwrap().settle_unentered().unwrap();
            }
            assert_eq!(
                source
                    .owner
                    .grants()
                    .issued_reference(&leaves[index].grant_id)
                    .unwrap(),
                Some(leaves[index].clone())
            );
        }
        assert_eq!(
            std::fs::read(path).unwrap(),
            grants,
            "restore and operation checks cannot issue or replace grants"
        );
        assert_eq!(
            c.f.store.get(&saved.row.key).await.unwrap().unwrap(),
            saved.row
        );
        assert_eq!(
            source
                .workflow_store()
                .unwrap()
                .load_setup_workflow(saved.membership.workflow())
                .await
                .unwrap()
                .revision(),
            workflow.revision()
        );
        assert_eq!(
            c.f.store.get_effect(&delivery.key).await.unwrap().unwrap(),
            delivery
        );
        assert_eq!(all_ids(&c).await, saved.ids);
        assert_eq!(histories(&c).await, saved.histories);
        assert_eq!(c.f.model_requests(), 0);
    }))
    .catch_unwind()
    .await;
    close(&c, body).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cold_effect_waiter_recovers_exact_issued_targets_without_new_work() {
    restored_targets(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cold_effect_waiter_restores_revoked_reference_but_only_live_target_can_enter() {
    restored_targets(true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cold_effect_waiter_departure_wins_before_atomic_attachment() {
    let saved = selected_profile_then_close(true).await;
    let c = PersistentDedicated::reopen(saved.durable.clone()).await;
    let body = AssertUnwindSafe(Box::pin(async {
        join_recovery_for_idle_restore(&c).await;
        let source = &c.f.source;
        source
            .with_synthetic_admission_diagnostics(
                c.member.restore_waiter_source(c.f.original_reference()),
            )
            .await
            .unwrap();
        c.member.restore_admitted_source().await.unwrap();
        let pause = Arc::new((tokio::sync::Notify::new(), tokio::sync::Notify::new()));
        let pending = super::super::restore::AFTER_MEMBERSHIP_READ.scope(
            pause.clone(),
            c.member
                .restore_waiter_interest(c.f.original_reference(), &saved.membership),
        );
        tokio::pin!(pending);
        tokio::select! {
            _ = pause.0.notified() => (),
            _ = &mut pending => panic!("actual canonical reads must be reached before publication"),
        }
        let plan = source
            .retained
            .lock()
            .unwrap()
            .iter()
            .find(|row| row.reference == c.parent.original)
            .unwrap()
            .workflow_plans
            .lock()
            .unwrap()[0]
            .clone();
        assert!(plan.interests.lock().unwrap().is_empty());
        let departed = plan.depart_waiter(&saved.membership).await.unwrap();
        assert!(departed.departed(&saved.membership).unwrap());
        pause.1.notify_one();
        assert!(matches!(
            pending.await,
            Err(PrivateOriginalError::Unavailable)
        ));
        let held = source
            .retained
            .lock()
            .unwrap()
            .iter()
            .find(|row| row.reference == c.lead.original)
            .unwrap()
            .clone();
        assert!(
            held.setup_workflow.get().is_none(),
            "neither controller gate nor effect relationships can publish after departure"
        );
        assert!(c.member.session().is_err());
        {
            let interests = plan.interests.lock().unwrap();
            assert_eq!(interests.len(), 1);
            assert!(interests[0].closed && interests[0].effects.is_none());
        }
        assert_eq!(
            source
                .recover_workflow_interest_source(&saved.row, || true)
                .await
                .unwrap(),
            WorkflowInterestRecovery::NotSelected
        );
        source
            .record_operation_cancelled(c.f.original_reference())
            .await
            .unwrap();
        assert_eq!(
            source
                .recover_workflow_interest_source(&saved.row, || true)
                .await,
            Err(PrivateOriginalError::Denied)
        );
        assert_eq!(all_ids(&c).await, saved.ids);
        assert_eq!(histories(&c).await, saved.histories);
        assert_eq!(c.f.model_requests(), 0);
    }))
    .catch_unwind()
    .await;
    close(&c, body).await;
}

async fn prepared_effect_launch(
    c: &PersistentDedicated,
    label: &str,
) -> (
    Arc<InstalledWorkflowPlanning>,
    SetupWorkflowSnapshot,
    crate::private_originals::acquisition::AcquisitionOriginal,
) {
    c.member
        .prepare_effect_waiter_source(c.f.original_reference())
        .await
        .unwrap();
    c.f.source
        .prepare_workflow_parent_source(&c.parent.original)
        .await
        .unwrap();
    let p =
        c.f.source
            .submit_authenticated_original(c.parent.clone())
            .await
            .unwrap();
    super::idle_restore_tests::finish(c, "planning_parent", &p).await;
    let probe = c.f.probe.as_ref().unwrap();
    probe.arm_next_input();
    let a = c.f.submit().await;
    c.f.finish(&a).await.unwrap();
    let dispatch = probe.captured();
    let original = c.f.source.acquisition_original(&dispatch).await.unwrap();
    let planning =
        c.f.source
            .prepare_workflow_planning(
                &c.parent.original,
                c.lead.clone(),
                c.terms.clone(),
                c.lineage.clone(),
            )
            .await
            .unwrap();
    let (_, snapshot) = join_waiter(
        &c.f, &planning, &original, &dispatch, &c.backend, &c.work, label,
    )
    .await;
    (planning, snapshot, original)
}

// Separate from the controller-only helper used by the existing controls.
async fn prepared_operational_public_effect_launch(
    c: &PersistentDedicated,
    label: &str,
) -> (
    Arc<InstalledWorkflowPlanning>,
    SetupWorkflowSnapshot,
    crate::private_originals::acquisition::AcquisitionOriginal,
) {
    c.member
        .prepare_effect_waiter_source(c.f.original_reference())
        .await
        .unwrap();
    use crate::installed_host::operator_setup::{
        WorkflowHelperAuthorityRequest, WorkflowParentAuthoritySelection,
        workflow_operation_envelope,
    };
    use meerkat_toolkit::host_lifetime::policy::CompiledMemberClassInstance;
    // G names P as a contributor. P must carry this actual finite operation
    // ceiling before its first native input, even though its own run is only
    // a controller run. No bound controller-only original is retrofitted.
    let native =
        c.f.lifetime()
            .selected_policy()
            .native_host
            .as_ref()
            .unwrap();
    let member = native
        .preparation()
        .members
        .into_iter()
        .find(|member| member.member.member == "planning_lead")
        .unwrap();
    assert_eq!(member.member, c.terms.lead);
    let instance = CompiledMemberClassInstance {
        class: id("operational-parent-class"),
        member,
    };
    let envelope = workflow_operation_envelope(native, &instance, 0, c.terms.deadline_ms).unwrap();
    let selected = WorkflowParentAuthoritySelection {
        commission_sha256: digest(b"case20 selected public workflow parent"),
        instance,
        authority: WorkflowHelperAuthorityRequest {
            parent_issuance_id: EvidenceId::new("case20-public-parent-root").unwrap(),
            leaf_issuance_id: EvidenceId::new("case20-public-parent-leaf").unwrap(),
            envelope,
        },
    };
    assert!(
        c.f.source
            .binding_evidence(&c.parent.original)
            .await
            .unwrap()
            .is_none()
    );
    c.f.source
        .service_operator_setup(c.f.lifetime().clone())
        .unwrap()
        .configure_workflow_parent(&c.f.source, &c.parent.original, &selected)
        .await
        .unwrap();
    let p =
        c.f.source
            .submit_authenticated_original(c.parent.clone())
            .await
            .unwrap();
    super::idle_restore_tests::finish(c, "planning_parent", &p).await;
    let parent =
        c.f.source
            .binding_evidence(&c.parent.original)
            .await
            .unwrap()
            .unwrap();
    let WorkAuthorityBasis::GrantLineage { lineage } = &parent.candidate().authority_basis else {
        panic!("the actual admitted P must retain its issued operation lineage");
    };
    assert_eq!(lineage.len(), 2);
    assert_eq!(lineage[0].grant_id, selected.authority.parent_issuance_id);
    assert_eq!(lineage[1].grant_id, selected.authority.leaf_issuance_id);
    let probe = c.f.probe.as_ref().unwrap();
    probe.arm_next_input();
    let a = c.f.submit().await;
    c.f.finish(&a).await.unwrap();
    let dispatch = probe.captured();
    let original = c.f.source.acquisition_original(&dispatch).await.unwrap();
    let planning =
        c.f.source
            .prepare_workflow_planning(
                &c.parent.original,
                c.lead.clone(),
                c.terms.clone(),
                c.lineage.clone(),
            )
            .await
            .unwrap();
    let (_, snapshot) = join_waiter(
        &c.f, &planning, &original, &dispatch, &c.backend, &c.work, label,
    )
    .await;
    (planning, snapshot, original)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn workflow_public_service_projects_exact_waiter_origin_and_call_bounds() {
    use crate::installed_host::{
        acquisition::ConfiguredHelperReportCapability, application_operations,
    };
    use meerkat_authorization::policy::LocalPolicyPurpose;
    use meerkat_core::authorization::{
        AuthorizationOperation, OperationAuthorizationFacts, PreparedAuthorizationBinding,
        ToolAuthorizationFacts, ToolAuthorizationTarget,
    };
    struct PublicTool(Arc<std::sync::atomic::AtomicUsize>);
    #[async_trait::async_trait]
    impl meerkat_core::AgentToolDispatcher for PublicTool {
        fn tools(&self) -> Arc<[Arc<meerkat_core::ToolDef>]> {
            vec![Arc::new(meerkat_core::ToolDef {
                name: meerkat_toolkit::setup::SETUP_PUBLIC_SERVICE_REQUEST.into(),
                description: "inert registered policy witness".into(),
                input_schema: serde_json::json!({"type":"object"}),
                provenance: None,
                audience: meerkat_core::ToolAudience::Model,
            })]
            .into()
        }
        async fn dispatch(
            &self,
            call: ToolCallView<'_>,
        ) -> Result<meerkat_core::ops::ToolDispatchOutcome, meerkat_core::ToolError> {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Err(meerkat_core::ToolError::unavailable(
                call.name,
                meerkat_core::ToolUnavailableReason::ExecutionOwnerChanged,
            ))
        }
    }
    let c = PersistentDedicated::persistent_public_waiter().await;
    let body = AssertUnwindSafe(Box::pin(async {
        let (planning, snapshot, _) = prepared_effect_launch(&c, "public-origin-selection").await;
        let g = native(planning.launch(&snapshot).await.unwrap());
        super::idle_restore_tests::finish(&c, "planning_lead", &g).await;
        let source = &c.f.source;
        let held = source
            .retained
            .lock()
            .unwrap()
            .iter()
            .find(|held| held.reference == c.lead.original)
            .unwrap()
            .clone();
        let association = held.original.association();
        assert!(
            source.helper_policy_source(association).unwrap().is_none(),
            "actual G is a workflow source, not an ordinary setup helper"
        );
        let workflow = held.setup_workflow.get().unwrap();
        let waiter = workflow
            .waiters
            .lock()
            .unwrap()
            .iter()
            .find(|waiter| {
                waiter.authority.target() == &ConfiguredHelperReportCapability::public_target()
            })
            .unwrap()
            .clone();
        assert!(waiter.original == *c.f.original_reference());
        let before_ids = all_ids(&c).await;
        let before_histories = histories(&c).await;
        let before_requests = c.f.model_requests();
        let grants_path = c.f.source_path.parent().unwrap().join("grants.jsonl");
        let before_grants = std::fs::read(&grants_path).unwrap();
        let before_a = source
            .store
            .get(&source.key(c.f.original_reference()).unwrap())
            .await
            .unwrap()
            .unwrap();
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let registered = Arc::new(meerkat_core::RegisteredToolBundleDispatcher::new(
            meerkat_toolkit::setup::SETUP_ACCOUNTS_BUNDLE,
            Arc::new(PublicTool(calls.clone())),
        ));
        let dispatcher: Arc<dyn meerkat_core::AgentToolDispatcher> =
            Arc::new(meerkat_core::DynamicToolComposite::new(vec![registered]));
        let name = meerkat_toolkit::setup::SETUP_PUBLIC_SERVICE_REQUEST;
        let args: Arc<serde_json::value::RawValue> = serde_json::value::RawValue::from_string(
            serde_json::json!({"setup_acquisition": waiter.membership.acquisition().id})
                .to_string(),
        )
        .unwrap()
        .into();
        let view = ToolCallView {
            id: "workflow-public-request",
            name,
            args: &args,
        };
        let plan = meerkat_core::resolve_tool_execution_plan_fenced(
            &dispatcher,
            view,
            &ToolDispatchContext::default(),
            &meerkat_core::ToolExecutionResolutionContext::new(
                meerkat_core::ToolDeadlineChain::new(vec![
                    meerkat_core::ToolDeadlineContributor::finite(
                        meerkat_core::ToolDeadlineOwner::CoreToolDispatch,
                        Duration::from_secs(60),
                    ),
                ])
                .unwrap(),
            ),
        )
        .unwrap();
        let facts = OperationAuthorizationFacts {
            operation_id: serde_json::from_str("\"00000000-0000-0000-0000-000000000007\"").unwrap(),
            // This binding is a policy projection, not a fabricated native input
            // context or an authorized effect. P, A and G above are actual inputs.
            execution_scope: OperationExecutionScope::Domain,
            run_id: None,
            context_revision: None,
            operation: AuthorizationOperation::Tool(ToolAuthorizationFacts {
                call_id: Arc::from(view.id),
                name: name.into(),
                arguments: args.clone(),
                target: ToolAuthorizationTarget::Dispatcher(Arc::new(plan)),
            }),
        };
        let binding = PreparedAuthorizationBinding::new(facts.clone());
        let now = meerkat_authorization::clock::LocalAuthorizationClock::now(
            &meerkat_authorization::clock::HostAuthorizationClock,
        )
        .unwrap()
        .unix_ms;
        let project = |binding: &PreparedAuthorizationBinding| {
            c.operation_owner.authorize_operation(
                association,
                binding,
                LocalPolicyPurpose::Operation,
                now,
            )
        };
        assert!(
            project(&binding).is_err(),
            "unstaged public requests cannot use a singleton fallback"
        );
        let entry = waiter.stage_call(view).unwrap();
        let allowance = project(&binding).unwrap();
        let (selected, origin) = source
            .workflow_call_for_policy(association, &binding)
            .unwrap()
            .unwrap();
        assert!(Arc::ptr_eq(&entry, &selected));
        assert!(origin == source.setup_origin_for(c.f.original_reference()).unwrap());
        assert!(origin != source.setup_origin_for(&c.lead.original).unwrap());
        let lifetime = source
            .host_lifetime
            .get()
            .and_then(std::sync::Weak::upgrade)
            .unwrap();
        let policy = lifetime.selected_policy();
        let native = policy.native_host.as_ref().unwrap();
        let claims = association.candidate();
        let expected = application_operations::setup_public_service_request_for_origin(
            &native.grant_administrator,
            &claims.logical_executor,
            &claims.target.audience,
            &origin,
        )
        .unwrap();
        assert_eq!(allowance.operation_values.len(), 1);
        let actual = &allowance.operation_values[0];
        assert_eq!(actual.action, expected.action);
        assert_eq!(actual.resource_domain, expected.resource_domain);
        assert_eq!(actual.processor, expected.processor);
        assert_eq!(actual.audience, expected.audience);
        let delegated = entry.current().unwrap();
        assert_ne!(
            delegated.restrictions(),
            &ExecutionRestrictions::unrestricted()
        );
        assert_eq!(&allowance.restrictions, delegated.restrictions());
        // This G has no setup-helper deadline. Its ordinary policy window is
        // separate from the resolved CoreToolDispatch timeout above; the actual
        // delegated grant and workflow terms must still narrow that window.
        let ordinary_deadline = now.saturating_add(30 * 60 * 1_000);
        let retained_deadline = delegated.expires_at_ms().min(c.terms.deadline_ms);
        assert!(
            retained_deadline < ordinary_deadline,
            "this fixture must exercise the shorter grant/workflow bounds"
        );
        assert_eq!(
            allowance.expires_at_ms,
            ordinary_deadline.min(retained_deadline)
        );
        assert!(
            project(&PreparedAuthorizationBinding::new(facts.clone())).is_err(),
            "equal facts cannot replace the exact prepared binding"
        );
        let mut changed = facts;
        let AuthorizationOperation::Tool(tool) = &mut changed.operation else {
            unreachable!()
        };
        tool.arguments = serde_json::value::RawValue::from_string("{}".into())
            .unwrap()
            .into();
        assert!(
            project(&PreparedAuthorizationBinding::new(changed)).is_err(),
            "changed arguments cannot reuse the staged call"
        );
        entry.settle_unentered().unwrap();
        assert!(
            project(&binding).is_err(),
            "settled call custody cannot rearm"
        );
        assert!(
            entry.current_relationship().is_ok(),
            "call settlement leaves the live A/G relationship intact"
        );
        source
            .record_operation_cancelled(&c.lead.original)
            .await
            .unwrap();
        assert!(
            entry.current_relationship().is_err(),
            "actual G cancellation retires that relationship"
        );
        assert!(project(&binding).is_err());
        assert!(
            waiter
                .stage_call(ToolCallView {
                    id: "cancelled-public-request",
                    name,
                    args: &args
                })
                .is_err(),
            "cancelled G cannot stage a fresh request for retained A"
        );
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert_eq!(std::fs::read(&grants_path).unwrap(), before_grants);
        assert_eq!(
            source.store.get(&before_a.key).await.unwrap().unwrap(),
            before_a
        );
        assert_eq!(all_ids(&c).await, before_ids);
        assert_eq!(histories(&c).await, before_histories);
        assert_eq!(c.f.model_requests(), before_requests);
    }))
    .catch_unwind()
    .await;
    close(&c, body).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn workflow_service_helper_uses_current_g_without_borrowing_fixed_metadata() {
    use crate::private_originals::setup_workflow::execution::{
        WorkflowTaskCustody, fixed_service_helper_matches,
    };
    let c = PersistentDedicated::persistent_effect_waiter().await;
    let body = AssertUnwindSafe(Box::pin(async {
        let (planning, snapshot, _) = prepared_effect_launch(&c, "service-helper-selection").await;
        let g = native(planning.launch(&snapshot).await.unwrap());
        super::idle_restore_tests::finish(&c, "planning_lead", &g).await;
        let source = &c.f.source;
        let held = source
            .retained
            .lock()
            .unwrap()
            .iter()
            .find(|held| held.reference == c.lead.original)
            .unwrap()
            .clone();
        let workflow = held.setup_workflow.get().unwrap();
        let waiter = workflow.waiters.lock().unwrap().first().unwrap().clone();
        let args = serde_json::value::RawValue::from_string("{}".into()).unwrap();
        let call = ToolCallView {
            id: "service-helper-read",
            name: "read",
            args: &args,
        };
        let entry = waiter.stage_call(call).unwrap();
        let custody = WorkflowTaskCustody {
            waiter,
            entry: entry.clone(),
        };
        let (task_snapshot, captured) = source
            .workflow_service_task_snapshot(&custody, &c.backend)
            .await
            .unwrap();
        assert_eq!(captured, c.terms.lead);
        let waiting = task_snapshot
            .request()
            .host()
            .continuation
            .native_original
            .as_ref()
            .unwrap();
        assert_eq!(waiting.origin_session_id, c.f.session);
        assert!(
            waiting.member.is_none(),
            "this waiter fixture deliberately omits member metadata"
        );
        let (_, parent, _) = source
            .selected_source_route(&c.parent.original.route)
            .unwrap();
        let parent_binding = source
            .binding_evidence(&c.parent.original)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            parent_binding.candidate().target.context.as_str(),
            c.parent_session.to_string()
        );
        assert_eq!(
            parent_binding.candidate().logical_executor,
            parent.logical_owner
        );
        let fixed = &parent.member;
        assert_ne!(
            fixed, &captured,
            "the actual selected P and current G are distinct members"
        );
        let before_ids = all_ids(&c).await;
        let before_histories = histories(&c).await;
        let before_requests = c.f.model_requests();
        let before_row = c.f.store.get(task_snapshot.key()).await.unwrap().unwrap();
        let grants_path = c.f.source_path.parent().unwrap().join("grants.jsonl");
        let before_grants = std::fs::read(&grants_path).unwrap();
        // P supplies only a distinct member-correlation input, not A's member
        // metadata or a selected helper grant. G uses this exact live custody.
        assert_eq!(
            fixed_service_helper_matches(fixed, &captured, None),
            Err(PrivateOriginalError::Denied)
        );
        assert_eq!(
            fixed_service_helper_matches(fixed, &captured, Some(&custody)),
            Ok(false),
            "current G may prepare the task, but cannot borrow fixed-helper warm metadata"
        );
        assert_eq!(
            fixed_service_helper_matches(&captured, &captured, Some(&custody)),
            Ok(true),
            "a genuinely matching static workflow helper keeps optional metadata eligibility"
        );
        assert_eq!(fixed_service_helper_matches(fixed, fixed, None), Ok(true));
        assert_eq!(
            fixed_service_helper_matches(fixed, fixed, Some(&custody)),
            Err(PrivateOriginalError::Denied),
            "workflow custody cannot authenticate a different captured member"
        );
        entry.settle_unentered().unwrap();
        assert_eq!(
            fixed_service_helper_matches(fixed, &captured, Some(&custody)),
            Ok(false),
            "settling this call does not retire the current workflow relationship"
        );
        assert_eq!(std::fs::read(&grants_path).unwrap(), before_grants);
        source
            .record_operation_cancelled(&c.lead.original)
            .await
            .unwrap();
        assert!(
            fixed_service_helper_matches(fixed, &captured, Some(&custody)).is_err(),
            "a cancelled actual G cannot reuse an earlier helper correlation"
        );
        assert!(
            source
                .workflow_service_task_snapshot(&custody, &c.backend)
                .await
                .is_err()
        );
        assert_eq!(
            std::fs::read(&grants_path).unwrap(),
            before_grants,
            "helper correlation and source cancellation cannot issue replacement permission"
        );
        assert_eq!(
            c.f.store.get(task_snapshot.key()).await.unwrap().unwrap(),
            before_row
        );
        assert_eq!(all_ids(&c).await, before_ids);
        assert_eq!(histories(&c).await, before_histories);
        assert_eq!(c.f.model_requests(), before_requests);
    }))
    .catch_unwind()
    .await;
    close(&c, body).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn effect_bind_refusal_retains_owner_until_durable_withdrawal() {
    use crate::original_ingress::custody_attempt::CustodyDecision;
    let c = PersistentDedicated::persistent_effect_waiter().await;
    let body = AssertUnwindSafe(Box::pin(async {
        let (planning, snapshot, original) =
            prepared_effect_launch(&c, "effect-bind-refusal").await;
        let before_ids = all_ids(&c).await;
        let before_histories = histories(&c).await;
        let before_requests = c.f.model_requests();
        // A's real source owner withdraws its admitted custody after effect
        // preparation. The G binder must recheck it before native entry.
        c.f.source
            .owner
            .release_original(&original.original)
            .unwrap();
        assert!(matches!(
            planning.launch(&snapshot).await.unwrap(),
            WorkflowLaunchObservation::NotAdmitted
        ));
        let held =
            c.f.source
                .retained
                .lock()
                .unwrap()
                .iter()
                .find(|held| held.reference == c.lead.original)
                .unwrap()
                .clone();
        assert!(
            held.setup_workflow.get().is_some(),
            "the actual G binder selected its holder"
        );
        let pending = held
            .first_custody
            .lock()
            .unwrap()
            .clone()
            .expect("the persisted first attempt keeps its actual recovery owner");
        assert!(held.unresolved.lock().unwrap().is_empty());
        assert!(
            held.setup_workflow
                .get()
                .unwrap()
                .waiters
                .lock()
                .unwrap()
                .is_empty()
        );
        assert!(
            matches!(
                planning.launch(&snapshot).await.unwrap(),
                WorkflowLaunchObservation::NotAdmitted
            ),
            "definitive refusal is observed without retry"
        );
        let key = c.f.source.key(&c.lead.original).unwrap();
        let before_close = c.f.store.get(&key).await.unwrap().unwrap();
        assert!(
            c.f.source
                .decode_record(&before_close, &c.lead.original)
                .unwrap()
                .custody_attempt
                .is_some(),
            "the canonical attempt exists before cleanup"
        );
        // DedicatedFixture already starts this owner through the actual
        // gateway.bind_identity_runtime path. Join that same owner, not a
        // replacement worker or a local candidate-drop approximation.
        c.f.source.stop_custody_recovery();
        tokio::time::timeout(GUARD, c.f.source.join_custody_recovery())
            .await
            .expect("the same actual recovery owner must join")
            .unwrap();
        let closed = c.f.store.get(&key).await.unwrap().unwrap();
        let record = c.f.source.decode_record(&closed, &c.lead.original).unwrap();
        assert!(
            record.custody_attempt.as_ref().unwrap().decision() == CustodyDecision::Withdrawn,
            "joined cleanup must settle the canonical attempt, not merely drop live custody"
        );
        assert!(matches!(
            &*pending.pending.lock().unwrap(),
            crate::private_originals::governed_submit::SourcePublication::Settled {
                source_acknowledged: false,
                result: Err(_),
            }
        ));
        assert!(
            c.f.source
                .owner
                .workflow_own_controller(&held.original)
                .is_err()
        );
        assert!(
            !c.f.source
                .owner
                .has_admitted_source(held.original.association())
                .unwrap()
        );
        assert_eq!(all_ids(&c).await, before_ids);
        assert_eq!(histories(&c).await, before_histories);
        assert_eq!(c.f.model_requests(), before_requests);
    }))
    .catch_unwind()
    .await;
    close(&c, body).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn effect_staging_refuses_publication_contention_without_replacing_exact_replay() {
    let c = PersistentDedicated::persistent_effect_waiter().await;
    let body = AssertUnwindSafe(Box::pin(async {
        let (planning, snapshot, _) =
            prepared_effect_launch(&c, "effect-publication-contention").await;
        let g = native(planning.launch(&snapshot).await.unwrap());
        super::idle_restore_tests::finish(&c, "planning_lead", &g).await;
        let held =
            c.f.source
                .retained
                .lock()
                .unwrap()
                .iter()
                .find(|held| held.reference == c.lead.original)
                .unwrap()
                .clone();
        let workflow = held.setup_workflow.get().unwrap().clone();
        let before_ids = all_ids(&c).await;
        let before_histories = histories(&c).await;
        let before_requests = c.f.model_requests();
        let publication = c.f.source.owner.publication();
        let change = publication.begin_owner_change().unwrap();
        planning.stage_effects(&c.f.source, &workflow).unwrap();
        // Retain one actual relationship aside under the writer to exercise a
        // partial staging state. Its native lineage and canonical waiter remain
        // unchanged; the other exact relationship must not be replaced.
        let removed = workflow.waiters.lock().unwrap().pop().unwrap();
        let prior = workflow.waiters.lock().unwrap().clone();
        assert_eq!(prior.len(), 1);
        let (sent, received) = std::sync::mpsc::channel();
        let checked_plan = planning.clone();
        let checked_source = c.f.source.clone();
        let checked_workflow = workflow.clone();
        let worker = std::thread::spawn(move || {
            let result = checked_plan.stage_effects(&checked_source, &checked_workflow);
            sent.send(result).unwrap();
        });
        let result = received.recv_timeout(Duration::from_secs(2));
        // Always release and join, including a regression that blocks on the
        // writer. A timeout fails this control; it is not treated as refusal.
        drop(change);
        worker.join().unwrap();
        assert!(matches!(result, Ok(Err(PrivateOriginalError::Unavailable))));
        let after = workflow.waiters.lock().unwrap().clone();
        assert_eq!(after.len(), 1);
        assert!(Arc::ptr_eq(&prior[0], &after[0]));
        planning.stage_effects(&c.f.source, &workflow).unwrap();
        let completed = workflow.waiters.lock().unwrap().clone();
        assert_eq!(completed.len(), 2);
        assert!(
            completed
                .iter()
                .any(|waiter| waiter.membership == removed.membership
                    && waiter.authority.same_selection(&removed.authority))
        );
        assert_eq!(all_ids(&c).await, before_ids);
        assert_eq!(histories(&c).await, before_histories);
        assert_eq!(c.f.model_requests(), before_requests);
    }))
    .catch_unwind()
    .await;
    close(&c, body).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn waiter_source_restore_awaits_existing_writer_before_native_observation() {
    let saved = selected_profile_then_close(true).await;
    let c = PersistentDedicated::reopen(saved.durable.clone()).await;
    let body = AssertUnwindSafe(Box::pin(async {
        join_recovery_for_idle_restore(&c).await;
        let source = &c.f.source;
        let grants_path = c.f.source_path.parent().unwrap().join("grants.jsonl");
        let grants = std::fs::read(&grants_path).unwrap();
        let source_row = c.f.store.get(&saved.row.key).await.unwrap().unwrap();
        assert!(
            !source
                .owner
                .has_admitted_source(&saved.association)
                .unwrap()
        );
        let writer = source.owner.source_writer().await;
        let mut restoring = Box::pin(source.with_synthetic_admission_diagnostics(
            c.member.restore_waiter_source(c.f.original_reference()),
        ));
        assert!(
            futures::poll!(&mut restoring).is_pending(),
            "ordinary async restoration must wait for this actual writer, not refuse contention"
        );
        assert!(
            !source
                .owner
                .has_admitted_source(&saved.association)
                .unwrap(),
            "waiting has not promoted historical custody"
        );
        drop(writer);
        tokio::time::timeout(GUARD, restoring)
            .await
            .expect("the same restoration future must finish after writer release")
            .unwrap();
        assert_eq!(
            c.f.cold_source_custody(&saved.association),
            Some((true, true))
        );
        assert!(
            c.member.session().is_err(),
            "A restoration does not complete G custody"
        );
        assert_eq!(
            c.f.store.get(&saved.row.key).await.unwrap().unwrap(),
            source_row
        );
        assert_eq!(std::fs::read(grants_path).unwrap(), grants);
        assert_eq!(all_ids(&c).await, saved.ids);
        assert_eq!(histories(&c).await, saved.histories);
        assert_eq!(c.f.model_requests(), 0);
    }))
    .catch_unwind()
    .await;
    close(&c, body).await;
}

// The proxy forwards real canonical data and can fail only an exact read.
use std::sync::atomic::AtomicUsize;
use toolkit_store::*;
struct WorkflowReadFault {
    inner: Arc<dyn ToolkitStore>,
    acquisition: StoreKey,
    reads: AtomicUsize,
    fail_at: std::sync::atomic::AtomicUsize,
    conflict: std::sync::atomic::AtomicBool,
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn workflow_task_reads_preserve_unavailability_without_mutating_custody() {
    use crate::private_originals::setup_workflow::execution::WorkflowTaskCustody;
    let c = PersistentDedicated::persistent_effect_waiter().await;
    let body = AssertUnwindSafe(Box::pin(async {
        tokio::time::timeout(GUARD, async {
            let (planning, snapshot, _) = prepared_effect_launch(&c, "availability-reads").await;
            let g = native(planning.launch(&snapshot).await.unwrap());
            super::idle_restore_tests::finish(&c, "planning_lead", &g).await;
            let source = &c.f.source;
            let held = source
                .retained
                .lock()
                .unwrap()
                .iter()
                .find(|held| held.reference == c.lead.original)
                .unwrap()
                .clone();
            let workflow = held.setup_workflow.get().unwrap();
            let waiter = workflow.waiters.lock().unwrap().first().unwrap().clone();
            let args = serde_json::value::RawValue::from_string("{}".into()).unwrap();
            let call = ToolCallView {
                id: "availability-read",
                name: "read",
                args: &args,
            };
            let entry = waiter.stage_call(call).unwrap();
            let custody = WorkflowTaskCustody {
                waiter: waiter.clone(),
                entry: entry.clone(),
            };
            let (expected, member) = source
                .workflow_service_task_snapshot(&custody, &c.backend)
                .await
                .unwrap();
            assert_eq!(member, c.terms.lead);
            let conditions = custody.conditions(source).await.unwrap();
            let before_ids = all_ids(&c).await;
            let before_histories = histories(&c).await;
            let before_requests = c.f.model_requests();
            let before = c.f.store.get(expected.key()).await.unwrap();
            let effects =
                c.f.store
                    .list_effects(expected.key(), None, 32)
                    .await
                    .unwrap();
            let workflow_before = c.f.store.get(snapshot.reference().key()).await.unwrap();
            let grants_path = c.f.source_path.parent().unwrap().join("grants.jsonl");
            let grants = std::fs::read(&grants_path).unwrap();
            let reads = WorkflowReadFault::new(c.f.store.clone(), expected.key().clone());
            let read_store = || {
                SetupStore::new(
                    reads.as_ref(),
                    SetupScope::new(id(&source.scope.realm), id(&source.scope.owner)),
                )
                .map_err(|_| PrivateOriginalError::Unavailable)
            };
            // First acquisition get is the conditions read. Second is the later
            // service snapshot, reached only after conditions passed unchanged.
            for fail_at in [1, 2] {
                reads.reads.store(0, Ordering::SeqCst);
                reads.fail_at.store(fail_at, Ordering::SeqCst);
                assert!(matches!(
                    source
                        .workflow_service_task_snapshot_from(&custody, &c.backend, &read_store)
                        .await,
                    Err(PrivateOriginalError::Unavailable)
                ));
                assert_eq!(reads.reads.load(Ordering::SeqCst), fail_at);
                assert!(Arc::ptr_eq(&entry, &waiter.stage_call(call).unwrap()));
                entry.current_relationship().unwrap();
            }
            reads.reads.store(0, Ordering::SeqCst);
            reads.fail_at.store(1, Ordering::SeqCst);
            reads.conflict.store(true, Ordering::SeqCst);
            assert!(matches!(
                custody.conditions_from(source, &read_store).await,
                Err(PrivateOriginalError::Conflict)
            ));
            assert_eq!(reads.reads.load(Ordering::SeqCst), 1);
            reads.conflict.store(false, Ordering::SeqCst);
            reads.fail_at.store(0, Ordering::SeqCst);
            let (again, again_member) = source
                .workflow_service_task_snapshot_from(&custody, &c.backend, &read_store)
                .await
                .unwrap();
            assert_eq!(again.key(), expected.key());
            assert_eq!(
                again.request().input_digest(),
                expected.request().input_digest()
            );
            assert_eq!(again_member, member);
            assert_eq!(
                custody.conditions_from(source, &read_store).await.unwrap(),
                conditions
            );
            assert_eq!(c.f.store.get(expected.key()).await.unwrap(), before);
            assert_eq!(
                c.f.store
                    .list_effects(expected.key(), None, 32)
                    .await
                    .unwrap(),
                effects
            );
            assert_eq!(
                c.f.store.get(snapshot.reference().key()).await.unwrap(),
                workflow_before
            );
            assert_eq!(std::fs::read(&grants_path).unwrap(), grants);
            assert_eq!(all_ids(&c).await, before_ids);
            assert_eq!(histories(&c).await, before_histories);
            assert_eq!(c.f.model_requests(), before_requests);
            entry.settle_unentered().unwrap();
            let replay = waiter.stage_call(call).unwrap();
            assert!(Arc::ptr_eq(&entry, &replay));
            assert!(
                replay.current().is_err(),
                "the explicit settlement cannot rearm this call"
            );
            source
                .record_operation_cancelled(&c.lead.original)
                .await
                .unwrap();
            assert!(matches!(
                source
                    .workflow_service_task_snapshot(&custody, &c.backend)
                    .await,
                Err(PrivateOriginalError::Denied)
            ));
        })
        .await
        .expect("bounded workflow read control");
    }))
    .catch_unwind()
    .await;
    close(&c, body).await;
}

// This ingress is only a failure injector. Success still reaches the actual
// native prepared work check; it supplies no grant or workflow identity.
struct WorkflowIngressFault(std::sync::Mutex<Option<meerkat_core::OperationAuthorizationError>>);
impl meerkat_core::ToolApplicationIngress for WorkflowIngressFault {
    fn revalidate(&self) -> Result<(), meerkat_core::OperationAuthorizationError> {
        match *self.0.lock().unwrap() {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
    fn as_any(&self) -> &(dyn std::any::Any + Send + Sync) {
        self
    }
}

struct WorkflowDispatchProbe(Arc<AtomicUsize>, Arc<std::sync::atomic::AtomicBool>);
#[async_trait::async_trait]
impl meerkat_core::AgentToolDispatcher for WorkflowDispatchProbe {
    fn tools(&self) -> Arc<[Arc<meerkat_core::ToolDef>]> {
        vec![Arc::new(meerkat_core::ToolDef::new(
            meerkat_toolkit::setup::SETUP_PUBLIC_SERVICE_REQUEST,
            "Inert exact workflow dispatch observer",
            serde_json::json!({"type":"object"}),
        ))]
        .into()
    }
    async fn dispatch(
        &self,
        call: ToolCallView<'_>,
    ) -> Result<meerkat_core::ops::ToolDispatchOutcome, meerkat_core::ToolError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        if self.1.load(Ordering::SeqCst) {
            return Err(meerkat_core::ToolError::unavailable(
                call.name,
                meerkat_core::ToolUnavailableReason::TemporarilyUnavailable,
            ));
        }
        Ok(meerkat_core::ToolResult::new(call.id.into(), "observed".into(), false).into())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn workflow_dispatch_preserves_native_unavailability_before_entry() {
    use crate::installed_host::acquisition::{AcquisitionBootstrap, InstalledServiceTaskSource};
    use meerkat_core::authorization::{
        AuthorizationOperation, OperationAuthorizationFacts, OperationObservationError,
        OperationRefusalKind, OperationRefused, PreparedAuthorizationBinding,
        PreparedOperationCheck, ToolAuthorizationFacts, ToolAuthorizationTarget,
    };
    use meerkat_core::{
        AgentToolDispatcher, OperationAuthorizationError, ToolApplicationControlRequest,
        ToolApplicationOperation, ToolApplicationRequest, ToolError,
    };
    let prepare_control =
        |stage: &str,
         work,
         binding: PreparedAuthorizationBinding,
         diagnose: &dyn Fn(&PreparedAuthorizationBinding)| {
            // Keep the refusal intact while exposing its safe typed disposition.
            // OperationRefused's Debug output deliberately redacts the diagnosis.
            let result = PreparedOperationCheck::prepare(work, binding.clone());
            if result.is_err() {
                // This later read is diagnostic only, never a replacement decision.
                // Preserve the native refusal even if the diagnostic itself panics.
                let completed =
                    std::panic::catch_unwind(AssertUnwindSafe(|| diagnose(&binding))).is_ok();
                eprintln!("workflow preparation diagnostic: completed={completed}");
            }
            match result {
                Ok(prepared) => prepared,
                Err(OperationAuthorizationError::Refused(refusal)) => {
                    panic!(
                        "{stage}: native preparation refused: kind={:?}, dispatch={:?}",
                        refusal.kind(),
                        refusal.dispatch()
                    )
                }
                Err(OperationAuthorizationError::Unavailable) => {
                    panic!("{stage}: native preparation authorization unavailable")
                }
                Err(OperationAuthorizationError::ObservationUnavailable(_)) => {
                    panic!("{stage}: native preparation observation unavailable")
                }
            }
        };
    let c = PersistentDedicated::persistent_operational_public_waiter().await;
    let body = AssertUnwindSafe(Box::pin(async {
        tokio::time::timeout(GUARD, async {
        let (planning, snapshot, _) = prepared_operational_public_effect_launch(&c, "availability-dispatch").await;
        c.f.probe.as_ref().unwrap().arm_next_input();
        // G first emits the real context probe. Hold its next authorized
        // response so the captured native run remains live during dispatch.
        let hold = c.f.hold_response_after(1);
        let g = native(planning.launch(&snapshot).await.unwrap());
        hold.entered().await;
        let captured = c.f.probe.as_ref().unwrap().captured();
        assert_eq!(captured.origin_session_id(), Some(&c.lead_session));
        let source = &c.f.source;
        let held = source.retained.lock().unwrap().iter()
            .find(|held| held.reference == c.lead.original).unwrap().clone();
        let parent = source.binding_evidence(&c.parent.original).await.unwrap().unwrap();
        assert_eq!(held.original.association().candidate().contributing_work,
            vec![parent.candidate().original_work.clone()], "G retains its actual P contributor");
        let workflow = held.setup_workflow.get().unwrap();
        let waiter = workflow.waiters.lock().unwrap().iter().find(|waiter|
            waiter.authority.target() == &crate::installed_host::acquisition::ConfiguredHelperReportCapability::public_target())
            .unwrap().clone();
        // Select the existing installed acquisition owner over this real source.
        // Its fixture continuation/helper defaults are never entered here.
        let acquisition = crate::installed_host::controller_first_input_fixture::bind_workflow_recovery(
            &c.f.store, &c.f.scope, source, c.f.lifetime(), &c.f.source_path);
        let native = c.f.lifetime().selected_policy().native_host.as_ref().unwrap();
        let helper = acquisition.helper().0;
        assert_eq!(&helper.member, native.members[0].setup_capability.as_ref().unwrap().helper.as_ref().unwrap(),
            "the fixture retains A's actual configured workflow helper");
        assert_eq!(helper.member.member, "planning_lead");
        assert!(matches!(&helper.operation_basis,
            WorkAuthorityBasis::TargetedGrantLineages { base: None, targets } if targets.is_empty()),
            "fixture construction supplies no helper operation authority");
        let mut bootstrap = AcquisitionBootstrap::selected(acquisition.clone());
        let prepared = bootstrap.prepare(None, None).unwrap().unwrap();
        assert!(Arc::ptr_eq(&prepared, &acquisition),
            "preparation retains the selected acquisition owner");
        drop(prepared);
        drop(acquisition);
        let selected = Arc::new(std::sync::Mutex::new(bootstrap));
        let tasks = InstalledServiceTaskSource::new(&selected);
        let calls = Arc::new(AtomicUsize::new(0));
        let after_entry_failure = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let registered: Arc<dyn AgentToolDispatcher> = Arc::new(meerkat_core::RegisteredToolBundleDispatcher::new(
            meerkat_toolkit::setup::SETUP_ACCOUNTS_BUNDLE, Arc::new(WorkflowDispatchProbe(calls.clone(), after_entry_failure.clone()))));
        let dispatcher = tasks.with_workflow_calls(registered);
        let args: Arc<serde_json::value::RawValue> = serde_json::value::RawValue::from_string(
            serde_json::json!({"setup_acquisition":waiter.membership.acquisition().id}).to_string()).unwrap().into();
        let name = meerkat_toolkit::setup::SETUP_PUBLIC_SERVICE_REQUEST;
        let view = ToolCallView { id: "workflow-availability", name, args: &args };
        let resolution = meerkat_core::ToolExecutionResolutionContext::new(
            meerkat_core::ToolDeadlineChain::new(vec![meerkat_core::ToolDeadlineContributor::finite(
                meerkat_core::ToolDeadlineOwner::CoreToolDispatch, Duration::from_secs(60))]).unwrap());
        let plan = meerkat_core::resolve_tool_execution_plan_fenced(&dispatcher, view, &captured, &resolution).unwrap();
        let work = captured.work_authorization().unwrap().clone();
        let binding = PreparedAuthorizationBinding::new(OperationAuthorizationFacts {
            operation_id: serde_json::from_str("\"00000000-0000-0000-0000-000000000071\"").unwrap(),
            execution_scope: work.execution_scope().clone(), run_id: captured.run_id().cloned(), context_revision: None,
            operation: AuthorizationOperation::Tool(ToolAuthorizationFacts {
                call_id: Arc::from(view.id), name: name.into(), arguments: args,
                target: ToolAuthorizationTarget::Dispatcher(Arc::new(plan)),
            }),
        });
        let AuthorizationOperation::Tool(tool) = &binding.facts().operation else { unreachable!() };
        let ToolAuthorizationTarget::Dispatcher(plan) = &tool.target else { unreachable!() };
        let call = tool.call();
        // Real native policy preparation binds this staged exact call. The
        // context uses G's actual admitted scope/run, not Domain or a copied A.
        let diagnose = |binding: &PreparedAuthorizationBinding| {
            use meerkat_authorization_contracts::constraints::{OperationRestrictionValues, RestrictionFailure};
            // Snapshot existing callback observations after preparation returned,
            // before the later passive reads. Release its lock before writing.
            // This drains diagnostics only; it performs no authorization query.
            let observations = source.review_observations().drain();
            observations.write_to(&mut std::io::stderr().lock());
            // Only the real source's existing G/waiter records are read. No
            // preparation, staging, issuance, entry or settlement is repeated.
            let observed = source.owner.publication().observe(|| {
                let target = crate::installed_host::acquisition::ConfiguredHelperReportCapability::public_target();
                let claims = held.original.association().candidate();
                let mut rows = vec![
                    ("scope_matches_captured_g", (binding.facts().execution_scope == *captured.work_authorization().unwrap().execution_scope()).to_string()),
                    ("run_matches_captured_g", (binding.facts().run_id.as_ref() == captured.run_id()).to_string()),
                    ("public_target_matches", (meerkat_authorization::grant_policy::operation_grant_target(binding).ok() == Some(Some(target.clone()))).to_string()),
                    ("g_source_controller_current", source.owner.workflow_own_controller(&held.original).is_ok().to_string()),
                ];
                let now = match source.owner.clock().now() {
                    Ok(now) => now.unix_ms,
                    Err(_) => { rows.push(("clock", "unavailable".into())); return rows; }
                };
                let origin = match source.setup_origin_for(&waiter.original) {
                    Ok(origin) => origin,
                    Err(error) => { rows.push(("waiter_origin", format!("{error:?}"))); return rows; }
                };
                let values = match crate::installed_host::application_operations::setup_public_service_request_for_origin(
                    &native.grant_administrator, &claims.logical_executor, &claims.target.audience, &origin) {
                    Ok(values) => values,
                    Err(_) => { rows.push(("operation_tuple", "unavailable".into())); return rows; }
                };
                let check_bounds = |restrictions: &ExecutionRestrictions| match restrictions.check_bounds(OperationRestrictionValues {
                    action: &values.action, resource_domain: &values.resource_domain,
                    processor: &values.processor, audience: &values.audience, now_ms: now,
                }) {
                    Ok(()) => "matched".to_owned(),
                    Err(RestrictionFailure::OutsideBounds { dimension }) => format!("outside:{dimension:?}"),
                    Err(RestrictionFailure::Unresolved { dimension, .. }) => format!("unresolved:{dimension:?}"),
                };
                rows.push(("g_admitted_ceiling", check_bounds(&claims.admitted_ceiling)));
                match claims.authority_basis.lineage_for(Some(&target)) {
                    None => rows.push(("g_lineage", "not_selected".into())),
                    Some(lineage) => match source.owner.grants().resolve_lineage(lineage,
                        &claims.logical_executor, claims.represented_subject.as_ref()) {
                        Ok(grant) => rows.push(("g_grant_bounds", check_bounds(grant.restrictions()))),
                        Err(error) => rows.push(("g_lineage", format!("{error:?}"))),
                    },
                }
                // The passive path refuses a binding that native preparation
                // never retained. It cannot install that missing association.
                match source.workflow_call_for_review(held.original.association(), binding) {
                    Ok(Some((call, _))) => match call.current() {
                        Ok(grant) => rows.push(("waiter_grant_bounds", check_bounds(grant.restrictions()))),
                        Err(_) => rows.push(("waiter_current", "unavailable_or_refused".into())),
                    },
                    Ok(None) => rows.push(("waiter_binding", "not_retained".into())),
                    Err(error) => rows.push(("waiter_binding", format!("{error:?}"))),
                }
                rows
            });
            match observed {
                Ok((rows, _)) => for (premise, result) in rows {
                    eprintln!("workflow preparation diagnostic: {premise}={result}");
                },
                Err(_) => eprintln!("workflow preparation diagnostic: publication_not_stable"),
            }
        };
        let prepared = prepare_control("pre-entry control", work, binding.clone(), &diagnose);
        let current = captured.clone().__test_with_prepared_authorization(prepared);
        let fault = Arc::new(WorkflowIngressFault(std::sync::Mutex::new(None)));
        let control = ToolApplicationControlRequest::from_trusted_ingress(c.lead_session.clone(),
            ToolApplicationRequest { tool_call_id: call.id.into(), extension: "workflow-control".into(),
                operation: ToolApplicationOperation::CallTool { name: name.into(),
                    arguments: serde_json::from_str(call.args.get()).unwrap() } }, fault.clone()).unwrap();
        let current = current.with_tool_application_control(control);
        current.check_tool_authorization(call, Some(plan)).unwrap();
        let custody = source.workflow_task_custody(call, &current, plan).unwrap().unwrap();
        let entry = custody.entry.clone();
        let before = c.f.store.get(waiter.membership.acquisition()).await.unwrap();
        let before_workflow = c.f.store.get(snapshot.reference().key()).await.unwrap();
        let before_ids = all_ids(&c).await;
        let before_histories = histories(&c).await;
        let before_requests = c.f.model_requests();
        let grants_path = c.f.source_path.parent().unwrap().join("grants.jsonl");
        let grants = std::fs::read(&grants_path).unwrap();
        for error in [OperationAuthorizationError::Unavailable,
            OperationAuthorizationError::ObservationUnavailable(OperationObservationError)] {
            *fault.0.lock().unwrap() = Some(error);
            assert!(matches!(current.check_tool_authorization(call, Some(plan)),
                Err(ToolError::OperationAuthorizationUnavailable | ToolError::OperationObservationUnavailable)));
            assert!(matches!(source.workflow_task_custody(call, &current, plan), Err(PrivateOriginalError::Unavailable)));
            assert!(matches!(dispatcher.dispatch_resolved_with_context(call, &current, plan).await,
                Err(ToolError::Unavailable { .. })));
            assert_eq!(calls.load(Ordering::SeqCst), 0);
            assert!(Arc::ptr_eq(&entry, &waiter.stage_call(call).unwrap()));
            entry.current().unwrap();
        }
        *fault.0.lock().unwrap() = Some(OperationRefused::new(OperationRefusalKind::Denied).into());
        assert!(matches!(dispatcher.dispatch_resolved_with_context(call, &current, plan).await,
            Err(ToolError::AccessDenied { .. })));
        *fault.0.lock().unwrap() = None;
        let changed = serde_json::value::RawValue::from_string(call.args.get().to_owned()).unwrap();
        assert!(matches!(current.check_tool_authorization(ToolCallView { args: &changed, ..call }, Some(plan)),
            Err(ToolError::AuthorizationRefused { .. })));
        assert!(matches!(source.workflow_task_custody(ToolCallView { args: &changed, ..call }, &current, plan),
            Err(PrivateOriginalError::Denied)));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(c.f.store.get(waiter.membership.acquisition()).await.unwrap(), before);
        assert_eq!(c.f.store.get(snapshot.reference().key()).await.unwrap(), before_workflow);
        assert_eq!(std::fs::read(&grants_path).unwrap(), grants);
        assert_eq!(all_ids(&c).await, before_ids);
        assert_eq!(histories(&c).await, before_histories);
        assert_eq!(c.f.model_requests(), before_requests);
        // Healthy control enters once through the same wrapper and completes
        // its ordinary synchronous settlement. Earlier failures did not enter.
        dispatcher.dispatch_resolved_with_context(call, &current, plan).await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(entry.current().is_err());
        entry.current_relationship().unwrap();
        // A distinct exact call enters the same adapter and then reports an
        // unavailable result. That is not pre-entry refusal or settlement.
        let next_view = ToolCallView { id: "workflow-after-entry", ..call };
        let next_plan = meerkat_core::resolve_tool_execution_plan_fenced(&dispatcher,
            next_view, &captured, &resolution).unwrap();
        let mut next_facts = binding.facts().clone();
        next_facts.operation_id = serde_json::from_str("\"00000000-0000-0000-0000-000000000072\"").unwrap();
        let AuthorizationOperation::Tool(next_tool) = &mut next_facts.operation else { unreachable!() };
        next_tool.call_id = Arc::from(next_view.id);
        next_tool.target = ToolAuthorizationTarget::Dispatcher(Arc::new(next_plan));
        let next_binding = PreparedAuthorizationBinding::new(next_facts);
        let AuthorizationOperation::Tool(next_tool) = &next_binding.facts().operation else { unreachable!() };
        let ToolAuthorizationTarget::Dispatcher(next_plan) = &next_tool.target else { unreachable!() };
        let next_call = next_tool.call();
        let next_prepared = prepare_control("post-entry control",
            current.work_authorization().unwrap().clone(), next_binding.clone(), &diagnose);
        let next_context = captured.clone().__test_with_prepared_authorization(next_prepared);
        let next_entry = source.workflow_task_custody(next_call, &next_context, next_plan).unwrap().unwrap().entry;
        after_entry_failure.store(true, Ordering::SeqCst);
        assert!(matches!(dispatcher.dispatch_resolved_with_context(next_call, &next_context, next_plan).await,
            Err(ToolError::Unavailable { .. })));
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        next_entry.current().unwrap();
        assert!(!next_entry.settle_unentered().unwrap(), "entered unavailable result must retain custody");
        // Only this inert leaf's joined owner now supplies explicit settlement.
        // No external operation was sent and no asynchronous child exists.
        next_entry.settle().unwrap();
        source.record_operation_cancelled(&c.lead.original).await.unwrap();
        assert!(entry.current_relationship().is_err());
        assert!(matches!(dispatcher.dispatch_resolved_with_context(call, &current, plan).await,
            Err(ToolError::AccessDenied { .. })));
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        hold.release();
        super::idle_restore_tests::finish(&c, "planning_lead", &g).await;
        }).await.expect("bounded workflow dispatch control");
    })).catch_unwind().await;
    close(&c, body).await;
}
impl WorkflowReadFault {
    fn new(inner: Arc<dyn ToolkitStore>, acquisition: StoreKey) -> Arc<Self> {
        Arc::new(Self {
            inner,
            acquisition,
            reads: 0.into(),
            fail_at: 0.into(),
            conflict: false.into(),
        })
    }
}
#[allow(unused_variables)]
#[async_trait::async_trait]
impl ToolkitStore for WorkflowReadFault {
    fn capabilities(&self) -> StoreCapabilities {
        self.inner.capabilities()
    }
    async fn now_ms(&self) -> StoreResult<i64> {
        self.inner.now_ms().await
    }
    async fn admit(&self, _: AdmissionRequest) -> StoreResult<AdmissionOutcome> {
        panic!("workflow read control must not admit records")
    }
    async fn get(&self, key: &StoreKey) -> StoreResult<Option<StoredRecord>> {
        if key == &self.acquisition {
            let read = self.reads.fetch_add(1, Ordering::SeqCst) + 1;
            if self.fail_at.load(Ordering::SeqCst) == read {
                return Err(if self.conflict.load(Ordering::SeqCst) {
                    StoreError::Conflict
                } else {
                    StoreError::Worker
                });
            }
        }
        self.inner.get(key).await
    }
    async fn list_records(
        &self,
        scope: &StoreScope,
        after: Option<&str>,
        limit: u32,
    ) -> StoreResult<Vec<StoredRecord>> {
        self.inner.list_records(scope, after, limit).await
    }
    async fn compare_and_swap(&self, transition: Transition) -> StoreResult<StoredRecord> {
        panic!("workflow read control must not update records")
    }
    async fn claim_record(
        &self,
        key: &StoreKey,
        revision: u64,
        worker: &str,
        ttl: u64,
    ) -> StoreResult<RecordLease> {
        panic!("workflow read control must not mutate effects or claims")
    }
    async fn renew_record_claim(&self, lease: &RecordLease, ttl: u64) -> StoreResult<RecordLease> {
        panic!("workflow read control must not mutate effects or claims")
    }
    async fn recover_record_claim(&self, prior: &RecordLease) -> StoreResult<RecordLease> {
        panic!("workflow read control must not mutate effects or claims")
    }
    async fn release_record_claim(&self, lease: &RecordLease) -> StoreResult<()> {
        panic!("workflow read control must not mutate effects or claims")
    }
    async fn read_events(
        &self,
        key: &StoreKey,
        after: u64,
        limit: u32,
    ) -> StoreResult<Vec<StoredEvent>> {
        self.inner.read_events(key, after, limit).await
    }
    async fn get_effect(&self, key: &EffectKey) -> StoreResult<Option<StoredEffect>> {
        self.inner.get_effect(key).await
    }
    async fn list_effects(
        &self,
        key: &StoreKey,
        after: Option<&str>,
        limit: u32,
    ) -> StoreResult<Vec<StoredEffect>> {
        self.inner.list_effects(key, after, limit).await
    }
    async fn claim_effect(
        &self,
        key: &EffectKey,
        revision: u64,
        worker: &str,
        ttl: u64,
    ) -> StoreResult<EffectClaimOutcome> {
        panic!("workflow read control must not mutate effects or claims")
    }
    async fn acknowledge_effect(
        &self,
        lease: &EffectLease,
        outcome: EffectOutcome,
    ) -> StoreResult<StoredEffect> {
        panic!("workflow read control must not mutate effects or claims")
    }
    async fn claim_reconciliation(
        &self,
        key: &EffectKey,
        worker: &str,
        ttl: u64,
    ) -> StoreResult<StoredEffect> {
        panic!("workflow read control must not mutate effects or claims")
    }
    async fn reconcile_effect(
        &self,
        lease: &EffectLease,
        resolution: Reconciliation,
    ) -> StoreResult<StoredEffect> {
        panic!("workflow read control must not mutate effects or claims")
    }
}
