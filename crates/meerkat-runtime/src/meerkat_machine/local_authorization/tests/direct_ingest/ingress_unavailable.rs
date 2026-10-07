//! The installed ingress owner can be unavailable without claiming a denial.
use super::*;
use std::sync::atomic::AtomicBool;

#[tokio::test]
async fn direct_ingest_unavailable_ingress_preserves_readiness_then_same_input_completes() {
    let (mut configuration, template, _, _) = mutable_controller_configuration(true);
    let underlying_ingress = configuration.ingress.clone();
    let available = Arc::new(AtomicBool::new(false));
    let owner_available = available.clone();
    configuration.ingress = Arc::new(move |runtime, input, current, association| {
        // Keep the real fixture's exact caller/mandate checks before injecting
        // a current-owner outage. This flag is not an authorization allowance.
        underlying_ingress(runtime, input, current, association)?;
        if !owner_available.load(Ordering::SeqCst) {
            return Err(crate::input_authority::NativeAdmissionError::Readiness(
                crate::traits::ControllerReadinessFailure::PolicyUnavailable,
            ));
        }
        Ok(())
    });
    let machine = Arc::new(
        MeerkatMachine::ephemeral()
            .with_local_grant_authorization(configuration)
            .expect("actual native/grant owner"),
    );
    let mut attached = AttachedSession::attach(&machine).await;
    let selection = selected(&template, "candidate-controller", "candidate-controller");
    let store = Arc::new(EphemeralTokenStore::new());
    install_credential(&machine, &store, &selection).await;
    let input = input_for(&template, &attached, selection);
    let id = input.id().clone();
    let before = machine
        .session_dsl_state(&attached.session)
        .await
        .expect("actual state");
    let observation = Observation::install(dsl::SessionId::from_domain(&attached.session));
    let unavailable = tokio::time::timeout(BOUND, machine.ingest(&attached.runtime, input.clone()))
        .await
        .expect("admission returns without an owner retry loop");
    assert!(
        matches!(
            &unavailable,
            Err(
                crate::traits::RuntimeControlPlaneError::ControllerReadinessUnavailable {
                    reason: crate::traits::ControllerReadinessFailure::PolicyUnavailable,
                }
            )
        ),
        "actual outer admission preserves ingress unavailability: {unavailable:?}"
    );
    assert!(
        observation.effects().is_empty(),
        "no generated admission effect"
    );
    let after = machine
        .session_dsl_state(&attached.session)
        .await
        .expect("session retained");
    assert_eq!(after.lifecycle_phase, before.lifecycle_phase);
    assert_eq!(after.current_run_id, before.current_run_id);
    assert!(!after.input_phases.contains_key(&id.to_string()));
    assert!(!after.input_authority_bindings.contains_key(&id.to_string()));
    let driver = machine
        .sessions
        .read()
        .await
        .get(&attached.session)
        .expect("actual session")
        .driver
        .clone();
    {
        let locked = driver.lock().await;
        let DriverEntry::Ephemeral(driver) = &*locked else {
            panic!("storeless owner");
        };
        assert!(driver.ledger().get(&id).is_none(), "no accepted row");
    }
    assert_eq!(
        attached.calls.load(Ordering::SeqCst),
        0,
        "no executor entry"
    );

    // The same request and its current credential/grants remain usable once
    // the owner recovers. No new session, permission grant or input ID is used.
    available.store(true, Ordering::SeqCst);
    let accepted = machine
        .ingest(&attached.runtime, input)
        .await
        .expect("restored ingress");
    assert!(matches!(accepted, AcceptOutcome::Accepted { input_id, .. } if input_id == id));
    let started = attached.start().await;
    assert_eq!(started.ids, vec![id.clone()]);
    let check = meerkat_core::authorization::PreparedOperationCheck::prepare(
        started.context.clone(),
        plain_controller_binding(&started.context, &started.run),
    )
    .expect("actual current native/grant/account policy after recovery");
    check.current().expect("current retained work");
    attached.finish(&machine, &id).await;
}
