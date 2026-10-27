//! Actual approved B, canonical owners and native final Apply callback. A
//! deliberate await stands between native preparation and entry. No MCP
//! transport is started and this does not claim actor B/R attachment coverage.
use super::*;
use meerkat_authorization_contracts::work_association::OriginalWorkRef;
use meerkat_core::mcp_config::McpScope;
use meerkat_toolkit::setup::SetupAccess;

struct CancellationDetail(ContextReference);
impl ContextReferenceOwner for CancellationDetail {
    fn verify_access(
        &self,
        reference: &ContextReference,
        evidence: &SetupRequestEvidence,
        context: &SetupUseContext<'_>,
    ) -> Result<(), RequestError> {
        if reference != &self.0 {
            return Err(RequestError::ContextUnavailable);
        }
        evidence
            .check_context(context)
            .map_err(|_| RequestError::ContextUnavailable)
    }
}

#[derive(Clone, Copy)]
enum Mutation {
    SetupCancellation,
    SourceCancellation,
    GrantRevocation,
}

async fn mutate(
    fixture: &ContinuationFixture,
    snapshot: &SetupSnapshot,
    dispatch: &ToolDispatchContext,
    b: &PrivateOriginalRef,
    mutation: Mutation,
) {
    match mutation {
        Mutation::SetupCancellation => {
            let evidence = snapshot.request().evidence();
            let constraints = snapshot.request().constraints();
            let target = &evidence.association().candidate().target;
            let context = SetupUseContext {
                tenant: &constraints.tenant,
                deployment: &constraints.deployment,
                realm: &evidence.association().candidate().ingress_namespace.realm,
                target,
                policy_revision: &constraints.policy_revision,
                now_ms: u64::try_from(fixture.store.now_ms().await.unwrap()).unwrap(),
                authorization: dispatch.work_authorization().unwrap(),
            };
            let detail = ContextReference {
                owner: id("service-admission-test"),
                id: id("cancel-detail"),
                revision: revision("1"),
                content_digest: toolkit_package::ContentDigest::parse("0".repeat(64)).unwrap(),
            };
            let references = CancellationDetail(detail.clone());
            let store = SetupStore::new_conditional(
                fixture.store.as_ref(),
                SetupScope::new(constraints.tenant.clone(), constraints.deployment.clone()),
            )
            .unwrap();
            store
                .cancel(
                    &id(&snapshot.key().id),
                    detail,
                    SetupAccess {
                        evidence,
                        context: &context,
                        references: &references,
                    },
                )
                .await
                .unwrap();
        }
        Mutation::SourceCancellation => {
            fixture.source.record_task_group_cancelled(b).await.unwrap();
        }
        Mutation::GrantRevocation => {
            let source = &fixture.source;
            let stored = source
                .store
                .get(&source.key(b).unwrap())
                .await
                .unwrap()
                .unwrap();
            let record = source.decode_record(&stored, b).unwrap();
            let grant = &record.approved_task.as_ref().unwrap().singleton_grant().unwrap();
            let route = source.route(&b.route).unwrap();
            let original = OriginalWorkRef {
                authority: route.source_authority.clone(),
                work: b.original.clone(),
            };
            let lineage = source
                .service_grant_lineage(grant, &original)
                .await
                .unwrap();
            let mut custody = fixture.machine.try_controller_grant_mutation().unwrap();
            source
                .owner
                .grants()
                .revoke(&route.grant_caller, lineage.last().unwrap(), &mut custody)
                .unwrap();
        }
    }
}

