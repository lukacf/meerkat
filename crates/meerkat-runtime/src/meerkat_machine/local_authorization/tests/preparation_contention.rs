//! B1: coordination contention is not loss of controller permission.
//! These exercise actual preparation and row-bound audit, not HTTP transport.
use super::*;
use meerkat_authorization_contracts::audit::{
    AuditObservation, StoredAuthorizationAuditObservation,
};
use meerkat_core::authorization::{OperationObservedOutcome, PreparedOperationCheck};
use meerkat_core::{OperationAuthorizationError, OperationId};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::Duration;

#[derive(Clone, Copy)]
enum HeldCoordination {
    Sessions,
    Mutation,
    Driver,
    Administration,
}

fn observed_fixture_attempt(
    context: WorkAuthorizationContext,
    bound: PreparedAuthorizationBinding,
    bodies: &AtomicUsize,
) -> Result<(), OperationAuthorizationError> {
    let prepared = PreparedOperationCheck::prepare(context, bound)?;
    let current = prepared.current()?;
    current.observe_entry()?;
    // The recording transport fixture returns a real local error. This is not
    // a model response or a claim that any external provider was contacted.
    bodies.fetch_add(1, Ordering::SeqCst);
    let transport_result: Result<(), std::io::Error> =
        Err(std::io::Error::from(std::io::ErrorKind::ConnectionAborted));
    let outcome = match transport_result {
        Err(_) => OperationObservedOutcome::TransportError,
        Ok(()) => unreachable!("fixture has no successful transport branch"),
    };
    current.observe_outcome(outcome)?;
    Ok(())
}

fn exercise_while_held<G>(
    guard: G,
    context: WorkAuthorizationContext,
    bound: PreparedAuthorizationBinding,
    bodies: Arc<AtomicUsize>,
) -> (
    Result<Result<(), OperationAuthorizationError>, RecvTimeoutError>,
    bool,
) {
    let (tx, rx) = mpsc::sync_channel(1);
    let worker = std::thread::spawn(move || {
        let result = observed_fixture_attempt(context, bound, &bodies);
        // No assertion/panic may bypass the main thread's release and join.
        let _ = tx.send(result);
    });
    let before_release = rx.recv_timeout(Duration::from_secs(2));
    // Always release the actual native coordination guard before joining,
    // including baseline refusal, worker panic and the bounded timeout case.
    drop(guard);
    let joined = worker.join().is_ok();
    // In particular, a success produced only after release is NOT substituted
    // for a timeout. That would hide an implementation that reacquires G.
    (before_release, joined)
}

fn records(
    audit: &crate::input_audit::InputAuthorizationAudit,
) -> Vec<StoredAuthorizationAuditObservation> {
    serde_json::from_value(serde_json::to_value(audit).expect("actual native audit snapshot"))
        .expect("typed native audit observations")
}

fn assert_complete_attempt(
    all: &[StoredAuthorizationAuditObservation],
    operation: &OperationId,
    input: &meerkat_core::InputId,
    run: &meerkat_core::RunId,
) {
    let rows: Vec<_> = all
        .iter()
        .filter(|row| row.observation.operation_id == *operation)
        .collect();
    assert_eq!(
        rows.len(),
        3,
        "exactly Prepared, Entry and Outcome, no refusal"
    );
    assert!(matches!(
        rows[0].observation.observation,
        AuditObservation::Prepared { .. }
    ));
    assert!(matches!(
        rows[1].observation.observation,
        AuditObservation::Entry
    ));
    assert!(matches!(
        rows[2].observation.observation,
        AuditObservation::Outcome {
            outcome: OperationObservedOutcome::TransportError
        }
    ));
    for row in rows {
        assert_eq!(row.contributors.len(), 1);
        assert_eq!(&row.contributors[0].input_id, input);
        assert_eq!(row.observation.run_id.as_ref(), Some(run));
    }
}

