//! Native recovered-input lifetime across the standalone construction cleanup
//! boundary. The invalid setting reaches the real factory; no executor, model
//! answer, rollback result or durable input row is fabricated.
use super::*;
use futures_util::FutureExt;
use meerkat_runtime::{
    LogicalRuntimeId,
    completion::CompletionWaitError,
    input_state::{
        InputAbandonReason, InputLifecycleState, InputTerminalOutcome, StopRequeueMark,
        StoredInputState,
    },
    meerkat_machine::UnregisterQueuedInputs,
    store::RuntimeStore,
    terminal_status::TerminalWitnessSource,
};
use std::panic::{AssertUnwindSafe, resume_unwind};

struct Fixture {
    _temp: tempfile::TempDir,
    service: Arc<PersistentSessionService<FactoryAgentBuilder>>,
    machine: Arc<MeerkatMachine>,
    store: Arc<dyn RuntimeStore>,
    session: Session,
}

impl Fixture {
    async fn open() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let storage = crate::RealmOptions {
            context_root: temp.path().to_owned(),
            state_root: Some(temp.path().join("realms")),
            user_config_root: Some(temp.path().join("home")),
        }
        .resolve("failed-materialization")
        .unwrap();
        let mut startup = SessionStartup::with_config(storage, Config::default())
            .await
            .unwrap();
        let store = startup.persistence().await.unwrap().runtime_store();
        let SessionStartup {
            factory,
            config,
            persistence,
            ..
        } = startup;
        let (service, machine) =
            build_persistent_service_with_runtime_adapter(factory, config, 1, persistence.unwrap())
                .unwrap();
        Self {
            _temp: temp,
            service: Arc::new(service),
            machine,
            store,
            session: Session::new(),
        }
    }

    async fn rows(&self) -> Vec<StoredInputState> {
        self.store
            .load_input_states_strict(&LogicalRuntimeId::for_session(self.session.id()))
            .await
            .unwrap()
    }

    // A real durable prompt is queued without an executor, then the exact
    // registration is orderly removed with native Requeue. The next prepared
    // registration must recover this row from the same native store.
    async fn queue_for_next_registration(&self) -> Input {
        let id = self.session.id();
        self.machine.register_session(id.clone()).await.unwrap();
        let registration = self
            .machine
            .current_session_registration_witness(id)
            .await
            .unwrap();
        let input = Input::Prompt(PromptInput::new("retained predecessor prompt", None));
        let (admission, completion) = self
            .machine
            .accept_input_with_completion(id, input.clone())
            .await
            .unwrap();
        assert!(
            matches!(admission, AcceptOutcome::Accepted { ref input_id, .. } if input_id == input.id())
        );
        let closed = self
            .machine
            .unregister_session_registration_until_terminal_with_queued_inputs_if_current(
                &registration,
                UnregisterQueuedInputs::Requeue {
                    max_age: std::time::Duration::from_secs(300),
                },
            )
            .await;
        let settled = completion.unwrap().try_wait_with_terminal_outcome().await;
        assert!(matches!(closed, Ok(true)));
        assert!(matches!(
            settled,
            Err(CompletionWaitError::RequeuedAcrossStop { .. })
        ));
        assert!(
            self.machine
                .current_session_registration_witness(id)
                .await
                .is_none()
        );
        let rows = self.rows().await;
        assert_eq!(rows.len(), 1);
        assert_eq!(&rows[0].state.input_id, input.id());
        assert_eq!(rows[0].seed.phase, InputLifecycleState::Queued);
        assert!(rows[0].seed.terminal_outcome.is_none() && rows[0].state.stop_requeue.is_some());
        input
    }

    fn refused_request() -> CreateSessionRequest {
        CreateSessionRequest {
            model: "gpt-5.4".into(),
            prompt: String::new().into(),
            injected_context: Vec::new(),
            system_prompt: SystemPromptOverride::Inherit,
            max_tokens: None,
            event_tx: None,
            initial_turn: InitialTurnPolicy::Defer,
            deferred_prompt_policy: DeferredPromptPolicy::Discard,
            build: Some(SessionBuildOptions {
                max_inline_peer_notifications: Some(-2),
                llm_client_override: Some(meerkat::encode_llm_client_override_for_service(
                    Arc::new(meerkat_client::TestClient::for_provider(
                        meerkat_core::Provider::OpenAI,
                    )),
                )),
                ..Default::default()
            }),
            labels: None,
        }
    }
}

