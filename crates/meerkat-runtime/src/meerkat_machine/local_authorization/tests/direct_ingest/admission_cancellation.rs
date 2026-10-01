//! Ordinary admission cancellation stops at the real credential-custody boundary.
//! submit_bounded deliberately owns a different observation-only timeout contract.
use super::*;
use crate::meerkat_machine::credential_custody::acquisition_test_observer::{
    Observation as AcquisitionObservation, Stage,
};
use crate::service_ext::SessionServiceRuntimeExt;
use crate::traits::{RuntimeControlPlaneError, RuntimeDriverError};

#[derive(Clone, Copy, Debug)]
enum Route {
    WithCompletion,
    WithoutWake,
    Ingest,
}

enum SubmitError {
    Driver(RuntimeDriverError),
    Control(RuntimeControlPlaneError),
}

impl std::fmt::Debug for SubmitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Driver(error) => std::fmt::Debug::fmt(error, f),
            Self::Control(error) => std::fmt::Debug::fmt(error, f),
        }
    }
}

async fn submit(
    machine: Arc<MeerkatMachine>,
    session: SessionId,
    runtime: LogicalRuntimeId,
    input: Input,
    route: Route,
) -> Result<AcceptOutcome, SubmitError> {
    match route {
        Route::WithCompletion => machine
            .accept_input_with_completion(&session, input)
            .await
            .map(|(outcome, _)| outcome)
            .map_err(SubmitError::Driver),
        Route::WithoutWake => machine
            .accept_input_without_wake(&session, input)
            .await
            .map_err(SubmitError::Driver),
        Route::Ingest => machine
            .ingest(&runtime, input)
            .await
            .map_err(SubmitError::Control),
    }
}

async fn fixture() -> (Arc<MeerkatMachine>, AttachedSession, Input, LeaseKey) {
    let (configuration, template, _, _) = mutable_controller_configuration(true);
    let machine = Arc::new(
        MeerkatMachine::ephemeral()
            .with_local_grant_authorization(configuration)
            .expect("actual storeless host"),
    );
    let session = AttachedSession::attach(&machine).await;
    // Distinct actual lease identity for each test, including parallel runs.
    let selection = selected(
        &template,
        "candidate-controller",
        &format!("cancel-{}", session.session),
    );
    let key = LeaseKey::from_credential_identity(selection.credential());
    let store = EphemeralTokenStore::new();
    install_credential(&machine, &store, &selection).await;
    let mut input = input_for(&template, &session, selection.clone());
    let observed = input
        .header()
        .ingress_context
        .as_ref()
        .expect("trusted ingress")
        .clone();
    // A real key makes the old orphaned acceptance mechanically deduplicate.
    // Rebind the trusted observation after editing the exact admitted header.
    input.header_mut().idempotency_key = Some(crate::identifiers::IdempotencyKey::new(format!(
        "ordinary-cancellation-{}",
        input.id()
    )));
    let ingress = NativeIngressContext::from_trusted_ingress(
        &input,
        observed.requester().clone(),
        observed.ingress_actor().clone(),
        observed.realm().clone(),
        observed.authentication().clone(),
    )
    .expect("exact keyed input observation")
    .with_controller_client(
        &input,
        ControllerModelClient::new(selection.clone(), Arc::new(SelectedClient(selection))),
    )
    .expect("same actual immutable selected child");
    let input = input
        .with_ingress_context(ingress)
        .expect("exact final keyed input");
    (machine, session, input, key)
}

async fn row_exists(machine: &MeerkatMachine, session: &SessionId, input: &InputId) -> bool {
    let driver = machine
        .sessions
        .read()
        .await
        .get(session)
        .expect("actual entry")
        .driver
        .clone();
    let locked = driver.lock().await;
    let DriverEntry::Ephemeral(driver) = &*locked else {
        panic!("storeless")
    };
    driver.ledger().get(input).is_some()
}

async fn lease_fence(key: &LeaseKey) {
    // This acquisition is queued after caller abort/join. Because Waiting was
    // emitted only after the real mutex future returned Pending, an orphaned
    // earlier admission must finish with its guard before this fence returns.
    let result =
        tokio::time::timeout(BOUND, meerkat_core::acquire_auth_login_lifecycle_guard(key)).await;
    let succeeded = result.is_ok();
    drop(result);
    assert!(
        succeeded,
        "same-owner fence must settle; no sleeps or detached polling"
    );
}

async fn complete_same_input(
    machine: &Arc<MeerkatMachine>,
    session: &mut AttachedSession,
    input: &InputId,
    route: Route,
) {
    if matches!(route, Route::WithoutWake) {
        assert!(
            machine
                .wake_runtime_if_active_inputs(&session.session)
                .await
                .expect("explicit real wake")
        );
    }
    let started = session.start().await;
    assert_eq!(started.ids, vec![input.clone()]);
    let prepared = meerkat_core::authorization::PreparedOperationCheck::prepare(
        started.context.clone(),
        plain_controller_binding(&started.context, &started.run),
    )
    .expect("actual admitted controller permission");
    prepared
        .current()
        .expect("actual same-run controller currentness");
    session.finish(machine, input).await;
}

fn assert_admission_effects(effects: &[dsl::MeerkatMachineEffect], route: Route) {
    if matches!(route, Route::Ingest) {
        assert!(
            effects
                .iter()
                .any(|effect| matches!(effect, dsl::MeerkatMachineEffect::ResolveAdmission))
        );
    }
    assert_eq!(
        effects
            .iter()
            .filter(|effect| matches!(effect, dsl::MeerkatMachineEffect::IngressAccepted))
            .count(),
        1,
        "one actual admission, not a preview or duplicate acknowledgement"
    );
}

