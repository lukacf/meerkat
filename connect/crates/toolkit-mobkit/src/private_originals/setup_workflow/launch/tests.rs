//! One actual host-selected G route through native Original admission. The
//! model transport is scripted. These controls do not qualify waiter disclosure,
//! shared SDK discovery, approval/activation or dynamic member creation.
use super::*;
use crate::{
    original_ingress::tests::private_event,
    private_originals::governed_submit::tests::first_input_tests::FirstFixture,
};
use futures::FutureExt;
use meerkat_authorization_contracts::constraints::{ExecutionRestrictions, LifetimeRestriction};
use meerkat_core::{ToolDispatchContext, exact_operation::OperationExecutionScope};
use meerkat_toolkit::{
    authority::{
        Action, ActionPermission, Ceiling, SetupProposalConstraints, SetupRequestEvidence,
        SetupUseContext,
    },
    request::{
        ContextReference, ContextReferenceOwner, FindAndInstallRequest, HostSetupRequest,
        NativeOriginalTask, RequestError, SetupLimits, TaskContinuation, validate_request,
    },
    setup::{SetupWorkBackend, SetupWorkContext},
};
use meerkat_workgraph::{
    ClaimWorkItemRequest, CreateWorkItemRequest, LinkWorkItemsRequest, SqliteWorkGraphStore,
    WorkEdgeKind, WorkGraphService, WorkItem, WorkItemRef, WorkNamespace, WorkOwner, WorkOwnerKey,
};
use std::{collections::BTreeSet, panic::AssertUnwindSafe, time::Duration};
use toolkit_package::Revision;
use toolkit_store::ToolkitStore;

const GUARD: Duration = Duration::from_secs(30);
const CYCLE: &str = "planning-launch-control";
fn id(value: &str) -> Id {
    Id::new(value).unwrap()
}
fn revision(value: &str) -> Revision {
    Revision::new(value).unwrap()
}
fn native(observation: WorkflowLaunchObservation) -> PrivateOriginalSubmission {
    match observation {
        WorkflowLaunchObservation::Native { observation } => observation,
        WorkflowLaunchObservation::NotAdmitted => {
            panic!("unexpected definitive native nonadmission")
        }
        WorkflowLaunchObservation::HistoricalAdmitted { .. } => {
            panic!("historical observation cannot stand in for a native submission")
        }
    }
}
struct Inline;
impl ContextReferenceOwner for Inline {
    fn verify_access(
        &self,
        _: &ContextReference,
        _: &SetupRequestEvidence,
        _: &SetupUseContext<'_>,
    ) -> Result<(), RequestError> {
        Err(RequestError::ContextUnavailable)
    }
}
fn work_ref(item: &WorkItem) -> WorkItemRef {
    WorkItemRef {
        realm_id: item.realm_id.clone(),
        namespace: item.namespace.clone(),
        item_id: item.id.clone(),
    }
}
async fn claim(service: &WorkGraphService, name: &str) -> WorkItem {
    let item = service
        .create(CreateWorkItemRequest {
            title: name.into(),
            labels: [CYCLE.into()].into(),
            ..Default::default()
        })
        .await
        .unwrap();
    service
        .claim(ClaimWorkItemRequest {
            id: item.id,
            realm_id: None,
            namespace: None,
            expected_revision: item.revision,
            owner: WorkOwner {
                key: WorkOwnerKey::agent(name).unwrap(),
                display_name: None,
            },
            lease_seconds: Some(300),
            lease_expires_at: None,
        })
        .await
        .unwrap()
}
async fn event(
    f: &FirstFixture,
    label: &str,
    contributors: Vec<EvidenceId>,
) -> PrivateOriginalSubmit {
    let mut event = private_event(label);
    event.route = f.original_reference().route.clone();
    event.content_sha256 = digest(label.as_bytes());
    event.contributors = contributors;
    let authentication = event.original_authentication.clone();
    let reference = f.source.retain_event(event).await.unwrap();
    PrivateOriginalSubmit {
        original: reference,
        content: label.into(),
        transport_authentication: authentication,
    }
}

struct Case {
    planning: Arc<InstalledWorkflowPlanning>,
    membership: SetupWorkflowWaiter,
    snapshot: SetupWorkflowSnapshot,
    a: crate::private_originals::acquisition::AcquisitionOriginal,
    a_dispatch: ToolDispatchContext,
    work: SetupWorkContext,
    _backend: SetupWorkBackend,
}

async fn prepare(f: &FirstFixture) -> Case {
    prepare_with_waiter_options(f, None).await
}

async fn prepare_with_waiter_options(f: &FirstFixture, lifecycle: Option<(u64, bool)>) -> Case {
    let a = f.submit().await;
    f.finish(&a).await.unwrap();
    let dispatch = f.probe.as_ref().unwrap().captured();
    let original = f.source.acquisition_original(&dispatch).await.unwrap();
    // P is a separate actual admitted input. A is never G's parent and its
    // cancellation cannot revoke the independently selected planning lineage.
    let parent = event(f, "planning-parent", vec![]).await;
    let admitted_parent = f
        .source
        .submit_authenticated_original(parent.clone())
        .await
        .unwrap();
    f.finish(&Ok::<_, String>(admitted_parent)).await.unwrap();
    let parent_binding = f
        .source
        .binding_evidence(&parent.original)
        .await
        .unwrap()
        .unwrap();
    let submission = event(
        f,
        "planning-lead",
        vec![parent_binding.candidate().original_work.work.clone()],
    )
    .await;
    let route = f.source.route(&submission.original.route).unwrap();
    let native = f.lifetime().selected_policy().native_host.as_ref().unwrap();
    let now = u64::try_from(f.store.now_ms().await.unwrap()).unwrap();
    let deadline = now + 60_000;
    let grants = f.source.owner.grants();
    let bounds = ExecutionRestrictions {
        delegation_depth: DelegationDepth::remaining(1),
        lifetime: LifetimeRestriction::window(now, deadline + 60_000),
        ..ExecutionRestrictions::unrestricted()
    };
    let root_authority = meerkat_core::auth::PrincipalRef::in_domain(
        meerkat_core::auth::PrincipalKind::ServiceAccount,
        "admin",
        meerkat_core::auth::TrustDomainId::new("factory-test").unwrap(),
    )
    .unwrap();
    let root = grants
        .issue_root(
            &root_authority,
            EvidenceId::new("planning-operation-parent").unwrap(),
            native.grant_administrator.clone(),
            route.represented_subject.clone(),
            bounds.clone(),
        )
        .unwrap();
    let leaf_bounds = bounds
        .for_child(&ExecutionRestrictions {
            delegation_depth: DelegationDepth::remaining(0),
            ..ExecutionRestrictions::unrestricted()
        })
        .unwrap();
    let leaf = grants
        .issue_child(
            &native.grant_administrator,
            &root,
            EvidenceId::new("planning-operation-leaf").unwrap(),
            route.logical_executor.clone(),
            leaf_bounds.clone(),
        )
        .unwrap();
    let lineage = vec![root, leaf];
    let service = WorkGraphService::with_scope(
        Arc::new(
            SqliteWorkGraphStore::open(f.source_path.with_extension("planning-work.sqlite"))
                .unwrap(),
        ),
        f.scope.realm.clone(),
        WorkNamespace::new("planning-control").unwrap(),
    );
    let parent_work = claim(&service, "parent").await;
    let child_work = claim(&service, "waiter-a").await;
    service
        .link(LinkWorkItemsRequest {
            realm_id: None,
            namespace: None,
            kind: WorkEdgeKind::Parent,
            from_id: child_work.id.clone(),
            to_id: parent_work.id.clone(),
        })
        .await
        .unwrap();
    let backend = SetupWorkBackend::new(
        service,
        "sqlite-planning-control".into(),
        id("generation-one"),
        &[meerkat_core::DurabilityDeclaration::durable(
            "workgraph",
            meerkat_core::DurabilityResolution::Persistent,
        )],
    )
    .unwrap();
    let work = SetupWorkContext::from_owner(
        backend.pin().clone(),
        work_ref(&parent_work),
        work_ref(&child_work),
        parent_work.owner.as_ref().unwrap().key.clone(),
        child_work.owner.as_ref().unwrap().key.clone(),
        CYCLE.into(),
    )
    .unwrap();
    let terms = SetupWorkflowTerms {
        key: id("planning-workflow"),
        backend: backend.pin().clone(),
        parent: work_ref(&parent_work),
        parent_owner: parent_work.owner.as_ref().unwrap().key.clone(),
        cycle_label: CYCLE.into(),
        requester: route.requester.clone(),
        realm: route.realm.clone(),
        authority_revision: revision("planning-policy"),
        planning_source: f.source.key(&submission.original).unwrap(),
        lead: native.members[0].member.clone(),
        envelope_digest: ContentDigest::of_bytes(&canonical_json(&leaf_bounds).unwrap()),
        deadline_ms: deadline,
        max_waiters: 8,
    };
    let planning = f
        .source
        .prepare_workflow_planning(
            &parent.original,
            submission.clone(),
            terms.clone(),
            lineage.clone(),
        )
        .await
        .unwrap();
    let replay = f
        .source
        .prepare_workflow_planning(&parent.original, submission, terms.clone(), lineage)
        .await
        .unwrap();
    assert!(
        Arc::ptr_eq(&planning, &replay),
        "same actual P retains one planning owner"
    );
    let (membership, snapshot) = join_waiter_options(
        f, &planning, &original, &dispatch, &backend, &work, "waiter-a", lifecycle,
    )
    .await;
    Case {
        planning,
        membership,
        snapshot,
        a: original,
        a_dispatch: dispatch,
        work,
        _backend: backend,
    }
}