fn same_row(expected: &StoredInputState, actual: &StoredInputState) {
    assert_eq!(
        actual.seed, expected.seed,
        "preserve every native lifecycle seed field"
    );
    assert_eq!(
        serde_json::to_value(actual).unwrap(),
        serde_json::to_value(expected).unwrap(),
        "preserve the complete input envelope, payload, history and existing requeue mark"
    );
}

fn stopped(row: &StoredInputState, prior_mark: Option<StopRequeueMark>) {
    assert_eq!(row.seed.phase, InputLifecycleState::Abandoned);
    assert_eq!(
        row.seed.terminal_outcome,
        Some(InputTerminalOutcome::Abandoned {
            reason: InputAbandonReason::Stopped
        })
    );
    // Core retains the earlier stop's historical mark when a later Stop
    // terminalizes the input. The terminal seed, not marker absence, decides
    // whether the input can be recovered into a work lane.
    assert_eq!(
        row.state.stop_requeue, prior_mark,
        "explicit stop preserves the earlier mark without creating a new requeue"
    );
}

fn exact_refusal(error: &ConnectError) -> bool {
    matches!(error, ConnectError::Native { operation, source }
        if *operation == "Build governed setup actor"
            && source.downcast_ref::<meerkat::surface::SurfaceRuntimeMaterializeError>().is_some()
            && source.to_string().contains("max_inline_peer_notifications=-2 is invalid"))
}