async fn cancel_before_custody(route: Route) {
    let (machine, mut session, input, key) = fixture().await;
    let id = input.id().clone();
    let before = machine
        .session_dsl_state(&session.session)
        .await
        .expect("actual before state");
    let effects = Observation::install(dsl::SessionId::from_domain(&session.session));
    let mut acquisition = AcquisitionObservation::install(&id);
    let held = meerkat_core::acquire_auth_login_lifecycle_guard(&key).await;
    let caller = tokio::spawn(submit(
        machine.clone(),
        session.session.clone(),
        session.runtime.clone(),
        input.clone(),
        route,
    ));
    let observed = tokio::time::timeout(BOUND, acquisition.next()).await;
    caller.abort();
    let joined = caller.await;
    drop(held);
    lease_fence(&key).await;
    // All real guards are released before assertions. In the baseline, the
    // orphaned owned task has now reached admission and is observable here.
    assert!(
        matches!(observed, Ok(Some(Stage::Waiting))),
        "must reach the actual owner's pending poll: {observed:?}"
    );
    assert!(
        matches!(joined, Err(error) if error.is_cancelled()),
        "the ordinary caller was actually dropped"
    );
    let after_cancel = machine
        .session_dsl_state(&session.session)
        .await
        .expect("actual after state");
    let canceled_row_exists = row_exists(&machine, &session.session, &id).await;
    let canceled_effects = effects.effects();
    let canceled_calls = session.calls.load(Ordering::SeqCst);
    drop(effects);
    drop(acquisition);

    // Reach the positive body/completion even on the old implementation when
    // the orphaned task caused a duplicate. Only a fresh Accepted is success.
    let positive_effects = Observation::install(dsl::SessionId::from_domain(&session.session));
    let accepted = tokio::time::timeout(
        BOUND,
        submit(
            machine.clone(),
            session.session.clone(),
            session.runtime.clone(),
            input,
            route,
        ),
    )
    .await
    .expect("same-input retry bounded")
    .expect("same legitimate input remains usable");
    complete_same_input(&machine, &mut session, &id, route).await;
    let positive_effects = positive_effects.effects();
    assert!(
        !canceled_row_exists,
        "canceled pre-custody caller must not create a native input row"
    );
    assert!(
        canceled_effects.is_empty(),
        "canceled pre-custody caller emitted actual effects: {canceled_effects:?}"
    );
    assert_eq!(
        after_cancel, before,
        "no admission or lifecycle mutation before the retry"
    );
    assert_eq!(
        canceled_calls, 0,
        "no executor body from the canceled caller"
    );
    assert!(
        matches!(accepted, AcceptOutcome::Accepted { input_id, .. } if input_id == id),
        "same-input positive must be newly Accepted, not Deduplicated"
    );
    assert_admission_effects(&positive_effects, route);
}

async fn drop_after_custody(route: Route) {
    let (machine, mut session, input, key) = fixture().await;
    let id = input.id().clone();
    let mutation = machine
        .sessions
        .read()
        .await
        .get(&session.session)
        .expect("actual entry")
        .mutation_gate
        .clone();
    let held = mutation.lock().await;
    let effects = Observation::install(dsl::SessionId::from_domain(&session.session));
    let mut acquisition = AcquisitionObservation::install(&id);
    let caller = tokio::spawn(submit(
        machine.clone(),
        session.session.clone(),
        session.runtime.clone(),
        input,
        route,
    ));
    let observed = tokio::time::timeout(BOUND, acquisition.next()).await;
    // Current-thread test runtime: Acquired adds no await after Poll::Ready.
    // The caller runs through the owned handoff before this task is repolled.
    // The real native mutation gate, not an observation hook, holds publication.
    caller.abort();
    let joined = caller.await;
    let row_before_release = row_exists(&machine, &session.session, &id).await;
    let effects_before_release = effects.effects();
    drop(held);
    lease_fence(&key).await;
    assert!(
        matches!(observed, Ok(Some(Stage::Acquired))),
        "actual custody reached without a pending fake: {observed:?}"
    );
    assert!(
        matches!(joined, Err(error) if error.is_cancelled()),
        "caller acknowledgement dropped"
    );
    assert!(
        !row_before_release && effects_before_release.is_empty(),
        "native publication waited on the real gate"
    );
    assert!(
        row_exists(&machine, &session.session, &id).await,
        "the post-custody owned transaction must publish despite lost acknowledgement"
    );
    assert_admission_effects(&effects.effects(), route);
    drop(acquisition);
    drop(effects);
    complete_same_input(&machine, &mut session, &id, route).await;
}

#[tokio::test(flavor = "current_thread")]
async fn ordinary_accept_with_completion_cancels_before_credential_custody() {
    cancel_before_custody(Route::WithCompletion).await;
}
#[tokio::test(flavor = "current_thread")]
async fn ordinary_accept_without_wake_cancels_before_credential_custody() {
    cancel_before_custody(Route::WithoutWake).await;
}
#[tokio::test(flavor = "current_thread")]
async fn ordinary_ingest_cancels_before_credential_custody() {
    cancel_before_custody(Route::Ingest).await;
}
#[tokio::test(flavor = "current_thread")]
async fn ordinary_accept_with_completion_stays_owned_after_credential_custody() {
    drop_after_custody(Route::WithCompletion).await;
}
#[tokio::test(flavor = "current_thread")]
async fn ordinary_accept_without_wake_stays_owned_after_credential_custody() {
    drop_after_custody(Route::WithoutWake).await;
}
#[tokio::test(flavor = "current_thread")]
async fn ordinary_ingest_stays_owned_after_credential_custody() {
    drop_after_custody(Route::Ingest).await;
}
