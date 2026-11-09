//! Actual service forwarding/refusal and retained native claim controls.
//! These exercise the production R5 call boundary but deliberately select an
//! unsupported WholeBlob store. They do not claim positive cold settlement,
//! full G restoration, or availability of R5 in the currently selected tuple.
use super::*;
use futures_util::FutureExt;
use meerkat_runtime::{
    LogicalRuntimeId, PredecessorSettlementRefusal, RuntimeSessionPersistenceProfile,
    store::{RuntimeStore, SqliteRuntimeStore},
};
use meerkat_session::ColdPredecessorSettleRefusal;
use std::{
    panic::{AssertUnwindSafe, resume_unwind},
    time::Duration,
};

#[tokio::test]
async fn cold_native_refusal_and_cancelled_observer_retain_same_real_claim_and_history() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    std::fs::create_dir(root.join("hosting")).unwrap();
    std::fs::create_dir(root.join("delivery")).unwrap();
    let store: Arc<dyn RuntimeStore> = Arc::new(
        SqliteRuntimeStore::new_whole_blob(root.join("runtime.sqlite3"))
            .unwrap()
            .with_hosting_paths(meerkat_core::session_hosting::HostingPaths {
                hosting_lock_dir: root.join("hosting"),
                cold_delivery_lock: root.join("delivery/cold.lock"),
                database: None,
            }),
    );
    let machine =
        Arc::new(meerkat_runtime::MeerkatMachine::persistent_without_blobs(store.clone()).unwrap());
    let other = meerkat_runtime::MeerkatMachine::persistent_without_blobs(store.clone()).unwrap();
    let service = Arc::new(meerkat_session::PersistentSessionService::new(
        meerkat::FactoryAgentBuilder::new(
            meerkat::AgentFactory::minimal(),
            meerkat::Config::default(),
        ),
        1,
        Arc::new(meerkat_store::MemoryStore::new()),
        store.clone(),
        Arc::new(meerkat_store::MemoryBlobStore::new()),
    ));
    let spec = meerkat_mobkit::MobBootstrapSpec::new(
        meerkat_mob::MobDefinition::explicit(meerkat_mob::MobId::from("cold-refusal")),
        meerkat_mob::MobStorage::in_memory(),
        service.clone(),
    )
    .with_session_runtime_adapter(machine.clone())
    .unwrap();
    let live = super::super::tests::uncaptured_live();
    live.capture_native(&spec).unwrap();
    let session = SessionId::new();
    let native = live.native_context().unwrap();
    let retained = Arc::new(tokio::sync::Mutex::new(ColdState {
        claim: Some(machine.grant_session_hosting(&session).unwrap()),
        ..Default::default()
    }));
    let slot = Arc::new(tokio::sync::Mutex::new(None));
    let caught = AssertUnwindSafe(async {
        tokio::time::timeout(Duration::from_secs(30), async {
            let mut state = retained.lock().await;
            assert!(state.claim.as_ref().unwrap().is_cross_process());
            assert!(Arc::ptr_eq(&native.service, &spec.session_service));
            assert!(Arc::ptr_eq(native.machine().unwrap(), &machine));
            let before = store.load_input_states_strict(&LogicalRuntimeId::for_session(&session)).await.unwrap();
            assert!(before.is_empty());
            for count in 1..=2 {
                let observed = state.observe_native(&native, &session,
                    Instant::now().checked_add(Duration::from_secs(5)).unwrap()).await.unwrap();
                let error = observed.as_ref().as_ref().expect_err("WholeBlob is explicitly unsupported by R5");
                assert!(matches!(&error.refusal, ColdPredecessorSettleRefusal::Settlement(
                    PredecessorSettlementRefusal::ProfileUnsupported { profile:RuntimeSessionPersistenceProfile::WholeBlobV1 })),
                    "the actual Persistent path must answer its profile refusal, not an omitted-forwarder default");
                assert_eq!(error.session_id, session);
                assert!(error.progress.inputs.is_empty() && error.progress.tail.is_none());
                assert!(error.progress.owner_loss.is_none() && error.progress.terminals_owed.is_empty());
                assert_eq!(state.native.len(), count);
                assert!(Arc::ptr_eq(state.native.last().unwrap(), &observed));
                drop(observed);
                assert!(other.grant_session_hosting(&session).is_err(),
                    "returning or dropping a failed observation must retain the actual external claim");
                assert!(state.claim.as_ref().unwrap().completed_owner_loss().is_none());
                assert!(machine.current_session_registration_witness(&session).await.is_none());
                assert!(store.load_input_states_strict(&LogicalRuntimeId::for_session(&session)).await.unwrap().is_empty());
            }
            assert!(!state.adoption_claimed && state.physical.is_none() && state.result.is_none());
            drop(state);
            // Hold the retained task before its actual native call. This
            // proves disposable observer cancellation does not drop the task
            // or external claim; it does not simulate native in-flight work.
            let (release, released) = tokio::sync::oneshot::channel();
            let owned = retained.clone(); let captured = native.clone(); let id = session.clone();
            *slot.lock().await = Some(tokio::spawn(async move {
                released.await.map_err(|_| HostLifetimeError::NotSettled)?;
                let observed = owned.lock().await.observe_native(&captured, &id,
                    Instant::now().checked_add(Duration::from_secs(5)).unwrap()).await?;
                if observed.is_err() { Err(HostLifetimeError::NotSettled) } else { Ok(()) }
            }));
            let mut observer = Box::pin(async {
                let mut held = slot.lock().await;
                join_native_attempt(&mut held).await
            });
            assert!(futures_util::poll!(observer.as_mut()).is_pending());
            drop(observer);
            assert!(slot.lock().await.is_some());
            assert!(other.grant_session_hosting(&session).is_err());
            release.send(()).unwrap();
            {
                let mut held = slot.lock().await;
                assert_eq!(join_native_attempt(&mut held).await, Err(HostLifetimeError::NotSettled));
            }
            assert!(slot.lock().await.is_none());
            let state = retained.lock().await;
            assert_eq!(state.native.len(), 3);
            assert!(matches!(&state.native[2].as_ref().as_ref().err().unwrap().refusal,
                ColdPredecessorSettleRefusal::Settlement(PredecessorSettlementRefusal::ProfileUnsupported { .. })));
            assert!(!state.adoption_claimed && state.physical.is_none() && state.result.is_none());
            assert!(machine.current_session_registration_witness(&session).await.is_none());
        }).await.expect("bounded actual native refusal control");
    }).catch_unwind().await;
    // Cleanup is joined after the caught body, including timeout/assertion
    // failure. This isolated-fixture emergency path is not a recovery proof.
    let remaining = {
        let mut held = slot.lock().await;
        if held.is_some() {
            Some(join_native_attempt(&mut held).await)
        } else {
            None
        }
    };
    let emergency =
        if let Some(witness) = machine.current_session_registration_witness(&session).await {
            machine
                .unregister_session_registration_until_terminal_if_current(&witness)
                .await
                .map(|_| ())
        } else {
            Ok(())
        };
    let shutdown = service.try_shutdown().await;
    let closed = live
        .join_after_native_close(emergency.is_ok() && shutdown.is_ok())
        .await;
    if let Err(panic) = caught {
        resume_unwind(panic);
    }
    if let Some(result) = remaining {
        assert!(result.is_err(), "the retained WholeBlob call must refuse");
    }
    emergency.unwrap();
    shutdown.unwrap();
    closed.unwrap();
    assert!(
        other.grant_session_hosting(&session).is_err(),
        "service-reference retirement did not drop the retained claim"
    );
    drop(retained);
    let next = other.grant_session_hosting(&session).unwrap();
    assert!(next.is_cross_process());
}