async fn failed_materialization_case(f: &Fixture, admit_live_input: bool) {
    let recovered = f.queue_for_next_registration().await;
    let queued = f.rows().await;
    assert_eq!(queued.len(), 1);
    let recovered_mark = queued[0].state.stop_requeue;
    assert!(recovered_mark.is_some());
    let admission = f.service.reserve_create_session_admission().await.unwrap();
    let mut prepared = f
        .machine
        .prepare_session_materialization(f.session.id().clone())
        .await
        .unwrap();
    let registration = f
        .machine
        .current_session_registration_witness(f.session.id())
        .await
        .unwrap();
    let before = f.rows().await;
    assert_eq!(before.len(), 1);
    assert_eq!(&before[0].state.input_id, recovered.id());
    assert_eq!(before[0].state.stop_requeue, recovered_mark);
    assert_eq!(before[0].seed.phase, InputLifecycleState::Queued);
    assert!(before[0].seed.terminal_outcome.is_none());
    assert!(
        f.machine
            .current_executor_attachment_witness(f.session.id())
            .await
            .is_none()
    );

    // Core defines an actual admission as serving live work for rollback,
    // even before an executor exists. Do not mistake it for model execution.
    let live = if admit_live_input {
        let input = Input::Prompt(PromptInput::new("newly admitted prompt", None));
        let (admitted, completion) = f
            .machine
            .accept_input_with_completion(f.session.id(), input.clone())
            .await
            .unwrap();
        assert!(
            matches!(admitted, AcceptOutcome::Accepted { ref input_id, .. } if input_id == input.id())
        );
        Some((input, completion.unwrap()))
    } else {
        None
    };

    let materialized = Box::pin(
        meerkat::surface::materialize_prepared_session_actor_unattached_with_actor_slot(
            &f.service,
            &f.machine,
            &mut prepared,
            f.session.clone(),
            Fixture::refused_request(),
            admission,
        ),
    )
    .await;
    let failure = match materialized {
        Err(error) => error,
        Ok(_) => {
            let _ = f
                .machine
                .unregister_session_registration_until_terminal_if_current(&registration)
                .await;
            let _ = f.service.try_shutdown().await;
            panic!("the actual invalid factory setting must refuse materialization");
        }
    };
    let primary = ConnectError::native("Build governed setup actor", failure);
    // Same follow-up rollback and actual cleanup helper as standalone.rs.
    // Native materialization has already consumed the successful rollback;
    // false here must not become a second terminal-join receipt.
    let repeated_rollback = prepared.rollback_now().await;
    let after_native_rollback = f.rows().await;
    let error = close_failed_construction(
        primary,
        f.service.clone(),
        f.machine.clone(),
        Some(registration.clone()),
        None,
        None,
        None,
        None,
    )
    .await;
    let after_connect_cleanup = f.rows().await;
    let live_settlement = match live {
        Some((input, completion)) => {
            Some((input, completion.try_wait_with_terminal_outcome().await))
        }
        None => None,
    };
    let retained = match &error {
        ConnectError::Cleanup { primary, cleanup } if exact_refusal(primary) => {
            match cleanup.as_ref() {
                ConnectError::Native { source, .. } => {
                    source.downcast_ref::<RetainedSetupConstructionError>()
                }
                _ => None,
            }
        }
        _ => None,
    }
    .expect("Connect preserves the typed factory refusal and exact unconfirmed construction owner");
    assert_eq!(retained.observed_unregister, Some(false));
    assert!(
        Arc::ptr_eq(&retained._service, &f.service) && Arc::ptr_eq(&retained._machine, &f.machine)
    );
    assert!(
        retained
            ._registration
            .as_ref()
            .is_some_and(|held| held.session_id() == registration.session_id()
                && held.epoch_id() == registration.epoch_id())
    );
    assert!(matches!(repeated_rollback, Ok(false)));
    assert!(
        f.machine
            .current_session_registration_witness(f.session.id())
            .await
            .is_none()
    );

    if let Some((input, settled)) = live_settlement {
        assert!(
            settled.is_ok(),
            "the native stop joins the admitted input completion"
        );
        assert_eq!(after_native_rollback.len(), 2);
        assert_eq!(after_connect_cleanup.len(), 2);
        for id in [recovered.id(), input.id()] {
            let native = after_native_rollback
                .iter()
                .find(|row| &row.state.input_id == id)
                .unwrap();
            let closed = after_connect_cleanup
                .iter()
                .find(|row| &row.state.input_id == id)
                .unwrap();
            stopped(
                native,
                if id == recovered.id() {
                    recovered_mark
                } else {
                    None
                },
            );
            same_row(native, closed);
        }
    } else {
        assert_eq!(after_native_rollback.len(), 1);
        assert_eq!(after_connect_cleanup.len(), 1);
        same_row(&before[0], &after_native_rollback[0]);
        same_row(&before[0], &after_connect_cleanup[0]);
    }

    // A successor is a new exact registration. Reusing the failed owner's
    // cleanup must neither select nor remove it, nor change its input rows.
    // This is machine-only preparation after shutdown of an empty service:
    // no new actor is built and no successful conversation recovery is claimed.
    let mut successor = f
        .machine
        .prepare_session_materialization(f.session.id().clone())
        .await
        .unwrap();
    let successor_registration = f
        .machine
        .current_session_registration_witness(f.session.id())
        .await
        .unwrap();
    assert_ne!(registration.epoch_id(), successor_registration.epoch_id());
    let successor_before = f.rows().await;
    let stale = close_failed_construction(
        ConnectError::MissingAttachment,
        f.service.clone(),
        f.machine.clone(),
        Some(registration),
        None,
        None,
        None,
        None,
    )
    .await;
    let successor_after = f.rows().await;
    let still_current = f
        .machine
        .current_session_registration_witness(f.session.id())
        .await
        .is_some_and(|current| current.epoch_id() == successor_registration.epoch_id());
    // In contrast, intentional close of the current successor is a Stop.
    // A retained recovered row is not permission to outlive that close.
    let intentional = close_failed_construction(
        ConnectError::MissingAttachment,
        f.service.clone(),
        f.machine.clone(),
        Some(successor_registration),
        None,
        None,
        None,
        None,
    )
    .await;
    let final_rows = f.rows().await;
    let successor_rollback = successor.rollback_now().await;
    let shutdown = f.service.try_shutdown().await;
    assert!(matches!(stale, ConnectError::Cleanup { .. }) && still_current);
    assert_eq!(successor_before.len(), successor_after.len());
    for before in &successor_before {
        same_row(
            before,
            successor_after
                .iter()
                .find(|row| row.state.input_id == before.state.input_id)
                .unwrap(),
        );
    }
    assert!(matches!(intentional, ConnectError::MissingAttachment));
    assert!(matches!(successor_rollback, Ok(false)) && shutdown.is_ok());
    assert!(
        f.machine
            .current_session_registration_witness(f.session.id())
            .await
            .is_none()
    );
    assert_eq!(final_rows.len(), if admit_live_input { 2 } else { 1 });
    for row in &final_rows {
        stopped(
            row,
            if &row.state.input_id == recovered.id() {
                recovered_mark
            } else {
                None
            },
        );
    }

    // A further real machine registration must recover the terminal seeds,
    // not turn a historical requeue mark back into queued work. This remains
    // a machine-only observation; no actor or successful conversation is built.
    let mut terminal_probe = f
        .machine
        .prepare_session_materialization(f.session.id().clone())
        .await
        .unwrap();
    let probe_registration = f
        .machine
        .current_session_registration_witness(f.session.id())
        .await
        .unwrap();
    let observed = f
        .machine
        .session_input_states(f.session.id())
        .await
        .unwrap();
    assert_eq!(observed.source, TerminalWitnessSource::LiveRuntime);
    assert_eq!(observed.report.len(), final_rows.len());
    for row in &observed.report {
        let durable = final_rows
            .iter()
            .find(|stored| stored.state.input_id == row.state.input_id)
            .unwrap();
        assert_eq!(
            row.seed, durable.seed,
            "fresh native registration retains the exact terminal seed"
        );
        stopped(row, durable.state.stop_requeue);
    }
    assert!(
        f.machine
            .current_executor_attachment_witness(f.session.id())
            .await
            .is_none()
    );
    assert!(matches!(terminal_probe.rollback_now().await, Ok(true)));
    assert!(
        f.machine
            .current_session_registration_witness(probe_registration.session_id())
            .await
            .is_none()
    );
    let after_probe = f.rows().await;
    assert_eq!(after_probe.len(), final_rows.len());
    for row in &after_probe {
        same_row(
            final_rows
                .iter()
                .find(|stored| stored.state.input_id == row.state.input_id)
                .unwrap(),
            row,
        );
    }
}

