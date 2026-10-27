//! Restore a genuinely enrolled original through the same persistent owners.
//! This isolated component control does not submit after reopen or prove
//! concurrent census progress or delivery wake.
use super::*;
use crate::original_ingress::{RetainedToolkitOriginal, custody_attempt::{CustodyAttempt, CustodyDecision}};
use meerkat_runtime::{LogicalRuntimeId, input_state::{InputLifecycleState, verify_exact_replay}};

type ColdFixture = FirstFixture<meerkat::PersistentSessionService<FactoryAgentBuilder>>;

// Own the same actual close future across the test body's cancellation. The
// completed result carries no fixture Arc, so old-owner death remains testable.
pub(crate) struct RetainedDurableClose {
    future: Option<std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>>>>>,
    completed: Option<Result<(), String>>,
}
impl RetainedDurableClose {
    pub(crate) async fn join(&mut self) -> Result<(), String> {
        if self.completed.is_none() {
            let result = self.future.as_mut().expect("one retained close future").as_mut().await;
            self.completed = Some(result);
            self.future = None;
        }
        self.completed.as_ref().unwrap().clone()
    }
}

impl ColdFixture {
    pub(crate) fn retained_close_for_test(self: &Arc<Self>) -> RetainedDurableClose {
        let fixture = self.clone();
        RetainedDurableClose {
            future: Some(Box::pin(async move { fixture.close_durable_owners().await })),
            completed: None,
        }
    }

    /// Reuse the production cold-binding callback for the exact saved source.
    /// The returned handle has no Candidate or admitted source custody yet.
    pub(crate) async fn bind_existing_source_for_recovery_test(
        &self, association: &InputAuthorityAssociation,
    ) -> Arc<RetainedToolkitOriginal> {
        let reference = self.original_reference();
        let before = self.cold_source_record().await;
        assert_eq!(self.binding().await.unwrap().as_ref(), Some(association));
        assert!(self.cold_source_custody(association).is_none());
        let selected = self.lifetime().selected_policy();
        let native = selected.native_host.as_ref().unwrap();
        let route = native.routes.iter().find(|route| route.id == reference.route).unwrap();
        let (_, requester, actor) = route.source_binding(selected).unwrap();
        let claims = association.candidate();
        assert_eq!(&claims.requester, requester);
        assert_eq!(&claims.ingress_actor, actor);
        let custody = meerkat_runtime::meerkat_machine::AuthenticatedOriginal {
            original_work: claims.original_work.clone(),
            occurrence_scope: claims.ingress_namespace.occurrence_scope.clone(),
            original_authentication: claims.original_authentication.clone(),
            root_event: claims.root_event.clone(),
            contributing_work: claims.contributing_work.clone(),
            target: claims.target.clone(),
            authority_basis: claims.authority_basis.clone(),
        };
        // This is the unchanged selected-setup branch in first_input.rs. The
        // real callback validates current selection and exact canonical facts;
        // it must not issue another child or manufacture first-write provenance.
        let original = self.source.owner.bind_issued_access_async(
            association.clone(), custody, requester, actor, Some(&native.grant_administrator),
            |association| async move {
                self.source.persist_or_observe_binding(reference, &association, true).await
            },
        ).await.unwrap();
        assert_eq!(original.association(), association);
        assert!(!original.first_binding_for(&before.key));
        self.source.hold_binding(reference.clone(), original.clone()).await.unwrap();
        assert!(!self.source.owner.has_admitted_source(association).unwrap());
        assert_eq!(self.cold_source_record().await, before);
        original
    }
}

