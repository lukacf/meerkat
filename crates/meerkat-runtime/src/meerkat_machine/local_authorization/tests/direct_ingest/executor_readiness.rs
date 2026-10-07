//! Actual registered-but-unattached admission. No support flag is set by tests.
use super::*;
use crate::service_ext::SessionServiceRuntimeExt;
use crate::traits::{ControllerReadinessFailure, RuntimeControlPlaneError, RuntimeDriverError};

#[derive(Clone, Copy, Debug)]
enum Route {
    AcceptWithCompletion,
    Ingest,
}

#[derive(Debug)]
enum Failure {
    Driver(RuntimeDriverError),
    Control(RuntimeControlPlaneError),
}

impl Failure {
    fn readiness(&self) -> Option<&ControllerReadinessFailure> {
        match self {
            Self::Driver(RuntimeDriverError::ControllerReadinessUnavailable { reason })
            | Self::Control(RuntimeControlPlaneError::ControllerReadinessUnavailable { reason }) => {
                Some(reason)
            }
            _ => None,
        }
    }
}

async fn submit(
    machine: &MeerkatMachine,
    session: &AttachedSession,
    input: Input,
    route: Route,
) -> Result<AcceptOutcome, Failure> {
    tokio::time::timeout(BOUND, async {
        match route {
            Route::AcceptWithCompletion => machine
                .accept_input_with_completion(&session.session, input)
                .await
                .map(|(outcome, _)| outcome)
                .map_err(Failure::Driver),
            Route::Ingest => machine
                .ingest(&session.runtime, input)
                .await
                .map_err(Failure::Control),
        }
    })
    .await
    .expect("public admission returns within the fixture bound")
}

// Reuse only the fixture's result receiver and completion helpers. Until attach
// is called, this value owns an actual bare registration and no executor.
async fn register_unattached(
    machine: &MeerkatMachine,
) -> (AttachedSession, tokio::sync::mpsc::UnboundedSender<Started>) {
    let session = SessionId::new();
    machine
        .register_session(session.clone())
        .await
        .expect("public bare registration");
    machine
        .prepare_bindings(session.clone())
        .await
        .expect("actual bindings");
    let runtime = machine
        .sessions
        .read()
        .await
        .get(&session)
        .expect("registered entry")
        .runtime_id
        .clone();
    let (tx, started) = tokio::sync::mpsc::unbounded_channel();
    (
        AttachedSession {
            session,
            runtime,
            gate: Arc::new(tokio::sync::Semaphore::new(0)),
            calls: Arc::new(AtomicUsize::new(0)),
            started,
        },
        tx,
    )
}

struct UnsupportedExecutor(GatedExecutor);

#[async_trait::async_trait]
impl CoreExecutor for UnsupportedExecutor {
    fn supports_work_authorization(&self) -> bool {
        false
    }
    async fn apply(
        &mut self,
        run: RunId,
        primitive: RunPrimitive,
    ) -> Result<CoreApplyOutput, CoreExecutorError> {
        self.0.apply(run, primitive).await
    }
    async fn cancel_after_boundary(&mut self, reason: String) -> Result<(), CoreExecutorError> {
        self.0.cancel_after_boundary(reason).await
    }
    async fn stop_runtime_executor(&mut self, reason: String) -> Result<(), CoreExecutorError> {
        self.0.stop_runtime_executor(reason).await
    }
}

async fn attach(
    machine: &Arc<MeerkatMachine>,
    session: &AttachedSession,
    sender: tokio::sync::mpsc::UnboundedSender<Started>,
    supported: bool,
) {
    let executor = GatedExecutor {
        session: session.session.clone(),
        gate: session.gate.clone(),
        calls: session.calls.clone(),
        started: sender,
    };
    let executor: Box<dyn CoreExecutor> = if supported {
        Box::new(executor)
    } else {
        Box::new(UnsupportedExecutor(executor))
    };
    machine
        .register_session_with_executor(session.session.clone(), executor)
        .await
        .expect("actual attachment and startup");
}

async fn row_exists(machine: &MeerkatMachine, session: &AttachedSession, id: &InputId) -> bool {
    let driver = machine
        .sessions
        .read()
        .await
        .get(&session.session)
        .expect("same registration")
        .driver
        .clone();
    let locked = driver.lock().await;
    let DriverEntry::Ephemeral(driver) = &*locked else {
        panic!("storeless fixture")
    };
    driver.ledger().get(id).is_some()
}