async fn join_waiter<S>(
    f: &FirstFixture<S>,
    planning: &Arc<InstalledWorkflowPlanning>,
    original: &crate::private_originals::acquisition::AcquisitionOriginal,
    dispatch: &ToolDispatchContext,
    backend: &SetupWorkBackend,
    work: &SetupWorkContext,
    label: &str,
) -> (SetupWorkflowWaiter, SetupWorkflowSnapshot) {
    join_waiter_options(f, planning, original, dispatch, backend, work, label, None).await
}

// Only lifecycle controls select a shorter deadline or a bounded readiness
// proposal. None preserves every existing first-launch/control input exactly.
async fn join_waiter_options<S>(
    f: &FirstFixture<S>,
    planning: &Arc<InstalledWorkflowPlanning>,
    original: &crate::private_originals::acquisition::AcquisitionOriginal,
    dispatch: &ToolDispatchContext,
    backend: &SetupWorkBackend,
    work: &SetupWorkContext,
    label: &str,
    lifecycle: Option<(u64, bool)>,
) -> (SetupWorkflowWaiter, SetupWorkflowSnapshot) {
    let (membership, workflow, _) = join_waiter_context(
        f, planning, original, dispatch, backend, work, label, lifecycle, None,
    )
    .await;
    (membership, workflow)
}

pub(super) async fn join_waiter_context<S>(
    f: &FirstFixture<S>,
    planning: &Arc<InstalledWorkflowPlanning>,
    original: &crate::private_originals::acquisition::AcquisitionOriginal,
    dispatch: &ToolDispatchContext,
    backend: &SetupWorkBackend,
    work: &SetupWorkContext,
    label: &str,
    lifecycle: Option<(u64, bool)>,
    correlation: Option<Id>,
) -> (
    SetupWorkflowWaiter,
    SetupWorkflowSnapshot,
    meerkat_toolkit::setup::SetupSnapshot,
) {
    join_waiter_context_with_member(
        f,
        planning,
        original,
        dispatch,
        backend,
        work,
        label,
        lifecycle,
        correlation,
        None,
    )
    .await
}

async fn join_waiter_context_with_member<S>(
    f: &FirstFixture<S>,
    planning: &Arc<InstalledWorkflowPlanning>,
    original: &crate::private_originals::acquisition::AcquisitionOriginal,
    dispatch: &ToolDispatchContext,
    backend: &SetupWorkBackend,
    work: &SetupWorkContext,
    label: &str,
    lifecycle: Option<(u64, bool)>,
    correlation: Option<Id>,
    member: Option<meerkat_core::MobMemberBinding>,
) -> (
    SetupWorkflowWaiter,
    SetupWorkflowSnapshot,
    meerkat_toolkit::setup::SetupSnapshot,
) {
    join_waiter_context_with_effects(
        f,
        planning,
        original,
        dispatch,
        backend,
        work,
        label,
        lifecycle,
        correlation,
        member,
        true,
    )
    .await
}

async fn join_waiter_context_with_effects<S>(
    f: &FirstFixture<S>,
    planning: &Arc<InstalledWorkflowPlanning>,
    original: &crate::private_originals::acquisition::AcquisitionOriginal,
    dispatch: &ToolDispatchContext,
    backend: &SetupWorkBackend,
    work: &SetupWorkContext,
    label: &str,
    lifecycle: Option<(u64, bool)>,
    correlation: Option<Id>,
    member: Option<meerkat_core::MobMemberBinding>,
    prepare_effects: bool,
) -> (
    SetupWorkflowWaiter,
    SetupWorkflowSnapshot,
    meerkat_toolkit::setup::SetupSnapshot,
) {
    let terms = &planning.terms;
    let deadline = lifecycle.map_or(terms.deadline_ms, |value| value.0);
    let now = u64::try_from(f.store.now_ms().await.unwrap()).unwrap();
    let route = f.source.route(&planning.submission.original.route).unwrap();
    let tenant = id(&f.scope.realm);
    let deployment = id(&f.scope.owner);
    let receiver = id("agent");
    let audience = BTreeSet::from([id(label)]);
    let evidence = SetupRequestEvidence::new(
        original.original.association().clone(),
        SetupProposalConstraints {
            tenant: tenant.clone(),
            deployment: deployment.clone(),
            policy_revision: terms.authority_revision.clone(),
            issued_at_ms: now,
            expires_at_ms: deadline,
            ceiling: if lifecycle.is_some_and(|value| value.1) {
                lifecycle_tests::readiness_ceiling(&receiver, &audience, deadline)
            } else {
                Ceiling {
                    actions: [ActionPermission {
                        action: Action::FindAndInstall,
                        resource: receiver.clone(),
                    }]
                    .into(),
                    services: BTreeSet::new(),
                    frontends: BTreeSet::new(),
                    output_audiences: audience.clone(),
                }
            },
        },
    )
    .unwrap();
    let target = evidence.association().candidate().target.clone();
    let authorization = dispatch.work_authorization().unwrap();
    let context = SetupUseContext {
        tenant: &tenant,
        deployment: &deployment,
        realm: &route.realm,
        target: &target,
        policy_revision: &terms.authority_revision,
        now_ms: now,
        authorization,
    };
    let OperationExecutionScope::RuntimeInput {
        canonical_input_id, ..
    } = authorization.execution_scope()
    else {
        panic!("actual waiter input scope required")
    };
    let retained = dispatch.retained_work().unwrap().clone();
    let request = validate_request(
        FindAndInstallRequest {
            request: format!("Prepare a bounded integration for {label}"),
            user_context: vec![],
            caller_observations: vec![],
        },
        HostSetupRequest {
            setup_helper: None,
            work_context: Some(work.clone()),
            target: receiver,
            correlation: correlation.unwrap_or_else(|| id(&format!("{label}-call"))),
            parent_operation: None,
            continuation: TaskContinuation {
                owner: id("planning-test"),
                task: id(&retained.run_id().to_string()),
                revision: revision(&canonical_input_id.to_string()),
                delivery_key: id(&format!("planning-test:{label}")),
                audience,
                native_original: Some(NativeOriginalTask {
                    origin_session_id: f.session.clone(),
                    retained_work: retained,
                    member,
                }),
            },
            environment_revision: revision("planning-environment"),
            limits: SetupLimits {
                deadline_ms: deadline,
                max_workers: 1,
                max_effect_attempts: 2,
            },
        },
        &evidence,
        &context,
        &Inline,
    )
    .unwrap();
    let store = f.source.workflow_store().unwrap();
    let acquisition = store
        .admit(&request, &evidence, &context, &Inline)
        .await
        .unwrap()
        .snapshot;
    let membership = store
        .workflow_waiter(terms, &acquisition, original.source_record.key.clone())
        .unwrap();
    let joined = store
        .join_setup_workflow(
            terms,
            &membership,
            SetupAccess {
                evidence: &evidence,
                context: &context,
                references: &Inline,
            },
            backend,
        )
        .await
        .unwrap();
    planning
        .retain_launch_interest(&joined.snapshot, &membership, original)
        .await
        .unwrap();
    if prepare_effects && matches!(f.source.decode_record(&original.source_record, &original.reference).unwrap().planning_source_evidence,
        Some(crate::private_originals::planning_source_evidence::PlanningSourceEvidence::EffectWaiter { .. })) {
        planning.prepare_waiter_effects(&membership, original, SetupAccess {
            evidence: &evidence, context: &context, references: &Inline,
        }).await.unwrap();
    }
    (membership, joined.snapshot, acquisition)
}

struct DepartAfterBind {
    inner: Arc<WorkflowBinder>,
    planning: Arc<InstalledWorkflowPlanning>,
    membership: SetupWorkflowWaiter,
    pending_seen: AtomicBool,
}
#[async_trait::async_trait]
impl GovernedInputBinder for DepartAfterBind {
    async fn bind(
        &self,
        context: &GovernedBindContext,
        input: Input,
    ) -> Result<BoundGovernedInput, String> {
        let bound = self.inner.bind(context, input).await?;
        let source = self
            .planning
            .source
            .upgrade()
            .ok_or("fixture source unavailable")?;
        let pending = source
            .retained
            .lock()
            .map_err(|_| "fixture owner unavailable")?
            .iter()
            .find(|held| held.reference == self.planning.submission.original)
            .cloned()
            .and_then(|held| held.setup_workflow.get().cloned())
            .is_some_and(|workflow| {
                !workflow.accepted.load(Ordering::Acquire) && workflow.current_source().is_err()
            });
        self.pending_seen.store(pending, Ordering::Release);
        self.planning
            .depart_waiter(&self.membership)
            .await
            .map_err(|error| error.to_string())?;
        // Return the actual bound input, source guard and production fence.
        // Native still makes its real final admission decision.
        Ok(bound)
    }
    fn retain_unresolved(&self, settlement: Box<dyn GovernedSettlement>, detail: &str) {
        self.inner.retain_unresolved(settlement, detail);
    }
}