async fn cold_restore_control(tampered: bool) {
    let (fixture, history, retained, adapter) = ColdFixture::for_durable_continuation().await;
    let mut active = Some(Arc::new(fixture));
    let mut native_close = active.as_ref().unwrap().retained_close_for_test();
    let mut runtime_store = Some(history);
    let mut retained = Some(retained);
    let mut continuation = Some(adapter);
    let mut first_cold_round = None;
    let checked = std::panic::AssertUnwindSafe(tokio::time::timeout(WAIT * 4, Box::pin(async {
        let (association, original_input, native_record, source_record, grants, session) = {
            let fixture = active.as_ref().unwrap();
            assert!(fixture.source.owner.has_installed_pair_recovery().unwrap(),
                "only the production installed opener selects this intact pair");
            fixture.source.start_custody_recovery(
                fixture.continuation_native().continuation_source_announcer(),
            ).unwrap();
            let admitted = fixture.submit().await;
            assert!(matches!(&admitted, Ok(PrivateOriginalSubmission::Admitted {
                deduplicated: false,
                source_settlement: PrivateSourceSettlement::FreshAcceptancePromoted,
                ..
            })), "first input must actually admit and publish its source");
            fixture.finish(&admitted).await.unwrap();
            let context = fixture.probe.as_ref().unwrap().captured();
            tokio::time::timeout(WAIT, fixture.machine.wait_run_settled(
                &fixture.session, context.retained_work().unwrap().run_id(),
            )).await.unwrap();
            let association = fixture.binding().await.unwrap().unwrap();
            // Live Task custody retains its real issuer; the exact cold rebind preserves it.
            assert_eq!(fixture.cold_source_custody(&association), Some((true, false)));
            let held = fixture.source.retained.lock().unwrap().iter()
                .find(|held| held.reference == *fixture.original_reference()).cloned().unwrap();
            let pending = held.first_custody.lock().unwrap().clone()
                .expect("C1 was selected before the actual first invocation");
            assert!(pending.handoff.enrolled());
            let native_receipt = pending.native_admission.lock().unwrap().clone().unwrap();
            assert_eq!(native_receipt.session_id, fixture.session);
            assert_eq!(native_receipt.input_id, *pending.input.id());

            fixture.source.stop_custody_recovery();
            let (native, source) = tokio::join!(
                native_close.join(), fixture.source.join_custody_recovery());
            assert!(native.is_ok(), "first native and local owners: {native:?}");
            assert!(source.is_ok(), "first source owner: {source:?}");
            let source_record = fixture.cold_source_record().await;
            assert_eq!(source_record.data.version, 4);
            let attempt: CustodyAttempt = serde_json::from_value(
                source_record.data.body["custody_attempt"].clone()).unwrap();
            assert!(matches!(attempt.decision(), CustodyDecision::Promoted));
            attempt.validate_binding(&association).unwrap();
            let rows = runtime_store.as_ref().unwrap().load_input_states_strict(
                &LogicalRuntimeId::for_session(&fixture.session)).await.unwrap();
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].seed.phase, InputLifecycleState::Consumed);
            assert!(rows[0].state.persisted_input.is_none(), "completed native payload was retired");
            let original_input: meerkat_runtime::Input = serde_json::from_value(
                source_record.data.body["bound_service_input"].clone()).unwrap();
            assert!(original_input.header().ingress_context.is_none(), "canonical bytes retain no process owner");
            assert_eq!(rows[0].state.input_id, native_receipt.input_id);
            assert!(attempt.matches_input(&original_input).unwrap());
            verify_exact_replay(&rows[0].state, &original_input).unwrap();
            let grants = std::fs::read(fixture.source_path.parent().unwrap().join("grants.jsonl")).unwrap();
            (association, original_input, serde_json::to_value(&rows[0]).unwrap(),
                source_record, grants, fixture.session.clone())
        };

        let weak_source = Arc::downgrade(&active.as_ref().unwrap().source);
        let weak_machine = Arc::downgrade(&active.as_ref().unwrap().machine);
        let weak_service = Arc::downgrade(&active.as_ref().unwrap().service);
        let weak_lifetime = Arc::downgrade(active.as_ref().unwrap().lifetime());
        let weak_store = Arc::downgrade(runtime_store.as_ref().unwrap());
        let weak_adapter = Arc::downgrade(continuation.as_ref().unwrap());
        drop(continuation.take());
        drop(active.take());
        drop(runtime_store.take());
        assert!(weak_adapter.upgrade().is_none(), "first continuation owner remains");
        assert!(weak_source.upgrade().is_none(), "first source owner remains");
        assert!(weak_machine.upgrade().is_none(), "first native machine remains");
        assert!(weak_service.upgrade().is_none(), "first service remains");
        assert!(weak_lifetime.upgrade().is_none(), "first installed lifetime remains");
        assert!(weak_store.upgrade().is_none(), "first runtime store remains");

        let (fixture, history, adapter) = ColdFixture::reopen_durable_continuation(
            retained.take().unwrap()).await;
        active = Some(Arc::new(fixture));
        native_close = active.as_ref().unwrap().retained_close_for_test();
        runtime_store = Some(history);
        continuation = Some(adapter);
        let fixture = active.as_ref().unwrap();
        assert_eq!(fixture.session, session);
        assert_eq!(fixture.model_requests(), 0);
        assert!(fixture.cold_source_custody(&association).is_none());
        assert_eq!(fixture.cold_source_record().await, source_record);
        assert!(fixture.source.owner.has_installed_pair_recovery().unwrap());
        assert!(!fixture.source.owner.has_live_first_binding(&source_record.key).unwrap());
        // Select the real recovery owner, but isolate this one-shot exact-input
        // restoration from its background census. This permit holds no authority
        // lock and is released outside the cancellable body before owner join.
        first_cold_round = Some(fixture.source.hold_first_custody_round_for_test());
        fixture.source.start_custody_recovery(
            fixture.continuation_native().continuation_source_announcer(),
        ).unwrap();

        // No send/submit, new Input, or synthetic admission occurs after reopen.
        // The real machine must join the original persisted input to the current
        // registration, attachment and controller before publishing this source.
        let original = fixture.bind_existing_source_for_recovery_test(&association).await;
        // The tuple is (Admitted, Task issuer is None), not (admitted, restored).
        // This exact rebind retained its selected task issuer before recovery.
        let bound_custody = fixture.cold_source_custody(&association);
        assert_eq!(bound_custody, Some((false, false)));
        assert!(!fixture.source.owner.has_admitted_source(&association).unwrap());
        if tampered {
            // Make the changed source internally consistent, so a decoder or
            // Connect fingerprint rejection cannot stand in for native proof.
            // No altered prompt is ever passed to an input submission API.
            let mut altered = original_input.clone();
            let meerkat_runtime::Input::Prompt(prompt) = &mut altered else { panic!("canonical prompt") };
            let changed_content = "A different request that native never admitted";
            prompt.content = meerkat_core::ContentInput::Text(changed_content.into());
            let mut data = source_record.data.clone();
            data.body["bound_service_input"] = serde_json::to_value(&altered).unwrap();
            data.body["event"]["content_sha256"] = serde_json::json!(
                crate::private_originals::digest(changed_content.as_bytes()));
            // This is the documented exact-input fingerprint domain of the
            // canonical witness, not native's private prompt replay identity.
            data.body["custody_attempt"]["prepared_input_sha256"] = serde_json::json!(
                crate::private_originals::digest(&toolkit_package::canonical_json(&(
                    "toolkit.prepared-original-input.v1", &altered)).unwrap()));
            let changed = fixture.store.compare_and_swap(toolkit_store::Transition {
                key: source_record.key.clone(), expected_revision: source_record.revision,
                expected_digest: Some(source_record.content_digest.clone()), claim: None,
                data, events: vec![], effects: vec![],
            }).await.unwrap();
            let decoded = fixture.source.decode_stored(&changed).unwrap();
            fixture.source.check_source_binding(&decoded, &association).unwrap();
            assert_eq!(changed.key, source_record.key);
            assert_eq!(decoded.final_association.as_ref(), Some(&association));
            assert_eq!(serde_json::to_value(decoded.bound_service_input.as_ref().unwrap().decode().unwrap()).unwrap(),
                serde_json::to_value(&altered).unwrap());
            assert!(decoded.custody_attempt.as_ref().unwrap().matches_input(&altered).unwrap());
            let rows = runtime_store.as_ref().unwrap().load_input_states_strict(
                &LogicalRuntimeId::for_session(&session)).await.unwrap();
            assert_eq!(rows.len(), 1);
            assert_eq!(serde_json::to_value(&rows[0]).unwrap(), native_record);
            assert!(rows[0].state.persisted_input.is_none());
            assert!(verify_exact_replay(&rows[0].state, &altered).is_err(),
                "native retained replay authority rejects the changed canonical witness");
            assert!(matches!(fixture.source.restore_promoted_custody(&source_record.key, &original).await,
                Err(crate::private_originals::PrivateOriginalError::Conflict)));
            assert_eq!(fixture.cold_source_custody(&association), bound_custody,
                "refused native replay preserves the exact preexisting bound custody");
            assert!(!fixture.source.owner.has_admitted_source(&association).unwrap());
            assert_eq!(fixture.cold_source_record().await, changed,
                "refusal does not write or restore source custody");
            let rows = runtime_store.as_ref().unwrap().load_input_states_strict(
                &LogicalRuntimeId::for_session(&session)).await.unwrap();
            assert_eq!(rows.len(), 1);
            assert_eq!(serde_json::to_value(&rows[0]).unwrap(), native_record);
            verify_exact_replay(&rows[0].state, &original_input).unwrap();
            assert_eq!(std::fs::read(fixture.source_path.parent().unwrap().join("grants.jsonl")).unwrap(), grants);
            assert_eq!(fixture.model_requests(), 0);
            return;
        }
        fixture.source.restore_promoted_custody(&source_record.key, &original).await.unwrap();
        assert_eq!(fixture.cold_source_custody(&association), Some((true, false)));
        assert_eq!(fixture.source.route_for_binding(&association).unwrap(), fixture.original_reference().route);
        assert_eq!(fixture.cold_source_record().await, source_record);
        assert_eq!(std::fs::read(fixture.source_path.parent().unwrap().join("grants.jsonl")).unwrap(), grants);
        let rows = runtime_store.as_ref().unwrap().load_input_states_strict(
            &LogicalRuntimeId::for_session(&session)).await.unwrap();
        assert_eq!(rows.len(), 1, "restoration must not submit another input");
        assert_eq!(&rows[0].state.input_id, original_input.id());
        assert_eq!(rows[0].seed.phase, InputLifecycleState::Consumed);
        assert_eq!(serde_json::to_value(&rows[0]).unwrap(), native_record,
            "source restoration preserves the complete native terminal record");
        verify_exact_replay(&rows[0].state, &original_input).unwrap();
        assert_eq!(rows[0].state.authority_contributors[0].association(), &association);
        assert_eq!(fixture.model_requests(), 0, "source restoration does not replay the first turn");
    }))).catch_unwind().await;
    drop(first_cold_round.take());
    let cleanup = if let Some(fixture) = active.as_ref() {
        fixture.source.stop_custody_recovery();
        let (native, source) = tokio::join!(
            native_close.join(), fixture.source.join_custody_recovery());
        native.and(source.map_err(|error| error.to_string()))
    } else { Ok(()) };
    drop(continuation);
    drop(active);
    drop(runtime_store);
    match checked {
        Err(payload) => {
            if let Err(error) = cleanup { eprintln!("cold restoration cleanup also failed: {error}"); }
            std::panic::resume_unwind(payload);
        }
        Ok(result) => {
            assert!(cleanup.is_ok(), "{cleanup:?}");
            assert!(result.is_ok(), "native cold restoration observation timed out");
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn installed_promoted_attempt_restores_from_same_native_input_without_resubmission() {
    cold_restore_control(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retired_native_payload_rejects_changed_canonical_input_without_restoration() {
    cold_restore_control(true).await;
}
