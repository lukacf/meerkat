//! One cold selected-interest case on the existing real stdio Ready/close driver.
//! The test holds the real writer once; only the production worker revisits it.
use super::*;
use crate::private_originals::{
    governed_submit::tests::first_input_tests::dedicated::{DedicatedFixture, DurableDedicated},
    setup_workflow::launch::tests::waiter_restore_tests::selected_fixture_then_close,
};
use meerkat_authorization_contracts::work_association::GrantTarget;
use meerkat_mobkit::identity_first::IdentityLifecycleState;

const TEST: &str = concat!(
    module_path!(),
    "::actual_ready_recovery_revisits_source_writer_contention_without_new_work"
);

// Same persisted runtime census as the metadata case, with original messages
// instead of incidental session timestamps that a genuine actor resume may update.
async fn work_census(directory: &Path, sessions: &[SessionId]) -> Value {
    let mut census = persisted(directory, sessions).await;
    for record in census["records"].as_array_mut().unwrap() {
        let messages = record["session"]["messages"]
            .as_array()
            .expect("actual committed Session messages")
            .clone();
        record["session"] = json!({"messages": messages});
    }
    census
}

async fn child() {
    let seed = DedicatedFixture::persistent_effect_waiter_for_gateway().await;
    let old_source = Arc::downgrade(&seed.f.source);
    let old_machine = Arc::downgrade(&seed.f.machine);
    let saved = selected_fixture_then_close(seed, true).await;
    assert!(
        old_source.upgrade().is_none() && old_machine.upgrade().is_none(),
        "cold means the old native and source owners are gone"
    );
    let mut reopened = saved.durable.reopen_stdio_host().await;
    let directory = reopened.directory.clone();
    let sessions = saved.durable.sessions().to_vec();
    let before = work_census(directory.path(), &sessions).await;
    let source = reopened.originals.clone();
    let store = reopened.store.clone();
    let probe = Arc::new(MetadataRecoveryProbe::for_interest(saved.row.key.clone()));
    assert!(source.metadata_probe.set(probe.clone()).is_ok());
    let _release = Release(probe.clone());
    let grants_path = reopened.source_path.parent().unwrap().join("grants.jsonl");
    let grants = std::fs::read(&grants_path).unwrap();
    let workflow = source
        .workflow_store()
        .unwrap()
        .load_setup_workflow(saved.membership.workflow())
        .await
        .unwrap();
    let delivery = store
        .get_effect(&saved.membership.delivery_effect().unwrap())
        .await
        .unwrap()
        .unwrap();
    let (config, roster) = saved.durable.gateway_configuration();
    let (model, requests) = DurableDedicated::gateway_model();
    let (mut host, _private_open) = recovery_host(&mut reopened, &saved.durable);
    host.model = Some(model.clone());
    host.tool_probe = Some(Arc::new(
        crate::installed_host::acquisition::ContextProbe::default(),
    ));
    host.definition = Some(meerkat_mob::MobDefinition::from_toml(config).unwrap());
    let member = host
        .gateway
        .reserve_workflow_member(
            saved.durable.terms(),
            saved.durable.lead_source(),
            &roster[2],
        )
        .await
        .unwrap();
    assert!(
        member.session().is_err(),
        "selection cannot fabricate native capture or attachment"
    );
    let host = Arc::new(host);
    println!(
        "{}",
        json!({"fixture":"prepared", "state":directory.path(), "mob_config":config, "roster":roster})
    );
    let observed_host = host.clone();
    let observed_source = source.clone();
    let observed_probe = probe.clone();
    let observed_directory = directory.clone();
    let observed_sessions = sessions.clone();
    let expected_before = before.clone();
    let observer = tokio::spawn(async move {
        let _release = Release(observed_probe.clone());
        let checked = AssertUnwindSafe(async {
            until(|| observed_host.ready.load(Ordering::SeqCst)).await;
            observed_probe.ready.store(true, Ordering::SeqCst);
            assert!(!observed_host.closing.load(Ordering::SeqCst));
            let runtime = observed_source.bound_identity_runtime().unwrap();
            let machine = observed_source.machine().unwrap();
            let pins = observed_source.controller_pin_source.get().and_then(std::sync::Weak::upgrade).unwrap();
            let handle = runtime.owner().handle();
            for (index, spec) in roster.iter().enumerate() {
                let status = runtime.status(&spec.identity).await.unwrap();
                assert_eq!(status.state, IdentityLifecycleState::Active);
                assert_eq!(status.session_id.as_ref(), Some(&observed_sessions[index]),
                    "actual roster restoration must keep each original session, with no fresh fallback");
                // Residency-neutral: each restored member is seated on its
                // exact original session, its durable binding. The default
                // on-demand restore may leave a member Dormant or live, which
                // is the owner's internal state, so neither is asserted.
                let member = meerkat_mob::AgentIdentity::from(
                    status.member_id.as_deref().expect("a seated roster member"),
                );
                assert_eq!(handle.resolve_bridge_session_id(&member).await.as_ref(), Some(&observed_sessions[index]),
                    "each member stays bound to its original session");
            }
            // The pin below needs G's live actor. When no live actor serves
            // G's session, wake exactly that session through Core's
            // session-owner wake: no input and no turn.
            if pins.live_actor(&observed_sessions[2]).await.is_none() {
                let woke = meerkat_core::session_owner_wake::SessionOwnerWakeRegistry::global()
                    .wake(&observed_sessions[2])
                    .await;
                assert!(
                    matches!(woke, Some(Ok(meerkat_core::session_owner_wake::SessionOwnerWakeOutcome::Ready))),
                    "the restored lead wakes on its own session: {woke:?}"
                );
            }
            assert!(machine.current_session_registration_witness(&observed_sessions[2]).await.is_some());
            // Reuse the seed's controlled credential on the new actual native
            // owner, selecting its identity from the real reconstructed pin.
            let actor = pins.live_actor(&observed_sessions[2]).await.unwrap();
            let pin = pins.pin_actor(&actor).await.unwrap();
            assert_eq!(Some(pin.selection()), model.controller_model_selection().as_ref());
            meerkat_core::publish_token_lifecycle_acquired_for_identity_within(
                &machine.generated_auth_lease_handle(), pin.selection().credential(),
                &meerkat_core::auth::PersistedTokens::api_key("synthetic-controller-only-fixture"),
                tokio::time::Instant::now() + WAIT,
            ).await.unwrap();
            tokio::time::timeout(WAIT, observed_probe.wait_for(|p| p.interest_boundary.load(Ordering::SeqCst))).await.unwrap();
            let writer = observed_source.owner.source_writer().await;
            assert!(observed_source.owner.try_source_writer().is_err(), "the real owner writer is held");
            observed_probe.release_interest();
            tokio::time::timeout(WAIT, observed_probe.wait_for(|p|
                p.writer_refusals.load(Ordering::SeqCst) >= 1
                    && p.interest_unavailable.load(Ordering::SeqCst) >= 1)).await.unwrap();
            assert_eq!(observed_probe.interest_restored.load(Ordering::SeqCst), 0);
            assert!(member.session().is_err(), "the refused attempt published no G attachment");
            drop(writer);
            // No direct restore call, worker wake or retry is made by the test.
            tokio::time::timeout(WAIT, observed_probe.wait_for(|p|
                p.interest_restored.load(Ordering::SeqCst) == 1
                    && p.custody_rounds.load(Ordering::SeqCst) > 0)).await.unwrap();
            assert_eq!(member.session().unwrap(), observed_sessions[2]);
            let held = observed_source.retained.lock().unwrap().iter()
                .find(|held| held.reference == *saved.durable.lead_source()).unwrap().clone();
            let attached = held.setup_workflow.get().unwrap();
            let waiters = attached.waiters.lock().unwrap().clone();
            assert_eq!(waiters.len(), 2, "the two exact already-issued MCP target relationships");
            assert!(waiters.iter().all(|waiter| waiter.membership == saved.membership));
            for server in ["restore-read-a", "restore-read-b"] {
                let target = GrantTarget::McpServer { server: server.into() };
                assert_eq!(waiters.iter().filter(|waiter| waiter.authority.target() == &target).count(), 1);
            }
            let publication = observed_source.owner.publication();
            let (_, stamp) = publication.observe(|| ()).unwrap();
            let rounds = observed_probe.custody_rounds.load(Ordering::SeqCst);
            observed_probe.release_replay();
            tokio::time::timeout(WAIT, observed_probe.wait_for(|p|
                p.interest_restored.load(Ordering::SeqCst) >= 2
                    && p.custody_rounds.load(Ordering::SeqCst) > rounds)).await.unwrap();
            stamp.check_current().expect("the worker's exact replay cannot republish replacement custody");
            {
                let again = attached.waiters.lock().unwrap();
                assert_eq!(again.len(), waiters.len());
                assert!(again.iter().zip(&waiters).all(|(a, b)| Arc::ptr_eq(a, b)));
            }
            assert_eq!(std::fs::read(&grants_path).unwrap(), grants, "no newly issued or replacement grant");
            assert_eq!(store.get(&saved.row.key).await.unwrap().unwrap(), saved.row);
            assert_eq!(observed_source.workflow_store().unwrap().load_setup_workflow(saved.membership.workflow()).await.unwrap().revision(), workflow.revision());
            assert_eq!(store.get_effect(&delivery.key).await.unwrap().unwrap(), delivery, "no result or delivery changed");
            assert_eq!(work_census(observed_directory.path(), &observed_sessions).await, expected_before,
                "no duplicate input or original model/tool history");
            assert_eq!(requests(), 0, "actual restoration and recovery make no model request");
            println!("{}", json!({"fixture":"fairness", "writer_refusal_observed":true,
                "acquisition_revisited":true, "custody_completed_rounds":observed_probe.custody_rounds.load(Ordering::SeqCst)}));
            until(|| observed_host.closing.load(Ordering::SeqCst)).await;
            tokio::time::timeout(WAIT, observed_host.join_close()).await.unwrap().unwrap();
            assert_eq!(requests(), 0);
            assert_eq!(std::fs::read(&grants_path).unwrap(), grants);
            assert_eq!(store.get(&saved.row.key).await.unwrap().unwrap(), saved.row);
            assert_eq!(store.get_effect(&delivery.key).await.unwrap().unwrap(), delivery);
        }).catch_unwind().await;
        if checked.is_err() {
            let _ = observed_host.begin_close(GatewayHostCloseReason::Shutdown);
        }
        checked
    });
    let outcome = meerkat_mobkit::rpc_gateway::run_persistent_with_host(None, host.clone()).await;
    probe.release();
    let checked = observer.await.expect("the retained observer joins");
    let cleanup = AssertUnwindSafe(async {
        assert!(
            outcome.is_ok(),
            "native driver and physical cleanup: {outcome:?}"
        );
        host.join_close().await.unwrap();
        assert_eq!(work_census(directory.path(), &sessions).await, before);
        assert!(source.retained.lock().unwrap().is_empty());
        assert!(source.workflow_context_transfers().unwrap().is_empty());
    })
    .catch_unwind()
    .await;
    if let Err(panic) = checked {
        std::panic::resume_unwind(panic);
    }
    if let Err(panic) = cleanup {
        std::panic::resume_unwind(panic);
    }
    println!("{}", json!({"fixture":"joined"}));
}

#[test]
fn actual_ready_recovery_revisits_source_writer_contention_without_new_work() {
    if std::env::var_os(CHILD).is_some() {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .enable_all()
            .build()
            .unwrap()
            .block_on(Box::pin(child()));
        return;
    }
    run_parent(TEST);
}