async fn exercise(case: u8) {
    let f = FirstFixture::for_continuation().await;
    let result = AssertUnwindSafe(tokio::time::timeout(GUARD, async {
        let mut c = prepare(&f).await;
        assert_eq!(
            f.inputs().await.unwrap(),
            2,
            "A and independent P admitted before G"
        );
        let before_delivery = f
            .store
            .get_effect(&c.membership.delivery_effect().unwrap())
            .await
            .unwrap()
            .unwrap();
        if case == 0 {
            // Explicit seal prevents new members but preserves existing A.
            c.snapshot = f
                .source
                .workflow_store()
                .unwrap()
                .seal_setup_workflow(c.snapshot.reference())
                .await
                .unwrap();
            assert!(c.snapshot.sealed());
            c.planning
                .retain_launch_interest(&c.snapshot, &c.membership, &c.a)
                .await
                .unwrap();
            let admitted = native(c.planning.launch(&c.snapshot).await.unwrap());
            assert!(matches!(
                &admitted,
                PrivateOriginalSubmission::Admitted {
                    source_settlement: PrivateSourceSettlement::FreshAcceptancePromoted,
                    ..
                }
            ));
            f.finish(&Ok::<_, String>(admitted)).await.unwrap();
            assert_eq!(f.inputs().await.unwrap(), 3);
            let g_row = f
                .store
                .get(&f.source.key(&c.planning.submission.original).unwrap())
                .await
                .unwrap()
                .unwrap();
            let g_record = f
                .source
                .decode_record(&g_row, &c.planning.submission.original)
                .unwrap();
            let Some(
                crate::private_originals::planning_source_evidence::PlanningSourceEvidence::Lead {
                    parent,
                },
            ) = &g_record.planning_source_evidence
            else {
                panic!("actual planning selection retains G evidence");
            };
            let p_row = f
                .store
                .get(&f.source.key(parent).unwrap())
                .await
                .unwrap()
                .unwrap();
            let p_record = f.source.decode_record(&p_row, parent).unwrap();
            assert!(p_record.final_association.as_ref() == Some(c.planning.parent.association()));
            assert!(
                p_record.planning_source_evidence.is_none()
                    && p_record.bound_service_input.is_none(),
                "selecting G cannot retrofit the already-admitted ordinary P"
            );
            assert!(
                g_record
                    .bound_service_input
                    .as_ref()
                    .unwrap()
                    .decode()
                    .unwrap()
                    .header()
                    .authority_association
                    == g_record.final_association
            );
            let held = f
                .source
                .retained
                .lock()
                .unwrap()
                .iter()
                .find(|held| held.reference == c.planning.submission.original)
                .cloned()
                .unwrap();
            assert!(
                held.setup_workflow.get().unwrap().current_source().is_ok(),
                "actual accepted callback promoted the staged G attachment"
            );
            let observed = native(c.planning.launch(&c.snapshot).await.unwrap());
            assert!(matches!(
                observed,
                PrivateOriginalSubmission::Admitted { .. }
            ));
            assert_eq!(
                f.inputs().await.unwrap(),
                3,
                "exact replay observed G without submitting another native input"
            );
            let launch = f
                .store
                .get_effect(&c.snapshot.reference().launch_effect())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(launch.status, EffectStatus::Succeeded);
            assert_eq!(launch.attempt, 1);
        } else if case == 1 {
            // Real dispatch succeeded but the outbox owner never receives its
            // acknowledgment. Its expired Some(lease) must reconcile, not send.
            let EffectClaimOutcome::Claimed(claimed) = f
                .store
                .claim_effect(
                    &c.snapshot.reference().launch_effect(),
                    c.snapshot.revision(),
                    "lost-ack-control",
                    1,
                )
                .await
                .unwrap()
            else {
                panic!("first launch effect claimed")
            };
            let expires = claimed.lease.as_ref().unwrap().expires_at_ms;
            let prepared = Arc::new(PreparedWorkflowLaunch {
                selection: c.planning.clone(),
                reference: c.snapshot.reference().clone(),
            });
            let admitted = native(prepared.submit(&f.source).await.unwrap());
            assert!(matches!(
                &admitted,
                PrivateOriginalSubmission::Admitted {
                    source_settlement: PrivateSourceSettlement::FreshAcceptancePromoted,
                    ..
                }
            ));
            f.finish(&Ok::<_, String>(admitted)).await.unwrap();
            tokio::time::timeout(GUARD, async {
                while f.store.now_ms().await.unwrap() < expires {
                    tokio::time::sleep(Duration::from_millis(1)).await;
                }
            })
            .await
            .unwrap();
            let lost = f.store.get_effect(&claimed.key).await.unwrap().unwrap();
            assert_eq!(lost.status, EffectStatus::InFlight);
            assert!(lost.lease.is_some());
            let observed = native(c.planning.launch(&c.snapshot).await.unwrap());
            assert!(matches!(
                observed,
                PrivateOriginalSubmission::Admitted { .. }
            ));
            let reconciled = f.store.get_effect(&claimed.key).await.unwrap().unwrap();
            assert_eq!(reconciled.status, EffectStatus::Succeeded);
            assert!(reconciled.lease.is_none());
            assert_eq!(reconciled.attempt, 1);
            assert_eq!(f.inputs().await.unwrap(), 3);
        } else if case == 2 {
            // Claim first, then close the actual reservation before native's
            // final protected entry. Durable departure is not the entry fence.
            let EffectClaimOutcome::Claimed(claimed) = f
                .store
                .claim_effect(
                    &c.snapshot.reference().launch_effect(),
                    c.snapshot.revision(),
                    "departure-control",
                    60_000,
                )
                .await
                .unwrap()
            else {
                panic!("first launch effect claimed")
            };
            let prepared = Arc::new(PreparedWorkflowLaunch {
                selection: c.planning.clone(),
                reference: c.snapshot.reference().clone(),
            });
            let route = f
                .source
                .route(&c.planning.submission.original.route)
                .unwrap();
            let binder = Arc::new(DepartAfterBind {
                inner: prepared.binder(&f.source, &route).await.unwrap(),
                planning: c.planning.clone(),
                membership: c.membership.clone(),
                pending_seen: AtomicBool::new(false),
            });
            let refused = prepared
                .submit_with_binder(&f.source, binder.clone())
                .await
                .unwrap();
            assert!(
                binder.pending_seen.load(Ordering::Acquire),
                "actual G binder staged fenced pending custody before departure"
            );
            let departed = f
                .source
                .workflow_store()
                .unwrap()
                .load_setup_workflow(c.snapshot.reference())
                .await
                .unwrap();
            assert_eq!(departed.active_waiters(), 0);
            assert!(matches!(refused, WorkflowLaunchObservation::NotAdmitted));
            assert_eq!(
                f.inputs().await.unwrap(),
                2,
                "closed final entry cannot admit G"
            );
            c.planning
                .record_launch_observation(&f.source, claimed.lease.as_ref().unwrap(), &refused)
                .await
                .unwrap();
            let retained = f.store.get_effect(&claimed.key).await.unwrap().unwrap();
            assert_eq!(retained.status, EffectStatus::Failed);
            assert!(retained.lease.is_none());
            let observed = c.planning.launch(&c.snapshot).await.unwrap();
            assert!(
                matches!(observed, WorkflowLaunchObservation::NotAdmitted),
                "later observation retains the exact failed launch decision"
            );
            assert_eq!(retained.attempt, 1);
            assert_eq!(
                f.inputs().await.unwrap(),
                2,
                "known nonadmission does not replay G"
            );
        } else if case == 3 {
            // Cancel the actual dispatch future after its store claim and
            // before constructing/handing a binder to the native runtime.
            let (claimed_tx, claimed_rx) = tokio::sync::oneshot::channel();
            let source = f.source.clone();
            let planning = c.planning.clone();
            let reference = c.snapshot.reference().clone();
            let revision = c.snapshot.revision();
            let mut dispatch = Box::pin(async move {
                let claimed = source
                    .store
                    .claim_effect(
                        &reference.launch_effect(),
                        revision,
                        "cancel-before-handoff",
                        60_000,
                    )
                    .await
                    .unwrap();
                let _ = claimed_tx.send(claimed);
                std::future::pending::<()>().await;
                let prepared = Arc::new(PreparedWorkflowLaunch {
                    selection: planning,
                    reference,
                });
                prepared.submit(&source).await
            });
            let claimed = tokio::select! {
                claimed = claimed_rx => claimed.unwrap(),
                _ = &mut dispatch => panic!("held dispatch cannot finish"),
            };
            drop(dispatch);
            let EffectClaimOutcome::Claimed(claimed) = claimed else {
                panic!("launch claim required")
            };
            c.planning.depart_waiter(&c.membership).await.unwrap();
            // This real source cancellation drains its owned SharedEntries.
            // The old eager reservation cannot settle and would hit its 30 s
            // drain bound; no native decision exists to wait for in this case.
            tokio::time::timeout(
                Duration::from_secs(5),
                f.source.cancel_original_operation(&c.a.reference),
            )
            .await
            .expect("no native reservation before handoff")
            .unwrap();
            assert_eq!(f.inputs().await.unwrap(), 2);
            let retained = f.store.get_effect(&claimed.key).await.unwrap().unwrap();
            assert_eq!(retained.status, EffectStatus::InFlight);
            assert!(retained.lease.is_some());
            assert_eq!(
                retained.attempt, 1,
                "local cleanup does not fabricate an outbox admission decision"
            );
        } else if case == 4 {
            // Both producer invocations observe no retained choice. Keep
            // them live concurrently, then order None's publication first.
            // Some must not replace it even before a durable CAS is available.
            let correlation = id("divergent-first-producer");
            let barrier = std::sync::Barrier::new(3);
            let (none_done, after_none) = std::sync::mpsc::channel();
            let ((none_before, none_result), (some_before, some_result)) =
                std::thread::scope(|threads| {
                    let source = &f.source;
                    let original = &c.a;
                    let work = &c.work;
                    let planning = &c.planning;
                    let correlation = &correlation;
                    let gate = &barrier;
                    let none = threads.spawn(move || {
                        let before =
                            source.retained_workflow_selection(&original.original, correlation);
                        gate.wait();
                        let result = source.retain_workflow_selection(
                            original,
                            correlation,
                            None,
                            Some(work),
                        );
                        let _ = none_done.send(());
                        (before, result)
                    });
                    let some = threads.spawn(move || {
                        let before =
                            source.retained_workflow_selection(&original.original, correlation);
                        gate.wait();
                        let _ = after_none.recv();
                        let result = source.retain_workflow_selection(
                            original,
                            correlation,
                            Some(planning),
                            Some(work),
                        );
                        (before, result)
                    });
                    barrier.wait();
                    let none = none.join();
                    let some = some.join();
                    (none.unwrap(), some.unwrap())
                });
            assert!(none_before.unwrap().is_none() && some_before.unwrap().is_none());
            assert!(none_result.is_ok());
            assert!(matches!(some_result, Err(PrivateOriginalError::Conflict)));
            assert!(
                f.source
                    .retained_workflow_selection(&c.a.original, &correlation)
                    .unwrap()
                    .is_none()
            );
            f.source
                .retain_workflow_selection(&c.a, &correlation, None, Some(&c.work))
                .unwrap();
            assert_eq!(f.inputs().await.unwrap(), 2);
        } else {
            // A's actual cancellation owner fences it Pending before the
            // deliberately failed persistence callback releases the writer.
            let a_claims = c.a.original.association().candidate();
            let expected = c.a.original.association().clone();
            let record = c.a.source_record.clone();
            let cancelled = f
                .source
                .owner
                .record_operation_cancelled(
                    &a_claims.ingress_namespace.realm,
                    &a_claims.original_work,
                    move |binding, _| async move {
                        if binding.as_ref() != Some(&expected) {
                            return Err(crate::original_ingress::OriginalWriterError::Conflict);
                        }
                        Ok(record)
                    },
                    |_| async { Err(crate::original_ingress::OriginalWriterError::Unavailable) },
                )
                .await;
            assert!(matches!(
                cancelled,
                Err(crate::original_ingress::OriginalWriterError::Unavailable)
            ));
            assert!(matches!(
                f.source.owner.workflow_controller(&c.a.original),
                Err(crate::original_ingress::OriginalWriterError::Unavailable)
            ));
            let fence = LaunchFence {
                planning: c.planning.clone(),
            };
            assert!(
                matches!(fence.enter(), Err(GovernedEntryRefusal::Unavailable(_))),
                "A alone stays unavailable after its failed cancellation write"
            );
            assert!(c.planning.interests.lock().unwrap()[0].entries.is_empty());

            // Preserve A's actual context before rearming the existing probe.
            // B is a new authenticated native input, not another A correlation.
            let a_scope = c
                .a_dispatch
                .work_authorization()
                .unwrap()
                .execution_scope()
                .clone();
            f.probe.as_ref().unwrap().arm_next_input();
            let b_submission = event(&f, "planning-waiter-b", vec![]).await;
            let admitted_b = f
                .source
                .submit_authenticated_original(b_submission)
                .await
                .unwrap();
            f.finish(&Ok::<_, String>(admitted_b)).await.unwrap();
            let b_dispatch = f.probe.as_ref().unwrap().captured();
            let b = f.source.acquisition_original(&b_dispatch).await.unwrap();
            assert_ne!(
                &a_scope,
                b_dispatch.work_authorization().unwrap().execution_scope()
            );
            assert!(c.a.reference != b.reference);
            assert_ne!(c.a.source_record.key, b.source_record.key);
            f.source.owner.workflow_controller(&b.original).unwrap();
            let child = claim(c._backend.service(), "waiter-b").await;
            c._backend
                .service()
                .link(LinkWorkItemsRequest {
                    realm_id: None,
                    namespace: None,
                    kind: WorkEdgeKind::Parent,
                    from_id: child.id.clone(),
                    to_id: c.work.parent().item_id.clone(),
                })
                .await
                .unwrap();
            let work = SetupWorkContext::from_owner(
                c._backend.pin().clone(),
                c.work.parent().clone(),
                work_ref(&child),
                c.planning.terms.parent_owner.clone(),
                child.owner.as_ref().unwrap().key.clone(),
                CYCLE.into(),
            )
            .unwrap();
            let (membership, snapshot) = join_waiter(
                &f,
                &c.planning,
                &b,
                &b_dispatch,
                &c._backend,
                &work,
                "waiter-b",
            )
            .await;
            assert_ne!(c.membership.acquisition(), membership.acquisition());
            assert_ne!(c.work.child(), work.child());
            assert_eq!(snapshot.active_waiters(), 2);
            c.snapshot = snapshot;
            let b_delivery = f
                .store
                .get_effect(&membership.delivery_effect().unwrap())
                .await
                .unwrap()
                .unwrap();
            let admitted = native(c.planning.launch(&c.snapshot).await.unwrap());
            assert!(matches!(
                &admitted,
                PrivateOriginalSubmission::Admitted {
                    source_settlement: PrivateSourceSettlement::FreshAcceptancePromoted,
                    ..
                }
            ));
            f.finish(&Ok::<_, String>(admitted)).await.unwrap();
            assert_eq!(
                f.inputs().await.unwrap(),
                4,
                "A, P, independent B and exactly one G"
            );
            {
                let interests = c.planning.interests.lock().unwrap();
                assert_eq!(interests.len(), 2);
                assert!(
                    interests[0].entries.is_empty(),
                    "pending A supplies no entry"
                );
                assert_eq!(
                    interests[1].entries.len(),
                    1,
                    "the actual B entry admitted G"
                );
            }
            let observed = native(c.planning.launch(&c.snapshot).await.unwrap());
            assert!(matches!(
                observed,
                PrivateOriginalSubmission::Admitted { .. }
            ));
            assert_eq!(
                f.inputs().await.unwrap(),
                4,
                "observation does not submit another G"
            );
            let effect = f
                .store
                .get_effect(&c.snapshot.reference().launch_effect())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(effect.status, EffectStatus::Succeeded);
            assert_eq!(effect.attempt, 1);
            assert_eq!(
                f.store
                    .get_effect(&membership.delivery_effect().unwrap())
                    .await
                    .unwrap()
                    .unwrap(),
                b_delivery,
                "launch never acknowledges B context delivery"
            );
            assert!(
                matches!(
                    f.source.owner.workflow_controller(&c.a.original),
                    Err(crate::original_ingress::OriginalWriterError::Unavailable)
                ),
                "successful B launch cannot reopen A"
            );
        }
        assert_eq!(
            f.store
                .get_effect(&c.membership.delivery_effect().unwrap())
                .await
                .unwrap()
                .unwrap(),
            before_delivery,
            "G admission never acknowledges A context delivery"
        );
        assert!(c.a_dispatch.work_authorization().is_some());
    }))
    .catch_unwind()
    .await;
    let cleanup = f.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    match result {
        Ok(result) => result.expect("bounded planning launch control"),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actual_planning_input_promotes_once_and_preserves_sealed_waiter_delivery() {
    exercise(0).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn expired_inflight_launch_reconciles_actual_native_acceptance_without_resubmit() {
    exercise(1).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn last_departure_after_claim_refuses_actual_planning_admission() {
    exercise(2).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelled_before_native_handoff_leaves_no_pending_source_reservation() {
    exercise(3).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_first_producers_cannot_replace_retained_none_with_planning() {
    exercise(4).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pending_a_cancellation_preserves_unavailable_without_b_and_admits_from_live_b() {
    exercise(5).await;
}

fn admitted_input(
    f: &FirstFixture,
    admitted: &PrivateOriginalSubmission,
) -> meerkat_core::lifecycle::InputId {
    let PrivateOriginalSubmission::Admitted {
        session_id,
        input_id,
        ..
    } = admitted
    else {
        panic!("actual native input admission required")
    };
    assert_eq!(session_id, &f.session.to_string());
    meerkat_core::lifecycle::InputId::from_uuid(input_id.parse().unwrap())
}

async fn close_exercise(case: u8) {
    use meerkat_runtime::terminal_status::{InputTerminalReceiptRead, InteractionSelector};
    let f = FirstFixture::for_continuation().await;
    let result = AssertUnwindSafe(tokio::time::timeout(GUARD, async {
        let c = prepare(&f).await;
        if case >= 5 {
            // The real installed producer retains its optional planning choice
            // before durable membership. The outer cancellation caller recovers
            // it from this exact existing A owner, not from ambient workflow IDs.
            f.source.retain_workflow_selection(&c.a, &id("waiter-a-call"),
                Some(&c.planning), Some(&c.work)).unwrap();
        }
        let delivery_before = f.store.get_effect(&c.membership.delivery_effect().unwrap())
            .await.unwrap().unwrap();
        if case == 6 {
            // Canonical join is committed, but its local interest is absent.
            // This is missing-local-interest recovery, not an injected lost reply.
            c.planning.interests.lock().unwrap().clear();
            let cancelled = f.source.cancel_original_operation(f.original_reference()).await.unwrap();
            assert!(matches!(cancelled, PrivateOriginalCancellation::OperationsCancelledCleanupPending));
            assert_eq!(serde_json::to_value(cancelled).unwrap(), "operations_cancelled_cleanup_pending");
            let departed = f.source.workflow_store().unwrap().load_setup_workflow(c.snapshot.reference()).await.unwrap();
            assert!(departed.sealed() && departed.active_waiters() == 0 && departed.departed(&c.membership).unwrap());
            let close = f.store.get_effect(&c.snapshot.reference().close_effect()).await.unwrap().unwrap();
            assert_eq!(close.status, EffectStatus::Uncertain);
            assert_eq!(f.inputs().await.unwrap(), 2, "the cancellation caller never dispatches unknown G");
            assert!(c.planning.retain_launch_interest(&c.snapshot, &c.membership, &c.a).await.is_err(),
                "an old join snapshot cannot republish interest after cancellation");
            {
                let interests = c.planning.interests.lock().unwrap();
                assert_eq!(interests.len(), 1);
                assert!(interests[0].closed && interests[0].original.is_none());
            }
            f.source.cancel_original_operation(f.original_reference()).await.unwrap();
            let replay = f.store.get_effect(&close.key).await.unwrap().unwrap();
            assert_eq!(replay.status, EffectStatus::Uncertain);
            assert_eq!(replay.attempt, 1);
        } else if case == 2 {
            // A lost launch claim, with no native outcome, stays unresolved.
            let EffectClaimOutcome::Claimed(launch) = f.store.claim_effect(
                &c.snapshot.reference().launch_effect(), c.snapshot.revision(),
                "unknown-launch-close", 1).await.unwrap() else { panic!("launch claimed") };
            c.planning.depart_waiter(&c.membership).await.unwrap();
            assert_eq!(c.planning.observe_close(c.snapshot.reference()).await.unwrap(),
                WorkflowCloseObservation::Pending);
            assert_eq!(f.inputs().await.unwrap(), 2, "no blind launch or cancellation");
            let source = f.source.store.get(&c.planning.terms.planning_source).await.unwrap().unwrap();
            assert!(f.source.decode_stored(&source).unwrap().operation_cancelled);
            assert_eq!(f.store.get_effect(&launch.key).await.unwrap().unwrap(), launch,
                "close does not reset uncertain launch ownership");
            let first = f.store.get_effect(&c.snapshot.reference().close_effect()).await.unwrap().unwrap();
            assert_eq!(first.status, EffectStatus::Uncertain);
            assert_eq!(c.planning.observe_close(c.snapshot.reference()).await.unwrap(),
                WorkflowCloseObservation::Pending);
            let replay = f.store.get_effect(&first.key).await.unwrap().unwrap();
            assert_eq!(replay.status, EffectStatus::Uncertain);
            assert_eq!(replay.attempt, 1, "observation never makes another close dispatch");
            let newer = event(&f, "newer-after-unknown-close", vec![]).await;
            let accepted = f.source.submit_authenticated_original(newer).await.unwrap();
            f.finish(&Ok::<_, String>(accepted)).await.unwrap();
            assert_eq!(f.inputs().await.unwrap(), 3);
        } else if case == 3 {
            // Actual native final-entry refusal, not an absent-input inference.
            let EffectClaimOutcome::Claimed(launch) = f.store.claim_effect(
                &c.snapshot.reference().launch_effect(), c.snapshot.revision(),
                "known-nonadmission-close", 60_000).await.unwrap() else { panic!("launch claimed") };
            let prepared = Arc::new(PreparedWorkflowLaunch {
                selection: c.planning.clone(), reference: c.snapshot.reference().clone(),
            });
            let route = f.source.route(&c.planning.submission.original.route).unwrap();
            let binder = Arc::new(DepartAfterBind {
                inner: prepared.binder(&f.source, &route).await.unwrap(),
                planning: c.planning.clone(), membership: c.membership.clone(),
                pending_seen: AtomicBool::new(false),
            });
            let refused = prepared.submit_with_binder(&f.source, binder).await.unwrap();
            assert!(matches!(refused, WorkflowLaunchObservation::NotAdmitted));
            c.planning.record_launch_observation(&f.source, launch.lease.as_ref().unwrap(), &refused)
                .await.unwrap();
            assert_eq!(c.planning.observe_close(c.snapshot.reference()).await.unwrap(),
                WorkflowCloseObservation::SourceFencedAndNotAdmitted);
            assert_eq!(c.planning.observe_close(c.snapshot.reference()).await.unwrap(),
                WorkflowCloseObservation::SourceFencedAndNotAdmitted);
            assert_eq!(f.inputs().await.unwrap(), 2);
        } else {
            let hold = (case == 1).then(|| f.hold_next_response());
            let admitted = native(c.planning.launch(&c.snapshot).await.unwrap());
            let input = admitted_input(&f, &admitted);
            if let Some(hold) = &hold {
                hold.entered().await;
                c.planning.depart_waiter(&c.membership).await.unwrap();
                assert_eq!(c.planning.observe_close(c.snapshot.reference()).await.unwrap(),
                    WorkflowCloseObservation::Pending);
                let actual = f.machine.input_terminal_receipt_observation(&f.session,
                    InteractionSelector::InputId(input.clone())).await.unwrap().unwrap();
                assert!(matches!(actual.report,
                    meerkat_runtime::terminal_status::InputTerminalReceiptObservation::Pending { .. }),
                    "body-free observation keeps an entered G pending; close did not cancel its run");
                assert_eq!(f.store.get_effect(&c.snapshot.reference().close_effect())
                    .await.unwrap().unwrap().status, EffectStatus::Uncertain);
                hold.release();
                // Source fencing may deny subsequent native work. The runtime
                // owns that outcome; this test requires only its real terminal.
                let terminal = f.machine.wait_input_terminal_receipt(&f.session, &input)
                    .await.unwrap().unwrap();
                assert!(matches!(terminal,
                    meerkat_runtime::terminal_status::InputTerminalReceiptWait::Resolved(_)));
            } else {
                f.finish(&Ok::<_, String>(admitted)).await.unwrap();
                let expected = WorkflowCloseObservation::SourceFencedAndFinalized {
                    session_id: f.session.to_string(), input_id: input.to_string(),
                };
                if case == 4 {
                    let departed = c.planning.depart_waiter(&c.membership).await.unwrap();
                    let EffectClaimOutcome::Claimed(close) = f.store.claim_effect(
                        &c.snapshot.reference().close_effect(), departed.revision(),
                        "lost-close-ack", 1).await.unwrap() else { panic!("close claimed") };
                    // Actual source/native observation succeeds but its
                    // acknowledgment is lost. Only the existing lease expires.
                    assert_eq!(c.planning.close_observation(&f.source, c.snapshot.reference())
                        .await.unwrap(), expected);
                    let expires = close.lease.as_ref().unwrap().expires_at_ms;
                    while f.store.now_ms().await.unwrap() < expires {
                        tokio::time::sleep(Duration::from_millis(1)).await;
                    }
                    assert_eq!(f.store.get_effect(&close.key).await.unwrap().unwrap().status,
                        EffectStatus::InFlight);
                    assert_eq!(c.planning.observe_close(c.snapshot.reference()).await.unwrap(), expected);
                    let done = f.store.get_effect(&close.key).await.unwrap().unwrap();
                    assert_eq!(done.status, EffectStatus::Succeeded);
                    assert_eq!(done.attempt, 1);
                    assert!(done.lease.is_none());
                    assert_eq!(f.inputs().await.unwrap(), 3);
                } else {
                    // A different real input is held in the same session after
                    // G completed. Closing G must not interrupt this new run.
                    let newer_hold = f.hold_next_response();
                    let newer = event(&f, "newer-active-after-g", vec![]).await;
                    let newer_ref = newer.original.clone();
                    let accepted = f.source.submit_authenticated_original(newer).await.unwrap();
                    let newer_input = admitted_input(&f, &accepted);
                    assert_ne!(newer_input, input);
                    newer_hold.entered().await;
                    if case == 5 {
                        let cancelled = f.source.cancel_original_operation(f.original_reference()).await.unwrap();
                        assert!(matches!(cancelled, PrivateOriginalCancellation::OperationsCancelled));
                        let close = f.store.get_effect(&c.snapshot.reference().close_effect()).await.unwrap().unwrap();
                        assert_eq!(close.status, EffectStatus::Succeeded,
                            "actual outer cancellation invokes close for final departure");
                    } else { c.planning.depart_waiter(&c.membership).await.unwrap(); }
                    assert_eq!(c.planning.observe_close(c.snapshot.reference()).await.unwrap(), expected);
                    assert_eq!(c.planning.observe_close(c.snapshot.reference()).await.unwrap(), expected);
                    let actual = f.machine.input_terminal_receipt(&f.session,
                        InteractionSelector::InputId(newer_input.clone())).await.unwrap().unwrap();
                    assert!(matches!(actual.report, InputTerminalReceiptRead::Pending { .. }));
                    f.source.owner.workflow_controller(&c.planning.parent).unwrap();
                    if case == 5 {
                        assert!(f.source.owner.workflow_controller(&c.a.original).is_err(),
                            "outer cancellation fences A while P and the newer input stay independent");
                    } else { f.source.owner.workflow_controller(&c.a.original).unwrap(); }
                    let current = f.source.binding_evidence(&newer_ref).await.unwrap().unwrap();
                    assert!(current.candidate().original_work != c.planning.parent.association().candidate().original_work);
                    newer_hold.release();
                    f.finish(&Ok::<_, String>(accepted)).await.unwrap();
                    let done = f.machine.input_terminal_receipt(&f.session,
                        InteractionSelector::InputId(newer_input)).await.unwrap().unwrap();
                    assert!(matches!(done.report, InputTerminalReceiptRead::Finalized(_)));
                    assert_eq!(f.inputs().await.unwrap(), 4);
                }
            }
        }
        assert_eq!(f.store.get_effect(&c.membership.delivery_effect().unwrap()).await.unwrap().unwrap(),
            delivery_before, "close cannot acknowledge undisclosed waiter context");
    })).catch_unwind().await;
    let cleanup = f.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    match result {
        Ok(result) => result.expect("bounded workflow close control"),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn terminal_g_close_preserves_newer_native_input_and_replay() {
    close_exercise(0).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn active_g_close_fences_source_without_cancelling_shared_run() {
    close_exercise(1).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scoped_g_close_observes_finalization_after_native_unregister() {
    use self::idle_restore_tests::{close, finish};
    use self::waiter_restore_tests::{
        PersistentDedicated, all_ids, join_recovery_for_idle_restore,
    };
    use meerkat_runtime::{
        SessionServiceRuntimeExt,
        completion::CompletionClass,
        input_state::ScopedDestinationWitness,
        terminal_status::{
            InputTerminalReceiptObservation, InteractionSelector, TerminalWitnessSource,
        },
    };
    let c = PersistentDedicated::persistent().await;
    let body = AssertUnwindSafe(tokio::time::timeout(GUARD, async {
        let source = &c.f.source;
        source.prepare_workflow_parent_source(&c.parent.original).await.unwrap();
        let p = source.submit_authenticated_original(c.parent.clone()).await.unwrap();
        finish(&c, "planning_parent", &p).await;
        let probe = c.f.probe.as_ref().unwrap();
        probe.arm_next_input();
        let a = c.f.submit().await;
        c.f.finish(&a).await.unwrap();
        let dispatch = probe.captured();
        assert_eq!(dispatch.origin_session_id(), Some(&c.f.session));
        let original = source.acquisition_original(&dispatch).await.unwrap();
        let planning = source.prepare_workflow_planning(&c.parent.original,
            c.lead.clone(), c.terms.clone(), c.lineage.clone()).await.unwrap();
        let (membership, snapshot) = join_waiter(&c.f, &planning, &original, &dispatch,
            &c.backend, &c.work, "scoped-close-waiter").await;
        let admitted = native(planning.launch(&snapshot).await.unwrap());
        finish(&c, "planning_lead", &admitted).await;
        let PrivateOriginalSubmission::Admitted { session_id, input_id, .. } = admitted else {
            panic!("real native G admission required");
        };
        assert_eq!(session_id, c.lead_session.to_string());
        let input_id = meerkat_core::lifecycle::InputId::from_uuid(input_id.parse().unwrap());
        join_recovery_for_idle_restore(&c).await;
        let key = source.key(&c.lead.original).unwrap();
        let row = c.f.store.get(&key).await.unwrap().unwrap();
        let record = source.decode_record(&row, &c.lead.original).unwrap();
        let input = record.bound_service_input.as_ref().unwrap().decode().unwrap();
        assert_eq!(input.id(), &input_id);
        let stored = c.f.machine.input_state(&c.lead_session, &input_id).await.unwrap().unwrap();
        assert!(matches!(&stored.state.scoped_destination, ScopedDestinationWitness::Bound { .. }),
            "the actual G admission must have a native scoped destination");
        meerkat_runtime::input_state::verify_exact_replay(&stored.state, &input).unwrap();
        let before = c.f.machine.input_terminal_receipt_observation(&c.lead_session,
            InteractionSelector::InputId(input_id.clone())).await.unwrap().unwrap();
        let InputTerminalReceiptObservation::Finalized(summary) = &before.report else {
            panic!("successful G must have a native finalized receipt");
        };
        assert_eq!(summary.class(), CompletionClass::Completed);
        assert_eq!(summary.input_id(), &input_id);
        assert_eq!(summary.recipient_input_ids(), std::slice::from_ref(&input_id));
        let ids = all_ids(&c).await;
        assert_eq!(ids.iter().map(Vec::len).collect::<Vec<_>>(), vec![1, 1, 1]);
        let requests = c.f.model_requests();
        let launch = c.f.store.get_effect(&snapshot.reference().launch_effect()).await.unwrap().unwrap();
        let delivery = c.f.store.get_effect(&membership.delivery_effect().unwrap()).await.unwrap().unwrap();
        let closed = planning.depart_waiter(&membership).await.unwrap();
        assert!(closed.sealed() && closed.active_waiters() == 0);
        source.record_operation_cancelled(&c.lead.original).await.unwrap();
        let cancelled = c.f.store.get(&key).await.unwrap().unwrap();
        assert!(source.decode_record(&cancelled, &c.lead.original).unwrap().operation_cancelled);

        // Retire only the actual native registration. The host and its exact
        // source/close obligation remain live; no result is re-disclosed.
        let registration = c.f.machine.current_session_registration_witness(&c.lead_session).await.unwrap();
        assert!(c.f.machine.unregister_session_registration_until_terminal_if_current(&registration).await.unwrap());
        assert!(c.f.machine.current_session_registration_witness(&c.lead_session).await.is_none());
        assert!(matches!(c.f.machine.input_terminal_receipt(&c.lead_session,
            InteractionSelector::InputId(input_id.clone())).await,
            Err(meerkat_runtime::RuntimeDriverError::ScopedResultWithheld { input_id: withheld }) if withheld == input_id),
            "the old payload reader must still refuse this historical scoped result");
        let historical = c.f.machine.input_terminal_receipt_observation(&c.lead_session,
            InteractionSelector::InputId(input_id.clone())).await.unwrap().unwrap();
        assert_eq!(historical.source, TerminalWitnessSource::DurableStore);
        assert_eq!(historical.report, before.report, "native finalization facts survive unregister");
        let expected = WorkflowCloseObservation::SourceFencedAndFinalized { session_id, input_id: input_id.to_string() };
        assert_eq!(planning.observe_close(closed.reference()).await.unwrap(), expected);
        let close_effect = c.f.store.get_effect(&closed.reference().close_effect()).await.unwrap().unwrap();
        assert_eq!(close_effect.status, EffectStatus::Succeeded);
        assert_eq!(close_effect.attempt, 1);
        assert_eq!(planning.observe_close(closed.reference()).await.unwrap(), expected);
        assert_eq!(c.f.store.get_effect(&close_effect.key).await.unwrap().unwrap(), close_effect);
        assert_eq!(c.f.store.get_effect(&launch.key).await.unwrap().unwrap(), launch);
        assert_eq!(c.f.store.get_effect(&delivery.key).await.unwrap().unwrap(), delivery,
            "historical close cannot acknowledge waiter delivery");
        assert_eq!(c.f.store.get(&key).await.unwrap().unwrap(), cancelled);
        assert_eq!(all_ids(&c).await, ids);
        assert_eq!(c.f.model_requests(), requests, "close cannot replay any input or model work");
    })).catch_unwind().await;
    let body = match body {
        Ok(result) => result.map_err(|error| Box::new(error) as Box<dyn std::any::Any + Send>),
        Err(panic) => Err(panic),
    };
    close(&c, body).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unknown_launch_close_stays_uncertain_without_blind_cancellation() {
    close_exercise(2).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn known_native_nonadmission_settles_only_its_close_obligation() {
    close_exercise(3).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lost_close_ack_reconciles_exact_terminal_without_second_dispatch() {
    close_exercise(4).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn outer_cancellation_closes_terminal_workflow_and_preserves_newer_input() {
    close_exercise(5).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn outer_cancellation_recovers_membership_without_local_interest_and_refuses_late_publication()
 {
    close_exercise(6).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn joined_recovery_worker_finishes_pending_close_after_cancel_caller_returns() {
    use crate::installed_host::acquisition::recovery::AcquisitionRecovery;
    let f = FirstFixture::for_continuation().await;
    let recovery = AcquisitionRecovery::default();
    let result = AssertUnwindSafe(tokio::time::timeout(GUARD, async {
        let c = prepare(&f).await;
        f.source
            .retain_workflow_selection(&c.a, &id("waiter-a-call"), Some(&c.planning), Some(&c.work))
            .unwrap();
        let delivery = f
            .store
            .get_effect(&c.membership.delivery_effect().unwrap())
            .await
            .unwrap()
            .unwrap();
        let hold = f.hold_next_response();
        let admitted = native(c.planning.launch(&c.snapshot).await.unwrap());
        let input = admitted_input(&f, &admitted);
        hold.entered().await;
        // The actual cancellation caller finishes and drops its handles before
        // the existing joined recovery worker starts. It performs no replay.
        let source = f.source.clone();
        let original = f.original_reference().clone();
        let cancelled =
            tokio::spawn(async move { source.cancel_original_operation(&original).await })
                .await
                .unwrap()
                .unwrap();
        assert!(matches!(
            cancelled,
            PrivateOriginalCancellation::OperationsCancelledCleanupPending
        ));
        assert_eq!(
            serde_json::to_value(cancelled).unwrap(),
            "operations_cancelled_cleanup_pending"
        );
        let pending = f
            .store
            .get_effect(&c.snapshot.reference().close_effect())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(pending.status, EffectStatus::Uncertain);
        assert_eq!(pending.attempt, 1);
        assert!(f.source.owner.workflow_controller(&c.a.original).is_err());
        f.source
            .owner
            .workflow_controller(&c.planning.parent)
            .unwrap();
        let owner = crate::installed_host::controller_first_input_fixture::bind_workflow_recovery(
            &f.store,
            &f.scope,
            &f.source,
            f.lifetime(),
            &f.source_path,
        );
        recovery
            .start(recovery.prepare_start().unwrap(), owner)
            .unwrap();
        hold.release();
        let done = loop {
            let row = f.store.get_effect(&pending.key).await.unwrap().unwrap();
            if row.status == EffectStatus::Succeeded {
                break row;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        };
        assert_eq!(
            done.attempt, 1,
            "recovery observes the same close, never dispatches G"
        );
        assert!(done.lease.is_none());
        let Some(EffectOutcome::Succeeded { receipt }) = done.outcome else {
            panic!("exact close receipt")
        };
        assert_eq!(
            serde_json::from_value::<WorkflowCloseObservation>(receipt.body).unwrap(),
            WorkflowCloseObservation::SourceFencedAndFinalized {
                session_id: f.session.to_string(),
                input_id: input.to_string(),
            }
        );
        assert_eq!(f.inputs().await.unwrap(), 3, "A, P and one G only");
        assert_eq!(
            f.store
                .get_effect(&c.membership.delivery_effect().unwrap())
                .await
                .unwrap()
                .unwrap(),
            delivery,
            "cleanup cannot acknowledge undisclosed context"
        );
    }))
    .catch_unwind()
    .await;
    // The same worker retains entered work across stop or a cancelled join.
    // Always join before fixture/native teardown, including failed assertions.
    recovery.stop();
    let joined = recovery.join().await;
    let cleanup = f.shutdown().await;
    assert!(joined.is_ok(), "{joined:?}");
    assert!(cleanup.is_ok(), "{cleanup:?}");
    match result {
        Ok(result) => result.expect("bounded existing-worker close reconciliation"),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn planning_binding_refuses_distinct_issued_root_without_source_or_native_change() {
    let f = FirstFixture::for_continuation().await;
    let result = AssertUnwindSafe(tokio::time::timeout(GUARD, async {
        let c = prepare(&f).await;
        let reference = &c.planning.submission.original;
        let unbound_row = f
            .store
            .get(&f.source.key(reference).unwrap())
            .await
            .unwrap()
            .unwrap();
        let unbound = f.source.decode_record(&unbound_row, reference).unwrap();
        assert!(unbound.final_association.is_none());
        c.planning
            .retain_launch_interest(&c.snapshot, &c.membership, &c.a)
            .await
            .unwrap();
        let admitted = native(c.planning.launch(&c.snapshot).await.unwrap());
        assert!(matches!(
            &admitted,
            PrivateOriginalSubmission::Admitted {
                source_settlement: PrivateSourceSettlement::FreshAcceptancePromoted,
                ..
            }
        ));
        f.finish(&Ok::<_, String>(admitted)).await.unwrap();
        let association = f.source.binding_evidence(reference).await.unwrap().unwrap();
        let key = f.source.key(reference).unwrap();
        let before = f.store.get(&key).await.unwrap().unwrap();
        let input_count = f.inputs().await.unwrap();
        assert_eq!(input_count, 3);
        let route = f.source.route(&reference.route).unwrap();
        let native = f.lifetime().selected_policy().native_host.as_ref().unwrap();
        let grants = f.source.owner.grants();
        let bounds = grants
            .resolve_lineage(
                &c.planning.lineage,
                &route.logical_executor,
                route.represented_subject.as_ref(),
            )
            .unwrap()
            .restrictions()
            .clone();
        let other_root = grants
            .issue_root(
                &c.planning.lineage[0].root_authority,
                EvidenceId::new("different-planning-root").unwrap(),
                native.grant_administrator.clone(),
                route.represented_subject.clone(),
                bounds,
            )
            .unwrap();
        assert!(other_root != c.planning.lineage[0]);
        let mut candidate = association.candidate().clone();
        let meerkat_authorization_contracts::work_association::WorkAuthorityBasis::GrantLineage {
            lineage,
        } = &mut candidate.authority_basis
        else {
            panic!("expected exact planning lineage")
        };
        lineage[0] = other_root;
        let changed =
            meerkat_authorization_contracts::work_association::InputAuthorityAssociation::new(
                candidate,
            )
            .unwrap();
        // Use the actual pre-binding source row to distinguish planning
        // validation from the shared replay-equality refusal on a bound row.
        c.planning
            .check_persistence_binding(&f.source, reference, &unbound, &association)
            .unwrap();
        assert!(matches!(
            c.planning
                .check_persistence_binding(&f.source, reference, &unbound, &changed),
            Err(OriginalWriterError::Conflict)
        ));
        assert!(matches!(
            f.source
                .persist_or_observe_workflow_binding(reference, &changed, &c.planning)
                .await,
            Err(OriginalWriterError::Conflict)
        ));
        assert_eq!(f.store.get(&key).await.unwrap().unwrap(), before);
        assert_eq!(f.inputs().await.unwrap(), input_count);
        assert_eq!(
            f.source.binding_evidence(reference).await.unwrap(),
            Some(association)
        );
    }))
    .catch_unwind()
    .await;
    let closed = f.shutdown().await;
    assert!(closed.is_ok(), "{closed:?}");
    match result {
        Ok(result) => result.expect("planning binding control completes within guard"),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn planning_binding_preserves_native_unavailability_and_denial() {
    use meerkat_authorization::grants::GrantRefusal;

    let f = FirstFixture::for_continuation().await;
    let result = AssertUnwindSafe(tokio::time::timeout(GUARD, async {
        let c = prepare(&f).await;
        let reference = &c.planning.submission.original;
        let key = f.source.key(reference).unwrap();
        let before = f.store.get(&key).await.unwrap().unwrap();
        let input_count = f.inputs().await.unwrap();
        let model_requests = f.model_requests();
        let route = f.source.route(&reference.route).unwrap();
        let grants = f.source.owner.grants();
        c.planning.check_for_binding(&f.source, reference).unwrap();
        c.planning
            .check_child_preparation(&f.source, reference)
            .unwrap();
        let controller = c.planning.parent.association().candidate();
        grants
            .resolve_controller_lineage(
                &controller.controller_grant_lineage,
                &controller.logical_executor,
                controller.represented_subject.as_ref(),
            )
            .map_err(restore::grant_error)
            .unwrap();

        // Invalidate the actual resolved grant before its final currency check.
        // Both entry points must discard it and reobserve the same live owner;
        // the standalone invocation path formerly returned Unavailable here.
        for child_preparation in [false, true] {
            let publication = f.source.owner.publication();
            let called = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let observed = called.clone();
            *c.planning.binding_observed_for_test.lock().unwrap() = Some(Box::new(move || {
                drop(publication.begin_owner_change().unwrap());
                observed.store(true, std::sync::atomic::Ordering::SeqCst);
            }));
            if child_preparation {
                c.planning
                    .check_child_preparation(&f.source, reference)
                    .unwrap();
            } else {
                c.planning.check_for_binding(&f.source, reference).unwrap();
            }
            assert!(called.load(std::sync::atomic::Ordering::SeqCst));
            assert!(
                c.planning
                    .binding_observed_for_test
                    .lock()
                    .unwrap()
                    .is_none()
            );
        }
        assert_eq!(f.store.get(&key).await.unwrap().unwrap(), before);
        assert_eq!(f.inputs().await.unwrap(), input_count);
        assert_eq!(f.model_requests(), model_requests);

        // The real native owner refuses an observation during publication.
        // No await or grant mutation occurs while this guard is held.
        let publication = f.source.owner.publication();
        let change = publication.begin_owner_change().unwrap();
        assert!(matches!(
            grants.resolve_lineage(
                &c.planning.lineage,
                &route.logical_executor,
                route.represented_subject.as_ref()
            ),
            Err(GrantRefusal::Unavailable)
        ));
        assert!(matches!(
            c.planning.check_for_binding(&f.source, reference),
            Err(PrivateOriginalError::Unavailable)
        ));
        // These are the two native reads used by interest restoration. Test
        // their shared projection here, not the full cold-restoration path.
        assert!(matches!(
            grants
                .resolve_lineage(
                    &c.planning.lineage,
                    &route.logical_executor,
                    route.represented_subject.as_ref()
                )
                .map_err(restore::grant_error),
            Err(PrivateOriginalError::Unavailable)
        ));
        assert!(matches!(
            grants
                .resolve_controller_lineage(
                    &controller.controller_grant_lineage,
                    &controller.logical_executor,
                    controller.represented_subject.as_ref()
                )
                .map_err(restore::grant_error),
            Err(PrivateOriginalError::Unavailable)
        ));
        // The child command preserves persistent Changed after its bounded
        // reread. The standalone wrapper above still reports Unavailable.
        assert!(matches!(
            c.planning.check_child_preparation(&f.source, reference),
            Err(WorkflowChildPreparationError::ObservationChanged)
        ));
        drop(change);
        // A held publication can refuse before any prerequisite is read. Check
        // foreign-source denial only in a coherent observation.
        let mut foreign = reference.clone();
        foreign.original = EvidenceId::new("different-planning-source").unwrap();
        assert!(matches!(
            c.planning.check_for_binding(&f.source, &foreign),
            Err(PrivateOriginalError::Denied)
        ));
        assert!(matches!(
            c.planning.check_child_preparation(&f.source, &foreign),
            Err(WorkflowChildPreparationError::Refused(
                PrivateOriginalError::Denied
            ))
        ));
        c.planning.check_for_binding(&f.source, reference).unwrap();
        c.planning
            .check_child_preparation(&f.source, reference)
            .unwrap();
        grants
            .resolve_lineage(
                &c.planning.lineage,
                &route.logical_executor,
                route.represented_subject.as_ref(),
            )
            .map_err(restore::grant_error)
            .unwrap();
        grants
            .resolve_controller_lineage(
                &controller.controller_grant_lineage,
                &controller.logical_executor,
                controller.represented_subject.as_ref(),
            )
            .map_err(restore::grant_error)
            .unwrap();
        // The actual finite operation envelope cannot become a controller
        // lineage, whose lifetime must be unrestricted.
        assert!(matches!(
            grants
                .resolve_controller_lineage(
                    &c.planning.lineage,
                    &route.logical_executor,
                    route.represented_subject.as_ref()
                )
                .map_err(restore::grant_error),
            Err(PrivateOriginalError::Denied)
        ));

        // Revoke the actual selected G leaf after the standalone path resolved
        // it. The fresh read must return Denied, never the old successful facts.
        let native = f.lifetime().selected_policy().native_host.as_ref().unwrap();
        let administrator = native.grant_administrator.clone();
        let lineage = c.planning.lineage[1].clone();
        let revoke_grants = grants.clone();
        let machine = f.machine.clone();
        *c.planning.binding_observed_for_test.lock().unwrap() = Some(Box::new(move || {
            let mut custody = machine.try_controller_grant_mutation().unwrap();
            revoke_grants
                .revoke(&administrator, &lineage, &mut custody)
                .unwrap();
        }));
        assert!(matches!(
            c.planning.check_for_binding(&f.source, reference),
            Err(PrivateOriginalError::Denied)
        ));
        assert!(
            c.planning
                .binding_observed_for_test
                .lock()
                .unwrap()
                .is_none()
        );
        assert!(matches!(
            grants.resolve_lineage(
                &c.planning.lineage,
                &route.logical_executor,
                route.represented_subject.as_ref()
            ),
            Err(GrantRefusal::Denied)
        ));
        assert!(matches!(
            c.planning.check_for_binding(&f.source, reference),
            Err(PrivateOriginalError::Denied)
        ));
        assert!(matches!(
            grants
                .resolve_lineage(
                    &c.planning.lineage,
                    &route.logical_executor,
                    route.represented_subject.as_ref()
                )
                .map_err(restore::grant_error),
            Err(PrivateOriginalError::Denied)
        ));
        assert!(matches!(
            c.planning.check_child_preparation(&f.source, reference),
            Err(WorkflowChildPreparationError::Refused(
                PrivateOriginalError::Denied
            ))
        ));
        assert_eq!(f.store.get(&key).await.unwrap().unwrap(), before);
        assert_eq!(f.inputs().await.unwrap(), input_count);
        assert_eq!(f.model_requests(), model_requests);
        assert!(
            f.source
                .binding_evidence(reference)
                .await
                .unwrap()
                .is_none()
        );
    }))
    .catch_unwind()
    .await;
    let closed = f.shutdown().await;
    assert!(closed.is_ok(), "{closed:?}");
    match result {
        Ok(result) => result.expect("native grant projection control completes within guard"),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

// Controlled owner-read unit tests use real publication changes. They do not
// claim native route or grant evidence; the FirstFixture control above does.
#[test]
fn observed_child_read_preserves_inner_unavailable_after_owner_change() {
    use WorkflowChildPreparationError::Refused;
    use meerkat_authorization::publication::LocalAuthorizationPublication;
    let publication = LocalAuthorizationPublication::new();
    let mut reads = 0;
    let result = observe_child_preparation(&publication, |_| {
        reads += 1;
        let change = publication.begin_owner_change().unwrap();
        drop(change);
        Err(Refused(PrivateOriginalError::Unavailable))
    });
    assert_eq!(result, Err(Refused(PrivateOriginalError::Unavailable)));
    assert_eq!(reads, 1, "genuine unavailability must not rebuild the read");
}

#[test]
fn observed_child_read_reselects_prerequisites_after_owner_change() {
    use WorkflowChildPreparationError::Refused;
    use meerkat_authorization::publication::LocalAuthorizationPublication;
    let publication = LocalAuthorizationPublication::new();
    let mut route = "before";
    let mut selections = Vec::new();
    let result = observe_child_preparation(&publication, |_| {
        let selected_route = route;
        selections.push(selected_route);
        if selections.len() == 1 {
            // Publish between the controlled prerequisite read and its use.
            let change = publication.begin_owner_change().unwrap();
            route = "after";
            drop(change);
        }
        if selected_route != route {
            return Err(Refused(PrivateOriginalError::Denied));
        }
        Ok(())
    });
    assert_eq!(result, Ok(()));
    assert_eq!(selections, ["before", "after"]);
}

#[test]
fn observed_child_read_discards_other_stale_results_and_bounds_rebuild() {
    use WorkflowChildPreparationError::{ObservationChanged, Refused};
    use meerkat_authorization::publication::LocalAuthorizationPublication;
    for stale_result in [
        Ok(()),
        Err(Refused(PrivateOriginalError::Denied)),
        Err(Refused(PrivateOriginalError::Conflict)),
    ] {
        let publication = LocalAuthorizationPublication::new();
        let mut reads = 0;
        let result = observe_child_preparation(&publication, |_| {
            reads += 1;
            if reads == 1 {
                let change = publication.begin_owner_change().unwrap();
                drop(change);
                return stale_result;
            }
            Err(Refused(PrivateOriginalError::Denied))
        });
        assert_eq!(result, Err(Refused(PrivateOriginalError::Denied)));
        assert_eq!(reads, 2, "only the second coherent result is authoritative");
    }
    let publication = LocalAuthorizationPublication::new();
    let mut reads = 0;
    let result = observe_child_preparation(&publication, |_| {
        reads += 1;
        let change = publication.begin_owner_change().unwrap();
        drop(change);
        Ok(())
    });
    assert_eq!(result, Err(ObservationChanged));
    assert_eq!(reads, 2, "persistent change is stale, not unavailable");
}

#[path = "member_tests.rs"]
mod member_tests;

#[path = "lifecycle_tests.rs"]
mod lifecycle_tests;

#[path = "bind_tests.rs"]
mod bind_tests;

#[path = "idle_restore_tests.rs"]
mod idle_restore_tests;

#[path = "waiter_restore_tests.rs"]
pub(super) mod waiter_restore_tests;

#[path = "interest_restore_tests.rs"]
mod interest_restore_tests;

#[path = "recovery_tests.rs"]
mod recovery_tests;

#[path = "effect_restore_tests.rs"]
mod effect_restore_tests;

// Exercise the same mapping used by both child work-read boundaries with the
// existing real SQLite graph. This is not a full late-child command fixture.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn child_work_read_preserves_backing_store_failure_and_authority_denial() {
    use meerkat_toolkit::setup::SetupError;
    use meerkat_workgraph::{CloseWorkItemRequest, WorkGraphError, WorkStatus};

    let f = FirstFixture::for_continuation().await;
    let path = f.source_path.with_extension("planning-work.sqlite");
    let retained = f
        .source_path
        .with_extension("planning-work-retained.sqlite");
    let result = AssertUnwindSafe(tokio::time::timeout(GUARD, async {
        let c = prepare(&f).await;
        c.work
            .check_current(&c._backend)
            .await
            .map_err(child::work_error)
            .unwrap();

        // Core's ordinary open never recreates missing backing state. Retain
        // and restore the exact real database rather than replacing its store.
        std::fs::rename(&path, &retained).unwrap();
        let retained_bytes = std::fs::read(&retained).unwrap();
        let read = c.work.check_current(&c._backend).await;
        assert!(matches!(&read, Err(SetupError::WorkGraph(
            WorkGraphError::BackingStoreUnavailable { backend, .. })) if backend == "sqlite"));
        assert_eq!(
            read.map_err(child::work_error),
            Err(PrivateOriginalError::Unavailable)
        );
        assert!(
            !path.exists(),
            "failed currentness read must not recreate the database"
        );
        assert_eq!(std::fs::read(&retained).unwrap(), retained_bytes);

        std::fs::rename(&retained, &path).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), retained_bytes);
        c.work
            .check_current(&c._backend)
            .await
            .map_err(child::work_error)
            .unwrap();
        let wrong_owner = SetupWorkContext::from_owner(
            c._backend.pin().clone(),
            c.work.parent().clone(),
            c.work.child().clone(),
            c.planning.terms.parent_owner.clone(),
            WorkOwnerKey::agent("not-the-selected-child-owner").unwrap(),
            CYCLE.into(),
        )
        .unwrap();
        assert_eq!(
            wrong_owner
                .check_current(&c._backend)
                .await
                .map_err(child::work_error),
            Err(PrivateOriginalError::Denied)
        );
        let child = c
            ._backend
            .service()
            .get(None, None, c.work.child().item_id.clone())
            .await
            .unwrap();
        c._backend
            .service()
            .close(CloseWorkItemRequest {
                id: child.id,
                realm_id: None,
                namespace: None,
                expected_revision: child.revision,
                status: WorkStatus::Completed,
            })
            .await
            .unwrap();
        assert_eq!(
            c.work
                .check_current(&c._backend)
                .await
                .map_err(child::work_error),
            Err(PrivateOriginalError::Denied)
        );
    }))
    .catch_unwind()
    .await;
    // Restore renamed backing state even if an assertion or deadline failed,
    // then join the fixture's actual native/host cleanup as usual.
    let restored = if retained.exists() {
        std::fs::rename(&retained, &path)
    } else {
        Ok(())
    };
    let closed = f.shutdown().await;
    assert!(restored.is_ok(), "{restored:?}");
    assert!(closed.is_ok(), "{closed:?}");
    match result {
        Ok(result) => result.expect("bounded real WorkGraph projection control"),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

#[path = "parent_authority_tests.rs"]
mod parent_authority_tests;

#[path = "observation_wait_tests.rs"]
mod observation_wait_tests;

#[path = "admission_refusal_tests.rs"]
mod admission_refusal_tests;

#[path = "post_commit_wait_contract_tests.rs"]
mod post_commit_wait_contract_tests;