async fn exercise(mutation: Option<Mutation>) {
    let fixture = ContinuationFixture::for_service_input().await;
    let result = AssertUnwindSafe(Box::pin(async {
        let a = fixture.submit().await;
        fixture.finish(&a).await.unwrap();
        let dispatch = fixture.probe.as_ref().unwrap().captured();
        let a_run = dispatch.retained_work().unwrap().run_id().clone();
        tokio::time::timeout(
            WAIT,
            fixture.machine.wait_run_settled(&fixture.session, &a_run),
        )
        .await
        .unwrap();
        assert_eq!(
            fixture
                .machine
                .run_turn_terminal(&fixture.session, &a_run)
                .await,
            Some(TurnTerminalOutcome::Completed)
        );
        let snapshot = acquisition(&fixture, &dispatch).await;
        let b = approve_b(&fixture, snapshot.clone()).await;
        let requests = fixture.model_requests();
        let setup_before = fixture.store.get(snapshot.key()).await.unwrap().unwrap();
        let b_stored = fixture
            .store
            .get(&fixture.source.key(&b).unwrap())
            .await
            .unwrap()
            .unwrap();
        let b_record = fixture.source.decode_record(&b_stored, &b).unwrap();
        let access_key = crate::private_originals::service_grants::tests::fixture_access_key(
            &fixture.source,
            &b_record.approved_task.as_ref().unwrap().singleton_grant().unwrap(),
        )
        .unwrap();
        let access_before = fixture.store.get(&access_key).await.unwrap().unwrap();
        let rows = fixture
            .store
            .list_records(&fixture.scope, None, 256)
            .await
            .unwrap();
        let server = reviewed_config(&fixture.scope.realm);
        let control = fixture
            .source
            .prepare_service_mcp_control(&fixture.store, &b, server.clone())
            .await
            .unwrap();
        let registration = fixture
            .machine
            .current_session_registration_witness(&fixture.session)
            .await
            .unwrap();
        let association = fixture.binding().await.unwrap().unwrap();
        let authority = fixture
            .machine
            .prepare_mcp_attachment(
                &registration,
                &association.candidate().target,
                control.clone(),
            )
            .await
            .unwrap();
        assert_eq!(
            fixture
                .store
                .list_records(&fixture.scope, None, 256)
                .await
                .unwrap(),
            rows,
            "preparation does not change canonical source, task or account records"
        );
        assert_eq!(
            fixture.store.get(snapshot.key()).await.unwrap(),
            Some(setup_before)
        );
        assert_eq!(
            fixture.store.get(&access_key).await.unwrap(),
            Some(access_before)
        );
        // These are actual native entry callbacks, not an invented permission
        // stub. The same fixture must reach a positive Apply before a negative
        // case can establish that later canonical mutation caused refusal.
        assert!(authority.authorize_persist(McpScope::User).is_err());
        assert_eq!(authority.authorize_apply(), Ok(()));
        assert!(
            authority.authorize_apply().is_err(),
            "one prepared entry is consumed once"
        );
        if let Some(mutation) = mutation {
            let control = fixture
                .source
                .prepare_service_mcp_control(&fixture.store, &b, server.clone())
                .await
                .unwrap();
            let authority = fixture
                .machine
                .prepare_mcp_attachment(
                    &registration,
                    &association.candidate().target,
                    control.clone(),
                )
                .await
                .unwrap();
            let release = Arc::new(tokio::sync::Notify::new());
            let waiting = release.clone();
            let (entered, held) = tokio::sync::oneshot::channel();
            let apply = tokio::spawn(async move {
                let _ = entered.send(());
                waiting.notified().await;
                authority.authorize_apply()
            });
            tokio::time::timeout(WAIT, held).await.unwrap().unwrap();
            mutate(&fixture, &snapshot, &dispatch, &b, mutation).await;
            release.notify_one();
            assert!(
                tokio::time::timeout(WAIT, apply)
                    .await
                    .unwrap()
                    .unwrap()
                    .is_err(),
                "the actual native entry refuses the old prepared observation"
            );
            assert!(
                fixture
                    .source
                    .prepare_service_mcp_control(&fixture.store, &b, server)
                    .await
                    .is_err(),
                "a stale control cannot be refreshed into current permission"
            );
        }
        assert_eq!(
            fixture.inputs().await,
            Ok(1),
            "no approved B input was admitted"
        );
        assert_eq!(
            fixture.model_requests(),
            requests,
            "no second model request was launched"
        );
    }))
    .catch_unwind()
    .await;
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok());
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reviewed_live_control_reaches_native_apply_once_without_persistence_or_b_input() {
    exercise(None).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn held_native_apply_refuses_after_canonical_setup_cancellation() {
    exercise(Some(Mutation::SetupCancellation)).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn held_native_apply_refuses_after_canonical_source_group_cancellation() {
    exercise(Some(Mutation::SourceCancellation)).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn held_native_apply_refuses_after_actual_service_grant_revocation() {
    exercise(Some(Mutation::GrantRevocation)).await;
}
