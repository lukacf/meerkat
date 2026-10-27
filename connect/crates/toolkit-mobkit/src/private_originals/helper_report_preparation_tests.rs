//! The real configured helper context exercises the ordinary preparation APIs.
//! The model is mechanical; there is no account/provider I/O or service grant.
use super::*;
use crate::installed_host::acquisition::{AcquisitionBootstrap, InstalledServiceTaskSource};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn software_authenticated_and_report_only_public_capture_keep_legacy_preparation() {
    let script = acquisition_denial::Script::for_approved();
    let report = Report::new(script.tools().result_tool());
    let (fixture, _runtime_store) =
        Fixture::for_configured_helper_report(script.clone(), report.clone()).await;
    let checked = std::panic::AssertUnwindSafe(tokio::time::timeout(
        WAIT,
        Box::pin(async {
            let first = fixture.submit().await;
            fixture.finish(&first).await.unwrap();
            let feedback = script.observations().0;
            assert_eq!(feedback.len(), 1);
            assert!(!feedback[0].is_error, "{feedback:?}");
            let context = report.take_context().await;
            let native = fixture
                .lifetime()
                .selected_policy()
                .native_host
                .as_ref()
                .unwrap();
            let configured = native
                .members
                .iter()
                .find(|member| member.member.member == MEMBER)
                .unwrap()
                .setup_capability
                .as_ref()
                .unwrap();
            assert_eq!(configured.helper_targets.len(), 3);
            assert!(
                configured
                    .helper_targets
                    .iter()
                    .all(|target| target.target.is_some()),
                "this actual legacy configured selection has no catalog opt-in"
            );

            // Reuse the actual installed acquisition owner; this bootstrap view
            // opens no runtime/store and creates no replacement helper custody.
            let owner = script.installed_owner();
            let mut bootstrap = AcquisitionBootstrap::selected(owner.clone());
            assert!(Arc::ptr_eq(
                &bootstrap.prepare(None, None).unwrap().unwrap(),
                &owner
            ));
            let bootstrap = Arc::new(Mutex::new(bootstrap));
            let tasks = InstalledServiceTaskSource::new(&bootstrap);
            let source = fixture
                .source
                .setup_result_source(&context, report.selection.get().unwrap())
                .await
                .unwrap();
            let acquisition_key = source.operation.acquisition.clone();
            let original_key = fixture.source.key(&fixture.submission.original).unwrap();
            let acquisition_before = fixture.store.get(&acquisition_key).await.unwrap().unwrap();
            let original_before = fixture.store.get(&original_key).await.unwrap().unwrap();
            let journal = fixture.source_path.with_file_name("grants.jsonl");
            let grants_before = std::fs::read(&journal).unwrap();
            let software = report
                .prepare_task(&tasks, ReportPreparation::Software)
                .await
                .unwrap();
            let authenticated = report
                .prepare_task(&tasks, ReportPreparation::CapturedConsumer)
                .await
                .unwrap();
            let explicit = report
                .prepare_task(&tasks, ReportPreparation::ExplicitConsumer)
                .await
                .unwrap();
            let public = report
                .prepare_task(&tasks, ReportPreparation::PublicService)
                .await
                .unwrap();
            for task in [&software, &authenticated, &explicit, &public] {
                assert_eq!(task.consumer_member(), Some(MEMBER));
                assert_eq!(task.deadline_ms(), software.deadline_ms());
                assert!(!task.has_configured_setup());
                task.check_setup_current().await.unwrap();
            }
            assert_eq!(
                fixture.store.get(&acquisition_key).await.unwrap().unwrap(),
                acquisition_before
            );
            assert_eq!(
                fixture.store.get(&original_key).await.unwrap().unwrap(),
                original_before
            );
            assert_eq!(std::fs::read(journal).unwrap(), grants_before);
        }),
    ))
    .catch_unwind()
    .await;
    report.release();
    report.leaf.context.lock().unwrap().take();
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    match checked {
        Ok(Ok(())) => (),
        Ok(Err(_)) => panic!("legacy task preparation did not settle before the fixture deadline"),
        Err(payload) => std::panic::resume_unwind(payload),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn same_executor_legacy_helper_prepares_public_without_warm_selection() {
    use crate::installed_host::controller_first_input_fixture::WarmHelperProfile;
    legacy_helper_preparation(&[WarmHelperProfile::SameExecutor]).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cross_audience_legacy_helper_prepares_public_without_warm_selection() {
    use crate::installed_host::controller_first_input_fixture::WarmHelperProfile;
    legacy_helper_preparation(&[WarmHelperProfile::CrossAudience]).await;
}

async fn legacy_helper_preparation(
    profiles: &[crate::installed_host::controller_first_input_fixture::WarmHelperProfile],
) {
    use crate::installed_host::controller_first_input_fixture::WarmHelperProfile;
    use meerkat_authorization_contracts::constraints::{ExactRestriction, ProcessorRef};
    for &profile in profiles {
        let script = acquisition_denial::Script::for_approved();
        let report = Report::new(script.tools().result_tool());
        let (fixture, _) =
            Fixture::for_warm_helper_profile(script.clone(), report.clone(), profile).await;
        let checked = std::panic::AssertUnwindSafe(tokio::time::timeout(
            WAIT,
            Box::pin(async {
                let first = fixture.submit().await;
                fixture.finish(&first).await.unwrap();
                let feedback = script.observations().0;
                assert_eq!(feedback.len(), 1);
                assert!(!feedback[0].is_error, "{feedback:?}");
                let context = report.take_context().await;
                let native = fixture
                    .lifetime()
                    .selected_policy()
                    .native_host
                    .as_ref()
                    .unwrap();
                let original = &native.members[0];
                let helper = &native.members[1];
                assert!(
                    original
                        .setup_capability
                        .as_ref()
                        .unwrap()
                        .helper_targets
                        .iter()
                        .any(|target| target.target.is_none()),
                    "catalog capability is genuinely configured"
                );
                match profile {
                    WarmHelperProfile::SameExecutor => {
                        assert!(original.audience == helper.audience);
                        assert!(original.logical_owner == helper.logical_owner);
                    }
                    WarmHelperProfile::CrossAudience => {
                        assert!(original.audience != helper.audience);
                        assert!(original.logical_owner != helper.logical_owner);
                    }
                    WarmHelperProfile::Standard => unreachable!(),
                }
                for target in &original.setup_capability.as_ref().unwrap().helper_targets {
                    assert!(
                        target.restrictions.processors
                            == ExactRestriction::exact([ProcessorRef::Principal {
                                principal: helper.logical_owner.clone()
                            },])
                    );
                    assert!(
                        target.restrictions.audiences
                            == ExactRestriction::exact([helper.audience.clone()])
                    );
                }
                let owner = script.installed_owner();
                let mut bootstrap = AcquisitionBootstrap::selected(owner.clone());
                assert!(Arc::ptr_eq(
                    &bootstrap.prepare(None, None).unwrap().unwrap(),
                    &owner
                ));
                let bootstrap = Arc::new(Mutex::new(bootstrap));
                let tasks = InstalledServiceTaskSource::new(&bootstrap);
                let key = fixture.source.key(fixture.original_reference()).unwrap();
                let before = fixture.store.get(&key).await.unwrap().unwrap();
                let journal = fixture.source_path.with_file_name("grants.jsonl");
                let grants_before = std::fs::read(&journal).unwrap();
                let public = report
                    .prepare_task(&tasks, ReportPreparation::PublicService)
                    .await
                    .unwrap();
                assert!(
                    !public.has_configured_setup(),
                    "valid legacy profiles do not acquire broader metadata scope"
                );
                assert_eq!(public.consumer_member(), Some(MEMBER));
                public.check_setup_current().await.unwrap();
                if matches!(profile, WarmHelperProfile::CrossAudience) {
                    // Optional fallback cannot hide contradictory helper facts.
                    // This negative owner lookup changes no retained policy or
                    // grants; positive preparation above uses the actual call.
                    let mut contradictory = helper.clone();
                    contradictory.audience = original.audience.clone();
                    assert!(matches!(
                        fixture.source.select_service_setup(
                            fixture.original_reference(),
                            &fixture.session,
                            public.deadline_ms(),
                            report.selection.get().unwrap(),
                            &contradictory,
                            context.origin_session_id().unwrap(),
                        ),
                        Err(PrivateOriginalError::Conflict)
                    ));
                }
                for task in [
                    report
                        .prepare_task(&tasks, ReportPreparation::Software)
                        .await
                        .unwrap(),
                    report
                        .prepare_task(&tasks, ReportPreparation::CapturedConsumer)
                        .await
                        .unwrap(),
                ] {
                    assert!(!task.has_configured_setup());
                    assert_eq!(task.deadline_ms(), public.deadline_ms());
                    task.check_setup_current().await.unwrap();
                }
                assert_eq!(fixture.store.get(&key).await.unwrap().unwrap(), before);
                assert_eq!(std::fs::read(&journal).unwrap(), grants_before);
            }),
        ))
        .catch_unwind()
        .await;
        report.release();
        report.leaf.context.lock().unwrap().take();
        let cleanup = fixture.shutdown().await;
        assert!(cleanup.is_ok(), "{cleanup:?}");
        match checked {
            Ok(Ok(())) => (),
            Ok(Err(_)) => panic!("legacy profile preparation exceeded its bounded deadline"),
            Err(payload) => std::panic::resume_unwind(payload),
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn helper_result_accepts_exact_custody_progress_and_refuses_changed_source() {
    use crate::original_ingress::custody_attempt::{CustodyAttempt, CustodyChange};
    let script = acquisition_denial::Script::for_approved();
    let report = Report::new(script.tools().result_tool());
    let (fixture, _) = Fixture::for_configured_helper_report(script.clone(), report.clone()).await;
    let checked = std::panic::AssertUnwindSafe(tokio::time::timeout(WAIT, Box::pin(async {
        fixture.source.start_custody_recovery(
            fixture.native.mob_runtime().continuation_source_announcer(),
        ).unwrap();
        let first = fixture.submit().await;
        fixture.finish(&first).await.unwrap();
        let context = report.take_context().await;
        let session = context.origin_session_id().unwrap();
        // This is the actual helper's selected native context, not a fabricated
        // source or controller. The old exact-revision guard fails this lookup.
        let source = fixture.source.setup_result_source(
            &context, report.selection.get().unwrap()).await.unwrap();
        let held = fixture.source.helper_sources.lock().unwrap().iter()
            .find(|held| held.original().association() == &source.helper).unwrap().clone();
        assert_eq!(held.source_record().data.version, 3);
        assert_eq!(held.source_record().revision, 2);
        let saved = loop {
            let saved = fixture.store.get(&source.source.key).await.unwrap().unwrap();
            assert_eq!(saved.data.version, 5);
            if saved.data.body["custody_attempt"]["decision"] == "promoted" { break saved; }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        };
        assert!(saved.revision >= 4);
        let source = fixture.source.setup_result_source(
            &context, report.selection.get().unwrap()).await.unwrap();
        assert_eq!(source.source.revision, saved.revision);
        assert_eq!(source.source.content_digest, saved.content_digest);
        assert!(held.check_result_record(&saved, session).is_ok());
        let (initial, retained_input, changed_input, changed_attempt) = {
            let pending = held.first_custody.lock().unwrap();
            let pending = pending.as_ref().unwrap();
            let initial = CustodyAttempt::for_prepared(
                &pending.session, &pending.input, held.original().association()).unwrap();
            let mut changed = pending.input.clone();
            let meerkat_runtime::Input::Prompt(prompt) = &mut changed else { unreachable!() };
            prompt.content = meerkat_core::ContentInput::Text("different retained helper input".into());
            let attempt = CustodyAttempt::for_prepared(
                &pending.session, &changed, held.original().association()).unwrap()
                .changed(CustodyChange::Promote).unwrap();
            (initial, crate::private_originals::RetainedBoundInput::from_bound(&pending.input).unwrap(), changed, attempt)
        };
        let mut changed_facts = saved.clone();
        changed_facts.data.body["facts"]["operation"]["effect"] = serde_json::json!("different-effect");
        changed_facts.content_digest = changed_facts.data.digest().unwrap();
        let mut changed_input_row = saved.clone();
        changed_input_row.data.body["bound_input"] = serde_json::to_value(&changed_input).unwrap();
        changed_input_row.data.body["custody_attempt"] = serde_json::to_value(changed_attempt).unwrap();
        changed_input_row.content_digest = changed_input_row.data.digest().unwrap();
        let mut withdrawn = saved.clone();
        withdrawn.data.body["custody_attempt"] = serde_json::to_value(
            initial.changed(CustodyChange::Withdraw).unwrap()).unwrap();
        withdrawn.content_digest = withdrawn.data.digest().unwrap();
        // A reader with a later actual source snapshot must reject a
        // regressing row and changed content under the same revision.
        let retained_current = crate::original_ingress::helper_originals::RetainedHelperSource {
            original: held.source.original.clone(),
            source_record: Arc::new(saved.clone()),
            event_key: held.source.event_key.clone(),
            authentication: held.source.authentication.clone(),
        };
        let mut regressed = saved.clone(); regressed.revision -= 1;
        let mut same_revision = saved.clone();
        same_revision.data.body["custody_attempt"]["spent_steps"] = serde_json::json!(1);
        same_revision.content_digest = same_revision.data.digest().unwrap();
        for changed in [&regressed, &same_revision] {
            assert!(retained_current.check_result_record(changed,
                Some((&retained_input, session))).is_err());
        }
        let mut wrong_key = saved.clone(); wrong_key.key.id.push_str("-other");
        let mut wrong_schema = saved.clone(); wrong_schema.data.schema.push_str("-other");
        wrong_schema.content_digest = wrong_schema.data.digest().unwrap();
        let mut bad_digest = saved.clone(); bad_digest.content_digest = held.source_record().content_digest.clone();
        for (label, changed) in [
            ("immutable facts", changed_facts.clone()), ("self-consistent changed input/attempt", changed_input_row),
            ("withdrawal", withdrawn),
            ("source key", wrong_key), ("schema", wrong_schema), ("digest", bad_digest),
        ] {
            assert!(matches!(held.check_result_record(&changed, session), Err(PrivateOriginalError::Conflict)), "{label}");
        }
        // Exercise the actual canonical read as well as the narrow comparator.
        let changed = fixture.store.compare_and_swap(toolkit_store::Transition {
            key: saved.key.clone(), expected_revision: saved.revision,
            expected_digest: Some(saved.content_digest.clone()), data: changed_facts.data,
            claim: None, events: vec![], effects: vec![],
        }).await.unwrap();
        assert!(matches!(fixture.source.setup_result_source(
            &context, report.selection.get().unwrap()).await, Err(PrivateOriginalError::Conflict)));
        let restored = fixture.store.compare_and_swap(toolkit_store::Transition {
            key: changed.key, expected_revision: changed.revision,
            expected_digest: Some(changed.content_digest), data: saved.data,
            claim: None, events: vec![], effects: vec![],
        }).await.unwrap();
        let current = fixture.source.setup_result_source(
            &context, report.selection.get().unwrap()).await.unwrap();
        assert_eq!(current.source.revision, restored.revision);
        // Matching helper accounting never overrides the real contributor's
        // cancellation. This invokes the existing source cancellation owner.
        fixture.source.record_operation_cancelled(&fixture.submission.original).await.unwrap();
        assert!(matches!(fixture.source.setup_result_source(
            &context, report.selection.get().unwrap()).await, Err(PrivateOriginalError::Denied)));
    }))).catch_unwind().await;
    fixture.source.stop_custody_recovery();
    report.release();
    report.leaf.context.lock().unwrap().take();
    let cleanup = fixture.shutdown().await;
    let source_cleanup = fixture.source.join_custody_recovery().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    assert!(source_cleanup.is_ok(), "{source_cleanup:?}");
    match checked {
        Ok(Ok(())) => (),
        Ok(Err(_)) => panic!("helper result progress control exceeded its deadline"),
        Err(payload) => std::panic::resume_unwind(payload),
    }
}