#[test]
fn abandoned_adoption_fence_survives_reopen_and_never_retries_uncertain_preparation() {
    let temp = tempfile::tempdir().unwrap();
    let directory = temp.path().canonicalize().unwrap();
    // Inert source artifacts test only the durable one-shot file boundary. The
    // test-local private wrapper is not asserted to be a native/consent proof.
    let association =
        write_private(&directory, "association.json", b"selected association").unwrap();
    let raw = toolkit_package::canonical_json(&json!({"association":association,
        "scope":"abandoned_native_settled_consent_withdrawn_physical_not_entered"}))
    .unwrap();
    let aggregate = AbandonedNoEntryDisposition {
        association: association.clone(),
        artifact: write_private(&directory, "aggregate.json", &raw).unwrap(),
    };
    claim_adoption(&association, &aggregate).unwrap();
    let path = directory.join(format!("abandoned-adoption-{}.json", association.sha256));
    let before = std::fs::read(&path).unwrap();
    let reopened: Value = serde_json::from_slice(&before).unwrap();
    assert_eq!(
        reopened["association"],
        serde_json::to_value(&association).unwrap()
    );
    assert_eq!(
        reopened["aggregate"],
        serde_json::to_value(&aggregate.artifact).unwrap()
    );
    assert!(claim_adoption(&association, &aggregate).is_err());
    let different = AbandonedNoEntryDisposition {
        association: association.clone(),
        artifact: write_private(&directory, "other-aggregate.json", &raw).unwrap(),
    };
    assert!(
        claim_adoption(&association, &different).is_err(),
        "later native convergence cannot rerun a started physical adoption"
    );
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert_eq!(
        read_selected(&association).unwrap(),
        b"selected association"
    );
    assert_eq!(read_selected(&aggregate.artifact).unwrap(), raw);
}

mod segmented;