// Retain the actual fixture outside the timed body. A panic or timeout may
// drop a prepared native owner, whose Drop can start rollback asynchronously;
// always await fixture teardown before rethrowing the original failure.
async fn guarded_failed_materialization_case(admit_live_input: bool) {
    let f = Fixture::open().await;
    let body = AssertUnwindSafe(async {
        tokio::time::timeout(
            std::time::Duration::from_secs(30),
            Box::pin(failed_materialization_case(&f, admit_live_input)),
        )
        .await
        .expect("bounded recovered-input rollback and actual Connect cleanup");
    })
    .catch_unwind()
    .await;
    let cleanup = AssertUnwindSafe(async {
        // This lookup only cleans up the fixture's sole isolated session.
        // It never supplies custody for the old-owner assertions above.
        let unregister = match f
            .machine
            .current_session_registration_witness(f.session.id())
            .await
        {
            Some(registration) => Some(
                f.machine
                    .unregister_session_registration_until_terminal_if_current(&registration)
                    .await,
            ),
            None => None,
        };
        // Attempt service cleanup even when exact unregister reports failure.
        let shutdown = f.service.try_shutdown().await;
        let absent = f
            .machine
            .current_session_registration_witness(f.session.id())
            .await
            .is_none();
        assert!(
            unregister
                .as_ref()
                .is_none_or(|result| matches!(result, Ok(true))),
            "{unregister:?}"
        );
        assert!(
            shutdown.is_ok() && absent,
            "fixture cleanup must join: {shutdown:?}"
        );
    })
    .catch_unwind()
    .await;
    if let Err(panic) = body {
        resume_unwind(panic);
    }
    if let Err(panic) = cleanup {
        resume_unwind(panic);
    }
}

#[tokio::test]
async fn refused_materialization_preserves_recovered_input_and_exact_cleanup_custody() {
    guarded_failed_materialization_case(false).await;
}

#[tokio::test]
async fn refused_materialization_after_live_admission_stops_every_input() {
    guarded_failed_materialization_case(true).await;
}