async fn assert_coordination_is_not_permission(which: HeldCoordination) {
    let (configuration, prompt, _, publication) = mutable_controller_configuration(true);
    let (machine, session, prompt) = pending_controller_input(configuration, prompt).await;
    install_owner_fixture_credential(&machine, &prompt);
    let (input, run, context) = stage_controller_input(&machine, &session, prompt).await;
    let (driver, mutation) = {
        let sessions = machine.sessions.read().await;
        let entry = sessions.get(&session).expect("actual registered session");
        (Arc::clone(&entry.driver), Arc::clone(&entry.mutation_gate))
    };
    let (audit, retained_contributors) = {
        let locked = driver.lock().await;
        let DriverEntry::Ephemeral(driver) = &*locked else {
            panic!("storeless native fixture")
        };
        let row = driver.ledger().get(&input).expect("actual accepted row");
        (
            row.authorization_audit.clone(),
            row.authority_contributors.clone(),
        )
    };
    assert!(records(&audit).is_empty(), "setup performs no operation");
    let bodies = Arc::new(AtomicUsize::new(0));
    let positive = plain_controller_binding(&context, &run);
    let positive_id = positive.facts().operation_id.clone();
    observed_fixture_attempt(context.clone(), positive, &bodies)
        .expect("permitted positive control");
    assert_complete_attempt(&records(&audit), &positive_id, &input, &run);
    assert_eq!(bodies.load(Ordering::SeqCst), 1);

    // A second registered session makes Administration an actual multi-session
    // scan. It has no input and cannot confer permission on the active session.
    if matches!(which, HeldCoordination::Administration) {
        machine
            .register_session(SessionId::new())
            .await
            .expect("unrelated registered session");
    }
    let ((), stamp) = publication
        .observe(|| ())
        .expect("stable publication before coordination");
    let attempt = plain_controller_binding(&context, &run);
    let attempt_id = attempt.facts().operation_id.clone();
    let (before_release, joined) = match which {
        HeldCoordination::Sessions => exercise_while_held(
            machine.sessions.write().await,
            context.clone(),
            attempt,
            Arc::clone(&bodies),
        ),
        HeldCoordination::Mutation => exercise_while_held(
            mutation.lock().await,
            context.clone(),
            attempt,
            Arc::clone(&bodies),
        ),
        HeldCoordination::Driver => exercise_while_held(
            driver.lock().await,
            context.clone(),
            attempt,
            Arc::clone(&bodies),
        ),
        HeldCoordination::Administration => exercise_while_held(
            machine
                .try_controller_grant_mutation()
                .expect("actual all-session administrative custody"),
            context.clone(),
            attempt,
            Arc::clone(&bodies),
        ),
    };
    // All assertions happen after guard release and worker join. An ordinary
    // baseline refusal cannot strand custody or turn cleanup into a second failure.
    assert!(joined, "worker must not panic");
    stamp.check_current().expect("no policy mutation occurred");
    {
        let locked = driver.lock().await;
        let DriverEntry::Ephemeral(driver) = &*locked else {
            panic!("storeless native fixture")
        };
        assert_eq!(driver.current_run_id().as_ref(), Some(&run));
        assert_eq!(
            driver
                .ledger()
                .get(&input)
                .expect("row retained")
                .authority_contributors,
            retained_contributors,
            "contention must not alter actual input authority"
        );
    }
    // Continue independently after release even on the baseline. This ensures
    // the fixture did not configure a denied controller or destroy the run.
    let after = plain_controller_binding(&context, &run);
    let after_id = after.facts().operation_id.clone();
    observed_fixture_attempt(context, after, &bodies)
        .expect("same controller usable after release");
    assert_complete_attempt(&records(&audit), &after_id, &input, &run);
    assert!(
        matches!(before_release, Ok(Ok(()))),
        "unchanged controller must finish before releasing coordination custody: {before_release:?}"
    );
    assert_eq!(
        bodies.load(Ordering::SeqCst),
        3,
        "one body per permitted attempt"
    );
    let all = records(&audit);
    assert_complete_attempt(&all, &attempt_id, &input, &run);
    assert_eq!(all.len(), 9, "no hidden retry or extra refusal observation");
}

#[tokio::test]
async fn controller_prepare_ignores_sessions_coordination_contention() {
    assert_coordination_is_not_permission(HeldCoordination::Sessions).await;
}

#[tokio::test]
async fn controller_prepare_ignores_mutation_coordination_contention() {
    assert_coordination_is_not_permission(HeldCoordination::Mutation).await;
}

#[tokio::test]
async fn controller_prepare_ignores_driver_coordination_contention() {
    assert_coordination_is_not_permission(HeldCoordination::Driver).await;
}

#[tokio::test]
async fn controller_prepare_ignores_unrelated_administration_scan() {
    assert_coordination_is_not_permission(HeldCoordination::Administration).await;
}
