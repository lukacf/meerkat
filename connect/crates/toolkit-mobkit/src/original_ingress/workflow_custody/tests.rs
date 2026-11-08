//! Native grant/source-owner controls only. These do not launch a lead, enter
//! a native effect, deliver context, or establish installed SDK behavior.
use super::*;
use crate::original_ingress::tests::{claims, initialized, machine, principal, source};
use futures::FutureExt;
use meerkat_authorization_contracts::constraints::DomainRestriction;
use meerkat_core::OperationExecutionScope;
use meerkat_core::authorization::{
    AuthorizationOperation, OperationAuthorizationFacts, ToolAuthorizationFacts,
    ToolAuthorizationTarget,
};
use std::os::unix::fs::PermissionsExt;
use toolkit_store::{AdmissionRequest, RecordData, StoreKey, ToolkitStore};

fn id(value: &str) -> EvidenceId {
    EvidenceId::new(value).unwrap()
}
fn target() -> GrantTarget {
    GrantTarget::RegisteredToolBundleOperation {
        bundle: "workflow-owner-fixture".into(),
        operation: "prepare".into(),
    }
}
struct Fixture {
    _directory: tempfile::TempDir,
    owner: Arc<ToolkitOriginalOwner>,
    g: Arc<RetainedToolkitOriginal>,
    a: Arc<RetainedToolkitOriginal>,
    b: Arc<RetainedToolkitOriginal>,
    a_child: Vec<GrantLineageRef>,
    b_child: Vec<GrantLineageRef>,
}
impl Fixture {
    fn new() -> Self {
        Self::with_source_custody(true, true).0
    }
    fn with_source_custody(
        admit_g: bool,
        admit_a: bool,
    ) -> (
        Self,
        Option<PendingToolkitOriginal>,
        Option<PendingToolkitOriginal>,
    ) {
        let directory = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let owner = initialized(&directory.path().join("grants"));
        let root = owner
            .grants
            .issue_root(
                &meerkat_core::AdmissionGuard::none(),
                &principal("admin"),
                id("base"),
                principal("admin"),
                None,
                ExecutionRestrictions::unrestricted(),
            )
            .unwrap();
        let make = |label: &str, executor: &str, admit: bool| {
            let mut candidate = claims(label, &root).candidate().clone();
            candidate.logical_executor = principal(executor);
            candidate.target.logical_owner = principal(executor);
            candidate.target.audience =
                meerkat_authorization_contracts::constraints::AudienceRef::Principal {
                    principal: principal(&format!("{label}-audience")),
                };
            let association = InputAuthorityAssociation::new(candidate).unwrap();
            let original = owner
                .enroll_new_authenticated(
                    association.clone(),
                    source(&association),
                    &principal("user"),
                    &principal("frontend"),
                    &principal("admin"),
                    id(&format!("{label}-original")),
                    ExecutionRestrictions::unrestricted(),
                    |_| Ok(None),
                )
                .unwrap();
            // Explicit native source-owner acknowledgment, not runtime input
            // admission. The existing owner/grant mechanisms are real here.
            let pending = owner.record_candidate(&original).unwrap();
            let pending = if admit {
                pending.admit().unwrap();
                None
            } else {
                Some(pending)
            };
            (original, pending)
        };
        let (g, pending_g) = make("g", "planning", admit_g);
        let (a, pending_a) = make("a", "waiter-a", admit_a);
        let (b, _) = make("b", "waiter-b", true);
        let delegate = |original: &Arc<RetainedToolkitOriginal>, label: &str| {
            let parent = original
                .association
                .candidate()
                .authority_basis
                .lineage_for(Some(&target()))
                .unwrap();
            let child = owner
                .grants
                .issue_child(
                    &meerkat_core::AdmissionGuard::none(),
                    &original.association.candidate().logical_executor,
                    parent.last().unwrap(),
                    id(label),
                    principal("planning"),
                    ExecutionRestrictions::unrestricted(),
                )
                .unwrap();
            let mut lineage = parent.to_vec();
            lineage.push(child);
            lineage
        };
        let a_child = delegate(&a, "a-to-g");
        let b_child = delegate(&b, "b-to-g");
        (
            Self {
                _directory: directory,
                owner,
                g,
                a,
                b,
                a_child,
                b_child,
            },
            pending_g,
            pending_a,
        )
    }
    fn relationship(
        &self,
        original: &Arc<RetainedToolkitOriginal>,
        child: Vec<GrantLineageRef>,
    ) -> Result<Arc<WorkflowWaiterCustody>, OriginalWriterError> {
        let _writer = self.owner.mutation.try_lock().unwrap();
        self.owner.workflow_waiter_custody(
            self.g.clone(),
            original.clone(),
            Id::new(original.association.candidate().original_work.work.as_str()).unwrap(),
            target(),
            child,
            u64::MAX,
        )
    }
    fn staged_relationship(
        &self,
        original: &Arc<RetainedToolkitOriginal>,
        child: Vec<GrantLineageRef>,
    ) -> Result<Arc<WorkflowWaiterCustody>, OriginalWriterError> {
        let _writer = self.owner.mutation.try_lock().unwrap();
        self.owner.stage_workflow_waiter_custody(
            self.g.clone(),
            original.clone(),
            Id::new(original.association.candidate().original_work.work.as_str()).unwrap(),
            target(),
            child,
            u64::MAX,
        )
    }
    fn child_naming_g(&self) -> Arc<RetainedToolkitOriginal> {
        let root = &self
            .g
            .association
            .candidate()
            .authority_basis
            .lineage_for(Some(&target()))
            .unwrap()[0];
        let mut candidate = claims("g-named-child", root).candidate().clone();
        candidate.contributing_work = vec![self.g.association.candidate().original_work.clone()];
        let association = InputAuthorityAssociation::new(candidate).unwrap();
        self.owner
            .enroll_new_authenticated(
                association.clone(),
                source(&association),
                &principal("user"),
                &principal("frontend"),
                &principal("admin"),
                id("g-named-child-original"),
                ExecutionRestrictions::unrestricted(),
                |_| Ok(None),
            )
            .unwrap()
    }
}
struct Dispatcher;
#[async_trait::async_trait]
impl meerkat_core::AgentToolDispatcher for Dispatcher {
    fn tools(&self) -> Arc<[Arc<meerkat_core::ToolDef>]> {
        vec![Arc::new(meerkat_core::ToolDef {
            name: "prepare".into(),
            description: "inert workflow owner fixture".into(),
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
        Err(meerkat_core::ToolError::unavailable(
            call.name,
            meerkat_core::ToolUnavailableReason::ExecutionOwnerChanged,
        ))
    }
}
struct Call {
    facts: ToolAuthorizationFacts,
}
impl Call {
    fn new(id: &str, arguments: &str) -> Self {
        let args: Arc<serde_json::value::RawValue> =
            serde_json::value::RawValue::from_string(arguments.into())
                .unwrap()
                .into();
        let root: Arc<dyn meerkat_core::AgentToolDispatcher> =
            Arc::new(meerkat_core::RegisteredToolBundleDispatcher::new(
                "workflow-owner-fixture",
                Arc::new(Dispatcher),
            ));
        let plan = meerkat_core::resolve_tool_execution_plan_fenced(
            &root,
            ToolCallView {
                id,
                name: "prepare",
                args: &args,
            },
            &meerkat_core::ToolDispatchContext::default(),
            &meerkat_core::ToolExecutionResolutionContext::new(
                meerkat_core::ToolDeadlineChain::new(vec![
                    meerkat_core::ToolDeadlineContributor::finite(
                        meerkat_core::ToolDeadlineOwner::CoreToolDispatch,
                        std::time::Duration::from_secs(60),
                    ),
                ])
                .unwrap(),
            ),
        )
        .unwrap();
        Self {
            facts: ToolAuthorizationFacts {
                call_id: Arc::from(id),
                name: "prepare".into(),
                arguments: args,
                target: ToolAuthorizationTarget::Dispatcher(Arc::new(plan)),
            },
        }
    }
    fn binding(&self) -> PreparedAuthorizationBinding {
        let uuid = |value: &str| format!("\"00000000-0000-0000-0000-{value}\"");
        PreparedAuthorizationBinding::new(OperationAuthorizationFacts {
            operation_id: serde_json::from_str(&uuid("000000000005")).unwrap(),
            execution_scope: OperationExecutionScope::RuntimeInput {
                owner_session_id: meerkat_core::SessionId::new(),
                runtime_epoch_id: serde_json::from_str(&uuid("000000000002")).unwrap(),
                submitted_input_id: serde_json::from_str(&uuid("000000000003")).unwrap(),
                canonical_input_id: serde_json::from_str(&uuid("000000000003")).unwrap(),
            },
            run_id: None,
            context_revision: None,
            operation: AuthorizationOperation::Tool(self.facts.clone()),
        })
    }
}

#[test]
fn candidate_g_uses_exact_waiter_custody_while_named_source_remains_pending() {
    let (f, pending_g, _) = Fixture::with_source_custody(false, true);
    let pending_g = pending_g.unwrap();
    assert!(pending_g.is_first_candidate());
    f.owner.workflow_own_source_current(&f.g).unwrap();
    let relationship = f.staged_relationship(&f.a, f.a_child.clone()).unwrap();
    let call = Call::new("candidate-g-own-call", "{}");
    let staged = relationship.stage_call(call.facts.call()).unwrap();
    let binding = call.binding();
    assert!(Arc::ptr_eq(
        &relationship
            .prepared_call(f.g.association(), &binding)
            .unwrap()
            .unwrap(),
        &staged,
    ));
    staged.current().unwrap().check_current().unwrap();
    staged.enter_source_gate().unwrap();

    // Own-operation custody does not promote G or make it a named source for
    // another input. These are real source-owner guards, not runtime receipts.
    assert!(matches!(
        f.owner.workflow_source_current(&f.g),
        Err(OriginalWriterError::Unavailable)
    ));
    assert!(matches!(
        f.owner.enter_workflow_launch(&f.g),
        Err(OriginalWriterError::Unavailable)
    ));
    let child = f.child_naming_g();
    assert!(matches!(
        f.owner.record_candidate(&child),
        Err(OriginalWriterError::Conflict)
    ));
    let claims = f.g.association.candidate();
    // No runtime submission/handoff exists in this owner fixture, so the
    // native retained-source callback truthfully reports NotRetained, not a
    // fabricated AdmissionPending receipt.
    assert!(matches!(
        f.owner
            .retained_custody(&claims.ingress_namespace, &claims.original_work),
        Ok(RetainedCustody::NotRetained)
    ));
    assert!(
        f.owner
            .admitted_original_basis(&claims.ingress_namespace, &claims.original_work)
            .unwrap()
            .is_none()
    );

    let promoted = pending_g.admit().unwrap();
    assert!(Arc::ptr_eq(&promoted, &f.g));
    f.owner.workflow_source_current(&f.g).unwrap();
    let launch = f.owner.enter_workflow_launch(&f.g).unwrap();
    launch.settle().unwrap();
    drop(f.owner.record_candidate(&child).unwrap());
    assert!(
        f.owner
            .admitted_original_basis(&claims.ingress_namespace, &claims.original_work)
            .unwrap()
            .is_some()
    );
    assert!(matches!(
        f.owner
            .retained_custody(&claims.ingress_namespace, &claims.original_work),
        Ok(RetainedCustody::Retained(_))
    ));
    // Promotion does not replace the operation, source or exact target child.
    assert!(Arc::ptr_eq(
        &relationship
            .prepared_call(f.g.association(), &binding)
            .unwrap()
            .unwrap(),
        &staged,
    ));
    assert_eq!(relationship.delegation, f.a_child);
    staged.settle().unwrap();
}

#[test]
fn candidate_waiter_cannot_lend_authority_to_candidate_g() {
    let (f, _pending_g, pending_a) = Fixture::with_source_custody(false, false);
    assert!(matches!(
        f.staged_relationship(&f.a, f.a_child.clone()),
        Err(OriginalWriterError::Unavailable)
    ));
    assert!(matches!(
        f.owner.workflow_source_current(&f.a),
        Err(OriginalWriterError::Unavailable)
    ));
    pending_a.unwrap().admit().unwrap();
    f.owner.workflow_source_current(&f.a).unwrap();
    assert!(matches!(
        f.owner.workflow_source_current(&f.g),
        Err(OriginalWriterError::Unavailable)
    ));
    assert!(matches!(
        f.staged_relationship(&f.a, f.b_child.clone()),
        Err(OriginalWriterError::WrongOwner)
    ));
    let relationship = f.staged_relationship(&f.a, f.a_child.clone()).unwrap();
    let call = Call::new("now-admitted-a", "{}");
    let staged = relationship.stage_call(call.facts.call()).unwrap();
    relationship
        .prepared_call(f.g.association(), &call.binding())
        .unwrap();
    staged.enter_source_gate().unwrap();
    staged.settle().unwrap();
}

#[test]
fn withdrawing_candidate_g_invalidates_prepared_own_operation_without_promoting_it() {
    let (f, pending_g, _) = Fixture::with_source_custody(false, true);
    let relationship = f.staged_relationship(&f.a, f.a_child.clone()).unwrap();
    let call = Call::new("withdrawn-g", "{}");
    let staged = relationship.stage_call(call.facts.call()).unwrap();
    let binding = call.binding();
    relationship
        .prepared_call(f.g.association(), &binding)
        .unwrap();
    let (_, stamp) = f.owner.publication.observe(|| ()).unwrap();
    drop(pending_g);
    assert!(stamp.check_current().is_err());
    assert!(matches!(
        staged.current(),
        Err(OriginalWriterError::Unavailable)
    ));
    assert!(staged.enter_source_gate().is_err());
    assert!(
        relationship
            .prepared_call(f.g.association(), &binding)
            .is_err()
    );
    assert!(
        relationship
            .stage_call(Call::new("late-withdrawn-g", "{}").facts.call())
            .is_err()
    );
    assert!(matches!(
        f.owner.workflow_source_current(&f.g),
        Err(OriginalWriterError::Unavailable)
    ));
    f.owner.workflow_source_current(&f.a).unwrap();
    staged.settle().unwrap();
}

#[tokio::test]
async fn cancelling_candidate_g_fences_own_operation_before_durable_settlement() {
    let (f, _pending_g, _) = Fixture::with_source_custody(false, true);
    let relationship = f.staged_relationship(&f.a, f.a_child.clone()).unwrap();
    let call = Call::new("cancelled-g", "{}");
    let staged = relationship.stage_call(call.facts.call()).unwrap();
    let binding = call.binding();
    relationship
        .prepared_call(f.g.association(), &binding)
        .unwrap();
    let store =
        toolkit_store::SqliteStore::open(f._directory.path().join("cancel.sqlite")).unwrap();
    let source = store
        .admit(AdmissionRequest {
            key: StoreKey {
                realm: "test-realm".into(),
                owner: "workflow-source".into(),
                id: "g".into(),
            },
            admission_key: "g".into(),
            data: RecordData {
                schema: "test.source".into(),
                version: 1,
                body: serde_json::json!({}),
            },
            events: vec![],
            effects: vec![],
        })
        .await
        .unwrap()
        .record()
        .clone();
    let claims = f.g.association.candidate();
    let expected = f.g.association.clone();
    let (release, released) = tokio::sync::oneshot::channel();
    let cancellation = f.owner.record_operation_cancelled(
        &claims.ingress_namespace.realm,
        &claims.original_work,
        move |binding, prior| async move {
            assert_eq!(binding.as_ref(), Some(&expected));
            assert!(prior.is_none());
            Ok(Arc::new(source))
        },
        move |_| async move {
            released.await.unwrap();
            Ok(Arc::new(
                store
                    .admit(AdmissionRequest {
                        key: StoreKey {
                            realm: "test-realm".into(),
                            owner: "workflow-source".into(),
                            id: "g-cancelled".into(),
                        },
                        admission_key: "g-cancelled".into(),
                        data: RecordData {
                            schema: "test.cancelled".into(),
                            version: 1,
                            body: serde_json::json!({}),
                        },
                        events: vec![],
                        effects: vec![],
                    })
                    .await
                    .unwrap()
                    .record()
                    .clone(),
            ))
        },
    );
    tokio::pin!(cancellation);
    assert!(cancellation.as_mut().now_or_never().is_none());
    assert!(matches!(
        staged.current(),
        Err(OriginalWriterError::Unavailable)
    ));
    assert!(staged.enter_source_gate().is_err());
    release.send(()).unwrap();
    cancellation.await.unwrap();
    assert!(matches!(
        staged.current(),
        Err(OriginalWriterError::OperationCancelled)
    ));
    assert!(staged.enter_source_gate().is_err());
    assert!(
        relationship
            .prepared_call(f.g.association(), &binding)
            .is_err()
    );
    assert!(
        relationship
            .stage_call(Call::new("late-cancelled-g", "{}").facts.call())
            .is_err()
    );
    f.owner.workflow_source_current(&f.a).unwrap();
    staged.settle().unwrap();
}

#[test]
fn departed_waiter_cannot_reenter_candidate_g_while_other_waiter_remains_live() {
    let (f, _pending_g, _) = Fixture::with_source_custody(false, true);
    let a = f.staged_relationship(&f.a, f.a_child.clone()).unwrap();
    let b = f.staged_relationship(&f.b, f.b_child.clone()).unwrap();
    let ca = Call::new("candidate-departed-a", "{}");
    let cb = Call::new("candidate-live-b", "{}");
    let aa = a.stage_call(ca.facts.call()).unwrap();
    let bb = b.stage_call(cb.facts.call()).unwrap();
    a.prepared_call(f.g.association(), &ca.binding()).unwrap();
    b.prepared_call(f.g.association(), &cb.binding()).unwrap();
    {
        let _writer = f.owner.mutation.try_lock().unwrap();
        f.owner
            .retire_workflow_effects_under_writer(vec![a.clone()])
            .unwrap();
    }
    assert!(aa.current().is_err());
    assert!(aa.enter_source_gate().is_err());
    assert!(
        a.stage_call(Call::new("candidate-late-a", "{}").facts.call())
            .is_err()
    );
    bb.current().unwrap().check_current().unwrap();
    bb.enter_source_gate().unwrap();
    f.owner.workflow_source_current(&f.a).unwrap();
    f.owner.workflow_source_current(&f.b).unwrap();
    assert!(matches!(
        f.owner.workflow_source_current(&f.g),
        Err(OriginalWriterError::Unavailable)
    ));
    aa.settle().unwrap();
    bb.settle().unwrap();
}

#[test]
fn original_grant_wrong_executor_and_foreign_source_cannot_become_a_to_g_delegation() {
    let f = Fixture::new();
    let original =
        f.a.association
            .candidate()
            .authority_basis
            .lineage_for(Some(&target()))
            .unwrap()
            .to_vec();
    assert!(f.relationship(&f.a, original).is_err());
    assert!(f.relationship(&f.a, f.b_child.clone()).is_err());
    let foreign = Fixture::new();
    assert!(f.relationship(&foreign.a, foreign.a_child.clone()).is_err());
    let parent =
        f.a.association
            .candidate()
            .authority_basis
            .lineage_for(Some(&target()))
            .unwrap();
    let other = f
        .owner
        .grants
        .issue_child(
            &meerkat_core::AdmissionGuard::none(),
            &principal("waiter-a"),
            parent.last().unwrap(),
            id("a-other"),
            principal("other"),
            ExecutionRestrictions::unrestricted(),
        )
        .unwrap();
    let mut wrong = parent.to_vec();
    wrong.push(other);
    assert!(f.relationship(&f.a, wrong).is_err());
    assert_ne!(
        f.g.association.candidate().target.audience,
        f.a.association.candidate().target.audience
    );
    assert!(f.relationship(&f.a, f.a_child.clone()).is_ok());
    assert!(
        f.owner.workflow_controller(&f.g).is_err(),
        "fixture has no actual native controller"
    );
}

#[test]
fn staged_call_retains_operation_binding_and_refuses_equal_fact_reconstruction() {
    let f = Fixture::new();
    let relationship = f.relationship(&f.a, f.a_child.clone()).unwrap();
    let call = Call::new("a-call", "{\"waiter\":\"a\"}");
    let (_, before) = f.owner.publication.observe(|| ()).unwrap();
    let staged = relationship.stage_call(call.facts.call()).unwrap();
    assert!(before.check_current().is_err());
    let binding = call.binding();
    let (_, passive_stamp) = f.owner.publication.observe(|| ()).unwrap();
    assert!(matches!(
        relationship.review_prepared_call(f.g.association(), &binding),
        Err(OriginalWriterError::Unavailable)
    ));
    assert!(
        staged.binding.get().is_none(),
        "review must not prepare a binding"
    );
    passive_stamp.check_current().unwrap();
    let prepared = relationship
        .prepared_call(f.g.association(), &binding)
        .unwrap()
        .unwrap();
    assert!(Arc::ptr_eq(&prepared, &staged));
    let (_, material_stamp) = f.owner.publication.observe(|| ()).unwrap();
    assert!(Arc::ptr_eq(
        &relationship
            .review_prepared_call(f.g.association(), &binding)
            .unwrap()
            .unwrap(),
        &staged
    ));
    material_stamp.check_current().unwrap();
    assert!(
        relationship
            .prepared_call(f.g.association(), &binding.clone())
            .unwrap()
            .is_some()
    );
    let reconstructed = PreparedAuthorizationBinding::new(binding.facts().clone());
    assert!(
        relationship
            .review_prepared_call(f.g.association(), &reconstructed)
            .is_err()
    );
    material_stamp.check_current().unwrap();
    assert!(
        relationship
            .prepared_call(f.g.association(), &reconstructed)
            .is_err()
    );
    let (_, replay_stamp) = f.owner.publication.observe(|| ()).unwrap();
    let replay = relationship.stage_call(call.facts.call()).unwrap();
    assert!(Arc::ptr_eq(&replay, &staged));
    assert!(replay.binding.get().unwrap().same_operation(&binding));
    replay_stamp.check_current().unwrap();
    let changed = Call::new("a-call", "{\"waiter\":\"b\"}");
    assert!(matches!(
        relationship.stage_call(changed.facts.call()),
        Err(OriginalWriterError::Conflict)
    ));
    replay_stamp.check_current().unwrap();
    assert!(
        relationship
            .prepared_call(f.a.association(), &binding)
            .is_err()
    );
    staged.settle().unwrap();
    assert!(staged.current().is_err());
    let entries = f.owner.shared_entries.lock().unwrap().len();
    let settled_replay = relationship.stage_call(call.facts.call()).unwrap();
    assert!(Arc::ptr_eq(&settled_replay, &staged));
    assert!(settled_replay.entry.settled());
    assert!(settled_replay.enter_source_gate().is_err());
    assert_eq!(f.owner.shared_entries.lock().unwrap().len(), entries);
    replay_stamp.check_current().unwrap();
}

#[tokio::test]
async fn unrelated_cancellation_writer_preserves_staging_and_refuses_cancelled_waiter() {
    let f = Fixture::new();
    let a = f.relationship(&f.a, f.a_child.clone()).unwrap();
    let b = f.relationship(&f.b, f.b_child.clone()).unwrap();
    let ca = Call::new("live-a-during-b-cancellation", "{}");
    let cb = Call::new("cancelled-b", "{}");
    let late_b = Call::new("late-b", "{}");
    let bb = b.stage_call(cb.facts.call()).unwrap();
    b.prepared_call(f.g.association(), &cb.binding()).unwrap();
    b.current_grant().unwrap();
    assert!(b.replay_staged_call(late_b.facts.call()).unwrap().is_none());
    let store =
        toolkit_store::SqliteStore::open(f._directory.path().join("cancel-b.sqlite")).unwrap();
    let source = store
        .admit(AdmissionRequest {
            key: StoreKey {
                realm: "test-realm".into(),
                owner: "workflow-source".into(),
                id: "b".into(),
            },
            admission_key: "b".into(),
            data: RecordData {
                schema: "test.source".into(),
                version: 1,
                body: serde_json::json!({}),
            },
            events: vec![],
            effects: vec![],
        })
        .await
        .unwrap()
        .record()
        .clone();
    let claims = f.b.association.candidate();
    let expected = f.b.association.clone();
    let (release, released) = tokio::sync::oneshot::channel();
    let cancellation = f.owner.record_operation_cancelled(
        &claims.ingress_namespace.realm,
        &claims.original_work,
        move |binding, prior| async move {
            assert_eq!(binding.as_ref(), Some(&expected));
            assert!(prior.is_none());
            Ok(Arc::new(source))
        },
        move |_| async move {
            released.await.unwrap();
            Ok(Arc::new(
                store
                    .admit(AdmissionRequest {
                        key: StoreKey {
                            realm: "test-realm".into(),
                            owner: "workflow-source".into(),
                            id: "b-cancelled".into(),
                        },
                        admission_key: "b-cancelled".into(),
                        data: RecordData {
                            schema: "test.cancelled".into(),
                            version: 1,
                            body: serde_json::json!({}),
                        },
                        events: vec![],
                        effects: vec![],
                    })
                    .await
                    .unwrap()
                    .record()
                    .clone(),
            ))
        },
    );
    tokio::pin!(cancellation);
    assert!(cancellation.as_mut().now_or_never().is_none());
    // The real cancellation has published Pending and closed B's entries. Its
    // durable callback still holds the application writer, without publication.
    assert!(f.owner.try_source_writer().is_err());
    assert!(matches!(
        b.current_grant(),
        Err(OriginalWriterError::Unavailable)
    ));
    assert!(bb.entry.check_open().is_err());
    let entries = f.owner.shared_entries.lock().unwrap().len();
    let aa = a.stage_call(ca.facts.call()).unwrap();
    assert_eq!(f.owner.shared_entries.lock().unwrap().len(), entries + 1);
    let (_, replay_stamp) = f.owner.publication.observe(|| ()).unwrap();
    assert!(Arc::ptr_eq(&a.stage_call(ca.facts.call()).unwrap(), &aa));
    replay_stamp.check_current().unwrap();
    assert!(f.owner.try_source_writer().is_err());
    assert!(matches!(
        b.stage_call(cb.facts.call()),
        Err(OriginalWriterError::Unavailable)
    ));
    assert!(matches!(
        b.stage_call(late_b.facts.call()),
        Err(OriginalWriterError::Unavailable)
    ));
    assert!(matches!(
        b.stage_current_call(late_b.facts.call()),
        Err(OriginalWriterError::Unavailable)
    ));
    assert!(bb.enter_source_gate().is_err());
    assert_eq!(f.owner.shared_entries.lock().unwrap().len(), entries + 1);
    a.prepared_call(f.g.association(), &ca.binding()).unwrap();
    aa.enter_source_gate().unwrap();
    // B never entered. Settle that exact unused source reservation so the real
    // cancellation can join its drain; A's entered reservation remains owned.
    assert!(bb.settle_unentered().unwrap());
    release.send(()).unwrap();
    cancellation.await.unwrap();
    assert!(matches!(
        b.current_grant(),
        Err(OriginalWriterError::OperationCancelled)
    ));
    a.current_grant().unwrap().check_current().unwrap();
    assert!(!aa.entry.settled());
    aa.settle().unwrap();
}

#[tokio::test]
async fn queued_followup_rechecks_cancellation_and_dropped_waiter_never_publishes() {
    for mode in ["unrelated", "own", "dropped"] {
        let f = Fixture::new();
        let cancelled = if mode == "own" { &f.a } else { &f.b };
        let claims = cancelled.association.candidate();
        let expected = cancelled.association.clone();
        let store =
            toolkit_store::SqliteStore::open(f._directory.path().join("queued-cancel.sqlite"))
                .unwrap();
        let source = store
            .admit(AdmissionRequest {
                key: StoreKey {
                    realm: "test-realm".into(),
                    owner: "workflow-source".into(),
                    id: mode.into(),
                },
                admission_key: mode.into(),
                data: RecordData {
                    schema: "test.source".into(),
                    version: 1,
                    body: serde_json::json!({}),
                },
                events: vec![],
                effects: vec![],
            })
            .await
            .unwrap()
            .record()
            .clone();
        let (release, released) = tokio::sync::oneshot::channel();
        let cancellation = f.owner.record_operation_cancelled(
            &claims.ingress_namespace.realm,
            &claims.original_work,
            move |binding, prior| async move {
                assert_eq!(binding.as_ref(), Some(&expected));
                assert!(prior.is_none());
                Ok(Arc::new(source))
            },
            move |_| async move {
                released.await.unwrap();
                Ok(Arc::new(
                    store
                        .admit(AdmissionRequest {
                            key: StoreKey {
                                realm: "test-realm".into(),
                                owner: "workflow-source".into(),
                                id: "cancelled".into(),
                            },
                            admission_key: "cancelled".into(),
                            data: RecordData {
                                schema: "test.cancelled".into(),
                                version: 1,
                                body: serde_json::json!({}),
                            },
                            events: vec![],
                            effects: vec![],
                        })
                        .await
                        .unwrap()
                        .record()
                        .clone(),
                ))
            },
        );
        tokio::pin!(cancellation);
        assert!(cancellation.as_mut().now_or_never().is_none());
        assert!(f.owner.try_source_writer().is_err());
        let queued = f.owner.observe_source_writer_wait_for_test();
        let published = std::sync::atomic::AtomicBool::new(false);
        let mut followup = Box::pin(f.owner.retain_followup_source(f.a.association(), || async {
            published.store(true, std::sync::atomic::Ordering::SeqCst);
            f.owner.retain_shared_entry(f.a.association(), None)
        }));
        assert!(followup.as_mut().now_or_never().is_none());
        queued.await.unwrap();
        assert!(!published.load(std::sync::atomic::Ordering::SeqCst));
        assert!(f.owner.shared_entries.lock().unwrap().is_empty());
        if mode == "dropped" {
            // Dropping a queued future cannot run its publication callback later.
            drop(followup);
            release.send(()).unwrap();
            cancellation.await.unwrap();
            assert!(!published.load(std::sync::atomic::Ordering::SeqCst));
            assert!(f.owner.shared_entries.lock().unwrap().is_empty());
            drop(f.owner.try_source_writer().unwrap());
            continue;
        }
        release.send(()).unwrap();
        cancellation.await.unwrap();
        let result = followup.await;
        if mode == "own" {
            assert!(matches!(
                result,
                Err(OriginalWriterError::OperationCancelled)
            ));
            assert!(!published.load(std::sync::atomic::Ordering::SeqCst));
            assert!(f.owner.shared_entries.lock().unwrap().is_empty());
        } else {
            let entry = result.unwrap();
            assert!(published.load(std::sync::atomic::Ordering::SeqCst));
            assert_eq!(f.owner.shared_entries.lock().unwrap().len(), 1);
            entry.enter().unwrap();
            assert!(entry.enter().is_err());
            entry.settle().unwrap();
            assert!(entry.settled());
        }
        drop(f.owner.try_source_writer().unwrap());
    }
}

#[test]
fn racing_absent_calls_reuse_one_custody_and_keep_later_replay_read_only() {
    let f = Fixture::new();
    let relationship = f.relationship(&f.a, f.a_child.clone()).unwrap();
    let call = Call::new("raced-call", "{}");
    // Deterministically interleave the actual optimistic and mutation phases:
    // both entrants saw absence before either obtained the publication writer.
    relationship.current_grant().unwrap();
    assert!(
        relationship
            .replay_staged_call(call.facts.call())
            .unwrap()
            .is_none()
    );
    relationship.current_grant().unwrap();
    assert!(
        relationship
            .replay_staged_call(call.facts.call())
            .unwrap()
            .is_none()
    );
    let entries = f.owner.shared_entries.lock().unwrap().len();
    let first = relationship.stage_current_call(call.facts.call()).unwrap();
    let binding = call.binding();
    relationship
        .prepared_call(f.g.association(), &binding)
        .unwrap();
    let (_, first_stamp) = f.owner.publication.observe(|| ()).unwrap();
    let second = relationship.stage_current_call(call.facts.call()).unwrap();
    assert!(Arc::ptr_eq(&first, &second));
    assert!(second.binding.get().unwrap().same_operation(&binding));
    assert_eq!(relationship.calls.lock().unwrap().len(), 1);
    assert_eq!(f.owner.shared_entries.lock().unwrap().len(), entries + 1);
    // Public begin_owner_change already published on the raced slow path.
    // This conservative invalidation does not mint a second call or binding.
    assert!(first_stamp.check_current().is_err());
    let (_, replay_stamp) = f.owner.publication.observe(|| ()).unwrap();
    assert!(Arc::ptr_eq(
        &relationship.stage_call(call.facts.call()).unwrap(),
        &first
    ));
    replay_stamp.check_current().unwrap();
    first.settle().unwrap();
}

#[test]
fn revoking_a_after_preparation_invalidates_its_publication_while_b_remains_usable() {
    let f = Fixture::new();
    let a = f.relationship(&f.a, f.a_child.clone()).unwrap();
    let b = f.relationship(&f.b, f.b_child.clone()).unwrap();
    let ca = Call::new("a-call", "{}");
    let cb = Call::new("b-call", "{}");
    let aa = a.stage_call(ca.facts.call()).unwrap();
    let bb = b.stage_call(cb.facts.call()).unwrap();
    let ba = ca.binding();
    let binding_b = cb.binding();
    let (_, stamp) = f
        .owner
        .publication
        .observe(|| {
            a.prepared_call(f.g.association(), &ba).unwrap().unwrap();
            b.prepared_call(f.g.association(), &binding_b)
                .unwrap()
                .unwrap();
        })
        .unwrap();
    let native = machine(&f.owner);
    let mut custody = native.try_controller_grant_mutation().unwrap();
    f.owner
        .grants
        .revoke(
            &meerkat_core::AdmissionGuard::none(),
            &principal("waiter-a"),
            f.a_child.last().unwrap(),
            &mut custody,
        )
        .unwrap();
    drop(custody);
    assert!(stamp.check_current().is_err());
    assert!(aa.current().is_err());
    assert!(a.prepared_call(f.g.association(), &ba).is_err());
    assert!(bb.current().is_ok());
    assert!(
        b.prepared_call(f.g.association(), &binding_b)
            .unwrap()
            .is_some()
    );
    // This is the actual local source gate, not Core's final native entry.
    assert!(aa.enter_source_gate().is_err());
    bb.enter_source_gate().unwrap();
    aa.settle().unwrap();
    bb.settle().unwrap();
}

#[test]
fn retiring_a_after_preparation_closes_only_a_and_does_not_reopen_on_replay() {
    let f = Fixture::new();
    let a = f.relationship(&f.a, f.a_child.clone()).unwrap();
    let b = f.relationship(&f.b, f.b_child.clone()).unwrap();
    let ca = Call::new("a-call", "{}");
    let cb = Call::new("b-call", "{}");
    let aa = a.stage_call(ca.facts.call()).unwrap();
    let bb = b.stage_call(cb.facts.call()).unwrap();
    a.prepared_call(f.g.association(), &ca.binding()).unwrap();
    b.prepared_call(f.g.association(), &cb.binding()).unwrap();
    let (_, stamp) = f.owner.publication.observe(|| ()).unwrap();
    // Both optimistic reads succeeded, but retirement wins publication before
    // the staging mutation. No new pending entry may escape its final recheck.
    a.current_grant().unwrap();
    let late = Call::new("late-a-call", "{}");
    assert!(a.replay_staged_call(late.facts.call()).unwrap().is_none());
    a.retire().unwrap();
    let count = f.owner.shared_entries.lock().unwrap().len();
    assert!(a.stage_current_call(late.facts.call()).is_err());
    assert_eq!(f.owner.shared_entries.lock().unwrap().len(), count);
    assert!(stamp.check_current().is_err());
    assert!(aa.enter_source_gate().is_err());
    let (_, retired_stamp) = f.owner.publication.observe(|| ()).unwrap();
    assert!(a.stage_call(ca.facts.call()).is_err());
    retired_stamp.check_current().unwrap();
    assert!(bb.current().is_ok());
    bb.enter_source_gate().unwrap();
    aa.settle().unwrap();
    bb.settle().unwrap();
}

#[test]
fn delegation_requires_its_own_connector_record_and_actual_g_audience() {
    use meerkat_authorization::mcp_installation::mcp_connector_action;
    use meerkat_authorization::policy::LocalOperationValues;
    use meerkat_authorization_contracts::constraints::{
        AudienceRef, ExactRestriction, OperationRestrictionValues, ProcessorRef,
    };
    use meerkat_core::{McpConnectorApproval, OperationReviewTier};
    let f = Fixture::new();
    let domain = ResourceDomain {
        authority: principal("operator"),
        namespace: "realm/current/mcp".into(),
    };
    let connector = mcp_connector_action("setup", &McpConnectorApproval::NoConnector);
    let g_audience = f.g.association.candidate().target.audience.clone();
    let recorded = |approval: &McpConnectorApproval, audience: AudienceRef| ExecutionRestrictions {
        actions: ExactRestriction::exact([mcp_connector_action("setup", approval)]),
        resource_domains: DomainRestriction::exact([domain.clone()]),
        audiences: ExactRestriction::exact([audience]),
        ..ExecutionRestrictions::unrestricted()
    };
    let resolve = |label: &str, restrictions: ExecutionRestrictions| {
        let parent = f
            .owner
            .grants
            .issue_root(
                &meerkat_core::AdmissionGuard::none(),
                &principal("admin"),
                id(&format!("{label}-root")),
                principal("waiter-a"),
                None,
                restrictions,
            )
            .unwrap();
        let child = f
            .owner
            .grants
            .issue_child(
                &meerkat_core::AdmissionGuard::none(),
                &principal("waiter-a"),
                &parent,
                id(&format!("{label}-to-g")),
                principal("planning"),
                ExecutionRestrictions::unrestricted(),
            )
            .unwrap();
        f.owner
            .grants
            .resolve_lineage(&[parent, child], &principal("planning"), None)
            .unwrap()
    };
    // This independent grant explicitly records the actual G connector. It
    // cannot lend that record to any of the distinct A delegations below.
    let g = resolve(
        "g-recorded",
        recorded(&McpConnectorApproval::NoConnector, g_audience.clone()),
    );
    let values = LocalOperationValues {
        action: connector,
        resource_domain: domain.clone(),
        processor: ProcessorRef::Principal {
            principal: principal("planning"),
        },
        audience: g_audience.clone(),
    };
    let allowance = || LocalPolicyAllowance {
        operation_values: vec![values.clone()],
        restrictions: g.restrictions().clone(),
        expires_at_ms: u64::MAX,
        review_tier: OperationReviewTier::R1,
    };
    let matches_actual = |allowance: &LocalPolicyAllowance| {
        allowance
            .restrictions
            .check_bounds(OperationRestrictionValues {
                action: &values.action,
                resource_domain: &values.resource_domain,
                processor: &values.processor,
                audience: &values.audience,
                now_ms: 10,
            })
    };
    let unrecorded = resolve("a-unrecorded", ExecutionRestrictions::unrestricted());
    assert!(matches!(
        narrow_delegation_allowance(&unrecorded, u64::MAX, &mut allowance(), 10),
        Err(OperationAuthorizationError::Unavailable)
    ));
    let other = resolve(
        "a-other",
        recorded(
            &McpConnectorApproval::Connector("sha256:other".into()),
            g_audience.clone(),
        ),
    );
    assert!(matches!(
        narrow_delegation_allowance(&other, u64::MAX, &mut allowance(), 10),
        Err(OperationAuthorizationError::Refused(_))
    ));
    let approved = resolve(
        "a-approved",
        recorded(&McpConnectorApproval::NoConnector, g_audience.clone()),
    );
    let mut checked = allowance();
    narrow_delegation_allowance(&approved, u64::MAX, &mut checked, 10).unwrap();
    matches_actual(&checked).unwrap();
    // Distinct source audiences are allowed, but the actual G recipient must
    // remain in the independently delegated A grant's bound.
    let excludes_g = resolve(
        "a-excludes-g",
        recorded(
            &McpConnectorApproval::NoConnector,
            f.a.association.candidate().target.audience.clone(),
        ),
    );
    let mut denied = allowance();
    narrow_delegation_allowance(&excludes_g, u64::MAX, &mut denied, 10).unwrap();
    assert!(matches_actual(&denied).is_err());
}

#[tokio::test]
async fn terminal_membership_retirement_preserves_a_original_and_b_staged_call() {
    let f = Fixture::new();
    let a = f.relationship(&f.a, f.a_child.clone()).unwrap();
    let b = f.relationship(&f.b, f.b_child.clone()).unwrap();
    let ca = Call::new("terminal-a", "{}");
    let entered_a = Call::new("entered-a", "{}");
    let cb = Call::new("live-b", "{}");
    let aa = a.stage_call(ca.facts.call()).unwrap();
    let entered = a.stage_call(entered_a.facts.call()).unwrap();
    let bb = b.stage_call(cb.facts.call()).unwrap();
    let ba = ca.binding();
    let be = entered_a.binding();
    let binding_b = cb.binding();
    let (_, stamp) = f
        .owner
        .publication
        .observe(|| {
            a.prepared_call(f.g.association(), &ba).unwrap().unwrap();
            a.prepared_call(f.g.association(), &be).unwrap().unwrap();
            b.prepared_call(f.g.association(), &binding_b)
                .unwrap()
                .unwrap();
        })
        .unwrap();
    entered.enter_source_gate().unwrap();
    // This is the actual local staged-call owner, not native effect-entry proof.
    // Retirement uses the same writer as relationship insertion. An unrelated
    // owner/Original in the selected vector refuses before any retirement.
    assert!(
        f.owner
            .retire_workflow_membership(f.a.association(), || Ok(vec![b.clone()]))
            .await
            .is_err()
    );
    assert!(aa.current().is_ok() && bb.current().is_ok());
    {
        let _writer = f.owner.mutation.try_lock().unwrap();
        assert!(
            f.owner
                .retire_workflow_membership(f.a.association(), || Ok(vec![a.clone()]))
                .await
                .is_err()
        );
    }
    f.owner
        .retire_workflow_membership(f.a.association(), || Ok(vec![a.clone()]))
        .await
        .unwrap();
    assert!(stamp.check_current().is_err());
    assert!(aa.current().is_err());
    assert!(aa.enter_source_gate().is_err());
    assert!(
        a.stage_call(Call::new("late-a", "{}").facts.call())
            .is_err()
    );
    assert!(bb.current().is_ok());
    bb.enter_source_gate().unwrap();
    f.owner.workflow_source_current(&f.a).unwrap();
    f.owner.workflow_source_current(&f.b).unwrap();
    assert!(
        !entered.entry.settled(),
        "retirement does not invent entered completion"
    );
    let (_, replay_stamp) = f.owner.publication.observe(|| ()).unwrap();
    f.owner
        .retire_workflow_membership(f.a.association(), || Ok(vec![a.clone()]))
        .await
        .unwrap();
    replay_stamp.check_current().unwrap();
    entered.settle().unwrap();
    aa.settle().unwrap();
    bb.settle().unwrap();
}