async fn refuse_without_effects(
    machine: &MeerkatMachine,
    session: &AttachedSession,
    input: Input,
    route: Route,
) -> Failure {
    let id = input.id().clone();
    let before = machine
        .session_dsl_state(&session.session)
        .await
        .expect("actual state");
    let observation = Observation::install(dsl::SessionId::from_domain(&session.session));
    let result = submit(machine, session, input, route).await;
    let effects = observation.effects();
    drop(observation);
    assert!(
        effects.is_empty(),
        "refusal must precede actual generated admission: {effects:?}"
    );
    assert!(
        !row_exists(machine, session, &id).await,
        "no accepted input row"
    );
    assert_eq!(
        machine
            .session_dsl_state(&session.session)
            .await
            .expect("registration retained"),
        before,
        "no lifecycle or admission mutation"
    );
    assert_eq!(session.calls.load(Ordering::SeqCst), 0, "no executor entry");
    result.expect_err("a valid governed input needs an executor with actual support")
}

fn assert_executor_unavailable(error: &Failure) {
    assert!(
        matches!(
            error.readiness(),
            Some(ControllerReadinessFailure::ExecutorUnavailable)
        ),
        "executor absence retains its exact pre-admission readiness cause: {error:?}"
    );
}

async fn unattached_then_attached(route: Route) {
    let (configuration, template, _, _) = mutable_controller_configuration(true);
    let machine = Arc::new(
        MeerkatMachine::ephemeral()
            .with_local_grant_authorization(configuration)
            .expect("actual host"),
    );
    let (mut session, sender) = register_unattached(&machine).await;
    let selection = selected(&template, "candidate-controller", "candidate-controller");
    let store = EphemeralTokenStore::new();
    install_credential(&machine, &store, &selection).await;
    let input = input_for(&template, &session, selection);
    let id = input.id().clone();

    // A malformed input must not gain a readiness classification merely because
    // it also lacks an executor. Retain the existing unconfigured-input path.
    let mut malformed = input.clone();
    malformed.header_mut().authority_association = None;
    let malformed_error = refuse_without_effects(&machine, &session, malformed, route).await;
    assert!(
        malformed_error.readiness().is_none(),
        "malformed input remains a validation failure"
    );

    let error = refuse_without_effects(&machine, &session, input.clone(), route).await;
    attach(&machine, &session, sender, true).await;
    let observation = Observation::install(dsl::SessionId::from_domain(&session.session));
    let accepted = submit(&machine, &session, input, route)
        .await
        .expect("same legitimate input accepted after actual attachment");
    assert!(matches!(accepted, AcceptOutcome::Accepted { input_id, .. } if input_id == id));
    assert!(row_exists(&machine, &session, &id).await);
    let effects = observation.effects();
    if matches!(route, Route::Ingest) {
        assert!(
            effects
                .iter()
                .any(|effect| matches!(effect, dsl::MeerkatMachineEffect::ResolveAdmission))
        );
    }
    assert!(
        effects
            .iter()
            .any(|effect| matches!(effect, dsl::MeerkatMachineEffect::IngressAccepted)),
        "positive control proves actual admission observer is connected"
    );
    drop(observation);
    let started = session.start().await;
    assert_eq!(started.ids, vec![id.clone()]);
    let prepared = meerkat_core::authorization::PreparedOperationCheck::prepare(
        started.context.clone(),
        plain_controller_binding(&started.context, &started.run),
    )
    .expect("same real account/grant/native owners permit the controller");
    prepared
        .current()
        .expect("actual admitted controller is current");
    session.finish(&machine, &id).await;
    // Keep the positive control reachable on the old implementation before its
    // intended classification failure, rather than stopping at the first error.
    assert_executor_unavailable(&error);
}

#[tokio::test]
async fn unattached_accept_is_readiness_then_same_input_completes_after_attachment() {
    unattached_then_attached(Route::AcceptWithCompletion).await;
}

#[tokio::test]
async fn unattached_ingest_is_readiness_then_same_input_completes_after_attachment() {
    unattached_then_attached(Route::Ingest).await;
}

async fn unsupported_executor(route: Route) {
    let (configuration, template, _, _) = mutable_controller_configuration(true);
    let machine = Arc::new(
        MeerkatMachine::ephemeral()
            .with_local_grant_authorization(configuration)
            .expect("actual host"),
    );
    let (session, sender) = register_unattached(&machine).await;
    let selection = selected(&template, "candidate-controller", "candidate-controller");
    let store = EphemeralTokenStore::new();
    install_credential(&machine, &store, &selection).await;
    let input = input_for(&template, &session, selection);
    attach(&machine, &session, sender, false).await;
    let error = refuse_without_effects(&machine, &session, input, route).await;
    assert_executor_unavailable(&error);
}

#[tokio::test]
async fn unsupported_executor_accept_preserves_readiness() {
    unsupported_executor(Route::AcceptWithCompletion).await;
}

#[tokio::test]
async fn unsupported_executor_ingest_preserves_readiness() {
    unsupported_executor(Route::Ingest).await;
}
