use super::*;
use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroU32;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use crate::meerkat_machine::{CommsDrainMode, CommsDrainPhase, DrainExitReason};
use chrono::Utc;
use meerkat_core::agent::{CommsCapabilityError, CommsRuntime};
use meerkat_core::lifecycle::core_executor::{CoreApplyOutput, CoreExecutor, CoreExecutorError};
use meerkat_core::lifecycle::run_control::RunControlCommand;
use meerkat_core::lifecycle::run_primitive::{RunApplyBoundary, RunPrimitive};
use meerkat_core::lifecycle::run_receipt::RunBoundaryReceipt;
use meerkat_core::lifecycle::{InputId, RunId};
use meerkat_core::ops::{OperationId, OperationResult};
use meerkat_core::ops_lifecycle::{
    OperationKind, OperationProgressUpdate, OperationSpec, OpsLifecycleRegistry,
};
use meerkat_machine_kernels::generated::meerkat as modeled_meerkat_kernel;
use meerkat_machine_kernels::test_oracle::{
    GeneratedMachineKernel, KernelEffect, KernelInput, KernelState, KernelValue, TransitionOutcome,
    TransitionRefusal,
};
use meerkat_machine_schema::catalog::dsl::dsl_meerkat_machine as schema_meerkat_machine;
use meerkat_machine_schema::{MachineSchema, TypeRef};
use serde::Serialize;
use tokio::sync::Notify;

use crate::completion::CompletionOutcome;
use crate::identifiers::IdempotencyKey;
use crate::meerkat_machine::dsl as mm_dsl;
use crate::meerkat_machine_types::{
    ImageOperationRoutingRequest, ImageOperationRoutingResult, ModelRoutingApprovalDisposition,
    ModelRoutingRealtimePolicy, SwitchTurnRequest,
};

fn uuid(n: u128) -> uuid::Uuid {
    uuid::Uuid::from_u128(n)
}

fn model(id: &str) -> meerkat_core::lifecycle::run_primitive::ModelId {
    meerkat_core::lifecycle::run_primitive::ModelId::new(id)
}

fn realtime_policy(target_realtime_capable: bool) -> ModelRoutingRealtimePolicy {
    ModelRoutingRealtimePolicy {
        target_realtime_capable,
        allow_realtime_detach: false,
    }
}

fn memory_blob_store() -> Arc<dyn meerkat_core::BlobStore> {
    Arc::new(meerkat_store::MemoryBlobStore::new())
}

fn runtime_id_for_session(session_id: &SessionId) -> LogicalRuntimeId {
    MeerkatMachine::logical_runtime_id(session_id)
}

#[derive(Default)]
struct RecordingMeerkatSignalSurface {
    log: tokio::sync::Mutex<
        Vec<(
            meerkat_machine_schema::identity::SignalVariantId,
            Vec<(
                meerkat_machine_schema::identity::FieldId,
                crate::composition::OwnedFieldValue,
            )>,
        )>,
    >,
}

#[async_trait::async_trait]
impl crate::composition::SignalConsumerSurface for RecordingMeerkatSignalSurface {
    fn instance_id(&self) -> &meerkat_machine_schema::identity::MachineInstanceId {
        static ID: std::sync::OnceLock<meerkat_machine_schema::identity::MachineInstanceId> =
            std::sync::OnceLock::new();
        ID.get_or_init(|| {
            meerkat_machine_schema::identity::MachineInstanceId::parse("mob")
                .expect("canonical instance id")
        })
    }

    async fn receive_signal(
        &self,
        variant: meerkat_machine_schema::identity::SignalVariantId,
        projected_fields: Vec<(
            meerkat_machine_schema::identity::FieldId,
            crate::composition::OwnedFieldValue,
        )>,
    ) -> Result<(), String> {
        self.log.lock().await.push((variant, projected_fields));
        Ok(())
    }
}

struct RejectingMeerkatSignalSurface;

#[async_trait::async_trait]
impl crate::composition::SignalConsumerSurface for RejectingMeerkatSignalSurface {
    fn instance_id(&self) -> &meerkat_machine_schema::identity::MachineInstanceId {
        static ID: std::sync::OnceLock<meerkat_machine_schema::identity::MachineInstanceId> =
            std::sync::OnceLock::new();
        ID.get_or_init(|| {
            meerkat_machine_schema::identity::MachineInstanceId::parse("mob")
                .expect("canonical instance id")
        })
    }

    async fn receive_signal(
        &self,
        _variant: meerkat_machine_schema::identity::SignalVariantId,
        _projected_fields: Vec<(
            meerkat_machine_schema::identity::FieldId,
            crate::composition::OwnedFieldValue,
        )>,
    ) -> Result<(), String> {
        Err("injected signal commit failure".to_string())
    }
}

fn prepare_bindings_input(
    session_id: &SessionId,
    runtime_id: &str,
    fence_token: u64,
) -> dsl::MeerkatMachineInput {
    dsl::MeerkatMachineInput::PrepareBindings {
        agent_runtime_id: dsl::AgentRuntimeId::from(runtime_id.to_string()),
        fence_token: dsl::FenceToken(fence_token),
        generation: dsl::Generation(0),
        session_id: dsl::SessionId::from_domain(session_id),
    }
}

fn install_recording_meerkat_signal_dispatcher(
    machine: &MeerkatMachine,
) -> Arc<RecordingMeerkatSignalSurface> {
    let signal_surface = Arc::new(RecordingMeerkatSignalSurface::default());
    let schema = meerkat_machine_schema::catalog::meerkat_mob_seam_composition();
    let table = crate::composition::RouteTable::from_schema(&schema).expect("catalog routes");
    let dispatcher: crate::composition::CatalogCompositionSignalDispatcher<
        crate::meerkat_machine::composition::MeerkatSeamSignal,
    > = crate::composition::CatalogCompositionSignalDispatcher::new(schema.name.clone(), table)
        .with_consumer(signal_surface.clone());
    machine.set_composition_signal_dispatcher(Arc::new(dispatcher));
    signal_surface
}

fn install_rejecting_meerkat_signal_dispatcher(machine: &MeerkatMachine) {
    let signal_surface = Arc::new(RejectingMeerkatSignalSurface);
    let schema = meerkat_machine_schema::catalog::meerkat_mob_seam_composition();
    let table = crate::composition::RouteTable::from_schema(&schema).expect("catalog routes");
    let dispatcher: crate::composition::CatalogCompositionSignalDispatcher<
        crate::meerkat_machine::composition::MeerkatSeamSignal,
    > = crate::composition::CatalogCompositionSignalDispatcher::new(schema.name.clone(), table)
        .with_consumer(signal_surface);
    machine.set_composition_signal_dispatcher(Arc::new(dispatcher));
}

async fn assert_no_runtime_binding(machine: &MeerkatMachine, session_id: &SessionId) {
    let state = machine
        .session_dsl_state(session_id)
        .await
        .expect("session DSL state");
    assert!(
        state.active_runtime_id.is_none(),
        "rollback must not leave a staged active_runtime_id"
    );
    assert!(
        state.active_fence_token.is_none(),
        "rollback must not leave a staged active_fence_token"
    );
}

#[test]
fn legacy_run_handler_does_not_restore_dsl_snapshots_by_hand() {
    let source = include_str!("meerkat_machine/dispatch_ingress.rs");
    let start = source
        .find("pub(super) async fn execute_meerkat_machine_legacy_run_command")
        .expect("legacy run handler should exist");
    let legacy_run_handler = &source[start..];

    assert!(
        !legacy_run_handler.contains("previous_dsl_state"),
        "legacy run must not manually snapshot DSL authority",
    );
    assert!(
        !legacy_run_handler.contains("restore_session_dsl_state"),
        "legacy run must not manually restore DSL authority",
    );
}

#[test]
fn legacy_run_handler_does_not_preflight_run_dsl_from_shell() {
    let source = include_str!("meerkat_machine/dispatch_ingress.rs");
    let start = source
        .find("pub(super) async fn execute_meerkat_machine_legacy_run_command")
        .expect("legacy run handler should exist");
    let legacy_run_handler = &source[start..];

    assert!(
        !legacy_run_handler.contains("preview_session_dsl_input"),
        "legacy run prepare must let the machine-owned run transition decide authority",
    );
}

#[test]
fn legacy_run_handler_does_not_string_match_commit_unregister_policy() {
    let source = include_str!("meerkat_machine/dispatch_ingress.rs");
    let start = source
        .find("pub(super) async fn execute_meerkat_machine_legacy_run_command")
        .expect("legacy run handler should exist");
    let legacy_run_handler = &source[start..];

    assert!(
        !legacy_run_handler.contains("to_string().contains"),
        "legacy run unregister policy must use typed machine-owned semantics",
    );
    assert!(
        !legacy_run_handler.contains("runtime boundary commit failed"),
        "legacy run unregister policy must not key off boundary failure text",
    );
}

#[tokio::test]
async fn provisional_dsl_stage_does_not_emit_routed_signal_until_authoritative_apply() {
    let machine = MeerkatMachine::ephemeral();
    let session_id = SessionId::new();
    machine.register_session(session_id.clone()).await;
    let signal_surface = install_recording_meerkat_signal_dispatcher(&machine);

    let previous_state = machine
        .stage_session_dsl_input(
            &session_id,
            prepare_bindings_input(&session_id, "rt-provisional", 7),
            "PrepareBindings(test provisional)",
        )
        .await
        .expect("provisional stage");

    assert!(
        signal_surface.log.lock().await.is_empty(),
        "provisional DSL effects must not be observable before commit"
    );

    machine
        .restore_session_dsl_state(&session_id, previous_state)
        .await;

    let (_, effects) = machine
        .apply_session_dsl_input(
            &session_id,
            prepare_bindings_input(&session_id, "rt-authoritative", 11),
            "PrepareBindings(test authoritative)",
        )
        .await
        .expect("authoritative apply");
    assert!(
        effects
            .iter()
            .any(|effect| matches!(effect, dsl::MeerkatMachineEffect::RuntimeBound { .. }))
    );

    let log = signal_surface.log.lock().await;
    assert_eq!(log.len(), 1);
    assert_eq!(log[0].0.as_str(), "ObserveRuntimeReady");
    assert_eq!(log[0].1[0].0.as_str(), "agent_runtime_id");
    assert!(
        matches!(&log[0].1[0].1, crate::composition::OwnedFieldValue::Str(value) if value == "rt-authoritative")
    );
}

#[tokio::test]
async fn provisional_dsl_rollback_after_shell_failure_leaks_no_routed_signal_or_state() {
    let machine = MeerkatMachine::ephemeral();
    let session_id = SessionId::new();
    machine.register_session(session_id.clone()).await;
    let signal_surface = install_recording_meerkat_signal_dispatcher(&machine);

    let previous_state = machine
        .stage_session_dsl_input(
            &session_id,
            prepare_bindings_input(&session_id, "rt-rolled-back", 17),
            "PrepareBindings(test rollback)",
        )
        .await
        .expect("provisional stage");
    machine
        .restore_session_dsl_state(&session_id, previous_state)
        .await;

    assert!(
        signal_surface.log.lock().await.is_empty(),
        "rolled-back provisional effects must not leak routed signals"
    );
    assert_no_runtime_binding(&machine, &session_id).await;
}

#[tokio::test]
async fn authoritative_dsl_apply_rolls_back_state_when_effect_dispatch_fails() {
    let machine = MeerkatMachine::ephemeral();
    let session_id = SessionId::new();
    machine.register_session(session_id.clone()).await;
    install_rejecting_meerkat_signal_dispatcher(&machine);

    let err = match machine
        .apply_session_dsl_input(
            &session_id,
            prepare_bindings_input(&session_id, "rt-dispatch-fails", 23),
            "PrepareBindings(test dispatch failure)",
        )
        .await
    {
        Ok(_) => panic!("dispatch failure should reject authoritative apply"),
        Err(err) => err,
    };
    assert!(err.contains("injected signal commit failure"), "{err}");
    assert_no_runtime_binding(&machine, &session_id).await;
}

#[tokio::test]
async fn control_plane_runtime_id_is_not_raw_session_uuid_alias() {
    let machine = MeerkatMachine::ephemeral();
    let session_id = SessionId::new();
    machine.register_session(session_id.clone()).await;

    let runtime_id = runtime_id_for_session(&session_id);
    let raw_session_alias = LogicalRuntimeId::legacy_session_uuid_alias(&session_id);
    assert_ne!(
        runtime_id, raw_session_alias,
        "runtime control-plane identity must not be the raw session UUID alias"
    );

    let state = crate::traits::RuntimeControlPlane::runtime_state(&machine, &runtime_id)
        .await
        .expect("canonical runtime id should resolve registered session");
    assert_eq!(state, RuntimeState::Idle);

    let err = crate::traits::RuntimeControlPlane::runtime_state(&machine, &raw_session_alias)
        .await
        .expect_err("raw session UUID alias must not resolve as a runtime id");
    assert!(matches!(
        err,
        crate::traits::RuntimeControlPlaneError::NotFound(_)
    ));
}

#[tokio::test]
async fn destroy_keeps_committed_dsl_state_when_runtime_destroyed_signal_dispatch_fails() {
    let machine = MeerkatMachine::ephemeral();
    let session_id = SessionId::new();
    machine
        .prepare_bindings(session_id.clone())
        .await
        .expect("prepare bindings before destroy");
    install_rejecting_meerkat_signal_dispatcher(&machine);

    let runtime_id = runtime_id_for_session(&session_id);
    let err = crate::traits::RuntimeControlPlane::destroy(&machine, &runtime_id)
        .await
        .expect_err("signal dispatch failure should surface");
    assert!(
        err.to_string().contains("injected signal commit failure"),
        "{err}"
    );
    assert_eq!(
        machine.existing_session_runtime_state(&session_id).await,
        Some(RuntimeState::Destroyed),
        "irreversible shell destroy must not roll DSL authority back to an earlier phase"
    );
}

#[tokio::test]
async fn persistent_retire_keeps_committed_dsl_state_when_runtime_retired_signal_dispatch_fails() {
    let store = Arc::new(crate::store::InMemoryRuntimeStore::new());
    let machine = MeerkatMachine::persistent(
        store.clone() as Arc<dyn crate::store::RuntimeStore>,
        memory_blob_store(),
    );
    let session_id = SessionId::new();
    machine.register_session(session_id.clone()).await;
    install_rejecting_meerkat_signal_dispatcher(&machine);

    let runtime_id = runtime_id_for_session(&session_id);
    let err = crate::traits::RuntimeControlPlane::retire(&machine, &runtime_id)
        .await
        .expect_err("signal dispatch failure should surface");
    assert!(
        err.to_string().contains("injected signal commit failure"),
        "{err}"
    );
    assert_eq!(
        machine.existing_session_runtime_state(&session_id).await,
        Some(RuntimeState::Retired),
        "durably committed retire must not roll live DSL authority back to an earlier phase"
    );
    assert_eq!(
        crate::store::RuntimeStore::load_runtime_state(store.as_ref(), &runtime_id)
            .await
            .expect("load persisted runtime state"),
        Some(RuntimeState::Retired),
        "persistent retire shell commit remains authoritative after routed-effect failure"
    );

    let recovered = MeerkatMachine::persistent(
        store as Arc<dyn crate::store::RuntimeStore>,
        memory_blob_store(),
    );
    recovered.register_session(session_id.clone()).await;
    assert_eq!(
        recovered.existing_session_runtime_state(&session_id).await,
        Some(RuntimeState::Retired),
        "cold restart must agree with the live process after failed retire signal dispatch"
    );
}

#[tokio::test]
async fn prepare_bindings_dispatches_runtime_bound_after_shell_commit() {
    let machine = MeerkatMachine::ephemeral();
    let session_id = SessionId::new();
    let signal_surface = install_recording_meerkat_signal_dispatcher(&machine);

    let bindings = machine
        .prepare_bindings(session_id.clone())
        .await
        .expect("prepare bindings commits");
    assert_eq!(bindings.session_id, session_id);

    let log = signal_surface.log.lock().await;
    assert_eq!(log.len(), 1);
    assert_eq!(log[0].0.as_str(), "ObserveRuntimeReady");
    assert_eq!(log[0].1[0].0.as_str(), "agent_runtime_id");
    assert!(matches!(
        &log[0].1[0].1,
        crate::composition::OwnedFieldValue::Str(value) if value == &runtime_id_for_session(&session_id).to_string()
    ));
}

#[tokio::test]
async fn rejected_provisional_dsl_transition_emits_no_routed_signal_or_state() {
    let machine = MeerkatMachine::ephemeral();
    let session_id = SessionId::new();
    machine.register_session(session_id.clone()).await;
    let signal_surface = install_recording_meerkat_signal_dispatcher(&machine);

    let err = machine
        .stage_session_dsl_input(
            &session_id,
            dsl::MeerkatMachineInput::Commit {
                input_id: dsl::InputId::from("missing-input"),
                run_id: dsl::RunId::from("missing-run"),
            },
            "Commit(test rejected provisional)",
        )
        .await
        .expect_err("commit is invalid outside Running");

    assert!(
        err.contains("no matching transition") || err.contains("guard rejected"),
        "{err}"
    );
    assert!(
        signal_surface.log.lock().await.is_empty(),
        "rejected provisional transition must not publish routed signals"
    );
    assert_no_runtime_binding(&machine, &session_id).await;
}

fn finite_switch_request(
    n: u128,
    target_model: &str,
    turns: meerkat_core::image_generation::FiniteScopedTurnDuration,
) -> SwitchTurnRequest {
    SwitchTurnRequest {
        request_id: meerkat_core::image_generation::SwitchTurnRequestId::new(uuid(n)),
        intent: meerkat_core::image_generation::SwitchTurnIntent {
            target_model: model(target_model),
            duration: meerkat_core::image_generation::SwitchTurnDuration::Finite {
                duration: turns,
            },
            origin: meerkat_core::image_generation::SwitchTurnOrigin::User {
                reason:
                    meerkat_core::image_generation::SwitchTurnReasonTextDisposition::NotProvided,
            },
        },
        target_realtime: realtime_policy(true),
        approval: ModelRoutingApprovalDisposition::NotRequired,
        approval_reason: None,
    }
}

fn image_request(n: u128, target_model: &str) -> ImageOperationRoutingRequest {
    ImageOperationRoutingRequest {
        operation_id: meerkat_core::image_generation::ImageOperationId::new(uuid(n)),
        target_model: model(target_model),
        target_realtime: realtime_policy(true),
        approval: ModelRoutingApprovalDisposition::NotRequired,
        approval_reason: None,
        requires_scoped_override: true,
    }
}

struct FakeDrainRuntime {
    notify: Arc<Notify>,
    dismiss: AtomicBool,
}

impl FakeDrainRuntime {
    fn dismissing() -> Self {
        Self {
            notify: Arc::new(Notify::new()),
            dismiss: AtomicBool::new(true),
        }
    }

    fn idle() -> Self {
        Self {
            notify: Arc::new(Notify::new()),
            dismiss: AtomicBool::new(false),
        }
    }
}

fn make_prompt(text: &str) -> Input {
    Input::Prompt(crate::input::PromptInput {
        header: crate::input::InputHeader {
            id: InputId::new(),
            timestamp: Utc::now(),
            source: crate::input::InputOrigin::Operator,
            durability: crate::input::InputDurability::Durable,
            visibility: crate::input::InputVisibility::default(),
            idempotency_key: None,
            supersession_key: None,
            correlation_id: None,
        },
        text: text.into(),
        blocks: None,
        turn_metadata: None,
    })
}

#[tokio::test]
async fn runtime_apply_failure_preserves_typed_cause_through_terminalization() {
    use meerkat_core::lifecycle::core_executor::CoreApplyFailureCause;

    struct TypedFailingExecutor;

    #[async_trait::async_trait]
    impl CoreExecutor for TypedFailingExecutor {
        async fn apply(
            &mut self,
            _run_id: RunId,
            _primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            Err(CoreExecutorError::ApplyFailed {
                cause: CoreApplyFailureCause::runtime_context_apply(
                    "context append failed before turn start",
                ),
            })
        }

        async fn control(&mut self, _command: RunControlCommand) -> Result<(), CoreExecutorError> {
            Ok(())
        }
    }

    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    adapter
        .register_session_with_executor(session_id.clone(), Box::new(TypedFailingExecutor))
        .await;

    adapter
        .accept_input(&session_id, make_prompt("typed apply failure"))
        .await
        .expect("input should be accepted");

    let (cause, message) = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let observed = {
                let sessions = adapter.sessions.read().await;
                let entry = sessions.get(&session_id).expect("session should exist");
                let authority = entry
                    .dsl_authority
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                authority
                    .state
                    .last_runtime_apply_failure_cause
                    .map(|cause| {
                        (
                            cause,
                            authority.state.last_runtime_apply_failure_message.clone(),
                        )
                    })
            };
            if let Some(observed) = observed {
                break observed;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("runtime loop should terminalize the typed apply failure");

    assert_eq!(cause, mm_dsl::RuntimeApplyFailureCause::RuntimeContextApply);
    assert_eq!(
        message.as_deref(),
        Some("context append failed before turn start")
    );
}

#[tokio::test]
async fn legacy_fail_does_not_fabricate_runtime_apply_failure_cause() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    adapter.register_session(session_id.clone()).await;

    let result: Result<(), RuntimeDriverError> = adapter
        .accept_input_and_run(
            &session_id,
            make_prompt("legacy fail"),
            |_run_id, _primitive| async {
                Err::<((), CoreApplyOutput), RuntimeDriverError>(RuntimeDriverError::Internal(
                    "legacy synchronous failure".to_string(),
                ))
            },
        )
        .await;

    assert!(matches!(result, Err(RuntimeDriverError::Internal(_))));

    let (cause, message) = {
        let sessions = adapter.sessions.read().await;
        let entry = sessions.get(&session_id).expect("session should exist");
        let authority = entry
            .dsl_authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        (
            authority.state.last_runtime_apply_failure_cause,
            authority.state.last_runtime_apply_failure_message.clone(),
        )
    };

    assert_eq!(cause, None);
    assert_eq!(message, None);
}

fn make_progress_input(label: &str) -> Input {
    Input::Peer(crate::input::PeerInput {
        header: crate::input::InputHeader {
            id: InputId::new(),
            timestamp: Utc::now(),
            source: crate::input::InputOrigin::Peer {
                peer_id: "peer-1".into(),
                display_identity: None,
                runtime_id: None,
            },
            durability: crate::input::InputDurability::Ephemeral,
            visibility: crate::input::InputVisibility::default(),
            idempotency_key: None,
            supersession_key: None,
            correlation_id: None,
        },
        convention: Some(crate::input::PeerConvention::ResponseProgress {
            request_id: format!("req-{label}"),
            phase: crate::input::ResponseProgressPhase::InProgress,
        }),
        body: format!("progress-{label}"),
        payload: Some(serde_json::json!({ "label": label })),
        blocks: None,
        handling_mode: None,
    })
}

#[async_trait::async_trait]
impl CommsRuntime for FakeDrainRuntime {
    async fn drain_messages(&self) -> Vec<String> {
        Vec::new()
    }

    fn inbox_notify(&self) -> Arc<Notify> {
        Arc::clone(&self.notify)
    }

    fn dismiss_received(&self) -> bool {
        self.dismiss.load(Ordering::Acquire)
    }

    async fn drain_classified_inbox_interactions(
        &self,
    ) -> Result<Vec<meerkat_core::interaction::ClassifiedInboxInteraction>, CommsCapabilityError>
    {
        Ok(Vec::new())
    }
}

async fn spawn_test_comms_drain(
    adapter: &Arc<MeerkatMachine>,
    session_id: &SessionId,
    mode: CommsDrainMode,
    comms_runtime: Arc<dyn CommsRuntime>,
    idle_timeout: Duration,
) {
    adapter.register_session(session_id.clone()).await;
    let mut sessions = adapter.sessions.write().await;
    let entry = sessions
        .get_mut(&session_id)
        .expect("register_session must have created the entry");
    let slot = &mut entry.drain_slot;
    slot.mode = Some(mode);
    slot.phase = CommsDrainPhase::Starting;
    slot.handle = Some(crate::comms_drain::spawn_comms_drain(
        Arc::clone(adapter),
        session_id.clone(),
        comms_runtime,
        Some(idle_timeout),
    ));
    slot.phase = CommsDrainPhase::Running;
}

async fn current_phase(
    adapter: &Arc<MeerkatMachine>,
    session_id: &SessionId,
) -> Option<CommsDrainPhase> {
    let sessions = adapter.sessions.read().await;
    sessions.get(session_id).map(|entry| entry.drain_slot.phase)
}

async fn handle_present(adapter: &Arc<MeerkatMachine>, session_id: &SessionId) -> bool {
    let sessions = adapter.sessions.read().await;
    sessions
        .get(session_id)
        .and_then(|entry| entry.drain_slot.handle.as_ref())
        .is_some()
}

async fn wait_for_phase(
    adapter: &Arc<MeerkatMachine>,
    session_id: &SessionId,
    expected: CommsDrainPhase,
) {
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if current_phase(adapter, session_id).await == Some(expected) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("phase transition");
}

#[tokio::test]
async fn dismiss_exit_updates_authority_before_join() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    let comms_runtime: Arc<dyn CommsRuntime> = Arc::new(FakeDrainRuntime::dismissing());

    spawn_test_comms_drain(
        &adapter,
        &session_id,
        CommsDrainMode::PersistentHost,
        comms_runtime,
        Duration::from_millis(25),
    )
    .await;

    wait_for_phase(&adapter, &session_id, CommsDrainPhase::Stopped).await;
    assert!(
        !handle_present(&adapter, &session_id).await,
        "drain task should clear its slot before wait_comms_drain joins"
    );

    adapter.wait_comms_drain(&session_id).await;
    assert_eq!(
        current_phase(&adapter, &session_id).await,
        Some(CommsDrainPhase::Stopped)
    );
}

#[tokio::test]
async fn idle_timeout_updates_authority_before_join() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    let comms_runtime: Arc<dyn CommsRuntime> = Arc::new(FakeDrainRuntime::idle());

    spawn_test_comms_drain(
        &adapter,
        &session_id,
        CommsDrainMode::Timed,
        comms_runtime,
        Duration::from_millis(25),
    )
    .await;

    wait_for_phase(&adapter, &session_id, CommsDrainPhase::Stopped).await;
    assert!(
        !handle_present(&adapter, &session_id).await,
        "drain task should clear its slot before wait_comms_drain joins"
    );

    adapter.wait_comms_drain(&session_id).await;
    assert_eq!(
        current_phase(&adapter, &session_id).await,
        Some(CommsDrainPhase::Stopped)
    );
}

#[tokio::test]
async fn unregister_session_aborts_and_removes_drain_slot() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    let comms_runtime: Arc<dyn CommsRuntime> = Arc::new(FakeDrainRuntime::idle());

    adapter.register_session(session_id.clone()).await;
    spawn_test_comms_drain(
        &adapter,
        &session_id,
        CommsDrainMode::PersistentHost,
        comms_runtime,
        Duration::from_secs(60),
    )
    .await;

    assert_eq!(
        current_phase(&adapter, &session_id).await,
        Some(CommsDrainPhase::Running)
    );
    assert!(handle_present(&adapter, &session_id).await);

    adapter.unregister_session(&session_id).await;

    // Wave-c C-H2: the drain slot is now owned by RuntimeSessionEntry,
    // so "slot removed" is structurally equivalent to "session removed".
    let sessions = adapter.sessions.read().await;
    assert!(
        !sessions.contains_key(&session_id),
        "unregister must remove the session entry (which owns the comms drain slot)"
    );
}

#[tokio::test]
async fn session_service_runtime_ext_write_side_follows_machine_control_surface() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    adapter.register_session(session_id.clone()).await;

    let state = <MeerkatMachine as SessionServiceRuntimeExt>::runtime_state(&adapter, &session_id)
        .await
        .expect("runtime state should route through the machine seam");
    assert_eq!(state, RuntimeState::Idle);

    let outcome = <MeerkatMachine as SessionServiceRuntimeExt>::accept_input(
        &adapter,
        &session_id,
        make_prompt("service-ext-write-side"),
    )
    .await
    .expect("accept_input should route through the machine seam");
    assert!(
        matches!(outcome, AcceptOutcome::Accepted { .. }),
        "prompt should still be admitted through the SessionServiceRuntimeExt seam"
    );

    let active =
        <MeerkatMachine as SessionServiceRuntimeExt>::list_active_inputs(&adapter, &session_id)
            .await
            .expect("active inputs should still be readable");
    assert_eq!(active.len(), 1, "accepted input should remain active");
    let active_state = <MeerkatMachine as SessionServiceRuntimeExt>::input_state(
        &adapter,
        &session_id,
        &active[0],
    )
    .await
    .expect("input_state should route through the machine seam");
    assert_eq!(
        active_state.map(|stored| stored.seed.phase),
        Some(crate::input_state::InputLifecycleState::Queued),
        "accepted prompt should still be visible through machine-routed input_state"
    );

    let retire_report =
        <MeerkatMachine as SessionServiceRuntimeExt>::retire_runtime(&adapter, &session_id)
            .await
            .expect("retire should route through the machine seam");
    assert_eq!(
        retire_report.inputs_abandoned, 1,
        "retire should still abandon queued work when no runtime loop is attached"
    );
    assert_eq!(retire_report.inputs_pending_drain, 0);

    let reset_report =
        <MeerkatMachine as SessionServiceRuntimeExt>::reset_runtime(&adapter, &session_id)
            .await
            .expect("reset should route through the machine seam");
    assert_eq!(
        reset_report.inputs_abandoned, 0,
        "reset after retire should not find residual queued work"
    );
}

#[tokio::test]
async fn model_routing_status_proves_finite_turn_and_operation_precedence() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    adapter.register_session(session_id.clone()).await;
    <MeerkatMachine as SessionServiceRuntimeExt>::configure_model_routing_baseline(
        &adapter,
        &session_id,
        model("baseline"),
        true,
    )
    .await
    .expect("baseline should be projected through machine command surface");

    let switch = finite_switch_request(
        101,
        "turn-target",
        meerkat_core::image_generation::FiniteScopedTurnDuration::OneTurn,
    );
    <MeerkatMachine as SessionServiceRuntimeExt>::request_switch_turn(
        &adapter,
        &session_id,
        switch,
    )
    .await
    .expect("finite switch_turn should be admitted");

    let before_boundary =
        <MeerkatMachine as SessionServiceRuntimeExt>::session_model_routing_status(
            &adapter,
            &session_id,
        )
        .await
        .expect("status should project");
    assert_eq!(before_boundary.effective_model.as_str(), "baseline");
    assert!(
        before_boundary.pending_switch_turn.is_some(),
        "finite switch_turn waits for the next admitted assistant-turn boundary"
    );

    <MeerkatMachine as SessionServiceRuntimeExt>::admit_model_routing_assistant_turn(
        &adapter,
        &session_id,
    )
    .await
    .expect("assistant-turn boundary should activate finite override");
    let turn_active = <MeerkatMachine as SessionServiceRuntimeExt>::session_model_routing_status(
        &adapter,
        &session_id,
    )
    .await
    .expect("status should project");
    assert_eq!(turn_active.effective_model.as_str(), "turn-target");

    let image = image_request(201, "operation-target");
    let operation_id = image.operation_id;
    let image_result = <MeerkatMachine as SessionServiceRuntimeExt>::begin_image_operation(
        &adapter,
        &session_id,
        image,
    )
    .await
    .expect("image operation should be admitted under turn override");
    assert!(matches!(
        image_result,
        ImageOperationRoutingResult::Accepted {
            phase: meerkat_core::image_generation::ImageOperationPhase::PlanResolved,
            ..
        }
    ));
    <MeerkatMachine as SessionServiceRuntimeExt>::activate_image_operation_override(
        &adapter,
        &session_id,
        operation_id,
    )
    .await
    .expect("operation-scoped override should activate");
    let operation_active =
        <MeerkatMachine as SessionServiceRuntimeExt>::session_model_routing_status(
            &adapter,
            &session_id,
        )
        .await
        .expect("status should project");
    assert_eq!(
        operation_active.effective_model.as_str(),
        "operation-target",
        "operation override takes precedence over the active turn override"
    );

    let empty_terminal = meerkat_core::image_generation::ImageOperationTerminalClass::EmptyResult {
        provider_text: meerkat_core::image_generation::ProviderTextDisposition::Captured {
            text_artifact_ref: meerkat_core::image_generation::TextArtifactRef::new(
                "provider-text-artifact",
            ),
        },
    };
    <MeerkatMachine as SessionServiceRuntimeExt>::complete_image_operation(
        &adapter,
        &session_id,
        operation_id,
        empty_terminal.clone(),
    )
    .await
    .expect("image operation should enter restore phase");
    let restored_phase =
        <MeerkatMachine as SessionServiceRuntimeExt>::restore_image_operation_override(
            &adapter,
            &session_id,
            operation_id,
        )
        .await
        .expect("operation restore should clear child override");
    assert_eq!(
        restored_phase,
        meerkat_core::image_generation::ImageOperationPhase::Terminal {
            terminal: empty_terminal
        },
        "restore should preserve the exact payload-bearing terminal passed to complete"
    );
    let restored_to_turn =
        <MeerkatMachine as SessionServiceRuntimeExt>::session_model_routing_status(
            &adapter,
            &session_id,
        )
        .await
        .expect("status should project");
    assert_eq!(restored_to_turn.effective_model.as_str(), "turn-target");

    <MeerkatMachine as SessionServiceRuntimeExt>::admit_model_routing_assistant_turn(
        &adapter,
        &session_id,
    )
    .await
    .expect("next admitted assistant turn should consume one-turn override");
    let consumed = <MeerkatMachine as SessionServiceRuntimeExt>::session_model_routing_status(
        &adapter,
        &session_id,
    )
    .await
    .expect("status should project");
    assert_eq!(consumed.effective_model.as_str(), "baseline");
    assert!(consumed.active_turn_override.is_none());
}

#[tokio::test]
async fn model_routing_denials_cover_approval_and_scoped_nesting_guards() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    adapter.register_session(session_id.clone()).await;
    <MeerkatMachine as SessionServiceRuntimeExt>::configure_model_routing_baseline(
        &adapter,
        &session_id,
        model("baseline"),
        true,
    )
    .await
    .unwrap();

    let mut denied_switch = finite_switch_request(
        301,
        "expensive-target",
        meerkat_core::image_generation::FiniteScopedTurnDuration::OneTurn,
    );
    denied_switch.approval = ModelRoutingApprovalDisposition::DeniedByUser;
    denied_switch.approval_reason =
        Some(meerkat_core::image_generation::SwitchTurnApprovalReason::CostExceedsThreshold);
    let denied = <MeerkatMachine as SessionServiceRuntimeExt>::request_switch_turn(
        &adapter,
        &session_id,
        denied_switch,
    )
    .await
    .expect("denial should terminalize through machine state");
    assert!(matches!(
        denied,
        meerkat_core::image_generation::SwitchTurnControlResult::Denied {
            reason: meerkat_core::image_generation::SwitchTurnDenialReason::DeniedDuringApproval { .. },
            ..
        }
    ));

    let mut denied_until_changed = SwitchTurnRequest {
        request_id: meerkat_core::image_generation::SwitchTurnRequestId::new(uuid(304)),
        intent: meerkat_core::image_generation::SwitchTurnIntent {
            target_model: model("persistent-denied-target"),
            duration: meerkat_core::image_generation::SwitchTurnDuration::UntilChanged,
            origin: meerkat_core::image_generation::SwitchTurnOrigin::SystemPolicy {
                reason: meerkat_core::image_generation::SwitchTurnPolicyReason::SafetyHandoff,
            },
        },
        target_realtime: realtime_policy(true),
        approval: ModelRoutingApprovalDisposition::RequiredButUnavailable,
        approval_reason: Some(
            meerkat_core::image_generation::SwitchTurnApprovalReason::UntilChangedFromModelOrigin,
        ),
    };
    let denied_persistent = <MeerkatMachine as SessionServiceRuntimeExt>::request_switch_turn(
        &adapter,
        &session_id,
        denied_until_changed.clone(),
    )
    .await
    .expect("UntilChanged approval-unavailable denial should terminalize");
    assert!(matches!(
        denied_persistent,
        meerkat_core::image_generation::SwitchTurnControlResult::Denied {
            reason:
                meerkat_core::image_generation::SwitchTurnDenialReason::ApprovalRequiredButUnavailable,
            ..
        }
    ));
    denied_until_changed.request_id =
        meerkat_core::image_generation::SwitchTurnRequestId::new(uuid(305));
    denied_until_changed.approval = ModelRoutingApprovalDisposition::DeniedByUser;
    let denied_by_user = <MeerkatMachine as SessionServiceRuntimeExt>::request_switch_turn(
        &adapter,
        &session_id,
        denied_until_changed,
    )
    .await
    .expect("UntilChanged user denial should terminalize");
    assert!(matches!(
        denied_by_user,
        meerkat_core::image_generation::SwitchTurnControlResult::Denied {
            reason: meerkat_core::image_generation::SwitchTurnDenialReason::DeniedDuringApproval { .. },
            ..
        }
    ));

    let active_switch = finite_switch_request(
        302,
        "turn-target",
        meerkat_core::image_generation::FiniteScopedTurnDuration::Turns {
            turns: NonZeroU32::new(2).unwrap(),
        },
    );
    <MeerkatMachine as SessionServiceRuntimeExt>::request_switch_turn(
        &adapter,
        &session_id,
        active_switch,
    )
    .await
    .unwrap();
    <MeerkatMachine as SessionServiceRuntimeExt>::admit_model_routing_assistant_turn(
        &adapter,
        &session_id,
    )
    .await
    .unwrap();

    let nested_switch = finite_switch_request(
        303,
        "nested-turn-target",
        meerkat_core::image_generation::FiniteScopedTurnDuration::OneTurn,
    );
    let nested_switch_result = <MeerkatMachine as SessionServiceRuntimeExt>::request_switch_turn(
        &adapter,
        &session_id,
        nested_switch,
    )
    .await
    .expect("scoped conflict should return typed denial");
    assert!(matches!(
        nested_switch_result,
        meerkat_core::image_generation::SwitchTurnControlResult::Denied {
            reason: meerkat_core::image_generation::SwitchTurnDenialReason::ScopedOverrideConflict,
            ..
        }
    ));

    let image = image_request(401, "operation-target");
    let operation_id = image.operation_id;
    <MeerkatMachine as SessionServiceRuntimeExt>::begin_image_operation(
        &adapter,
        &session_id,
        image,
    )
    .await
    .unwrap();
    <MeerkatMachine as SessionServiceRuntimeExt>::activate_image_operation_override(
        &adapter,
        &session_id,
        operation_id,
    )
    .await
    .unwrap();

    let nested_image = image_request(402, "nested-operation-target");
    let nested_image_result = <MeerkatMachine as SessionServiceRuntimeExt>::begin_image_operation(
        &adapter,
        &session_id,
        nested_image,
    )
    .await
    .expect("operation-in-operation conflict should terminalize");
    assert!(matches!(
        nested_image_result,
        ImageOperationRoutingResult::Denied {
            reason:
                meerkat_core::image_generation::ImageOperationDenialReason::ScopedOverrideConflict,
            ..
        }
    ));
}

#[tokio::test]
async fn until_changed_switch_turn_reconfigures_baseline_not_scoped_override() {
    struct NoopExecutor;

    #[async_trait::async_trait]
    impl CoreExecutor for NoopExecutor {
        async fn apply(
            &mut self,
            run_id: RunId,
            primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            Ok(CoreApplyOutput {
                receipt: RunBoundaryReceipt {
                    run_id,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                    sequence: 0,
                },
                session_snapshot: None,
                terminal: None,
            })
        }

        async fn control(&mut self, _command: RunControlCommand) -> Result<(), CoreExecutorError> {
            Ok(())
        }
    }

    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    adapter
        .prepare_bindings(session_id.clone())
        .await
        .expect("bindings should prepare");
    adapter
        .ensure_session_with_executor(session_id.clone(), Box::new(NoopExecutor))
        .await;
    let current_identity = Arc::new(std::sync::Mutex::new(meerkat_core::SessionLlmIdentity {
        model: "baseline".to_string(),
        provider: meerkat_core::Provider::OpenAI,
        self_hosted_server_id: None,
        provider_params: None,
        connection_ref: None,
    }));
    let current_visibility_state = Arc::new(std::sync::Mutex::new(
        meerkat_core::SessionToolVisibilityState::default(),
    ));
    adapter.set_session_llm_reconfigure_host(Arc::new(TestLlmReconfigureHost {
        current_identity: Arc::clone(&current_identity),
        current_visibility_state: Arc::clone(&current_visibility_state),
        target_identity: meerkat_core::SessionLlmIdentity {
            model: "persistent-target".to_string(),
            provider: meerkat_core::Provider::OpenAI,
            self_hosted_server_id: None,
            provider_params: None,
            connection_ref: None,
        },
        current_capability_surface: Some(test_llm_capability_surface(true)),
        target_capability_surface: test_llm_capability_surface(true),
        base_tool_names: BTreeSet::new(),
        fail_persist: false,
    }));
    <MeerkatMachine as SessionServiceRuntimeExt>::configure_model_routing_baseline(
        &adapter,
        &session_id,
        model("baseline"),
        true,
    )
    .await
    .unwrap();

    let request = SwitchTurnRequest {
        request_id: meerkat_core::image_generation::SwitchTurnRequestId::new(uuid(501)),
        intent: meerkat_core::image_generation::SwitchTurnIntent {
            target_model: model("persistent-target"),
            duration: meerkat_core::image_generation::SwitchTurnDuration::UntilChanged,
            origin: meerkat_core::image_generation::SwitchTurnOrigin::SystemPolicy {
                reason: meerkat_core::image_generation::SwitchTurnPolicyReason::SafetyHandoff,
            },
        },
        target_realtime: realtime_policy(true),
        approval: ModelRoutingApprovalDisposition::NotRequired,
        approval_reason: None,
    };
    <MeerkatMachine as SessionServiceRuntimeExt>::request_switch_turn(
        &adapter,
        &session_id,
        request,
    )
    .await
    .expect("UntilChanged should route through persistent reconfigure family");

    let status = <MeerkatMachine as SessionServiceRuntimeExt>::session_model_routing_status(
        &adapter,
        &session_id,
    )
    .await
    .unwrap();
    assert_eq!(status.baseline_model.as_str(), "persistent-target");
    assert_eq!(status.effective_model.as_str(), "persistent-target");
    assert!(status.active_turn_override.is_none());
    assert_eq!(
        current_identity
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .model,
        "persistent-target",
        "UntilChanged switch_turn must pass through the live LLM reconfigure host"
    );
}

#[tokio::test]
async fn realtime_policy_rejects_non_realtime_effective_targets_without_detach_permission() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    adapter.register_session(session_id.clone()).await;
    <MeerkatMachine as SessionServiceRuntimeExt>::configure_model_routing_baseline(
        &adapter,
        &session_id,
        model("realtime-baseline"),
        true,
    )
    .await
    .unwrap();
    adapter
        .project_realtime_attachment_intent(&session_id, true)
        .await
        .expect("realtime intent projection should succeed");

    let mut switch = finite_switch_request(
        601,
        "batch-only-target",
        meerkat_core::image_generation::FiniteScopedTurnDuration::OneTurn,
    );
    switch.target_realtime = realtime_policy(false);
    let switch_result = <MeerkatMachine as SessionServiceRuntimeExt>::request_switch_turn(
        &adapter,
        &session_id,
        switch,
    )
    .await
    .expect("realtime policy conflict should terminalize");
    assert!(matches!(
        switch_result,
        meerkat_core::image_generation::SwitchTurnControlResult::Denied {
            reason:
                meerkat_core::image_generation::SwitchTurnDenialReason::RealtimeTransportConflict,
            ..
        }
    ));

    let mut image = image_request(602, "batch-image-target");
    image.target_realtime = realtime_policy(false);
    let image_result = <MeerkatMachine as SessionServiceRuntimeExt>::begin_image_operation(
        &adapter,
        &session_id,
        image,
    )
    .await
    .expect("realtime policy conflict should terminalize");
    assert!(matches!(
        image_result,
        ImageOperationRoutingResult::Denied {
            reason: meerkat_core::image_generation::ImageOperationDenialReason::RealtimeTransportConflict,
            ..
        }
    ));
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_reports_registered_idle_session() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;

    let snapshot = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist for registered session");

    assert_eq!(snapshot.binding.session_id, session_id);
    assert_eq!(
        snapshot.binding.driver_kind,
        crate::meerkat_machine_types::MeerkatDriverKind::Ephemeral
    );
    assert!(snapshot.binding.driver_present);
    assert!(snapshot.binding.completions_present);
    assert!(snapshot.binding.ops_registry_present);
    assert_eq!(snapshot.control.phase, RuntimeState::Idle);
    assert!(!snapshot.binding.attachment_live);
    assert_eq!(snapshot.binding.cursor_state.agent_applied_cursor, 0);
    assert_eq!(snapshot.binding.cursor_state.runtime_observed_seq, 0);
    assert_eq!(snapshot.binding.cursor_state.runtime_last_injected_seq, 0);
    assert!(snapshot.inputs.admission_order.is_empty());
    assert!(snapshot.inputs.queue.is_empty());
    assert!(snapshot.inputs.steer_queue.is_empty());
    assert_eq!(snapshot.completion_waiters.input_count, 0);
    assert_eq!(snapshot.completion_waiters.waiter_count, 0);
    assert!(snapshot.completion_waiters.waiting_inputs.is_empty());
    assert!(!snapshot.drain.slot_present);
    assert_eq!(snapshot.drain.phase, None);
    assert_eq!(snapshot.drain.mode, None);
    assert!(!snapshot.drain.handle_present);
}

#[tokio::test]
async fn persistent_without_blobs_keeps_persistent_driver() {
    let store = Arc::new(crate::store::InMemoryRuntimeStore::new());
    let adapter = Arc::new(MeerkatMachine::persistent_without_blobs(
        store as Arc<dyn crate::store::RuntimeStore>,
    ));
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;

    let snapshot = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist for registered session");

    assert_eq!(
        snapshot.binding.driver_kind,
        crate::meerkat_machine_types::MeerkatDriverKind::Persistent,
        "persistent_without_blobs must keep durable runtime semantics and fail blob use explicitly instead of downgrading to ephemeral"
    );
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_tracks_queued_prompt_input() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;

    let input = make_prompt("hello from the runtime spine");
    let input_id = input.id().clone();

    let (_outcome, handle) = adapter
        .accept_input_with_completion(&session_id, input)
        .await
        .expect("prompt should be accepted");
    assert!(
        handle.is_some(),
        "queued prompt should register a completion"
    );

    let snapshot = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist for registered session");

    assert_eq!(snapshot.control.phase, RuntimeState::Idle);
    assert_eq!(snapshot.inputs.queue, vec![input_id.clone()]);
    assert!(snapshot.inputs.steer_queue.is_empty());
    assert_eq!(snapshot.inputs.current_run_id, None);
    assert_eq!(
        snapshot.inputs.current_run_contributors,
        Vec::<InputId>::new()
    );
    assert_eq!(snapshot.inputs.admission_order.len(), 1);
    assert_eq!(snapshot.completion_waiters.input_count, 1);
    assert_eq!(snapshot.completion_waiters.waiter_count, 1);
    assert_eq!(snapshot.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        snapshot.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert_eq!(
        snapshot.completion_waiters.waiting_inputs[0].waiter_count,
        1
    );

    let input_snapshot = &snapshot.inputs.admission_order[0];
    assert_eq!(input_snapshot.input_id, input_id);
    assert_eq!(
        input_snapshot.lifecycle,
        Some(crate::input_state::InputLifecycleState::Queued)
    );
    assert_eq!(
        input_snapshot.handling_mode,
        Some(meerkat_core::types::HandlingMode::Queue)
    );
    assert_eq!(snapshot.inputs.queue, vec![input_id.clone()]);
    assert!(snapshot.inputs.steer_queue.is_empty());
    assert!(input_snapshot.content_shape.is_some());
    assert_eq!(input_snapshot.last_run_id, None);
    assert_eq!(input_snapshot.last_boundary_sequence, None);
    assert!(input_snapshot.terminal_outcome.is_none());
    assert!(input_snapshot.is_prompt);
}

#[tokio::test]
async fn realtime_attachment_status_defaults_to_unattached() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;

    let status = <MeerkatMachine as SessionServiceRuntimeExt>::realtime_attachment_status(
        &adapter,
        &session_id,
    )
    .await
    .expect("registered session should expose live attachment status");

    assert_eq!(status, crate::RealtimeAttachmentStatus::Unattached);
}

#[tokio::test]
async fn realtime_attachment_status_reports_intent_present_unbound() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;
    adapter
        .project_realtime_attachment_intent(&session_id, true)
        .await
        .expect("intent projection should succeed");

    let status = <MeerkatMachine as SessionServiceRuntimeExt>::realtime_attachment_status(
        &adapter,
        &session_id,
    )
    .await
    .expect("registered session should expose live attachment status");

    assert_eq!(
        status,
        crate::RealtimeAttachmentStatus::IntentPresentUnbound
    );
}

#[tokio::test]
async fn realtime_attachment_status_reports_binding_not_ready_and_ready() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();

    adapter
        .register_session_with_executor(session_id.clone(), Box::new(RuntimeParityNoopExecutor))
        .await;
    adapter
        .project_realtime_attachment_intent(&session_id, true)
        .await
        .expect("intent projection should succeed");

    let authority = adapter
        .attach_live(&session_id)
        .await
        .expect("attach should mint authority");

    let not_ready_status =
        <MeerkatMachine as SessionServiceRuntimeExt>::realtime_attachment_status(
            &adapter,
            &session_id,
        )
        .await
        .expect("registered session should expose live attachment status");
    assert_eq!(
        not_ready_status,
        crate::RealtimeAttachmentStatus::BindingNotReady
    );

    adapter
        .publish_realtime_attachment_signal(
            authority.clone(),
            crate::RealtimeAttachmentStatus::BindingReady,
        )
        .await
        .expect("binding-ready signal should be accepted");

    let ready_status = <MeerkatMachine as SessionServiceRuntimeExt>::realtime_attachment_status(
        &adapter,
        &session_id,
    )
    .await
    .expect("registered session should expose live attachment status");
    assert_eq!(ready_status, crate::RealtimeAttachmentStatus::BindingReady);
}

#[tokio::test]
async fn realtime_attachment_status_reports_replacement_pending_and_reattach_required() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();

    adapter
        .register_session_with_executor(session_id.clone(), Box::new(RuntimeParityNoopExecutor))
        .await;
    adapter
        .project_realtime_attachment_intent(&session_id, true)
        .await
        .expect("intent projection should succeed");

    let initial_authority = adapter
        .attach_live(&session_id)
        .await
        .expect("attach should mint authority");

    adapter
        .publish_realtime_attachment_signal(
            initial_authority,
            crate::RealtimeAttachmentStatus::BindingReady,
        )
        .await
        .expect("binding-ready signal should be accepted");

    let replacement_authority = adapter
        .replace_realtime_attachment(&session_id)
        .await
        .expect("replacement should mint fresh authority");

    let replacement_pending =
        <MeerkatMachine as SessionServiceRuntimeExt>::realtime_attachment_status(
            &adapter,
            &session_id,
        )
        .await
        .expect("registered session should expose live attachment status");
    assert_eq!(
        replacement_pending,
        crate::RealtimeAttachmentStatus::ReplacementPending
    );

    adapter
        .require_realtime_attachment_reattach(&session_id)
        .await
        .expect("reattach requirement should succeed");

    let reattach_required =
        <MeerkatMachine as SessionServiceRuntimeExt>::realtime_attachment_status(
            &adapter,
            &session_id,
        )
        .await
        .expect("registered session should expose live attachment status");
    assert_eq!(
        reattach_required,
        crate::RealtimeAttachmentStatus::ReattachRequired
    );

    let stale_replacement = adapter
        .publish_realtime_attachment_signal(
            replacement_authority,
            crate::RealtimeAttachmentStatus::BindingReady,
        )
        .await
        .expect_err("reattach should invalidate replacement authority");
    assert!(
        matches!(
            stale_replacement,
            RuntimeDriverError::ValidationFailed { .. }
        ),
        "expected ValidationFailed, got {stale_replacement:?}"
    );
}

#[tokio::test]
async fn realtime_attachment_signal_rejects_stale_authority() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();

    adapter
        .register_session_with_executor(session_id.clone(), Box::new(RuntimeParityNoopExecutor))
        .await;
    adapter
        .project_realtime_attachment_intent(&session_id, true)
        .await
        .expect("intent projection should succeed");

    let authority = adapter
        .attach_live(&session_id)
        .await
        .expect("attach should mint authority");

    adapter
        .require_realtime_attachment_reattach(&session_id)
        .await
        .expect("reattach requirement should invalidate current authority");

    let err = adapter
        .publish_realtime_attachment_signal(
            authority,
            crate::RealtimeAttachmentStatus::BindingReady,
        )
        .await
        .expect_err("stale authority should be rejected");
    assert!(
        matches!(err, RuntimeDriverError::ValidationFailed { .. }),
        "expected ValidationFailed, got {err:?}"
    );
}

#[tokio::test]
async fn realtime_reconnect_retry_lifecycle_is_machine_owned() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();

    adapter
        .register_session_with_executor(session_id.clone(), Box::new(RuntimeParityNoopExecutor))
        .await;
    adapter
        .project_realtime_attachment_intent(&session_id, true)
        .await
        .expect("intent projection should succeed");
    let authority = adapter
        .attach_live(&session_id)
        .await
        .expect("attach should mint authority");
    adapter
        .publish_realtime_attachment_signal(
            authority,
            crate::RealtimeAttachmentStatus::BindingReady,
        )
        .await
        .expect("binding-ready signal should be accepted");

    adapter
        .require_realtime_attachment_reattach(&session_id)
        .await
        .expect("reattach requirement should succeed");
    adapter
        .begin_realtime_reconnect_cycle(&session_id, Some(1_000), Some(10_000))
        .await
        .expect("machine should begin reconnect cycle");

    let initial_status = <MeerkatMachine as SessionServiceRuntimeExt>::realtime_channel_status(
        &adapter,
        &session_id,
    )
    .await
    .expect("channel status should be readable");
    assert_eq!(
        initial_status.state,
        meerkat_contracts::RealtimeChannelState::Reconnecting
    );
    assert_eq!(initial_status.attempt_count, 1);
    assert_eq!(initial_status.next_retry_at, Some(rfc3339_ms(1_000)));
    assert_eq!(initial_status.deadline_at, Some(rfc3339_ms(10_000)));

    adapter
        .schedule_realtime_reconnect_retry(&session_id, Some(2_500))
        .await
        .expect("machine should record the next retry attempt");
    let retry_status = <MeerkatMachine as SessionServiceRuntimeExt>::realtime_channel_status(
        &adapter,
        &session_id,
    )
    .await
    .expect("channel status should be readable");
    assert_eq!(retry_status.attempt_count, 2);
    assert_eq!(retry_status.next_retry_at, Some(rfc3339_ms(2_500)));
    assert_eq!(retry_status.deadline_at, Some(rfc3339_ms(10_000)));

    adapter
        .exhaust_realtime_reconnect_cycle(&session_id)
        .await
        .expect("machine should own reconnect exhaustion");
    let exhausted_status = <MeerkatMachine as SessionServiceRuntimeExt>::realtime_channel_status(
        &adapter,
        &session_id,
    )
    .await
    .expect("channel status should be readable");
    assert_eq!(
        exhausted_status.state,
        meerkat_contracts::RealtimeChannelState::Error
    );
    assert_eq!(exhausted_status.attempt_count, 0);
    assert_eq!(exhausted_status.next_retry_at, None);
    assert_eq!(exhausted_status.deadline_at, None);
    assert!(
        exhausted_status
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("exhausted")),
        "exhausted status should explain reconnect exhaustion: {exhausted_status:?}"
    );
}

#[tokio::test]
async fn realtime_reconnect_progress_clears_after_recovery() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();

    adapter
        .register_session_with_executor(session_id.clone(), Box::new(RuntimeParityNoopExecutor))
        .await;
    adapter
        .project_realtime_attachment_intent(&session_id, true)
        .await
        .expect("intent projection should succeed");
    let authority = adapter
        .attach_live(&session_id)
        .await
        .expect("attach should mint authority");
    adapter
        .publish_realtime_attachment_signal(
            authority,
            crate::RealtimeAttachmentStatus::BindingReady,
        )
        .await
        .expect("binding-ready signal should be accepted");

    adapter
        .require_realtime_attachment_reattach(&session_id)
        .await
        .expect("reattach requirement should succeed");
    adapter
        .begin_realtime_reconnect_cycle(&session_id, Some(1_000), Some(10_000))
        .await
        .expect("machine should begin reconnect cycle");
    adapter
        .schedule_realtime_reconnect_retry(&session_id, Some(2_000))
        .await
        .expect("machine should record the second retry attempt");

    let reconnecting = <MeerkatMachine as SessionServiceRuntimeExt>::realtime_channel_status(
        &adapter,
        &session_id,
    )
    .await
    .expect("channel status should be readable");
    assert_eq!(reconnecting.attempt_count, 2);

    let recovered_authority = adapter
        .attach_live(&session_id)
        .await
        .expect("reattach should mint fresh authority");
    adapter
        .publish_realtime_attachment_signal(
            recovered_authority,
            crate::RealtimeAttachmentStatus::BindingReady,
        )
        .await
        .expect("recovered binding-ready signal should be accepted");

    let recovered = <MeerkatMachine as SessionServiceRuntimeExt>::realtime_channel_status(
        &adapter,
        &session_id,
    )
    .await
    .expect("channel status should be readable");
    assert_eq!(
        recovered.state,
        meerkat_contracts::RealtimeChannelState::Ready
    );
    assert_eq!(recovered.attempt_count, 0);
    assert_eq!(recovered.next_retry_at, None);
    assert_eq!(recovered.deadline_at, None);

    adapter
        .require_realtime_attachment_reattach(&session_id)
        .await
        .expect("second reattach requirement should succeed");
    adapter
        .begin_realtime_reconnect_cycle(&session_id, Some(3_000), Some(12_000))
        .await
        .expect("machine should begin a fresh reconnect cycle");
    let fresh_cycle = <MeerkatMachine as SessionServiceRuntimeExt>::realtime_channel_status(
        &adapter,
        &session_id,
    )
    .await
    .expect("channel status should be readable");
    assert_eq!(fresh_cycle.attempt_count, 1);
    assert_eq!(fresh_cycle.next_retry_at, Some(rfc3339_ms(3_000)));
    assert_eq!(fresh_cycle.deadline_at, Some(rfc3339_ms(12_000)));
}

fn rfc3339_ms(ms: u64) -> String {
    let secs = i64::try_from(ms / 1_000).expect("test timestamp should fit i64");
    let nanos = u32::try_from((ms % 1_000) * 1_000_000).expect("millis nanos should fit u32");
    chrono::DateTime::<chrono::Utc>::from_timestamp(secs, nanos)
        .expect("test timestamp should be valid")
        .to_rfc3339()
}

#[tokio::test]
async fn realtime_reattach_for_authority_rejects_stale_authority_without_mutation() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();

    adapter
        .register_session_with_executor(session_id.clone(), Box::new(RuntimeParityNoopExecutor))
        .await;
    adapter
        .project_realtime_attachment_intent(&session_id, true)
        .await
        .expect("intent projection should succeed");

    let stale_authority = adapter
        .attach_live(&session_id)
        .await
        .expect("attach should mint authority");
    adapter
        .publish_realtime_attachment_signal(
            stale_authority.clone(),
            crate::RealtimeAttachmentStatus::BindingReady,
        )
        .await
        .expect("binding-ready signal should be accepted");
    let live_authority = adapter
        .replace_realtime_attachment(&session_id)
        .await
        .expect("replacement should mint fresh authority");

    let stale_err = adapter
        .require_realtime_attachment_reattach_for_authority(stale_authority)
        .await
        .expect_err("stale authority should be rejected by DSL guard");
    assert!(
        matches!(stale_err, RuntimeDriverError::ValidationFailed { .. }),
        "expected ValidationFailed, got {stale_err:?}"
    );
    let status = <MeerkatMachine as SessionServiceRuntimeExt>::realtime_attachment_status(
        &adapter,
        &session_id,
    )
    .await
    .expect("registered session should expose live attachment status");
    assert_eq!(status, crate::RealtimeAttachmentStatus::ReplacementPending);

    adapter
        .require_realtime_attachment_reattach_for_authority(live_authority)
        .await
        .expect("current authority should require reattach");
    let status = <MeerkatMachine as SessionServiceRuntimeExt>::realtime_attachment_status(
        &adapter,
        &session_id,
    )
    .await
    .expect("registered session should expose live attachment status");
    assert_eq!(status, crate::RealtimeAttachmentStatus::ReattachRequired);
}

#[tokio::test]
async fn attach_live_rejects_sessions_without_executor_and_preserves_unbound_status() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;
    adapter
        .project_realtime_attachment_intent(&session_id, true)
        .await
        .expect("intent projection should succeed");

    let err = adapter
        .attach_live(&session_id)
        .await
        .expect_err("attach should require a live executor attachment");
    assert!(
        matches!(
            err,
            RuntimeDriverError::NotReady {
                state: RuntimeState::Idle
            }
        ),
        "expected NotReady(Idle), got {err:?}"
    );

    let snapshot = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist");
    assert!(
        !snapshot.binding.attachment_live,
        "failed attach must not change the runtime attachment discriminant"
    );

    let status = <MeerkatMachine as SessionServiceRuntimeExt>::realtime_attachment_status(
        &adapter,
        &session_id,
    )
    .await
    .expect("registered session should expose live attachment status");
    assert_eq!(
        status,
        crate::RealtimeAttachmentStatus::IntentPresentUnbound
    );
}

#[tokio::test]
async fn detach_live_clears_binding_but_preserves_intent_projection() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();

    adapter
        .register_session_with_executor(session_id.clone(), Box::new(RuntimeParityNoopExecutor))
        .await;
    adapter
        .project_realtime_attachment_intent(&session_id, true)
        .await
        .expect("intent projection should succeed");

    let authority = adapter
        .attach_live(&session_id)
        .await
        .expect("attach should mint authority");
    adapter
        .publish_realtime_attachment_signal(
            authority.clone(),
            crate::RealtimeAttachmentStatus::BindingReady,
        )
        .await
        .expect("binding-ready signal should be accepted");

    adapter
        .detach_live(&session_id)
        .await
        .expect("detach should clear runtime binding state");

    let snapshot = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist");
    assert!(
        snapshot.binding.attachment_live,
        "detaching voice must not tear down the runtime executor attachment"
    );

    let status = <MeerkatMachine as SessionServiceRuntimeExt>::realtime_attachment_status(
        &adapter,
        &session_id,
    )
    .await
    .expect("registered session should expose live attachment status");
    assert_eq!(
        status,
        crate::RealtimeAttachmentStatus::IntentPresentUnbound
    );
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_tracks_deduplicated_completion_waiters() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;

    let mut first = make_prompt("first deduplicated input");
    if let Input::Prompt(prompt) = &mut first {
        prompt.header.idempotency_key = Some(IdempotencyKey::new("same-input"));
    }
    let first_input_id = first.id().clone();

    let (first_outcome, first_handle) = adapter
        .accept_input_with_completion(&session_id, first)
        .await
        .expect("first input should be accepted");
    assert!(matches!(first_outcome, AcceptOutcome::Accepted { .. }));
    assert!(
        first_handle.is_some(),
        "first queued input should register a completion waiter"
    );

    let mut duplicate = make_prompt("second deduplicated input");
    if let Input::Prompt(prompt) = &mut duplicate {
        prompt.header.idempotency_key = Some(IdempotencyKey::new("same-input"));
    }

    let (duplicate_outcome, duplicate_handle) = adapter
        .accept_input_with_completion(&session_id, duplicate)
        .await
        .expect("duplicate input should deduplicate");
    match duplicate_outcome {
        AcceptOutcome::Deduplicated { existing_id, .. } => {
            assert_eq!(existing_id, first_input_id);
        }
        other => panic!("expected deduplicated outcome, got {other:?}"),
    }
    assert!(
        duplicate_handle.is_some(),
        "deduplicated in-flight input should join the existing waiter set"
    );

    let snapshot = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist for registered session");

    assert_eq!(snapshot.inputs.queue, vec![first_input_id.clone()]);
    assert_eq!(snapshot.inputs.admission_order.len(), 1);
    assert_eq!(snapshot.completion_waiters.input_count, 1);
    assert_eq!(snapshot.completion_waiters.waiter_count, 2);
    assert_eq!(snapshot.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        snapshot.completion_waiters.waiting_inputs[0].input_id,
        first_input_id
    );
    assert_eq!(
        snapshot.completion_waiters.waiting_inputs[0].waiter_count,
        2
    );
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_tracks_steered_prompt_input() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;

    let input = Input::Prompt(crate::input::PromptInput::new(
        "steer prompt",
        Some(
            meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                handling_mode: Some(meerkat_core::types::HandlingMode::Steer),
                ..Default::default()
            },
        ),
    ));
    let input_id = input.id().clone();

    let (_outcome, handle) = adapter
        .accept_input_with_completion(&session_id, input)
        .await
        .expect("steered prompt should be accepted");
    assert!(
        handle.is_some(),
        "steered prompt should register a completion"
    );

    let snapshot = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist for registered session");

    assert!(snapshot.inputs.queue.is_empty());
    assert_eq!(snapshot.inputs.steer_queue, vec![input_id.clone()]);
    assert_eq!(snapshot.inputs.admission_order.len(), 1);
    let input_snapshot = &snapshot.inputs.admission_order[0];
    assert_eq!(input_snapshot.input_id, input_id);
    assert_eq!(
        input_snapshot.handling_mode,
        Some(meerkat_core::types::HandlingMode::Steer)
    );
    assert_eq!(
        input_snapshot.lifecycle,
        Some(crate::input_state::InputLifecycleState::Queued)
    );
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_clears_completion_waiters_after_reset() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;

    let input = make_prompt("reset pending waiter");
    let input_id = input.id().clone();
    let (_outcome, handle) = adapter
        .accept_input_with_completion(&session_id, input)
        .await
        .expect("prompt should be accepted");
    let handle = handle.expect("queued prompt should register a waiter");

    let before_reset = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist before reset");
    assert_eq!(before_reset.completion_waiters.input_count, 1);
    assert_eq!(before_reset.completion_waiters.waiter_count, 1);
    assert_eq!(before_reset.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        before_reset.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );

    SessionServiceRuntimeExt::reset_runtime(&*adapter, &session_id)
        .await
        .expect("reset should succeed for idle queued runtime");

    let after_reset = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after reset");
    assert_eq!(after_reset.completion_waiters.input_count, 0);
    assert_eq!(after_reset.completion_waiters.waiter_count, 0);
    assert!(after_reset.completion_waiters.waiting_inputs.is_empty());

    match handle.wait().await {
        CompletionOutcome::RuntimeTerminated(reason) => {
            assert_eq!(reason, "runtime reset");
        }
        other => panic!("expected runtime reset termination, got {other:?}"),
    }
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_preserves_completion_waiters_after_recycle() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;

    let input = make_prompt("recycle pending waiter");
    let input_id = input.id().clone();
    let (_outcome, handle) = adapter
        .accept_input_with_completion(&session_id, input)
        .await
        .expect("prompt should be accepted");
    let handle = handle.expect("queued prompt should register a waiter");

    let before_recycle = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist before recycle");
    assert_eq!(before_recycle.control.phase, RuntimeState::Idle);
    assert_eq!(before_recycle.inputs.queue, vec![input_id.clone()]);
    assert_eq!(before_recycle.completion_waiters.input_count, 1);
    assert_eq!(before_recycle.completion_waiters.waiter_count, 1);
    assert_eq!(
        before_recycle.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );

    let runtime_id = runtime_id_for_session(&session_id);
    let report = crate::traits::RuntimeControlPlane::recycle(&*adapter, &runtime_id)
        .await
        .expect("recycle should preserve queued work");
    assert_eq!(report.inputs_transferred, 1);

    let after_recycle = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after recycle");
    assert_eq!(after_recycle.control.phase, RuntimeState::Idle);
    assert_eq!(after_recycle.inputs.queue, vec![input_id.clone()]);
    assert_eq!(after_recycle.completion_waiters.input_count, 1);
    assert_eq!(after_recycle.completion_waiters.waiter_count, 1);
    assert_eq!(after_recycle.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        after_recycle.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert_eq!(
        after_recycle.completion_waiters.waiting_inputs[0].waiter_count,
        1
    );

    SessionServiceRuntimeExt::reset_runtime(&*adapter, &session_id)
        .await
        .expect("reset should terminate preserved waiter at test end");
    match handle.wait().await {
        CompletionOutcome::RuntimeTerminated(reason) => {
            assert_eq!(reason, "runtime reset");
        }
        other => panic!("expected runtime reset termination, got {other:?}"),
    }
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_recycle_reconciles_stale_completion_waiters() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;

    let input = make_prompt("preserve active waiter");
    let input_id = input.id().clone();
    let (_outcome, active_handle) = adapter
        .accept_input_with_completion(&session_id, input)
        .await
        .expect("prompt should be accepted");
    let active_handle = active_handle.expect("queued prompt should register a waiter");

    let completions = {
        let sessions = adapter.sessions.read().await;
        Arc::clone(
            &sessions
                .get(&session_id)
                .expect("registered session should exist")
                .completions,
        )
    };
    let stale_input_id = InputId::new();
    let stale_handle = {
        let mut completions = completions.lock().await;
        completions.register(stale_input_id.clone())
    };

    let before_recycle = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist before recycle");
    assert_eq!(before_recycle.completion_waiters.input_count, 2);
    assert_eq!(before_recycle.completion_waiters.waiter_count, 2);
    assert!(
        before_recycle
            .completion_waiters
            .waiting_inputs
            .iter()
            .any(|entry| entry.input_id == input_id && entry.waiter_count == 1),
        "active queued input should have one visible waiter before recycle"
    );
    assert!(
        before_recycle
            .completion_waiters
            .waiting_inputs
            .iter()
            .any(|entry| entry.input_id == stale_input_id && entry.waiter_count == 1),
        "stale waiter should be visible before recycle reconciliation"
    );

    let runtime_id = runtime_id_for_session(&session_id);
    let report = crate::traits::RuntimeControlPlane::recycle(&*adapter, &runtime_id)
        .await
        .expect("recycle should reconcile waiters against active input truth");
    assert_eq!(report.inputs_transferred, 1);

    let after_recycle = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after recycle");
    assert_eq!(after_recycle.completion_waiters.input_count, 1);
    assert_eq!(after_recycle.completion_waiters.waiter_count, 1);
    assert_eq!(after_recycle.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        after_recycle.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert_eq!(
        after_recycle.completion_waiters.waiting_inputs[0].waiter_count,
        1
    );

    match stale_handle.wait().await {
        CompletionOutcome::RuntimeTerminated(reason) => {
            assert_eq!(reason, "recycled input no longer pending");
        }
        other => panic!("expected recycled stale waiter termination, got {other:?}"),
    }

    SessionServiceRuntimeExt::reset_runtime(&*adapter, &session_id)
        .await
        .expect("reset should terminate preserved waiter at test end");
    match active_handle.wait().await {
        CompletionOutcome::RuntimeTerminated(reason) => {
            assert_eq!(reason, "runtime reset");
        }
        other => panic!("expected runtime reset termination, got {other:?}"),
    }
}

#[tokio::test]
async fn shared_ingress_authority_is_identical_after_register_recover_and_recycle() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;
    let (session_authority, driver_authority) = adapter
        .debug_shared_ingress_authorities(&session_id)
        .await
        .expect("registered session should expose shared ingress authorities");
    assert!(
        Arc::ptr_eq(&session_authority, &driver_authority),
        "fresh registration should share one ingress authority between session and driver",
    );

    let runtime_id = runtime_id_for_session(&session_id);
    crate::traits::RuntimeControlPlane::recover(&*adapter, &runtime_id)
        .await
        .expect("recover should succeed");
    let (session_authority, driver_authority) = adapter
        .debug_shared_ingress_authorities(&session_id)
        .await
        .expect("recovered session should expose shared ingress authorities");
    assert!(
        Arc::ptr_eq(&session_authority, &driver_authority),
        "recover should preserve one shared ingress authority",
    );

    crate::traits::RuntimeControlPlane::recycle(&*adapter, &runtime_id)
        .await
        .expect("recycle should succeed");
    let (session_authority, driver_authority) = adapter
        .debug_shared_ingress_authorities(&session_id)
        .await
        .expect("recycled session should expose shared ingress authorities");
    assert!(
        Arc::ptr_eq(&session_authority, &driver_authority),
        "recycle should preserve one shared ingress authority",
    );
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_preserves_completion_waiters_after_recover() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;

    let input = make_prompt("recover pending waiter");
    let input_id = input.id().clone();
    let (_outcome, handle) = adapter
        .accept_input_with_completion(&session_id, input)
        .await
        .expect("prompt should be accepted");
    let handle = handle.expect("queued prompt should register a waiter");

    let before_recover = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist before recover");
    assert_eq!(before_recover.control.phase, RuntimeState::Idle);
    assert_eq!(before_recover.inputs.queue, vec![input_id.clone()]);
    assert_eq!(before_recover.completion_waiters.input_count, 1);
    assert_eq!(before_recover.completion_waiters.waiter_count, 1);
    assert_eq!(
        before_recover.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );

    let runtime_id = runtime_id_for_session(&session_id);
    let report = crate::traits::RuntimeControlPlane::recover(&*adapter, &runtime_id)
        .await
        .expect("recover should preserve queued work");
    assert_eq!(report.inputs_recovered, 1);

    let after_recover = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after recover");
    assert_eq!(after_recover.control.phase, RuntimeState::Idle);
    assert_eq!(after_recover.inputs.queue, vec![input_id.clone()]);
    assert_eq!(after_recover.completion_waiters.input_count, 1);
    assert_eq!(after_recover.completion_waiters.waiter_count, 1);
    assert_eq!(after_recover.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        after_recover.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert_eq!(
        after_recover.completion_waiters.waiting_inputs[0].waiter_count,
        1
    );

    SessionServiceRuntimeExt::reset_runtime(&*adapter, &session_id)
        .await
        .expect("reset should terminate preserved waiter at test end");
    match handle.wait().await {
        CompletionOutcome::RuntimeTerminated(reason) => {
            assert_eq!(reason, "runtime reset");
        }
        other => panic!("expected runtime reset termination, got {other:?}"),
    }
}

#[tokio::test]
async fn deduplicated_accept_with_completion_emits_no_new_signal() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;

    let mut first = make_prompt("dedup me");
    let key = IdempotencyKey::new("runtime-dedup");
    if let Input::Prompt(ref mut prompt) = first {
        prompt.header.idempotency_key = Some(key.clone());
    }
    let accepted = adapter
        .execute_meerkat_machine_command(
            None,
            MeerkatMachineCommand::AcceptWithCompletion {
                session_id: session_id.clone(),
                input: first,
            },
        )
        .await
        .expect("first input should be accepted");
    let MeerkatMachineCommandResult::AcceptWithCompletion {
        outcome: first_outcome,
        admission_signal: first_signal,
        ..
    } = accepted
    else {
        panic!("expected AcceptWithCompletion result");
    };
    assert!(first_outcome.is_accepted());
    assert_eq!(
        first_signal,
        crate::driver::ephemeral::PostAdmissionSignal::WakeLoop
    );

    let mut duplicate = make_prompt("dedup me too");
    if let Input::Prompt(ref mut prompt) = duplicate {
        prompt.header.idempotency_key = Some(key);
    }
    let duplicate_result = adapter
        .execute_meerkat_machine_command(
            None,
            MeerkatMachineCommand::AcceptWithCompletion {
                session_id: session_id.clone(),
                input: duplicate,
            },
        )
        .await
        .expect("duplicate input should return a deduplicated outcome");
    let MeerkatMachineCommandResult::AcceptWithCompletion {
        outcome,
        admission_signal,
        ..
    } = duplicate_result
    else {
        panic!("expected AcceptWithCompletion result");
    };
    assert!(outcome.is_deduplicated());
    assert_eq!(
        admission_signal,
        crate::driver::ephemeral::PostAdmissionSignal::None,
        "dedup should not emit a fresh admission signal",
    );

    let snapshot = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after dedup");
    assert_eq!(
        snapshot.inputs.post_admission_signal, "WakeLoop",
        "dedup should not overwrite the previously accumulated canonical signal",
    );
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_recover_reconciles_stale_completion_waiters() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;

    let input = make_prompt("preserve active waiter on recover");
    let input_id = input.id().clone();
    let (_outcome, active_handle) = adapter
        .accept_input_with_completion(&session_id, input)
        .await
        .expect("prompt should be accepted");
    let active_handle = active_handle.expect("queued prompt should register a waiter");

    let completions = {
        let sessions = adapter.sessions.read().await;
        Arc::clone(
            &sessions
                .get(&session_id)
                .expect("registered session should exist")
                .completions,
        )
    };
    let stale_input_id = InputId::new();
    let stale_handle = {
        let mut completions = completions.lock().await;
        completions.register(stale_input_id.clone())
    };

    let before_recover = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist before recover");
    assert_eq!(before_recover.completion_waiters.input_count, 2);
    assert_eq!(before_recover.completion_waiters.waiter_count, 2);
    assert!(
        before_recover
            .completion_waiters
            .waiting_inputs
            .iter()
            .any(|entry| entry.input_id == input_id && entry.waiter_count == 1),
        "active queued input should have one visible waiter before recover"
    );
    assert!(
        before_recover
            .completion_waiters
            .waiting_inputs
            .iter()
            .any(|entry| entry.input_id == stale_input_id && entry.waiter_count == 1),
        "stale waiter should be visible before recover reconciliation"
    );

    let runtime_id = runtime_id_for_session(&session_id);
    let report = crate::traits::RuntimeControlPlane::recover(&*adapter, &runtime_id)
        .await
        .expect("recover should reconcile waiters against active input truth");
    assert_eq!(report.inputs_recovered, 1);

    let after_recover = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after recover");
    assert_eq!(after_recover.completion_waiters.input_count, 1);
    assert_eq!(after_recover.completion_waiters.waiter_count, 1);
    assert_eq!(after_recover.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        after_recover.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert_eq!(
        after_recover.completion_waiters.waiting_inputs[0].waiter_count,
        1
    );

    match stale_handle.wait().await {
        CompletionOutcome::RuntimeTerminated(reason) => {
            assert_eq!(reason, "recovered input no longer pending");
        }
        other => panic!("expected recovered stale waiter termination, got {other:?}"),
    }

    SessionServiceRuntimeExt::reset_runtime(&*adapter, &session_id)
        .await
        .expect("reset should terminate preserved waiter at test end");
    match active_handle.wait().await {
        CompletionOutcome::RuntimeTerminated(reason) => {
            assert_eq!(reason, "runtime reset");
        }
        other => panic!("expected runtime reset termination, got {other:?}"),
    }
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_clears_completion_waiters_after_destroy() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;

    let input = make_prompt("destroy completion waiter");
    let input_id = input.id().clone();
    let (_outcome, handle) = adapter
        .accept_input_with_completion(&session_id, input)
        .await
        .expect("prompt should be accepted");
    let handle = handle.expect("queued prompt should register a completion waiter");

    let before_destroy = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist before destroy");
    assert_eq!(before_destroy.control.phase, RuntimeState::Idle);
    assert_eq!(before_destroy.completion_waiters.input_count, 1);
    assert_eq!(before_destroy.completion_waiters.waiter_count, 1);
    assert_eq!(before_destroy.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        before_destroy.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert_eq!(
        before_destroy.completion_waiters.waiting_inputs[0].waiter_count,
        1
    );

    let runtime_id = runtime_id_for_session(&session_id);
    let report = crate::traits::RuntimeControlPlane::destroy(&*adapter, &runtime_id)
        .await
        .expect("destroy should terminate active completion waiters");
    assert_eq!(report.inputs_abandoned, 1);

    let after_destroy = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after destroy");
    assert_eq!(after_destroy.control.phase, RuntimeState::Destroyed);
    assert!(
        after_destroy.inputs.queue.is_empty(),
        "destroy should not leave ordinary queued work behind once the runtime is destroyed"
    );
    assert!(
        after_destroy.inputs.steer_queue.is_empty(),
        "destroy should not leave steer-queued work behind once the runtime is destroyed"
    );
    assert_eq!(after_destroy.completion_waiters.input_count, 0);
    assert_eq!(after_destroy.completion_waiters.waiter_count, 0);
    assert!(
        after_destroy.completion_waiters.waiting_inputs.is_empty(),
        "destroy should clear the completion waiter carrier immediately"
    );

    match handle.wait().await {
        CompletionOutcome::RuntimeTerminated(reason) => {
            assert_eq!(reason, "runtime destroyed");
        }
        other => panic!("expected runtime destroyed termination, got {other:?}"),
    }
}

#[tokio::test]
async fn persistent_destroy_synchronizes_driver_control_projection_shadow() {
    let store = Arc::new(crate::store::InMemoryRuntimeStore::new());
    let adapter = Arc::new(MeerkatMachine::persistent(
        store as Arc<dyn crate::store::RuntimeStore>,
        memory_blob_store(),
    ));
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;

    let runtime_id = runtime_id_for_session(&session_id);
    let report = crate::traits::RuntimeControlPlane::destroy(&*adapter, &runtime_id)
        .await
        .expect("persistent destroy should succeed");
    assert_eq!(report.inputs_abandoned, 0);

    let sessions = adapter.sessions.read().await;
    let entry = sessions
        .get(&session_id)
        .expect("destroy keeps the session entry available for terminal snapshots");
    assert_eq!(
        entry.control_snapshot().phase,
        RuntimeState::Destroyed,
        "persistent destroy must not leave a stale driver-side control shadow",
    );
    let authority = entry
        .dsl_authority
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    assert_eq!(
        crate::meerkat_machine::dsl_authority::runtime_phase_from_authority(&authority),
        RuntimeState::Destroyed,
        "DSL remains the canonical destroyed authority",
    );
}

#[tokio::test]
async fn persistent_destroy_durable_commit_observes_canonical_destroy_truth() {
    struct BlockingDestroyCommitStore {
        inner: Arc<crate::store::InMemoryRuntimeStore>,
        destroy_commit_started: Notify,
        release_destroy_commit: Notify,
    }

    impl BlockingDestroyCommitStore {
        fn new() -> Self {
            Self {
                inner: Arc::new(crate::store::InMemoryRuntimeStore::new()),
                destroy_commit_started: Notify::new(),
                release_destroy_commit: Notify::new(),
            }
        }
    }

    #[async_trait::async_trait]
    impl RuntimeStore for BlockingDestroyCommitStore {
        async fn commit_session_boundary(
            &self,
            runtime_id: &LogicalRuntimeId,
            session_delta: crate::store::SessionDelta,
            run_id: RunId,
            boundary: RunApplyBoundary,
            contributing_input_ids: Vec<InputId>,
            input_updates: Vec<crate::input_state::StoredInputState>,
        ) -> Result<RunBoundaryReceipt, crate::store::RuntimeStoreError> {
            self.inner
                .commit_session_boundary(
                    runtime_id,
                    session_delta,
                    run_id,
                    boundary,
                    contributing_input_ids,
                    input_updates,
                )
                .await
        }

        async fn commit_session_snapshot(
            &self,
            runtime_id: &LogicalRuntimeId,
            session_delta: crate::store::SessionDelta,
        ) -> Result<(), crate::store::RuntimeStoreError> {
            self.inner
                .commit_session_snapshot(runtime_id, session_delta)
                .await
        }

        async fn atomic_apply(
            &self,
            runtime_id: &LogicalRuntimeId,
            session_delta: Option<crate::store::SessionDelta>,
            receipt: RunBoundaryReceipt,
            input_updates: Vec<crate::input_state::StoredInputState>,
            session_store_key: Option<SessionId>,
        ) -> Result<(), crate::store::RuntimeStoreError> {
            self.inner
                .atomic_apply(
                    runtime_id,
                    session_delta,
                    receipt,
                    input_updates,
                    session_store_key,
                )
                .await
        }

        async fn load_input_states(
            &self,
            runtime_id: &LogicalRuntimeId,
        ) -> Result<Vec<crate::input_state::StoredInputState>, crate::store::RuntimeStoreError>
        {
            self.inner.load_input_states(runtime_id).await
        }

        async fn load_boundary_receipt(
            &self,
            runtime_id: &LogicalRuntimeId,
            run_id: &RunId,
            sequence: u64,
        ) -> Result<Option<RunBoundaryReceipt>, crate::store::RuntimeStoreError> {
            self.inner
                .load_boundary_receipt(runtime_id, run_id, sequence)
                .await
        }

        async fn load_session_snapshot(
            &self,
            runtime_id: &LogicalRuntimeId,
        ) -> Result<Option<Vec<u8>>, crate::store::RuntimeStoreError> {
            self.inner.load_session_snapshot(runtime_id).await
        }

        async fn persist_input_state(
            &self,
            runtime_id: &LogicalRuntimeId,
            state: &crate::input_state::StoredInputState,
        ) -> Result<(), crate::store::RuntimeStoreError> {
            self.inner.persist_input_state(runtime_id, state).await
        }

        async fn load_input_state(
            &self,
            runtime_id: &LogicalRuntimeId,
            input_id: &InputId,
        ) -> Result<Option<crate::input_state::StoredInputState>, crate::store::RuntimeStoreError>
        {
            self.inner.load_input_state(runtime_id, input_id).await
        }

        async fn persist_runtime_state(
            &self,
            runtime_id: &LogicalRuntimeId,
            state: RuntimeState,
        ) -> Result<(), crate::store::RuntimeStoreError> {
            self.inner.persist_runtime_state(runtime_id, state).await
        }

        async fn load_runtime_state(
            &self,
            runtime_id: &LogicalRuntimeId,
        ) -> Result<Option<RuntimeState>, crate::store::RuntimeStoreError> {
            self.inner.load_runtime_state(runtime_id).await
        }

        async fn atomic_lifecycle_commit(
            &self,
            runtime_id: &LogicalRuntimeId,
            runtime_state: RuntimeState,
            input_states: &[crate::input_state::StoredInputState],
        ) -> Result<(), crate::store::RuntimeStoreError> {
            self.inner
                .atomic_lifecycle_commit(runtime_id, runtime_state, input_states)
                .await?;
            if runtime_state == RuntimeState::Destroyed {
                self.destroy_commit_started.notify_one();
                self.release_destroy_commit.notified().await;
            }
            Ok(())
        }
    }

    let store = Arc::new(BlockingDestroyCommitStore::new());
    let adapter = Arc::new(MeerkatMachine::persistent(
        Arc::clone(&store) as Arc<dyn crate::store::RuntimeStore>,
        memory_blob_store(),
    ));
    let session_id = SessionId::new();
    adapter.register_session(session_id.clone()).await;

    let runtime_id = LogicalRuntimeId::for_session(&session_id);
    let destroy_task = tokio::spawn({
        let adapter = Arc::clone(&adapter);
        let runtime_id = runtime_id.clone();
        async move { crate::traits::RuntimeControlPlane::destroy(&*adapter, &runtime_id).await }
    });

    tokio::time::timeout(
        Duration::from_secs(2),
        store.destroy_commit_started.notified(),
    )
    .await
    .expect("destroy should reach the durable lifecycle commit");

    assert_eq!(
        store.inner.load_runtime_state(&runtime_id).await.unwrap(),
        Some(RuntimeState::Destroyed),
        "test probe should observe the store after durable destroyed commit",
    );

    let sessions = adapter.sessions.read().await;
    let entry = sessions
        .get(&session_id)
        .expect("destroy keeps the session entry available for terminal snapshots");
    assert_eq!(
        entry.control_snapshot().phase,
        RuntimeState::Destroyed,
        "durable destroyed state must not become visible while the shared driver projection still trails canonical destroy",
    );
    let authority = entry
        .dsl_authority
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    assert_eq!(
        crate::meerkat_machine::dsl_authority::runtime_phase_from_authority(&authority),
        RuntimeState::Destroyed,
        "durable destroyed state must not race ahead of canonical DSL destroy truth",
    );
    drop(authority);
    drop(sessions);

    store.release_destroy_commit.notify_waiters();
    let report = tokio::time::timeout(Duration::from_secs(2), destroy_task)
        .await
        .expect("destroy task should finish after releasing the store")
        .expect("destroy task should not panic")
        .expect("destroy should succeed");
    assert_eq!(report.inputs_abandoned, 0);
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_destroy_clears_steered_waiter_and_queue_but_preserves_wait_all()
 {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;

    let input = Input::Prompt(crate::input::PromptInput::new(
        "destroy steered prompt",
        Some(
            meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                handling_mode: Some(meerkat_core::types::HandlingMode::Steer),
                ..Default::default()
            },
        ),
    ));
    let input_id = input.id().clone();
    let (_outcome, completion_handle) = adapter
        .accept_input_with_completion(&session_id, input)
        .await
        .expect("steered prompt should be accepted");
    let completion_handle =
        completion_handle.expect("steered prompt should register a completion waiter");

    let registry = adapter
        .ops_lifecycle_registry(&session_id)
        .await
        .expect("ops registry should exist for registered session");

    let operation_id = OperationId::new();
    registry
        .register_operation(OperationSpec {
            id: operation_id.clone(),
            kind: OperationKind::BackgroundToolOp,
            owner_session_id: session_id.clone(),
            display_name: "destroy steered wait target".into(),
            source_label: "meerkat_machine_test".into(),
            child_session_id: None,
            expect_peer_channel: false,
        })
        .expect("operation should register");
    registry
        .provisioning_succeeded(&operation_id)
        .expect("operation should enter running");

    let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

    let before_destroy = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist before destroy");
    let wait_request_id = before_destroy
        .ops
        .wait_request_id
        .clone()
        .expect("wait_all should register an authority-owned wait request");
    assert_eq!(before_destroy.control.phase, RuntimeState::Idle);
    assert!(before_destroy.inputs.queue.is_empty());
    assert_eq!(before_destroy.inputs.steer_queue, vec![input_id.clone()]);
    assert_eq!(before_destroy.completion_waiters.input_count, 1);
    assert_eq!(before_destroy.completion_waiters.waiter_count, 1);
    assert_eq!(before_destroy.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        before_destroy.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert!(before_destroy.ops.pending_wait_present);
    assert_eq!(
        before_destroy.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "pending wait carrier should track the same wait request id before destroy"
    );
    assert_eq!(
        before_destroy.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "wait_all should track the active operation before destroy"
    );

    let runtime_id = runtime_id_for_session(&session_id);
    let report = crate::traits::RuntimeControlPlane::destroy(&*adapter, &runtime_id)
        .await
        .expect("destroy should clear the steered completion waiter while preserving wait_all");
    assert_eq!(report.inputs_abandoned, 1);

    match completion_handle.wait().await {
        CompletionOutcome::RuntimeTerminated(reason) => {
            assert_eq!(reason, "runtime destroyed");
        }
        other => {
            panic!("expected runtime destroyed termination for steered input, got {other:?}")
        }
    }

    let after_destroy = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after destroy");
    assert_eq!(after_destroy.control.phase, RuntimeState::Destroyed);
    assert_eq!(after_destroy.control.current_run_id, None);
    assert_eq!(after_destroy.inputs.current_run_id, None);
    assert!(after_destroy.inputs.queue.is_empty());
    assert!(
        after_destroy.inputs.steer_queue.is_empty(),
        "destroy should clear steered queued work immediately on the plain runtime path"
    );
    assert_eq!(after_destroy.completion_waiters.input_count, 0);
    assert_eq!(after_destroy.completion_waiters.waiter_count, 0);
    assert!(
        after_destroy.completion_waiters.waiting_inputs.is_empty(),
        "destroy should clear input-owned steered completion waiters immediately"
    );
    assert_eq!(
        after_destroy.ops.wait_request_id,
        Some(wait_request_id.clone()),
        "destroy should preserve the authority-owned wait request after steered completion waiters clear"
    );
    assert!(after_destroy.ops.pending_wait_present);
    assert_eq!(
        after_destroy.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "destroy should preserve request-id agreement across the wait carrier seam"
    );
    assert_eq!(
        after_destroy.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "destroy should preserve the tracked wait target until the operation settles"
    );

    registry
        .complete_operation(
            &operation_id,
            OperationResult {
                id: operation_id.clone(),
                content: "done".into(),
                is_error: false,
                duration_ms: 1,
                tokens_used: 0,
            },
        )
        .expect("operation should complete after destroy");
    let wait_result = wait_future.await.expect("wait_all should still resolve");
    assert_eq!(wait_result.satisfied.wait_request_id, wait_request_id);
    assert_eq!(
        wait_result.satisfied.operation_ids,
        vec![operation_id.clone()]
    );

    let settled = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after wait_all settles");
    assert_eq!(settled.control.phase, RuntimeState::Destroyed);
    assert!(settled.inputs.queue.is_empty());
    assert!(settled.inputs.steer_queue.is_empty());
    assert_eq!(settled.ops.wait_request_id, None);
    assert!(!settled.ops.pending_wait_present);
    assert_eq!(settled.ops.pending_wait_request_id, None);
    assert!(settled.ops.wait_operation_ids.is_empty());
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_clears_completion_waiters_after_destroy_with_runtime_loop()
{
    struct RecordingExecutor {
        apply_calls: Arc<AtomicUsize>,
        control_calls: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl CoreExecutor for RecordingExecutor {
        async fn apply(
            &mut self,
            run_id: RunId,
            primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            self.apply_calls.fetch_add(1, Ordering::SeqCst);
            Ok(CoreApplyOutput {
                receipt: RunBoundaryReceipt {
                    run_id,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                    sequence: 0,
                },
                session_snapshot: None,
                terminal: None,
            })
        }

        async fn control(&mut self, _command: RunControlCommand) -> Result<(), CoreExecutorError> {
            self.control_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    let apply_calls = Arc::new(AtomicUsize::new(0));
    let control_calls = Arc::new(AtomicUsize::new(0));

    adapter
        .register_session_with_executor(
            session_id.clone(),
            Box::new(RecordingExecutor {
                apply_calls: Arc::clone(&apply_calls),
                control_calls: Arc::clone(&control_calls),
            }),
        )
        .await;

    let input = make_progress_input("destroy-with-loop");
    let input_id = input.id().clone();
    let outcome = adapter
        .accept_input_without_wake(&session_id, input)
        .await
        .expect("progress input should queue without waking the attached loop");
    assert!(outcome.is_accepted());

    let handle = {
        let completions = {
            let sessions = adapter.sessions.read().await;
            sessions
                .get(&session_id)
                .expect("attached session should exist")
                .completions
                .clone()
        };
        let mut completions = completions.lock().await;
        completions.register(input_id.clone())
    };

    let before_destroy = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist before destroy");
    assert_eq!(before_destroy.control.phase, RuntimeState::Attached);
    assert_eq!(before_destroy.completion_waiters.input_count, 1);
    assert_eq!(before_destroy.completion_waiters.waiter_count, 1);
    assert_eq!(before_destroy.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        before_destroy.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert_eq!(
        apply_calls.load(Ordering::SeqCst),
        0,
        "destroy should be able to abandon queued attached-loop work before apply runs"
    );
    assert_eq!(
        control_calls.load(Ordering::SeqCst),
        0,
        "destroy has not yet attempted any executor control"
    );

    let runtime_id = runtime_id_for_session(&session_id);
    let report = crate::traits::RuntimeControlPlane::destroy(&*adapter, &runtime_id)
        .await
        .expect("destroy should synchronously clear queued waiters even with a live loop");
    assert_eq!(report.inputs_abandoned, 1);

    let after_destroy = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after destroy");
    assert_eq!(after_destroy.control.phase, RuntimeState::Destroyed);
    assert!(
        after_destroy.inputs.queue.is_empty(),
        "destroy should not leave ordinary queued work behind even when an attached loop exists"
    );
    assert!(
        after_destroy.inputs.steer_queue.is_empty(),
        "destroy should not leave steer-queued work behind even when an attached loop exists"
    );
    assert_eq!(after_destroy.completion_waiters.input_count, 0);
    assert_eq!(after_destroy.completion_waiters.waiter_count, 0);
    assert!(
        after_destroy.completion_waiters.waiting_inputs.is_empty(),
        "destroy should clear the completion waiter carrier immediately even when a loop is attached"
    );
    assert_eq!(
        apply_calls.load(Ordering::SeqCst),
        0,
        "destroy should bypass queued attached-loop work entirely"
    );
    assert_eq!(
        control_calls.load(Ordering::SeqCst),
        0,
        "destroy currently bypasses the executor control seam and does not deliver an out-of-band control command"
    );

    match handle.wait().await {
        CompletionOutcome::RuntimeTerminated(reason) => {
            assert_eq!(reason, "runtime destroyed");
        }
        other => panic!("expected runtime destroyed termination, got {other:?}"),
    }
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_attached_steered_prompt_requests_immediate_processing() {
    struct BlockingExecutor {
        apply_calls: Arc<AtomicUsize>,
        control_calls: Arc<AtomicUsize>,
        apply_started: Arc<Notify>,
        apply_finished: Arc<Notify>,
        allow_finish: Arc<Notify>,
    }

    #[async_trait::async_trait]
    impl CoreExecutor for BlockingExecutor {
        async fn apply(
            &mut self,
            run_id: RunId,
            primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            self.apply_calls.fetch_add(1, Ordering::SeqCst);
            self.apply_started.notify_waiters();
            self.allow_finish.notified().await;
            self.apply_finished.notify_waiters();

            Ok(CoreApplyOutput {
                receipt: RunBoundaryReceipt {
                    run_id,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                    sequence: 0,
                },
                session_snapshot: None,
                terminal: None,
            })
        }

        async fn control(&mut self, _command: RunControlCommand) -> Result<(), CoreExecutorError> {
            self.control_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    let apply_calls = Arc::new(AtomicUsize::new(0));
    let control_calls = Arc::new(AtomicUsize::new(0));
    let apply_started = Arc::new(Notify::new());
    let apply_finished = Arc::new(Notify::new());
    let allow_finish = Arc::new(Notify::new());

    adapter
        .register_session_with_executor(
            session_id.clone(),
            Box::new(BlockingExecutor {
                apply_calls: Arc::clone(&apply_calls),
                control_calls: Arc::clone(&control_calls),
                apply_started: Arc::clone(&apply_started),
                apply_finished: Arc::clone(&apply_finished),
                allow_finish: Arc::clone(&allow_finish),
            }),
        )
        .await;

    let input = Input::Prompt(crate::input::PromptInput::new(
        "attached steered prompt",
        Some(
            meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                handling_mode: Some(meerkat_core::types::HandlingMode::Steer),
                ..Default::default()
            },
        ),
    ));
    let input_id = input.id().clone();
    let (outcome, completion_handle) = adapter
        .accept_input_with_completion(&session_id, input)
        .await
        .expect("attached steered prompt should be accepted");
    assert!(outcome.is_accepted());
    let completion_handle =
        completion_handle.expect("attached steered prompt should expose a completion waiter");

    tokio::time::timeout(Duration::from_secs(1), apply_started.notified())
        .await
        .expect("attached steered prompt should request immediate processing");

    let during_apply = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist while attached steered work is active");
    assert_eq!(
        during_apply.control.phase,
        RuntimeState::Running,
        "attached steered input should enter Running rather than remain in a queue-only attached state"
    );
    assert_eq!(
        during_apply.control.current_run_id, during_apply.inputs.current_run_id,
        "attached steered input should bind control and ingress to the same active run"
    );
    assert!(
        during_apply.control.current_run_id.is_some(),
        "attached steered input should create an active run binding"
    );
    assert!(
        during_apply.inputs.queue.is_empty(),
        "attached steered input should not occupy the ordinary queue while it is actively processing"
    );
    assert!(
        during_apply.inputs.steer_queue.is_empty(),
        "attached steered input should not remain in the steer queue once immediate processing begins"
    );
    assert_eq!(during_apply.completion_waiters.input_count, 1);
    assert_eq!(during_apply.completion_waiters.waiter_count, 1);
    assert_eq!(during_apply.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        during_apply.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert_eq!(
        apply_calls.load(Ordering::SeqCst),
        1,
        "attached steered input should wake the attached loop exactly once"
    );
    assert_eq!(
        control_calls.load(Ordering::SeqCst),
        1,
        "attached steered admission currently routes one control command through the executor seam while requesting immediate processing"
    );

    allow_finish.notify_waiters();
    tokio::time::timeout(Duration::from_secs(1), apply_finished.notified())
        .await
        .expect("attached steered prompt should finish after the executor is released");

    match completion_handle.wait().await {
        CompletionOutcome::CompletedWithoutResult => {}
        other => panic!(
            "expected attached steered prompt to complete through the live loop, got {other:?}"
        ),
    }

    let settled = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let snapshot = adapter
                .meerkat_machine_spine_snapshot(&session_id)
                .await
                .expect("snapshot should exist after attached steered work completes");
            if snapshot.control.phase == RuntimeState::Attached {
                break snapshot;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("attached runtime should return to Attached after steered work completes");
    assert_eq!(settled.control.current_run_id, None);
    assert_eq!(settled.inputs.current_run_id, None);
    assert!(settled.inputs.queue.is_empty());
    assert!(settled.inputs.steer_queue.is_empty());
    assert_eq!(settled.completion_waiters.input_count, 0);
    assert_eq!(settled.completion_waiters.waiter_count, 0);
    assert!(settled.completion_waiters.waiting_inputs.is_empty());
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_attached_steered_prompt_splits_completion_and_wait_all_lifetimes()
 {
    struct BlockingExecutor {
        apply_calls: Arc<AtomicUsize>,
        control_calls: Arc<AtomicUsize>,
        apply_started: Arc<Notify>,
        apply_finished: Arc<Notify>,
        allow_finish: Arc<Notify>,
    }

    #[async_trait::async_trait]
    impl CoreExecutor for BlockingExecutor {
        async fn apply(
            &mut self,
            run_id: RunId,
            primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            self.apply_calls.fetch_add(1, Ordering::SeqCst);
            self.apply_started.notify_waiters();
            self.allow_finish.notified().await;
            self.apply_finished.notify_waiters();

            Ok(CoreApplyOutput {
                receipt: RunBoundaryReceipt {
                    run_id,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                    sequence: 0,
                },
                session_snapshot: None,
                terminal: None,
            })
        }

        async fn control(&mut self, _command: RunControlCommand) -> Result<(), CoreExecutorError> {
            self.control_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    let apply_calls = Arc::new(AtomicUsize::new(0));
    let control_calls = Arc::new(AtomicUsize::new(0));
    let apply_started = Arc::new(Notify::new());
    let apply_finished = Arc::new(Notify::new());
    let allow_finish = Arc::new(Notify::new());

    adapter
        .register_session_with_executor(
            session_id.clone(),
            Box::new(BlockingExecutor {
                apply_calls: Arc::clone(&apply_calls),
                control_calls: Arc::clone(&control_calls),
                apply_started: Arc::clone(&apply_started),
                apply_finished: Arc::clone(&apply_finished),
                allow_finish: Arc::clone(&allow_finish),
            }),
        )
        .await;

    let registry = adapter
        .ops_lifecycle_registry(&session_id)
        .await
        .expect("ops registry should exist for attached session");

    let operation_id = OperationId::new();
    registry
        .register_operation(OperationSpec {
            id: operation_id.clone(),
            kind: OperationKind::BackgroundToolOp,
            owner_session_id: session_id.clone(),
            display_name: "attached steered wait target".into(),
            source_label: "meerkat_machine_test".into(),
            child_session_id: None,
            expect_peer_channel: false,
        })
        .expect("operation should register");
    registry
        .provisioning_succeeded(&operation_id)
        .expect("operation should enter running");

    let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

    let input = Input::Prompt(crate::input::PromptInput::new(
        "attached steered split lifetimes",
        Some(
            meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                handling_mode: Some(meerkat_core::types::HandlingMode::Steer),
                ..Default::default()
            },
        ),
    ));
    let input_id = input.id().clone();
    let (outcome, completion_handle) = adapter
        .accept_input_with_completion(&session_id, input)
        .await
        .expect("attached steered prompt should be accepted");
    assert!(outcome.is_accepted());
    let completion_handle =
        completion_handle.expect("attached steered prompt should expose a completion waiter");

    tokio::time::timeout(Duration::from_secs(1), apply_started.notified())
        .await
        .expect("attached steered prompt should request immediate processing");

    let during_apply = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist while attached steered work is active");
    let wait_request_id = during_apply
        .ops
        .wait_request_id
        .clone()
        .expect("wait_all should register an authority-owned wait request");
    assert_eq!(during_apply.control.phase, RuntimeState::Running);
    assert_eq!(
        during_apply.control.current_run_id,
        during_apply.inputs.current_run_id
    );
    assert!(during_apply.control.current_run_id.is_some());
    assert!(during_apply.inputs.queue.is_empty());
    assert!(during_apply.inputs.steer_queue.is_empty());
    assert_eq!(during_apply.completion_waiters.input_count, 1);
    assert_eq!(during_apply.completion_waiters.waiter_count, 1);
    assert_eq!(during_apply.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        during_apply.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert_eq!(
        during_apply.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "pending wait carrier should track the same wait request id while the attached steered prompt is active"
    );
    assert_eq!(
        during_apply.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "wait_all should track the background operation while attached steered work is active"
    );
    assert_eq!(
        apply_calls.load(Ordering::SeqCst),
        1,
        "attached steered work should wake the loop exactly once"
    );
    assert_eq!(
        control_calls.load(Ordering::SeqCst),
        1,
        "attached steered admission should still route one control command through the executor seam"
    );

    allow_finish.notify_waiters();
    tokio::time::timeout(Duration::from_secs(1), apply_finished.notified())
        .await
        .expect("attached steered prompt should finish after the executor is released");

    match completion_handle.wait().await {
        CompletionOutcome::CompletedWithoutResult => {}
        other => panic!(
            "expected attached steered prompt to complete while wait_all remains live, got {other:?}"
        ),
    }

    let after_completion = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let snapshot = adapter
                .meerkat_machine_spine_snapshot(&session_id)
                .await
                .expect("snapshot should exist after attached steered completion");
            if snapshot.control.phase == RuntimeState::Attached {
                break snapshot;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("attached runtime should return to Attached after steered work completes");
    assert_eq!(after_completion.control.current_run_id, None);
    assert_eq!(after_completion.inputs.current_run_id, None);
    assert!(after_completion.inputs.queue.is_empty());
    assert!(after_completion.inputs.steer_queue.is_empty());
    assert_eq!(after_completion.completion_waiters.input_count, 0);
    assert_eq!(after_completion.completion_waiters.waiter_count, 0);
    assert!(
        after_completion
            .completion_waiters
            .waiting_inputs
            .is_empty()
    );
    assert_eq!(
        after_completion.ops.wait_request_id,
        Some(wait_request_id.clone()),
        "attached steered completion should clear the input-owned completion waiter while preserving the ops-owned wait_all carrier"
    );
    assert!(after_completion.ops.pending_wait_present);
    assert_eq!(
        after_completion.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "request-id agreement should survive after attached steered completion clears the input waiter"
    );
    assert_eq!(
        after_completion.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "tracked wait target should remain present until the background operation itself settles"
    );

    registry
        .complete_operation(
            &operation_id,
            OperationResult {
                id: operation_id.clone(),
                content: "done".into(),
                is_error: false,
                duration_ms: 1,
                tokens_used: 0,
            },
        )
        .expect("operation should complete after attached steered completion");
    let wait_result = wait_future.await.expect("wait_all should still resolve");
    assert_eq!(wait_result.satisfied.wait_request_id, wait_request_id);
    assert_eq!(
        wait_result.satisfied.operation_ids,
        vec![operation_id.clone()]
    );

    let settled = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after wait_all settles");
    assert_eq!(settled.control.phase, RuntimeState::Attached);
    assert_eq!(settled.control.current_run_id, None);
    assert_eq!(settled.inputs.current_run_id, None);
    assert!(settled.inputs.queue.is_empty());
    assert!(settled.inputs.steer_queue.is_empty());
    assert_eq!(settled.completion_waiters.input_count, 0);
    assert_eq!(settled.completion_waiters.waiter_count, 0);
    assert!(settled.completion_waiters.waiting_inputs.is_empty());
    assert_eq!(settled.ops.wait_request_id, None);
    assert!(!settled.ops.pending_wait_present);
    assert_eq!(settled.ops.pending_wait_request_id, None);
    assert!(settled.ops.wait_operation_ids.is_empty());
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_attached_steered_prompt_preserves_completion_after_wait_all_settles()
 {
    struct BlockingExecutor {
        apply_calls: Arc<AtomicUsize>,
        control_calls: Arc<AtomicUsize>,
        apply_started: Arc<Notify>,
        apply_finished: Arc<Notify>,
        allow_finish: Arc<Notify>,
    }

    #[async_trait::async_trait]
    impl CoreExecutor for BlockingExecutor {
        async fn apply(
            &mut self,
            run_id: RunId,
            primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            self.apply_calls.fetch_add(1, Ordering::SeqCst);
            self.apply_started.notify_waiters();
            self.allow_finish.notified().await;
            self.apply_finished.notify_waiters();

            Ok(CoreApplyOutput {
                receipt: RunBoundaryReceipt {
                    run_id,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                    sequence: 0,
                },
                session_snapshot: None,
                terminal: None,
            })
        }

        async fn control(&mut self, _command: RunControlCommand) -> Result<(), CoreExecutorError> {
            self.control_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    let apply_calls = Arc::new(AtomicUsize::new(0));
    let control_calls = Arc::new(AtomicUsize::new(0));
    let apply_started = Arc::new(Notify::new());
    let apply_finished = Arc::new(Notify::new());
    let allow_finish = Arc::new(Notify::new());

    adapter
        .register_session_with_executor(
            session_id.clone(),
            Box::new(BlockingExecutor {
                apply_calls: Arc::clone(&apply_calls),
                control_calls: Arc::clone(&control_calls),
                apply_started: Arc::clone(&apply_started),
                apply_finished: Arc::clone(&apply_finished),
                allow_finish: Arc::clone(&allow_finish),
            }),
        )
        .await;

    let registry = adapter
        .ops_lifecycle_registry(&session_id)
        .await
        .expect("ops registry should exist for attached session");

    let operation_id = OperationId::new();
    registry
        .register_operation(OperationSpec {
            id: operation_id.clone(),
            // Use MobMemberChild here so the proof isolates completion-vs-wait_all
            // ordering without immediately arming the detached-wake continuation path.
            kind: OperationKind::MobMemberChild,
            owner_session_id: session_id.clone(),
            display_name: "attached steered wait-first child".into(),
            source_label: "meerkat_machine_test".into(),
            child_session_id: None,
            expect_peer_channel: true,
        })
        .expect("operation should register");
    registry
        .provisioning_succeeded(&operation_id)
        .expect("operation should enter running");

    let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

    let input = Input::Prompt(crate::input::PromptInput::new(
        "attached steered wait-first split",
        Some(
            meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                handling_mode: Some(meerkat_core::types::HandlingMode::Steer),
                ..Default::default()
            },
        ),
    ));
    let input_id = input.id().clone();
    let (outcome, completion_handle) = adapter
        .accept_input_with_completion(&session_id, input)
        .await
        .expect("attached steered prompt should be accepted");
    assert!(outcome.is_accepted());
    let completion_handle =
        completion_handle.expect("attached steered prompt should expose a completion waiter");

    tokio::time::timeout(Duration::from_secs(1), apply_started.notified())
        .await
        .expect("attached steered prompt should request immediate processing");

    let during_apply = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist while attached steered work is active");
    let wait_request_id = during_apply
        .ops
        .wait_request_id
        .clone()
        .expect("wait_all should register an authority-owned wait request");
    assert_eq!(during_apply.control.phase, RuntimeState::Running);
    assert_eq!(
        during_apply.control.current_run_id,
        during_apply.inputs.current_run_id
    );
    assert!(during_apply.control.current_run_id.is_some());
    assert!(during_apply.inputs.queue.is_empty());
    assert!(during_apply.inputs.steer_queue.is_empty());
    assert_eq!(during_apply.completion_waiters.input_count, 1);
    assert_eq!(during_apply.completion_waiters.waiter_count, 1);
    assert_eq!(during_apply.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        during_apply.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert_eq!(
        during_apply.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "pending wait carrier should track the same wait request id while attached steered work is active"
    );
    assert_eq!(
        during_apply.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "wait_all should track the live operation while attached steered work is active"
    );
    assert_eq!(apply_calls.load(Ordering::SeqCst), 1);
    assert_eq!(control_calls.load(Ordering::SeqCst), 1);

    registry
        .complete_operation(
            &operation_id,
            OperationResult {
                id: operation_id.clone(),
                content: "done".into(),
                is_error: false,
                duration_ms: 1,
                tokens_used: 0,
            },
        )
        .expect("operation should complete while attached steered work remains in flight");
    let wait_result = wait_future.await.expect("wait_all should resolve first");
    assert_eq!(wait_result.satisfied.wait_request_id, wait_request_id);
    assert_eq!(
        wait_result.satisfied.operation_ids,
        vec![operation_id.clone()]
    );

    let after_wait_all = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let snapshot = adapter
                .meerkat_machine_spine_snapshot(&session_id)
                .await
                .expect("snapshot should exist after wait_all settles");
            if snapshot.ops.wait_request_id.is_none() && !snapshot.ops.pending_wait_present {
                break snapshot;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("attached steered snapshot should eventually clear the wait_all carrier");
    assert_eq!(
        after_wait_all.control.phase,
        RuntimeState::Running,
        "attached steered work should remain Running while apply is still blocked even after wait_all settles"
    );
    assert_eq!(
        after_wait_all.control.current_run_id, after_wait_all.inputs.current_run_id,
        "attached steered work should keep control and ingress bound to the same active run until completion"
    );
    assert!(after_wait_all.control.current_run_id.is_some());
    assert!(after_wait_all.inputs.queue.is_empty());
    assert!(after_wait_all.inputs.steer_queue.is_empty());
    assert_eq!(after_wait_all.completion_waiters.input_count, 1);
    assert_eq!(after_wait_all.completion_waiters.waiter_count, 1);
    assert_eq!(after_wait_all.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        after_wait_all.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert_eq!(after_wait_all.ops.wait_request_id, None);
    assert!(!after_wait_all.ops.pending_wait_present);
    assert_eq!(after_wait_all.ops.pending_wait_request_id, None);
    assert!(
        after_wait_all.ops.wait_operation_ids.is_empty(),
        "wait_all should release the tracked wait target before the attached steered completion waiter clears"
    );

    allow_finish.notify_waiters();
    tokio::time::timeout(Duration::from_secs(1), apply_finished.notified())
        .await
        .expect("attached steered prompt should finish after the executor is released");

    match completion_handle.wait().await {
        CompletionOutcome::CompletedWithoutResult => {}
        other => panic!(
            "expected attached steered prompt to complete after wait_all settled first, got {other:?}"
        ),
    }

    let settled = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let snapshot = adapter
                .meerkat_machine_spine_snapshot(&session_id)
                .await
                .expect("snapshot should exist after attached steered completion");
            if snapshot.control.phase == RuntimeState::Attached {
                break snapshot;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("attached runtime should return to Attached after steered work completes");
    assert_eq!(settled.control.current_run_id, None);
    assert_eq!(settled.inputs.current_run_id, None);
    assert!(settled.inputs.queue.is_empty());
    assert!(settled.inputs.steer_queue.is_empty());
    assert_eq!(settled.completion_waiters.input_count, 0);
    assert_eq!(settled.completion_waiters.waiter_count, 0);
    assert!(settled.completion_waiters.waiting_inputs.is_empty());
    assert_eq!(settled.ops.wait_request_id, None);
    assert!(!settled.ops.pending_wait_present);
    assert_eq!(settled.ops.pending_wait_request_id, None);
    assert!(settled.ops.wait_operation_ids.is_empty());
    assert_eq!(
        apply_calls.load(Ordering::SeqCst),
        1,
        "isolated attached steered completion should not trigger a follow-on continuation run"
    );
    assert_eq!(
        control_calls.load(Ordering::SeqCst),
        1,
        "isolated attached steered completion should not emit extra executor control commands"
    );
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_attached_steered_prompt_destroy_splits_completion_and_wait_all_lifetimes()
 {
    struct BlockingExecutor {
        apply_calls: Arc<AtomicUsize>,
        control_calls: Arc<AtomicUsize>,
        apply_started: Arc<Notify>,
        apply_finished: Arc<Notify>,
        allow_finish: Arc<Notify>,
    }

    #[async_trait::async_trait]
    impl CoreExecutor for BlockingExecutor {
        async fn apply(
            &mut self,
            run_id: RunId,
            primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            self.apply_calls.fetch_add(1, Ordering::SeqCst);
            self.apply_started.notify_waiters();
            self.allow_finish.notified().await;
            self.apply_finished.notify_waiters();

            Ok(CoreApplyOutput {
                receipt: RunBoundaryReceipt {
                    run_id,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                    sequence: 0,
                },
                session_snapshot: None,
                terminal: None,
            })
        }

        async fn control(&mut self, _command: RunControlCommand) -> Result<(), CoreExecutorError> {
            self.control_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    let apply_calls = Arc::new(AtomicUsize::new(0));
    let control_calls = Arc::new(AtomicUsize::new(0));
    let apply_started = Arc::new(Notify::new());
    let apply_finished = Arc::new(Notify::new());
    let allow_finish = Arc::new(Notify::new());

    adapter
        .register_session_with_executor(
            session_id.clone(),
            Box::new(BlockingExecutor {
                apply_calls: Arc::clone(&apply_calls),
                control_calls: Arc::clone(&control_calls),
                apply_started: Arc::clone(&apply_started),
                apply_finished: Arc::clone(&apply_finished),
                allow_finish: Arc::clone(&allow_finish),
            }),
        )
        .await;

    let registry = adapter
        .ops_lifecycle_registry(&session_id)
        .await
        .expect("ops registry should exist for attached session");

    let operation_id = OperationId::new();
    registry
        .register_operation(OperationSpec {
            id: operation_id.clone(),
            kind: OperationKind::MobMemberChild,
            owner_session_id: session_id.clone(),
            display_name: "attached steered destroy wait target".into(),
            source_label: "meerkat_machine_test".into(),
            child_session_id: None,
            expect_peer_channel: true,
        })
        .expect("operation should register");
    registry
        .provisioning_succeeded(&operation_id)
        .expect("operation should enter running");

    let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

    let input = Input::Prompt(crate::input::PromptInput::new(
        "attached steered destroy split",
        Some(
            meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                handling_mode: Some(meerkat_core::types::HandlingMode::Steer),
                ..Default::default()
            },
        ),
    ));
    let input_id = input.id().clone();
    let (outcome, completion_handle) = adapter
        .accept_input_with_completion(&session_id, input)
        .await
        .expect("attached steered prompt should be accepted");
    assert!(outcome.is_accepted());
    let completion_handle =
        completion_handle.expect("attached steered prompt should expose a completion waiter");

    tokio::time::timeout(Duration::from_secs(1), apply_started.notified())
        .await
        .expect("attached steered prompt should request immediate processing");

    let during_apply = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist while attached steered work is active");
    let wait_request_id = during_apply
        .ops
        .wait_request_id
        .clone()
        .expect("wait_all should register an authority-owned wait request");
    assert_eq!(during_apply.control.phase, RuntimeState::Running);
    assert_eq!(
        during_apply.control.current_run_id,
        during_apply.inputs.current_run_id
    );
    assert!(during_apply.control.current_run_id.is_some());
    assert!(during_apply.inputs.queue.is_empty());
    assert!(during_apply.inputs.steer_queue.is_empty());
    assert_eq!(during_apply.completion_waiters.input_count, 1);
    assert_eq!(during_apply.completion_waiters.waiter_count, 1);
    assert_eq!(during_apply.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        during_apply.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert_eq!(
        during_apply.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "pending wait carrier should track the same wait request id while attached steered work is active"
    );
    assert_eq!(
        during_apply.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "wait_all should track the live operation while attached steered work is active"
    );
    assert_eq!(apply_calls.load(Ordering::SeqCst), 1);
    assert_eq!(control_calls.load(Ordering::SeqCst), 1);

    let runtime_id = runtime_id_for_session(&session_id);
    crate::traits::RuntimeControlPlane::destroy(&*adapter, &runtime_id)
        .await
        .expect("destroy should split attached steered completion and wait_all lifetimes");

    match completion_handle.wait().await {
        CompletionOutcome::RuntimeTerminated(reason) => {
            assert_eq!(reason, "runtime destroyed");
        }
        other => panic!(
            "expected attached steered completion waiter to terminate on destroy, got {other:?}"
        ),
    }

    let after_destroy = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after destroy");
    assert_eq!(after_destroy.control.phase, RuntimeState::Destroyed);
    assert!(after_destroy.inputs.queue.is_empty());
    assert!(after_destroy.inputs.steer_queue.is_empty());
    assert_eq!(after_destroy.completion_waiters.input_count, 0);
    assert_eq!(after_destroy.completion_waiters.waiter_count, 0);
    assert!(
        after_destroy.completion_waiters.waiting_inputs.is_empty(),
        "destroy should clear the steered completion waiter immediately even while apply remains blocked"
    );
    assert_eq!(
        after_destroy.ops.wait_request_id,
        Some(wait_request_id.clone()),
        "destroy should preserve the authority-owned wait request after the steered completion waiter clears"
    );
    assert!(after_destroy.ops.pending_wait_present);
    assert_eq!(
        after_destroy.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "destroy should preserve request-id agreement across the wait carrier seam"
    );
    assert_eq!(
        after_destroy.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "destroy should preserve the tracked wait target until the waited operation settles"
    );

    allow_finish.notify_waiters();
    tokio::time::timeout(Duration::from_secs(1), apply_finished.notified())
        .await
        .expect("blocked attached apply should finish once the executor is released");

    registry
        .complete_operation(
            &operation_id,
            OperationResult {
                id: operation_id.clone(),
                content: "done".into(),
                is_error: false,
                duration_ms: 1,
                tokens_used: 0,
            },
        )
        .expect("operation should complete after destroy");
    let wait_result = wait_future.await.expect("wait_all should still resolve");
    assert_eq!(wait_result.satisfied.wait_request_id, wait_request_id);
    assert_eq!(
        wait_result.satisfied.operation_ids,
        vec![operation_id.clone()]
    );

    let settled = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after wait_all settles");
    assert_eq!(settled.control.phase, RuntimeState::Destroyed);
    assert_eq!(settled.control.current_run_id, None);
    assert_eq!(settled.inputs.current_run_id, None);
    assert!(settled.inputs.queue.is_empty());
    assert!(settled.inputs.steer_queue.is_empty());
    assert_eq!(settled.completion_waiters.input_count, 0);
    assert_eq!(settled.completion_waiters.waiter_count, 0);
    assert!(settled.completion_waiters.waiting_inputs.is_empty());
    assert_eq!(settled.ops.wait_request_id, None);
    assert!(!settled.ops.pending_wait_present);
    assert_eq!(settled.ops.pending_wait_request_id, None);
    assert!(settled.ops.wait_operation_ids.is_empty());
}

#[tokio::test]
async fn interrupt_current_run_returns_not_ready_without_attached_loop() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;

    let err = adapter
        .interrupt_current_run(&session_id)
        .await
        .expect_err("interrupt should reject when no attached loop exists");
    match err {
        RuntimeDriverError::NotReady { state } => {
            assert_eq!(state, RuntimeState::Idle);
        }
        other => panic!("expected NotReady(Idle), got {other:?}"),
    }
}

#[tokio::test]
async fn cancel_after_boundary_returns_not_ready_without_attached_loop() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;

    let err = adapter
        .cancel_after_boundary(&session_id)
        .await
        .expect_err("boundary cancel should reject when no attached loop exists");
    match err {
        RuntimeDriverError::NotReady { state } => {
            assert_eq!(state, RuntimeState::Idle);
        }
        other => panic!("expected NotReady(Idle), got {other:?}"),
    }
}

#[tokio::test]
async fn interrupt_current_run_on_attached_runtime_is_deferred_until_apply_finishes() {
    struct BlockingExecutor {
        apply_calls: Arc<AtomicUsize>,
        cancel_calls: Arc<AtomicUsize>,
        apply_started: Arc<Notify>,
        apply_finished: Arc<Notify>,
        allow_finish: Arc<Notify>,
    }

    #[async_trait::async_trait]
    impl CoreExecutor for BlockingExecutor {
        async fn apply(
            &mut self,
            run_id: RunId,
            primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            self.apply_calls.fetch_add(1, Ordering::SeqCst);
            self.apply_started.notify_waiters();
            self.allow_finish.notified().await;
            self.apply_finished.notify_waiters();

            Ok(CoreApplyOutput {
                receipt: RunBoundaryReceipt {
                    run_id,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                    sequence: 0,
                },
                session_snapshot: None,
                terminal: None,
            })
        }

        async fn control(&mut self, command: RunControlCommand) -> Result<(), CoreExecutorError> {
            if matches!(command, RunControlCommand::CancelCurrentRun { .. }) {
                self.cancel_calls.fetch_add(1, Ordering::SeqCst);
            }
            Ok(())
        }
    }

    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    let apply_calls = Arc::new(AtomicUsize::new(0));
    let cancel_calls = Arc::new(AtomicUsize::new(0));
    let apply_started = Arc::new(Notify::new());
    let apply_finished = Arc::new(Notify::new());
    let allow_finish = Arc::new(Notify::new());

    adapter
        .register_session_with_executor(
            session_id.clone(),
            Box::new(BlockingExecutor {
                apply_calls: Arc::clone(&apply_calls),
                cancel_calls: Arc::clone(&cancel_calls),
                apply_started: Arc::clone(&apply_started),
                apply_finished: Arc::clone(&apply_finished),
                allow_finish: Arc::clone(&allow_finish),
            }),
        )
        .await;

    let input = Input::Prompt(crate::input::PromptInput::new(
        "attached steered deferred interrupt",
        Some(
            meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                handling_mode: Some(meerkat_core::types::HandlingMode::Steer),
                ..Default::default()
            },
        ),
    ));
    let input_id = input.id().clone();
    let (outcome, completion_handle) = adapter
        .accept_input_with_completion(&session_id, input)
        .await
        .expect("attached steered prompt should be accepted");
    assert!(outcome.is_accepted());
    let completion_handle =
        completion_handle.expect("attached steered prompt should expose a completion waiter");

    tokio::time::timeout(Duration::from_secs(1), apply_started.notified())
        .await
        .expect("attached steered prompt should request immediate processing");

    let during_apply = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist while apply is blocked");
    assert_eq!(during_apply.control.phase, RuntimeState::Running);
    assert_eq!(
        during_apply.control.current_run_id,
        during_apply.inputs.current_run_id
    );
    assert!(during_apply.control.current_run_id.is_some());
    assert!(during_apply.inputs.queue.is_empty());
    assert!(during_apply.inputs.steer_queue.is_empty());
    assert_eq!(during_apply.completion_waiters.input_count, 1);
    assert_eq!(during_apply.completion_waiters.waiter_count, 1);
    assert_eq!(during_apply.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        during_apply.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert_eq!(
        apply_calls.load(Ordering::SeqCst),
        1,
        "attached steered prompt should start the run exactly once"
    );
    assert_eq!(
        cancel_calls.load(Ordering::SeqCst),
        0,
        "no interrupt should reach the executor before it is requested"
    );

    adapter
        .interrupt_current_run(&session_id)
        .await
        .expect("interrupt should enqueue against the attached loop");

    let after_interrupt = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after interrupt is requested");
    assert_eq!(
        after_interrupt.control.phase,
        RuntimeState::Running,
        "interrupt should stay deferred while the attached executor is still inside apply()"
    );
    assert_eq!(
        after_interrupt.control.current_run_id,
        after_interrupt.inputs.current_run_id
    );
    assert!(after_interrupt.control.current_run_id.is_some());
    assert!(after_interrupt.inputs.queue.is_empty());
    assert!(after_interrupt.inputs.steer_queue.is_empty());
    assert_eq!(after_interrupt.completion_waiters.input_count, 1);
    assert_eq!(after_interrupt.completion_waiters.waiter_count, 1);
    assert_eq!(after_interrupt.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        after_interrupt.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert_eq!(
        cancel_calls.load(Ordering::SeqCst),
        0,
        "cancel should remain queued until apply returns"
    );

    allow_finish.notify_waiters();
    tokio::time::timeout(Duration::from_secs(1), apply_finished.notified())
        .await
        .expect("apply should finish after the executor is released");

    match completion_handle.wait().await {
        CompletionOutcome::CompletedWithoutResult => {}
        other => panic!(
            "expected attached queued prompt to complete normally before queued cancel drains, got {other:?}"
        ),
    }

    let settled = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let snapshot = adapter
                .meerkat_machine_spine_snapshot(&session_id)
                .await
                .expect("snapshot should exist after apply returns");
            if snapshot.control.phase == RuntimeState::Attached
                && snapshot.control.current_run_id.is_none()
                && snapshot.inputs.current_run_id.is_none()
                && snapshot.completion_waiters.waiter_count == 0
                && cancel_calls.load(Ordering::SeqCst) == 1
            {
                break snapshot;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("attached runtime should eventually return to Attached after queued cancel drains");
    assert!(settled.inputs.queue.is_empty());
    assert!(settled.inputs.steer_queue.is_empty());
    assert_eq!(settled.completion_waiters.input_count, 0);
    assert_eq!(settled.completion_waiters.waiter_count, 0);
    assert!(settled.completion_waiters.waiting_inputs.is_empty());
    assert_eq!(
        apply_calls.load(Ordering::SeqCst),
        1,
        "queued cancel should not replay the already-running attached steered turn"
    );
    assert_eq!(
        cancel_calls.load(Ordering::SeqCst),
        1,
        "queued cancel should reach the executor exactly once after apply finishes"
    );
}

#[tokio::test]
async fn cancel_after_boundary_on_attached_runtime_is_deferred_until_apply_finishes() {
    struct BlockingExecutor {
        apply_calls: Arc<AtomicUsize>,
        boundary_cancel_calls: Arc<AtomicUsize>,
        apply_started: Arc<Notify>,
        apply_finished: Arc<Notify>,
        allow_finish: Arc<Notify>,
    }

    #[async_trait::async_trait]
    impl CoreExecutor for BlockingExecutor {
        async fn apply(
            &mut self,
            run_id: RunId,
            primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            self.apply_calls.fetch_add(1, Ordering::SeqCst);
            self.apply_started.notify_waiters();
            self.allow_finish.notified().await;
            self.apply_finished.notify_waiters();

            Ok(CoreApplyOutput {
                receipt: RunBoundaryReceipt {
                    run_id,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                    sequence: 0,
                },
                session_snapshot: None,
                terminal: None,
            })
        }

        async fn control(&mut self, command: RunControlCommand) -> Result<(), CoreExecutorError> {
            if matches!(command, RunControlCommand::CancelAfterBoundary { .. }) {
                self.boundary_cancel_calls.fetch_add(1, Ordering::SeqCst);
            }
            Ok(())
        }
    }

    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    let apply_calls = Arc::new(AtomicUsize::new(0));
    let boundary_cancel_calls = Arc::new(AtomicUsize::new(0));
    let apply_started = Arc::new(Notify::new());
    let apply_finished = Arc::new(Notify::new());
    let allow_finish = Arc::new(Notify::new());

    adapter
        .register_session_with_executor(
            session_id.clone(),
            Box::new(BlockingExecutor {
                apply_calls: Arc::clone(&apply_calls),
                boundary_cancel_calls: Arc::clone(&boundary_cancel_calls),
                apply_started: Arc::clone(&apply_started),
                apply_finished: Arc::clone(&apply_finished),
                allow_finish: Arc::clone(&allow_finish),
            }),
        )
        .await;

    let input = Input::Prompt(crate::input::PromptInput::new(
        "attached steered deferred boundary cancel",
        Some(
            meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                handling_mode: Some(meerkat_core::types::HandlingMode::Steer),
                ..Default::default()
            },
        ),
    ));
    let (outcome, completion_handle) = adapter
        .accept_input_with_completion(&session_id, input)
        .await
        .expect("attached steered prompt should be accepted");
    assert!(outcome.is_accepted());
    let completion_handle =
        completion_handle.expect("attached steered prompt should expose a completion waiter");

    tokio::time::timeout(Duration::from_secs(1), apply_started.notified())
        .await
        .expect("attached steered prompt should request immediate processing");

    adapter
        .cancel_after_boundary(&session_id)
        .await
        .expect("boundary cancel should enqueue against the attached loop");

    let after_request = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after boundary cancel is requested");
    assert_eq!(after_request.control.phase, RuntimeState::Running);
    assert!(after_request.control.current_run_id.is_some());
    assert_eq!(
        boundary_cancel_calls.load(Ordering::SeqCst),
        0,
        "boundary cancel should remain queued until apply returns"
    );

    allow_finish.notify_waiters();
    tokio::time::timeout(Duration::from_secs(1), apply_finished.notified())
        .await
        .expect("apply should finish after the executor is released");

    match completion_handle.wait().await {
        CompletionOutcome::CompletedWithoutResult => {}
        other => panic!(
            "expected attached queued prompt to complete normally before queued boundary cancel drains, got {other:?}"
        ),
    }

    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let snapshot = adapter
                .meerkat_machine_spine_snapshot(&session_id)
                .await
                .expect("snapshot should exist after apply returns");
            if snapshot.control.phase == RuntimeState::Attached
                && snapshot.control.current_run_id.is_none()
                && snapshot.inputs.current_run_id.is_none()
                && boundary_cancel_calls.load(Ordering::SeqCst) == 1
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("attached runtime should eventually drain the queued boundary cancel");
    assert_eq!(
        apply_calls.load(Ordering::SeqCst),
        1,
        "queued boundary cancel should not replay the already-running attached steered turn"
    );
}

#[tokio::test]
async fn running_peer_message_interrupt_yielding_drains_before_next_apply() {
    struct BlockingThenImmediateExecutor {
        apply_calls: Arc<AtomicUsize>,
        interrupt_calls: Arc<AtomicUsize>,
        events: Arc<std::sync::Mutex<Vec<&'static str>>>,
        first_apply_started: Arc<Notify>,
        allow_first_finish: Arc<Notify>,
    }

    #[async_trait::async_trait]
    impl CoreExecutor for BlockingThenImmediateExecutor {
        async fn apply(
            &mut self,
            run_id: RunId,
            primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            let apply_index = self.apply_calls.fetch_add(1, Ordering::SeqCst);
            if apply_index == 0 {
                self.events
                    .lock()
                    .expect("events mutex poisoned")
                    .push("apply1_start");
                self.first_apply_started.notify_waiters();
                self.allow_first_finish.notified().await;
                self.events
                    .lock()
                    .expect("events mutex poisoned")
                    .push("apply1_finish");
            } else {
                self.events
                    .lock()
                    .expect("events mutex poisoned")
                    .push("apply2_start");
            }

            Ok(CoreApplyOutput {
                receipt: RunBoundaryReceipt {
                    run_id,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                    sequence: 0,
                },
                session_snapshot: None,
                terminal: None,
            })
        }

        async fn control(&mut self, command: RunControlCommand) -> Result<(), CoreExecutorError> {
            if matches!(command, RunControlCommand::InterruptYielding) {
                self.interrupt_calls.fetch_add(1, Ordering::SeqCst);
                self.events
                    .lock()
                    .expect("events mutex poisoned")
                    .push("interrupt_yielding");
            }
            Ok(())
        }
    }

    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    let apply_calls = Arc::new(AtomicUsize::new(0));
    let interrupt_calls = Arc::new(AtomicUsize::new(0));
    let events = Arc::new(std::sync::Mutex::new(Vec::new()));
    let first_apply_started = Arc::new(Notify::new());
    let allow_first_finish = Arc::new(Notify::new());

    adapter
        .register_session_with_executor(
            session_id.clone(),
            Box::new(BlockingThenImmediateExecutor {
                apply_calls: Arc::clone(&apply_calls),
                interrupt_calls: Arc::clone(&interrupt_calls),
                events: Arc::clone(&events),
                first_apply_started: Arc::clone(&first_apply_started),
                allow_first_finish: Arc::clone(&allow_first_finish),
            }),
        )
        .await;

    let first_input = Input::Prompt(crate::input::PromptInput::new(
        "attached running peer interrupt",
        Some(
            meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                handling_mode: Some(meerkat_core::types::HandlingMode::Steer),
                ..Default::default()
            },
        ),
    ));
    let first_input_id = first_input.id().clone();
    let (outcome, completion_handle) = adapter
        .accept_input_with_completion(&session_id, first_input)
        .await
        .expect("attached steered prompt should be accepted");
    assert!(outcome.is_accepted());
    let completion_handle =
        completion_handle.expect("attached steered prompt should expose a completion waiter");

    tokio::time::timeout(Duration::from_secs(1), first_apply_started.notified())
        .await
        .expect("attached steered prompt should request immediate processing");

    interrupt_calls.store(0, Ordering::SeqCst);
    events.lock().expect("events mutex poisoned").clear();

    let peer_input = Input::Peer(crate::input::PeerInput {
        header: crate::input::InputHeader {
            id: InputId::new(),
            timestamp: Utc::now(),
            source: crate::input::InputOrigin::Peer {
                peer_id: "peer-interrupt".into(),
                display_identity: None,
                runtime_id: None,
            },
            durability: crate::input::InputDurability::Durable,
            visibility: crate::input::InputVisibility::default(),
            idempotency_key: None,
            supersession_key: None,
            correlation_id: None,
        },
        convention: Some(crate::input::PeerConvention::Message),
        body: "interrupt while running".into(),
        payload: None,
        blocks: None,
        handling_mode: None,
    });
    let peer_input_id = peer_input.id().clone();
    let peer_outcome = adapter
        .accept_input(&session_id, peer_input)
        .await
        .expect("running peer message should be accepted");
    assert!(peer_outcome.is_accepted());

    let after_peer_accept = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist while the first apply is blocked");
    assert_eq!(after_peer_accept.control.phase, RuntimeState::Running);
    assert!(after_peer_accept.control.current_run_id.is_some());
    assert_eq!(
        after_peer_accept.control.current_run_id,
        after_peer_accept.inputs.current_run_id
    );
    assert_eq!(after_peer_accept.inputs.queue.len(), 1);
    assert_eq!(after_peer_accept.inputs.queue[0], peer_input_id);
    assert!(after_peer_accept.inputs.steer_queue.is_empty());
    assert_eq!(after_peer_accept.completion_waiters.input_count, 1);
    assert_eq!(after_peer_accept.completion_waiters.waiter_count, 1);
    assert_eq!(
        after_peer_accept.completion_waiters.waiting_inputs[0].input_id,
        first_input_id
    );
    assert_eq!(
        interrupt_calls.load(Ordering::SeqCst),
        0,
        "interrupt-yielding should remain queued until the running apply returns"
    );
    assert_eq!(
        apply_calls.load(Ordering::SeqCst),
        1,
        "the running turn should still be on its first apply"
    );
    {
        let event_log = events.lock().expect("events mutex poisoned");
        let first_apply_finish_index = event_log.iter().position(|event| *event == "apply1_finish");
        assert!(
            first_apply_finish_index.is_none(),
            "the first apply should still be blocked while interrupt-yielding is queued"
        );
        assert!(
            event_log.is_empty(),
            "no queued control or replay should be observed before the running apply returns: {event_log:?}"
        );
    }

    allow_first_finish.notify_waiters();

    let settled = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let snapshot = adapter
                .meerkat_machine_spine_snapshot(&session_id)
                .await
                .expect("snapshot should exist until runtime settles");
            if snapshot.control.phase == RuntimeState::Attached
                && snapshot.control.current_run_id.is_none()
                && snapshot.inputs.current_run_id.is_none()
                && snapshot.inputs.queue.is_empty()
                && snapshot.inputs.steer_queue.is_empty()
                && snapshot.completion_waiters.waiter_count == 0
                && interrupt_calls.load(Ordering::SeqCst) == 1
                && apply_calls.load(Ordering::SeqCst) == 2
            {
                break snapshot;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    let settled = match settled {
        Ok(snapshot) => snapshot,
        Err(_) => {
            let snapshot = adapter
                .meerkat_machine_spine_snapshot(&session_id)
                .await
                .expect("snapshot should still exist after timeout");
            let event_log = events.lock().expect("events mutex poisoned").clone();
            panic!(
                "attached runtime did not settle after interrupt-yielding as expected: phase={:?} control_run={:?} ingress_run={:?} queue={:?} steer_queue={:?} waiters={} interrupt_calls={} apply_calls={} events={:?}",
                snapshot.control.phase,
                snapshot.control.current_run_id,
                snapshot.inputs.current_run_id,
                snapshot.inputs.queue,
                snapshot.inputs.steer_queue,
                snapshot.completion_waiters.waiter_count,
                interrupt_calls.load(Ordering::SeqCst),
                apply_calls.load(Ordering::SeqCst),
                event_log,
            );
        }
    };
    assert_eq!(settled.control.phase, RuntimeState::Attached);
    assert!(settled.inputs.queue.is_empty());
    assert!(settled.inputs.steer_queue.is_empty());
    assert_eq!(settled.completion_waiters.input_count, 0);
    assert_eq!(settled.completion_waiters.waiter_count, 0);

    let (interrupt_index, second_apply_index) = {
        let event_log = events.lock().expect("events mutex poisoned");
        let interrupt_index = event_log
            .iter()
            .position(|event| *event == "interrupt_yielding")
            .expect("interrupt control should be delivered");
        let second_apply_index = event_log
            .iter()
            .position(|event| *event == "apply2_start")
            .expect("queued peer input should eventually start a second apply");
        (interrupt_index, second_apply_index)
    };
    assert!(
        interrupt_index < second_apply_index,
        "interrupt-yielding control must drain before the next queued input starts"
    );

    match completion_handle.wait().await {
        CompletionOutcome::CompletedWithoutResult => {}
        other => panic!(
            "expected first attached steered prompt to complete before queued peer input runs, got {other:?}"
        ),
    }
}

#[tokio::test]
async fn service_accept_input_interrupt_yielding_uses_live_control_handle() {
    struct LiveControlHandle {
        calls: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl meerkat_core::lifecycle::CoreExecutorControl for LiveControlHandle {
        async fn control(&self, command: RunControlCommand) -> Result<(), CoreExecutorError> {
            if matches!(command, RunControlCommand::InterruptYielding) {
                self.calls.fetch_add(1, Ordering::SeqCst);
            }
            Ok(())
        }
    }

    struct BlockingExecutor {
        apply_calls: Arc<AtomicUsize>,
        queued_control_calls: Arc<AtomicUsize>,
        live_control_calls: Arc<AtomicUsize>,
        apply_started: Arc<Notify>,
        allow_finish: Arc<Notify>,
    }

    #[async_trait::async_trait]
    impl CoreExecutor for BlockingExecutor {
        fn control_handle(&self) -> Option<Arc<dyn meerkat_core::lifecycle::CoreExecutorControl>> {
            Some(Arc::new(LiveControlHandle {
                calls: Arc::clone(&self.live_control_calls),
            }))
        }

        async fn apply(
            &mut self,
            run_id: RunId,
            primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            self.apply_calls.fetch_add(1, Ordering::SeqCst);
            self.apply_started.notify_waiters();
            self.allow_finish.notified().await;
            Ok(CoreApplyOutput {
                receipt: RunBoundaryReceipt {
                    run_id,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                    sequence: 0,
                },
                session_snapshot: None,
                terminal: None,
            })
        }

        async fn control(&mut self, command: RunControlCommand) -> Result<(), CoreExecutorError> {
            if matches!(command, RunControlCommand::InterruptYielding) {
                self.queued_control_calls.fetch_add(1, Ordering::SeqCst);
            }
            Ok(())
        }
    }

    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    let apply_calls = Arc::new(AtomicUsize::new(0));
    let queued_control_calls = Arc::new(AtomicUsize::new(0));
    let live_control_calls = Arc::new(AtomicUsize::new(0));
    let apply_started = Arc::new(Notify::new());
    let allow_finish = Arc::new(Notify::new());

    adapter
        .register_session_with_executor(
            session_id.clone(),
            Box::new(BlockingExecutor {
                apply_calls: Arc::clone(&apply_calls),
                queued_control_calls: Arc::clone(&queued_control_calls),
                live_control_calls: Arc::clone(&live_control_calls),
                apply_started: Arc::clone(&apply_started),
                allow_finish: Arc::clone(&allow_finish),
            }),
        )
        .await;

    let first_input = Input::Prompt(crate::input::PromptInput::new(
        "attached service ext running turn",
        Some(
            meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                handling_mode: Some(meerkat_core::types::HandlingMode::Steer),
                ..Default::default()
            },
        ),
    ));
    let (outcome, _completion_handle) = adapter
        .accept_input_with_completion(&session_id, first_input)
        .await
        .expect("attached steered prompt should be accepted");
    assert!(outcome.is_accepted());

    tokio::time::timeout(Duration::from_secs(1), apply_started.notified())
        .await
        .expect("first apply should start");

    live_control_calls.store(0, Ordering::SeqCst);
    queued_control_calls.store(0, Ordering::SeqCst);

    let peer_input = Input::Peer(crate::input::PeerInput {
        header: crate::input::InputHeader {
            id: InputId::new(),
            timestamp: Utc::now(),
            source: crate::input::InputOrigin::Peer {
                peer_id: "peer-service-ext-interrupt".into(),
                display_identity: None,
                runtime_id: None,
            },
            durability: crate::input::InputDurability::Durable,
            visibility: crate::input::InputVisibility::default(),
            idempotency_key: None,
            supersession_key: None,
            correlation_id: None,
        },
        convention: Some(crate::input::PeerConvention::Message),
        body: "interrupt through service ext ingest".into(),
        payload: None,
        blocks: None,
        handling_mode: None,
    });

    let peer_outcome = <MeerkatMachine as SessionServiceRuntimeExt>::accept_input(
        adapter.as_ref(),
        &session_id,
        peer_input,
    )
    .await
    .expect("service ext accept_input should accept running peer input");
    assert!(peer_outcome.is_accepted());

    assert_eq!(
        live_control_calls.load(Ordering::SeqCst),
        1,
        "service ext Ingest should signal the live out-of-band control handle while apply is blocked"
    );
    assert_eq!(
        queued_control_calls.load(Ordering::SeqCst),
        0,
        "ordered runtime-loop control still cannot drain until apply returns"
    );
    assert_eq!(apply_calls.load(Ordering::SeqCst), 1);

    allow_finish.notify_waiters();

    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if queued_control_calls.load(Ordering::SeqCst) == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("queued control should still drain after apply returns");
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_attached_steered_prompt_defers_stop_until_apply_finishes() {
    struct BlockingExecutor {
        apply_calls: Arc<AtomicUsize>,
        control_calls: Arc<AtomicUsize>,
        stop_calls: Arc<AtomicUsize>,
        apply_started: Arc<Notify>,
        apply_finished: Arc<Notify>,
        allow_finish: Arc<Notify>,
    }

    #[async_trait::async_trait]
    impl CoreExecutor for BlockingExecutor {
        async fn apply(
            &mut self,
            run_id: RunId,
            primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            self.apply_calls.fetch_add(1, Ordering::SeqCst);
            self.apply_started.notify_waiters();
            self.allow_finish.notified().await;
            self.apply_finished.notify_waiters();

            Ok(CoreApplyOutput {
                receipt: RunBoundaryReceipt {
                    run_id,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                    sequence: 0,
                },
                session_snapshot: None,
                terminal: None,
            })
        }

        async fn control(&mut self, command: RunControlCommand) -> Result<(), CoreExecutorError> {
            self.control_calls.fetch_add(1, Ordering::SeqCst);
            if matches!(command, RunControlCommand::StopRuntimeExecutor { .. }) {
                self.stop_calls.fetch_add(1, Ordering::SeqCst);
            }
            Ok(())
        }
    }

    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    let apply_calls = Arc::new(AtomicUsize::new(0));
    let control_calls = Arc::new(AtomicUsize::new(0));
    let stop_calls = Arc::new(AtomicUsize::new(0));
    let apply_started = Arc::new(Notify::new());
    let apply_finished = Arc::new(Notify::new());
    let allow_finish = Arc::new(Notify::new());

    adapter
        .register_session_with_executor(
            session_id.clone(),
            Box::new(BlockingExecutor {
                apply_calls: Arc::clone(&apply_calls),
                control_calls: Arc::clone(&control_calls),
                stop_calls: Arc::clone(&stop_calls),
                apply_started: Arc::clone(&apply_started),
                apply_finished: Arc::clone(&apply_finished),
                allow_finish: Arc::clone(&allow_finish),
            }),
        )
        .await;

    let registry = adapter
        .ops_lifecycle_registry(&session_id)
        .await
        .expect("ops registry should exist for attached session");

    let operation_id = OperationId::new();
    registry
        .register_operation(OperationSpec {
            id: operation_id.clone(),
            kind: OperationKind::MobMemberChild,
            owner_session_id: session_id.clone(),
            display_name: "attached steered deferred stop wait target".into(),
            source_label: "meerkat_machine_test".into(),
            child_session_id: None,
            expect_peer_channel: true,
        })
        .expect("operation should register");
    registry
        .provisioning_succeeded(&operation_id)
        .expect("operation should enter running");

    let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

    let input = Input::Prompt(crate::input::PromptInput::new(
        "attached steered deferred stop",
        Some(
            meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                handling_mode: Some(meerkat_core::types::HandlingMode::Steer),
                ..Default::default()
            },
        ),
    ));
    let input_id = input.id().clone();
    let (outcome, completion_handle) = adapter
        .accept_input_with_completion(&session_id, input)
        .await
        .expect("attached steered prompt should be accepted");
    assert!(outcome.is_accepted());
    let completion_handle =
        completion_handle.expect("attached steered prompt should expose a completion waiter");

    tokio::time::timeout(Duration::from_secs(1), apply_started.notified())
        .await
        .expect("attached steered prompt should request immediate processing");

    let during_apply = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist while attached steered work is active");
    let wait_request_id = during_apply
        .ops
        .wait_request_id
        .clone()
        .expect("wait_all should register an authority-owned wait request");
    assert_eq!(during_apply.control.phase, RuntimeState::Running);
    assert_eq!(
        during_apply.control.current_run_id,
        during_apply.inputs.current_run_id
    );
    assert!(during_apply.control.current_run_id.is_some());
    assert!(during_apply.inputs.queue.is_empty());
    assert!(during_apply.inputs.steer_queue.is_empty());
    assert_eq!(during_apply.completion_waiters.input_count, 1);
    assert_eq!(during_apply.completion_waiters.waiter_count, 1);
    assert_eq!(during_apply.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        during_apply.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert_eq!(
        during_apply.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "pending wait carrier should track the same wait request id while attached steered work is active"
    );
    assert_eq!(
        during_apply.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "wait_all should track the live operation while attached steered work is active"
    );
    assert_eq!(
        apply_calls.load(Ordering::SeqCst),
        1,
        "attached steered work should wake the loop exactly once"
    );
    assert_eq!(
        control_calls.load(Ordering::SeqCst),
        1,
        "attached steered admission should still route one control command through the executor seam"
    );
    assert_eq!(
        stop_calls.load(Ordering::SeqCst),
        0,
        "no explicit stop command should have reached the executor before it is requested"
    );

    adapter
        .stop_runtime_executor(
            &session_id,
            RunControlCommand::StopRuntimeExecutor {
                reason: "attached steered deferred stop".into(),
            },
        )
        .await
        .expect("stop should queue against the attached loop");

    let after_stop_request = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after stop is requested");
    assert_eq!(
        after_stop_request.control.phase,
        RuntimeState::Running,
        "stop should stay deferred while the attached executor is still inside apply()"
    );
    assert_eq!(
        after_stop_request.control.current_run_id,
        after_stop_request.inputs.current_run_id
    );
    assert!(after_stop_request.control.current_run_id.is_some());
    assert!(after_stop_request.inputs.queue.is_empty());
    assert!(after_stop_request.inputs.steer_queue.is_empty());
    assert_eq!(after_stop_request.completion_waiters.input_count, 1);
    assert_eq!(after_stop_request.completion_waiters.waiter_count, 1);
    assert_eq!(
        after_stop_request.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert_eq!(
        after_stop_request.ops.wait_request_id,
        Some(wait_request_id.clone()),
        "stop should not clear the authority-owned wait request while apply is still blocked"
    );
    assert!(after_stop_request.ops.pending_wait_present);
    assert_eq!(
        after_stop_request.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "stop should preserve request-id agreement while it remains queued behind apply()"
    );
    assert_eq!(
        after_stop_request.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "stop should preserve the tracked wait target while apply is still blocked"
    );
    assert_eq!(
        control_calls.load(Ordering::SeqCst),
        1,
        "the explicit stop command should still be queued instead of reaching the executor mid-apply"
    );
    assert_eq!(
        stop_calls.load(Ordering::SeqCst),
        0,
        "the explicit stop command should not be delivered until the loop drains controls after apply()"
    );

    registry
        .complete_operation(
            &operation_id,
            OperationResult {
                id: operation_id.clone(),
                content: "done".into(),
                is_error: false,
                duration_ms: 1,
                tokens_used: 0,
            },
        )
        .expect("operation should complete while stop is still deferred behind apply");
    let wait_result = wait_future
        .await
        .expect("wait_all should still resolve first");
    assert_eq!(wait_result.satisfied.wait_request_id, wait_request_id);
    assert_eq!(
        wait_result.satisfied.operation_ids,
        vec![operation_id.clone()]
    );

    let after_wait_all = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let snapshot = adapter
                .meerkat_machine_spine_snapshot(&session_id)
                .await
                .expect("snapshot should exist after wait_all settles");
            if snapshot.ops.wait_request_id.is_none() && !snapshot.ops.pending_wait_present {
                break snapshot;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("attached steered snapshot should eventually clear the wait_all carrier");
    assert_eq!(
        after_wait_all.control.phase,
        RuntimeState::Running,
        "stop should still be deferred while the attached executor remains inside apply()"
    );
    assert_eq!(
        after_wait_all.control.current_run_id,
        after_wait_all.inputs.current_run_id
    );
    assert!(after_wait_all.control.current_run_id.is_some());
    assert!(after_wait_all.inputs.queue.is_empty());
    assert!(after_wait_all.inputs.steer_queue.is_empty());
    assert_eq!(after_wait_all.completion_waiters.input_count, 1);
    assert_eq!(after_wait_all.completion_waiters.waiter_count, 1);
    assert_eq!(
        after_wait_all.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert_eq!(after_wait_all.ops.wait_request_id, None);
    assert!(!after_wait_all.ops.pending_wait_present);
    assert_eq!(after_wait_all.ops.pending_wait_request_id, None);
    assert!(after_wait_all.ops.wait_operation_ids.is_empty());
    assert_eq!(
        stop_calls.load(Ordering::SeqCst),
        0,
        "the queued stop command should still not reach the executor before apply() completes"
    );

    allow_finish.notify_waiters();
    tokio::time::timeout(Duration::from_secs(1), apply_finished.notified())
        .await
        .expect("attached steered prompt should finish after the executor is released");

    match completion_handle.wait().await {
        CompletionOutcome::CompletedWithoutResult => {}
        other => panic!(
            "expected attached steered completion to finish normally before queued stop drains, got {other:?}"
        ),
    }

    let settled = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let snapshot = adapter
                .meerkat_machine_spine_snapshot(&session_id)
                .await
                .expect("snapshot should exist after queued stop drains");
            if snapshot.control.phase == RuntimeState::Stopped {
                break snapshot;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("attached runtime should eventually publish Stopped after apply returns");
    assert_eq!(settled.control.current_run_id, None);
    assert_eq!(settled.inputs.current_run_id, None);
    assert!(settled.inputs.queue.is_empty());
    assert!(settled.inputs.steer_queue.is_empty());
    assert_eq!(settled.completion_waiters.input_count, 0);
    assert_eq!(settled.completion_waiters.waiter_count, 0);
    assert!(settled.completion_waiters.waiting_inputs.is_empty());
    assert_eq!(settled.ops.wait_request_id, None);
    assert!(!settled.ops.pending_wait_present);
    assert_eq!(settled.ops.pending_wait_request_id, None);
    assert!(settled.ops.wait_operation_ids.is_empty());
    assert_eq!(
        apply_calls.load(Ordering::SeqCst),
        1,
        "deferred stop should not replay the attached steered input"
    );
    assert_eq!(
        control_calls.load(Ordering::SeqCst),
        2,
        "one admission control plus one deferred stop command should reach the attached executor"
    );
    assert_eq!(
        stop_calls.load(Ordering::SeqCst),
        1,
        "the queued stop command should reach the executor exactly once after apply() finishes"
    );
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_clears_completion_waiters_after_reset_with_runtime_loop() {
    struct RecordingExecutor {
        apply_calls: Arc<AtomicUsize>,
        control_calls: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl CoreExecutor for RecordingExecutor {
        async fn apply(
            &mut self,
            run_id: RunId,
            primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            self.apply_calls.fetch_add(1, Ordering::SeqCst);
            Ok(CoreApplyOutput {
                receipt: RunBoundaryReceipt {
                    run_id,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                    sequence: 0,
                },
                session_snapshot: None,
                terminal: None,
            })
        }

        async fn control(&mut self, _command: RunControlCommand) -> Result<(), CoreExecutorError> {
            self.control_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    let apply_calls = Arc::new(AtomicUsize::new(0));
    let control_calls = Arc::new(AtomicUsize::new(0));

    adapter
        .register_session_with_executor(
            session_id.clone(),
            Box::new(RecordingExecutor {
                apply_calls: Arc::clone(&apply_calls),
                control_calls: Arc::clone(&control_calls),
            }),
        )
        .await;

    let input = make_progress_input("reset-with-loop");
    let input_id = input.id().clone();
    let outcome = adapter
        .accept_input_without_wake(&session_id, input)
        .await
        .expect("progress input should queue without waking the attached loop");
    assert!(outcome.is_accepted());

    let handle = {
        let completions = {
            let sessions = adapter.sessions.read().await;
            sessions
                .get(&session_id)
                .expect("attached session should exist")
                .completions
                .clone()
        };
        let mut completions = completions.lock().await;
        completions.register(input_id.clone())
    };

    let before_reset = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist before reset");
    assert_eq!(before_reset.control.phase, RuntimeState::Attached);
    assert_eq!(before_reset.completion_waiters.input_count, 1);
    assert_eq!(before_reset.completion_waiters.waiter_count, 1);
    assert_eq!(before_reset.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        before_reset.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert_eq!(
        apply_calls.load(Ordering::SeqCst),
        0,
        "reset should be able to discard queued attached-loop work before apply runs"
    );
    assert_eq!(
        control_calls.load(Ordering::SeqCst),
        0,
        "reset has not yet attempted any executor control"
    );

    let report = SessionServiceRuntimeExt::reset_runtime(&*adapter, &session_id)
        .await
        .expect("reset should succeed for attached queued runtime");
    assert_eq!(report.inputs_abandoned, 1);

    let after_reset = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after reset");
    assert_eq!(after_reset.control.phase, RuntimeState::Idle);
    assert_eq!(after_reset.completion_waiters.input_count, 0);
    assert_eq!(after_reset.completion_waiters.waiter_count, 0);
    assert!(
        after_reset.completion_waiters.waiting_inputs.is_empty(),
        "reset should clear the completion waiter carrier immediately even when a loop is attached"
    );
    assert_eq!(
        apply_calls.load(Ordering::SeqCst),
        0,
        "reset should bypass queued attached-loop work entirely"
    );
    assert_eq!(
        control_calls.load(Ordering::SeqCst),
        0,
        "reset currently bypasses the executor control seam and does not deliver an out-of-band control command"
    );

    match handle.wait().await {
        CompletionOutcome::RuntimeTerminated(reason) => {
            assert_eq!(reason, "runtime reset");
        }
        other => panic!("expected runtime reset termination, got {other:?}"),
    }
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_clears_completion_waiters_after_stop_runtime_executor() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;

    let input = make_prompt("stop completion waiter");
    let input_id = input.id().clone();
    let (_outcome, handle) = adapter
        .accept_input_with_completion(&session_id, input)
        .await
        .expect("prompt should be accepted");
    let handle = handle.expect("queued prompt should register a completion waiter");

    let before_stop = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist before stop");
    assert_eq!(before_stop.control.phase, RuntimeState::Idle);
    assert_eq!(before_stop.completion_waiters.input_count, 1);
    assert_eq!(before_stop.completion_waiters.waiter_count, 1);
    assert_eq!(before_stop.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        before_stop.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );

    adapter
        .stop_runtime_executor(
            &session_id,
            RunControlCommand::StopRuntimeExecutor {
                reason: "stop test".into(),
            },
        )
        .await
        .expect("stop should terminate active completion waiters");

    let after_stop = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after stop");
    assert_eq!(after_stop.control.phase, RuntimeState::Stopped);
    assert!(
        after_stop.inputs.queue.is_empty(),
        "stop should not leave ordinary queued work behind once the runtime is stopped"
    );
    assert!(
        after_stop.inputs.steer_queue.is_empty(),
        "stop should not leave steer-queued work behind once the runtime is stopped"
    );
    assert_eq!(after_stop.completion_waiters.input_count, 0);
    assert_eq!(after_stop.completion_waiters.waiter_count, 0);
    assert!(
        after_stop.completion_waiters.waiting_inputs.is_empty(),
        "stop should clear the completion waiter carrier immediately"
    );

    match handle.wait().await {
        CompletionOutcome::RuntimeTerminated(reason) => {
            assert_eq!(reason, "runtime stopped");
        }
        other => panic!("expected runtime stopped termination, got {other:?}"),
    }
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_clears_completion_waiters_after_stop_runtime_executor_with_runtime_loop()
 {
    struct RecordingExecutor {
        apply_calls: Arc<AtomicUsize>,
        stop_calls: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl CoreExecutor for RecordingExecutor {
        async fn apply(
            &mut self,
            run_id: RunId,
            primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            self.apply_calls.fetch_add(1, Ordering::SeqCst);
            Ok(CoreApplyOutput {
                receipt: RunBoundaryReceipt {
                    run_id,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                    sequence: 0,
                },
                session_snapshot: None,
                terminal: None,
            })
        }

        async fn control(&mut self, command: RunControlCommand) -> Result<(), CoreExecutorError> {
            if matches!(command, RunControlCommand::StopRuntimeExecutor { .. }) {
                self.stop_calls.fetch_add(1, Ordering::SeqCst);
            }
            Ok(())
        }
    }

    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    let apply_calls = Arc::new(AtomicUsize::new(0));
    let stop_calls = Arc::new(AtomicUsize::new(0));

    adapter
        .register_session_with_executor(
            session_id.clone(),
            Box::new(RecordingExecutor {
                apply_calls: Arc::clone(&apply_calls),
                stop_calls: Arc::clone(&stop_calls),
            }),
        )
        .await;

    let input = make_progress_input("stop-with-loop");
    let input_id = input.id().clone();
    let outcome = adapter
        .accept_input_without_wake(&session_id, input)
        .await
        .expect("progress input should queue without waking the attached loop");
    assert!(outcome.is_accepted());

    let handle = {
        let completions = {
            let sessions = adapter.sessions.read().await;
            sessions
                .get(&session_id)
                .expect("attached session should exist")
                .completions
                .clone()
        };
        let mut completions = completions.lock().await;
        completions.register(input_id.clone())
    };

    let before_stop = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist before stop");
    assert_eq!(before_stop.control.phase, RuntimeState::Attached);
    assert_eq!(before_stop.completion_waiters.input_count, 1);
    assert_eq!(before_stop.completion_waiters.waiter_count, 1);
    assert_eq!(before_stop.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        before_stop.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert_eq!(
        apply_calls.load(Ordering::SeqCst),
        0,
        "stop should be able to preempt queued attached-loop work before apply runs"
    );

    adapter
        .stop_runtime_executor(
            &session_id,
            RunControlCommand::StopRuntimeExecutor {
                reason: "stop attached-loop completion waiter".into(),
            },
        )
        .await
        .expect("stop should terminate queued completion waiters through the live control seam");

    match handle.wait().await {
        CompletionOutcome::RuntimeTerminated(reason) => {
            assert_eq!(reason, "runtime stopped");
        }
        other => panic!("expected runtime stopped termination, got {other:?}"),
    }

    let after_stop = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after stop");
    assert_eq!(after_stop.control.phase, RuntimeState::Stopped);
    assert!(
        after_stop.inputs.queue.is_empty(),
        "stop should not leave ordinary queued work behind even when an attached loop exists"
    );
    assert!(
        after_stop.inputs.steer_queue.is_empty(),
        "stop should not leave steer-queued work behind even when an attached loop exists"
    );
    assert_eq!(after_stop.completion_waiters.input_count, 0);
    assert_eq!(after_stop.completion_waiters.waiter_count, 0);
    assert!(
        after_stop.completion_waiters.waiting_inputs.is_empty(),
        "stop through the live loop should clear the completion waiter carrier"
    );
    assert_eq!(
        apply_calls.load(Ordering::SeqCst),
        0,
        "stop should beat queued ordinary work on an attached runtime loop"
    );
    assert_eq!(
        stop_calls.load(Ordering::SeqCst),
        1,
        "the attached executor should observe exactly one stop-runtime-executor control"
    );
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_clears_completion_waiters_after_retire_without_runtime_loop()
 {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;

    let input = make_prompt("retire completion waiter");
    let input_id = input.id().clone();
    let (_outcome, handle) = adapter
        .accept_input_with_completion(&session_id, input)
        .await
        .expect("prompt should be accepted");
    let handle = handle.expect("queued prompt should register a completion waiter");

    let before_retire = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist before retire");
    assert_eq!(before_retire.control.phase, RuntimeState::Idle);
    assert_eq!(before_retire.completion_waiters.input_count, 1);
    assert_eq!(before_retire.completion_waiters.waiter_count, 1);
    assert_eq!(before_retire.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        before_retire.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );

    let runtime_id = runtime_id_for_session(&session_id);
    let report = crate::traits::RuntimeControlPlane::retire(&*adapter, &runtime_id)
        .await
        .expect("retire should clear queued waiters when no runtime loop can drain");
    assert_eq!(report.inputs_abandoned, 1);
    assert_eq!(report.inputs_pending_drain, 0);

    let after_retire = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after retire");
    assert_eq!(after_retire.control.phase, RuntimeState::Retired);
    assert_eq!(after_retire.completion_waiters.input_count, 0);
    assert_eq!(after_retire.completion_waiters.waiter_count, 0);
    assert!(
        after_retire.completion_waiters.waiting_inputs.is_empty(),
        "retire without a live runtime loop should clear the completion waiter carrier immediately"
    );

    match handle.wait().await {
        CompletionOutcome::RuntimeTerminated(reason) => {
            assert_eq!(reason, "retired without runtime loop");
        }
        other => panic!("expected retired-without-runtime-loop termination, got {other:?}"),
    }
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_preserves_completion_waiters_after_retire_with_runtime_loop()
 {
    struct BlockingExecutor {
        apply_calls: Arc<AtomicUsize>,
        apply_started: Arc<Notify>,
        allow_finish: Arc<Notify>,
    }

    #[async_trait::async_trait]
    impl CoreExecutor for BlockingExecutor {
        async fn apply(
            &mut self,
            run_id: RunId,
            primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            self.apply_calls.fetch_add(1, Ordering::SeqCst);
            self.apply_started.notify_waiters();
            self.allow_finish.notified().await;

            Ok(CoreApplyOutput {
                receipt: RunBoundaryReceipt {
                    run_id,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                    sequence: 0,
                },
                session_snapshot: None,
                terminal: None,
            })
        }

        async fn control(&mut self, _command: RunControlCommand) -> Result<(), CoreExecutorError> {
            Ok(())
        }
    }

    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    let apply_calls = Arc::new(AtomicUsize::new(0));
    let apply_started = Arc::new(Notify::new());
    let allow_finish = Arc::new(Notify::new());

    adapter
        .register_session_with_executor(
            session_id.clone(),
            Box::new(BlockingExecutor {
                apply_calls: Arc::clone(&apply_calls),
                apply_started: Arc::clone(&apply_started),
                allow_finish: Arc::clone(&allow_finish),
            }),
        )
        .await;

    let input = make_progress_input("retire-with-loop");
    let input_id = input.id().clone();
    let (outcome, handle) = adapter
        .accept_input_with_completion(&session_id, input)
        .await
        .expect("progress input should be accepted");
    assert!(outcome.is_accepted());
    let handle = handle.expect("queued progress input should expose a completion waiter");

    let before_retire = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist before retire");
    assert_eq!(before_retire.control.phase, RuntimeState::Attached);
    assert_eq!(before_retire.completion_waiters.input_count, 1);
    assert_eq!(before_retire.completion_waiters.waiter_count, 1);
    assert_eq!(before_retire.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        before_retire.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert_eq!(
        apply_calls.load(Ordering::SeqCst),
        0,
        "queued progress input should remain pending until retire wakes the attached loop"
    );

    let runtime_id = runtime_id_for_session(&session_id);
    let report = crate::traits::RuntimeControlPlane::retire(&*adapter, &runtime_id)
        .await
        .expect("retire should preserve queued work for the live runtime loop to drain");
    assert_eq!(report.inputs_abandoned, 0);
    assert_eq!(report.inputs_pending_drain, 1);

    tokio::time::timeout(Duration::from_secs(1), apply_started.notified())
        .await
        .expect("retire should wake the attached runtime loop to drain queued work");

    let after_retire = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after retire wakes the loop");
    assert_eq!(
        after_retire.control.phase,
        RuntimeState::Retired,
        "post-`e5c5ecaf3` DSL-authoritative: retire holds lifecycle_phase at Retired through the drain window (Retire transition goes to Retired unconditionally); DSL is source of truth, not control_projection cache"
    );
    assert_eq!(after_retire.completion_waiters.input_count, 1);
    assert_eq!(after_retire.completion_waiters.waiter_count, 1);
    assert_eq!(after_retire.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        after_retire.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert_eq!(
        apply_calls.load(Ordering::SeqCst),
        1,
        "retire should wake the attached loop exactly once for the preserved queued work"
    );

    allow_finish.notify_waiters();

    match handle.wait().await {
        CompletionOutcome::CompletedWithoutResult => {}
        other => panic!("expected retire+drain to complete queued work, got {other:?}"),
    }

    let settled = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after drained completion settles");
    assert_eq!(settled.control.phase, RuntimeState::Retired);
    assert_eq!(settled.completion_waiters.input_count, 0);
    assert_eq!(settled.completion_waiters.waiter_count, 0);
    assert!(
        settled.completion_waiters.waiting_inputs.is_empty(),
        "retire+drain should clear the completion waiter carrier once the preserved work completes"
    );
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_preserves_completion_waiters_after_recover_with_runtime_loop()
 {
    struct BlockingExecutor {
        apply_calls: Arc<AtomicUsize>,
        control_calls: Arc<AtomicUsize>,
        apply_started: Arc<Notify>,
        apply_finished: Arc<Notify>,
        allow_finish: Arc<Notify>,
    }

    #[async_trait::async_trait]
    impl CoreExecutor for BlockingExecutor {
        async fn apply(
            &mut self,
            run_id: RunId,
            primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            self.apply_calls.fetch_add(1, Ordering::SeqCst);
            self.apply_started.notify_waiters();
            self.allow_finish.notified().await;
            self.apply_finished.notify_waiters();

            Ok(CoreApplyOutput {
                receipt: RunBoundaryReceipt {
                    run_id,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                    sequence: 0,
                },
                session_snapshot: None,
                terminal: None,
            })
        }

        async fn control(&mut self, _command: RunControlCommand) -> Result<(), CoreExecutorError> {
            self.control_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    let apply_calls = Arc::new(AtomicUsize::new(0));
    let control_calls = Arc::new(AtomicUsize::new(0));
    let apply_started = Arc::new(Notify::new());
    let apply_finished = Arc::new(Notify::new());
    let allow_finish = Arc::new(Notify::new());

    adapter
        .register_session_with_executor(
            session_id.clone(),
            Box::new(BlockingExecutor {
                apply_calls: Arc::clone(&apply_calls),
                control_calls: Arc::clone(&control_calls),
                apply_started: Arc::clone(&apply_started),
                apply_finished: Arc::clone(&apply_finished),
                allow_finish: Arc::clone(&allow_finish),
            }),
        )
        .await;

    let input = make_progress_input("recover-with-loop-completion");
    let input_id = input.id().clone();
    let (outcome, handle) = adapter
        .accept_input_with_completion(&session_id, input)
        .await
        .expect("progress input should be accepted");
    assert!(outcome.is_accepted());
    let handle = handle.expect("queued progress input should expose a completion waiter");

    let before_recover = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist before recover");
    assert_eq!(before_recover.control.phase, RuntimeState::Attached);
    assert_eq!(before_recover.completion_waiters.input_count, 1);
    assert_eq!(before_recover.completion_waiters.waiter_count, 1);
    assert_eq!(before_recover.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        before_recover.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert_eq!(
        before_recover.completion_waiters.waiting_inputs[0].waiter_count,
        1
    );
    assert_eq!(
        apply_calls.load(Ordering::SeqCst),
        0,
        "queued attached-loop work should remain pending until recover wakes the loop"
    );
    assert_eq!(
        control_calls.load(Ordering::SeqCst),
        0,
        "recover should not yet have attempted executor control"
    );

    let runtime_id = runtime_id_for_session(&session_id);
    let report = crate::traits::RuntimeControlPlane::recover(&*adapter, &runtime_id)
        .await
        .expect(
            "recover should preserve queued completion waiters while replaying attached-loop work",
        );
    assert_eq!(report.inputs_recovered, 1);

    tokio::time::timeout(Duration::from_secs(1), apply_started.notified())
        .await
        .expect("recover should wake the attached runtime loop to replay preserved queued work");

    let during_recover = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist while recover replay is in flight");
    assert_eq!(
        during_recover.control.phase,
        RuntimeState::Running,
        "post-#32 W6-J: during recover_with_runtime_loop, the runtime loop fires DSL `Prepare` which transitions lifecycle_phase to Running; the recover transition itself self-loops on Attached but the apply-in-flight binds to a run_id and sits at Running until Commit returns"
    );
    assert_eq!(during_recover.completion_waiters.input_count, 1);
    assert_eq!(during_recover.completion_waiters.waiter_count, 1);
    assert_eq!(during_recover.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        during_recover.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert_eq!(
        during_recover.completion_waiters.waiting_inputs[0].waiter_count,
        1
    );
    assert_eq!(
        apply_calls.load(Ordering::SeqCst),
        1,
        "recover should wake the attached loop exactly once for the recovered queued work"
    );
    assert_eq!(
        control_calls.load(Ordering::SeqCst),
        0,
        "recover should not route any out-of-band control command through the executor seam"
    );

    allow_finish.notify_waiters();
    tokio::time::timeout(Duration::from_secs(1), apply_finished.notified())
        .await
        .expect("attached loop should finish replaying the recovered queued work");

    match handle.wait().await {
        CompletionOutcome::CompletedWithoutResult => {}
        other => panic!("expected recover+replay to complete queued work, got {other:?}"),
    }

    let settled = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let snapshot = adapter
                .meerkat_machine_spine_snapshot(&session_id)
                .await
                .expect("snapshot should exist after attached-loop replay");
            if snapshot.control.phase != RuntimeState::Running {
                break snapshot;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("runtime should eventually leave Running once the recovered work finishes replaying");
    assert_eq!(
        settled.control.phase,
        RuntimeState::Attached,
        "recover should currently return to Attached once the recovered work finishes replaying"
    );
    assert_eq!(settled.completion_waiters.input_count, 0);
    assert_eq!(settled.completion_waiters.waiter_count, 0);
    assert!(
        settled.completion_waiters.waiting_inputs.is_empty(),
        "recover+replay should clear the completion waiter carrier once the preserved work completes"
    );
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_preserves_completion_waiters_after_recycle_with_runtime_loop()
 {
    struct BlockingExecutor {
        apply_calls: Arc<AtomicUsize>,
        control_calls: Arc<AtomicUsize>,
        apply_started: Arc<Notify>,
        apply_finished: Arc<Notify>,
        allow_finish: Arc<Notify>,
    }

    #[async_trait::async_trait]
    impl CoreExecutor for BlockingExecutor {
        async fn apply(
            &mut self,
            run_id: RunId,
            primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            self.apply_calls.fetch_add(1, Ordering::SeqCst);
            self.apply_started.notify_waiters();
            self.allow_finish.notified().await;
            self.apply_finished.notify_waiters();

            Ok(CoreApplyOutput {
                receipt: RunBoundaryReceipt {
                    run_id,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                    sequence: 0,
                },
                session_snapshot: None,
                terminal: None,
            })
        }

        async fn control(&mut self, _command: RunControlCommand) -> Result<(), CoreExecutorError> {
            self.control_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    let apply_calls = Arc::new(AtomicUsize::new(0));
    let control_calls = Arc::new(AtomicUsize::new(0));
    let apply_started = Arc::new(Notify::new());
    let apply_finished = Arc::new(Notify::new());
    let allow_finish = Arc::new(Notify::new());

    adapter
        .register_session_with_executor(
            session_id.clone(),
            Box::new(BlockingExecutor {
                apply_calls: Arc::clone(&apply_calls),
                control_calls: Arc::clone(&control_calls),
                apply_started: Arc::clone(&apply_started),
                apply_finished: Arc::clone(&apply_finished),
                allow_finish: Arc::clone(&allow_finish),
            }),
        )
        .await;

    let input = make_progress_input("recycle-with-loop-completion");
    let input_id = input.id().clone();
    let (outcome, handle) = adapter
        .accept_input_with_completion(&session_id, input)
        .await
        .expect("progress input should be accepted");
    assert!(outcome.is_accepted());
    let handle = handle.expect("queued progress input should expose a completion waiter");

    let before_recycle = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist before recycle");
    assert_eq!(before_recycle.control.phase, RuntimeState::Attached);
    assert_eq!(before_recycle.completion_waiters.input_count, 1);
    assert_eq!(before_recycle.completion_waiters.waiter_count, 1);
    assert_eq!(before_recycle.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        before_recycle.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert_eq!(
        before_recycle.completion_waiters.waiting_inputs[0].waiter_count,
        1
    );
    assert_eq!(
        apply_calls.load(Ordering::SeqCst),
        0,
        "queued attached-loop work should remain pending until recycle wakes the loop"
    );
    assert_eq!(
        control_calls.load(Ordering::SeqCst),
        0,
        "recycle should not yet have attempted executor control"
    );

    let runtime_id = runtime_id_for_session(&session_id);
    let report = crate::traits::RuntimeControlPlane::recycle(&*adapter, &runtime_id)
        .await
        .expect(
            "recycle should preserve queued completion waiters while replaying attached-loop work",
        );
    assert_eq!(report.inputs_transferred, 1);

    tokio::time::timeout(Duration::from_secs(1), apply_started.notified())
        .await
        .expect("recycle should wake the attached runtime loop to replay preserved queued work");

    let during_recycle = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist while recycle replay is in flight");
    assert_eq!(
        during_recycle.control.phase,
        RuntimeState::Running,
        "post-#32 W6-J: recycle from Attached starts the loop which fires DSL `Prepare` → Running; the apply-in-flight sits at Running during replay until Commit returns (DSL is source of truth, now with run-lifecycle properly DSL-wired)"
    );
    assert_eq!(during_recycle.completion_waiters.input_count, 1);
    assert_eq!(during_recycle.completion_waiters.waiter_count, 1);
    assert_eq!(during_recycle.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        during_recycle.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert_eq!(
        during_recycle.completion_waiters.waiting_inputs[0].waiter_count,
        1
    );
    assert_eq!(
        apply_calls.load(Ordering::SeqCst),
        1,
        "recycle should wake the attached loop exactly once for the preserved queued work"
    );
    assert_eq!(
        control_calls.load(Ordering::SeqCst),
        0,
        "recycle should not route any out-of-band control command through the executor seam"
    );

    allow_finish.notify_waiters();
    tokio::time::timeout(Duration::from_secs(1), apply_finished.notified())
        .await
        .expect("attached loop should finish replaying the preserved queued work");

    match handle.wait().await {
        CompletionOutcome::CompletedWithoutResult => {}
        other => panic!("expected recycle+replay to complete queued work, got {other:?}"),
    }

    let settled = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let snapshot = adapter
                .meerkat_machine_spine_snapshot(&session_id)
                .await
                .expect("snapshot should exist after attached-loop replay");
            if snapshot.control.phase == RuntimeState::Attached {
                break snapshot;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("runtime should return to Attached once the preserved work finishes replaying");
    assert_eq!(settled.completion_waiters.input_count, 0);
    assert_eq!(settled.completion_waiters.waiter_count, 0);
    assert!(
        settled.completion_waiters.waiting_inputs.is_empty(),
        "recycle+replay should clear the completion waiter carrier once the preserved work completes"
    );
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_tracks_epoch_cursor_state() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;

    let cursor_state = {
        let sessions = adapter.sessions.read().await;
        Arc::clone(
            &sessions
                .get(&session_id)
                .expect("registered session should exist")
                .cursor_state,
        )
    };
    cursor_state
        .agent_applied_cursor
        .store(7, Ordering::Release);
    cursor_state
        .runtime_observed_seq
        .store(11, Ordering::Release);
    cursor_state
        .runtime_last_injected_seq
        .store(13, Ordering::Release);

    let snapshot = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist for registered session");

    assert_eq!(snapshot.binding.cursor_state.agent_applied_cursor, 7);
    assert_eq!(snapshot.binding.cursor_state.runtime_observed_seq, 11);
    assert_eq!(snapshot.binding.cursor_state.runtime_last_injected_seq, 13);
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_tracks_runtime_ops_state() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;
    let registry = adapter
        .ops_lifecycle_registry(&session_id)
        .await
        .expect("ops registry should exist for registered session");

    let operation_id = OperationId::new();
    registry
        .register_operation(OperationSpec {
            id: operation_id.clone(),
            kind: OperationKind::BackgroundToolOp,
            owner_session_id: session_id.clone(),
            display_name: "background test op".into(),
            source_label: "meerkat_machine_test".into(),
            child_session_id: None,
            expect_peer_channel: false,
        })
        .expect("operation should register");
    registry
        .provisioning_succeeded(&operation_id)
        .expect("operation should enter running");
    registry
        .report_progress(
            &operation_id,
            OperationProgressUpdate {
                message: "still working".into(),
                percent: Some(0.5),
            },
        )
        .expect("progress update should be accepted");

    let snapshot = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist for registered session");

    assert_eq!(snapshot.ops.operation_count, 1);
    assert_eq!(snapshot.ops.active_count, 1);
    assert_eq!(snapshot.ops.wait_request_id, None);
    assert!(!snapshot.ops.pending_wait_present);
    assert_eq!(snapshot.ops.pending_wait_request_id, None);
    assert!(snapshot.ops.wait_operation_ids.is_empty());
    assert_eq!(snapshot.ops.operations.len(), 1);

    let op = &snapshot.ops.operations[0];
    assert_eq!(op.id, operation_id);
    assert_eq!(op.kind, OperationKind::BackgroundToolOp);
    assert_eq!(op.display_name, "background test op");
    assert_eq!(op.status.as_str(), "running");
    assert!(!op.peer_ready);
    assert!(op.peer_handle.is_none());
    assert_eq!(op.progress_count, 1);
    assert_eq!(op.watcher_count, 0);
    assert_eq!(op.terminal_outcome, None);
    assert!(op.started_at_ms.is_some());
    assert!(op.completed_at_ms.is_none());
    assert!(op.elapsed_ms.is_none());
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_tracks_wait_all_state() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;
    let registry = adapter
        .ops_lifecycle_registry(&session_id)
        .await
        .expect("ops registry should exist for registered session");

    let operation_id = OperationId::new();
    registry
        .register_operation(OperationSpec {
            id: operation_id.clone(),
            kind: OperationKind::BackgroundToolOp,
            owner_session_id: session_id.clone(),
            display_name: "wait target".into(),
            source_label: "meerkat_machine_test".into(),
            child_session_id: None,
            expect_peer_channel: false,
        })
        .expect("operation should register");
    registry
        .provisioning_succeeded(&operation_id)
        .expect("operation should enter running");

    let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

    let snapshot = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist for registered session");

    assert_eq!(snapshot.ops.operation_count, 1);
    assert_eq!(snapshot.ops.active_count, 1);
    let wait_request_id = snapshot
        .ops
        .wait_request_id
        .clone()
        .expect("wait_all should register an authority-owned wait request");
    assert!(
        snapshot.ops.pending_wait_present,
        "pending wait carrier should be present while wait_all is active"
    );
    assert_eq!(
        snapshot.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "pending wait carrier should track the same wait request id as the authority"
    );
    assert_eq!(snapshot.ops.wait_operation_ids, vec![operation_id.clone()]);

    registry
        .complete_operation(
            &operation_id,
            OperationResult {
                id: operation_id.clone(),
                content: "done".into(),
                is_error: false,
                duration_ms: 1,
                tokens_used: 0,
            },
        )
        .expect("operation should complete");
    let wait_result = wait_future.await.expect("wait_all should resolve");
    assert_eq!(wait_result.satisfied.wait_request_id, wait_request_id);
    assert_eq!(wait_result.satisfied.operation_ids, vec![operation_id]);
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_preserves_wait_all_after_recover() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;
    let registry = adapter
        .ops_lifecycle_registry(&session_id)
        .await
        .expect("ops registry should exist for registered session");

    let operation_id = OperationId::new();
    registry
        .register_operation(OperationSpec {
            id: operation_id.clone(),
            kind: OperationKind::BackgroundToolOp,
            owner_session_id: session_id.clone(),
            display_name: "recover wait target".into(),
            source_label: "meerkat_machine_test".into(),
            child_session_id: None,
            expect_peer_channel: false,
        })
        .expect("operation should register");
    registry
        .provisioning_succeeded(&operation_id)
        .expect("operation should enter running");

    let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

    let before_recover = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist before recover");
    let wait_request_id = before_recover
        .ops
        .wait_request_id
        .clone()
        .expect("wait_all should register an authority-owned wait request");
    assert!(before_recover.ops.pending_wait_present);
    assert_eq!(
        before_recover.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "pending wait carrier should track the same wait request id before recover"
    );
    assert_eq!(
        before_recover.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "wait_all should track the active operation before recover"
    );

    let runtime_id = runtime_id_for_session(&session_id);
    let report = crate::traits::RuntimeControlPlane::recover(&*adapter, &runtime_id)
        .await
        .expect("recover should preserve the active wait_all carrier");
    assert_eq!(report.inputs_recovered, 0);

    let after_recover = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after recover");
    assert_eq!(
        after_recover.ops.wait_request_id,
        Some(wait_request_id.clone()),
        "recover should preserve the authority-owned wait request"
    );
    assert!(
        after_recover.ops.pending_wait_present,
        "recover should preserve the pending wait carrier while the operation remains active"
    );
    assert_eq!(
        after_recover.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "recover should preserve request-id agreement across the wait carrier seam"
    );
    assert_eq!(
        after_recover.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "recover should preserve the tracked wait targets"
    );

    registry
        .complete_operation(
            &operation_id,
            OperationResult {
                id: operation_id.clone(),
                content: "done".into(),
                is_error: false,
                duration_ms: 1,
                tokens_used: 0,
            },
        )
        .expect("operation should complete after recover");
    let wait_result = wait_future.await.expect("wait_all should still resolve");
    assert_eq!(wait_result.satisfied.wait_request_id, wait_request_id);
    assert_eq!(
        wait_result.satisfied.operation_ids,
        vec![operation_id.clone()]
    );

    let settled = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after wait_all settles");
    assert_eq!(
        settled.control.current_run_id, None,
        "recycle should not leave a settled control-side current-run binding behind"
    );
    assert_eq!(
        settled.inputs.current_run_id, None,
        "recycle should not leave a settled ingress-side current-run binding behind"
    );
    assert_eq!(settled.ops.wait_request_id, None);
    assert!(!settled.ops.pending_wait_present);
    assert_eq!(settled.ops.pending_wait_request_id, None);
    assert!(settled.ops.wait_operation_ids.is_empty());
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_recover_splits_completion_and_wait_all_lifetimes() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;
    let registry = adapter
        .ops_lifecycle_registry(&session_id)
        .await
        .expect("ops registry should exist for registered session");

    let input = make_prompt("recover split lifetimes");
    let input_id = input.id().clone();
    let (_outcome, completion_handle) = adapter
        .accept_input_with_completion(&session_id, input)
        .await
        .expect("prompt should be accepted");
    let completion_handle =
        completion_handle.expect("queued prompt should register a completion waiter");

    let operation_id = OperationId::new();
    registry
        .register_operation(OperationSpec {
            id: operation_id.clone(),
            kind: OperationKind::BackgroundToolOp,
            owner_session_id: session_id.clone(),
            display_name: "recover split wait target".into(),
            source_label: "meerkat_machine_test".into(),
            child_session_id: None,
            expect_peer_channel: false,
        })
        .expect("operation should register");
    registry
        .provisioning_succeeded(&operation_id)
        .expect("operation should enter running");

    let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

    let before_recover = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist before recover");
    let wait_request_id = before_recover
        .ops
        .wait_request_id
        .clone()
        .expect("wait_all should register an authority-owned wait request");
    assert_eq!(before_recover.control.phase, RuntimeState::Idle);
    assert_eq!(before_recover.inputs.queue, vec![input_id.clone()]);
    assert_eq!(before_recover.completion_waiters.input_count, 1);
    assert_eq!(before_recover.completion_waiters.waiter_count, 1);
    assert_eq!(before_recover.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        before_recover.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert!(before_recover.ops.pending_wait_present);
    assert_eq!(
        before_recover.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "pending wait carrier should track the same wait request id before recover"
    );
    assert_eq!(
        before_recover.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "wait_all should track the active operation before recover"
    );

    let runtime_id = runtime_id_for_session(&session_id);
    let report = crate::traits::RuntimeControlPlane::recover(&*adapter, &runtime_id)
        .await
        .expect("recover should preserve both queued input and active wait_all");
    assert_eq!(report.inputs_recovered, 1);

    let after_recover = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after recover");
    assert_eq!(after_recover.control.phase, RuntimeState::Idle);
    assert_eq!(after_recover.inputs.queue, vec![input_id.clone()]);
    assert_eq!(after_recover.completion_waiters.input_count, 1);
    assert_eq!(after_recover.completion_waiters.waiter_count, 1);
    assert_eq!(after_recover.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        after_recover.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert_eq!(
        after_recover.ops.wait_request_id,
        Some(wait_request_id.clone()),
        "recover should preserve the authority-owned wait request"
    );
    assert!(after_recover.ops.pending_wait_present);
    assert_eq!(
        after_recover.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "recover should preserve request-id agreement across the wait carrier seam"
    );
    assert_eq!(
        after_recover.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "recover should preserve the tracked wait target"
    );

    registry
        .complete_operation(
            &operation_id,
            OperationResult {
                id: operation_id.clone(),
                content: "done".into(),
                is_error: false,
                duration_ms: 1,
                tokens_used: 0,
            },
        )
        .expect("operation should complete after recover");
    let wait_result = wait_future.await.expect("wait_all should still resolve");
    assert_eq!(wait_result.satisfied.wait_request_id, wait_request_id);
    assert_eq!(
        wait_result.satisfied.operation_ids,
        vec![operation_id.clone()]
    );

    let after_wait_all = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after wait_all settles");
    assert_eq!(after_wait_all.control.phase, RuntimeState::Idle);
    assert_eq!(
        after_wait_all.control.current_run_id, None,
        "recover should not leave a settled control-side current-run binding behind"
    );
    assert_eq!(
        after_wait_all.inputs.current_run_id, None,
        "recover should not leave a settled ingress-side current-run binding behind"
    );
    assert_eq!(after_wait_all.inputs.queue, vec![input_id.clone()]);
    assert_eq!(after_wait_all.completion_waiters.input_count, 1);
    assert_eq!(after_wait_all.completion_waiters.waiter_count, 1);
    assert_eq!(after_wait_all.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        after_wait_all.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert_eq!(after_wait_all.ops.wait_request_id, None);
    assert!(!after_wait_all.ops.pending_wait_present);
    assert_eq!(after_wait_all.ops.pending_wait_request_id, None);
    assert!(after_wait_all.ops.wait_operation_ids.is_empty());

    SessionServiceRuntimeExt::reset_runtime(&*adapter, &session_id)
        .await
        .expect("reset should terminate the preserved completion waiter at test end");
    match completion_handle.wait().await {
        CompletionOutcome::RuntimeTerminated(reason) => {
            assert_eq!(reason, "runtime reset");
        }
        other => panic!("expected runtime reset termination, got {other:?}"),
    }
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_recover_preserves_steered_input_and_wait_all() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;
    let registry = adapter
        .ops_lifecycle_registry(&session_id)
        .await
        .expect("ops registry should exist for registered session");

    let input = Input::Prompt(crate::input::PromptInput::new(
        "recover steered prompt",
        Some(
            meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                handling_mode: Some(meerkat_core::types::HandlingMode::Steer),
                ..Default::default()
            },
        ),
    ));
    let input_id = input.id().clone();
    let (_outcome, completion_handle) = adapter
        .accept_input_with_completion(&session_id, input)
        .await
        .expect("steered prompt should be accepted");
    let completion_handle =
        completion_handle.expect("steered prompt should register a completion waiter");

    let operation_id = OperationId::new();
    registry
        .register_operation(OperationSpec {
            id: operation_id.clone(),
            kind: OperationKind::BackgroundToolOp,
            owner_session_id: session_id.clone(),
            display_name: "recover steered wait target".into(),
            source_label: "meerkat_machine_test".into(),
            child_session_id: None,
            expect_peer_channel: false,
        })
        .expect("operation should register");
    registry
        .provisioning_succeeded(&operation_id)
        .expect("operation should enter running");

    let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

    let before_recover = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist before recover");
    let wait_request_id = before_recover
        .ops
        .wait_request_id
        .clone()
        .expect("wait_all should register an authority-owned wait request");
    assert_eq!(before_recover.control.phase, RuntimeState::Idle);
    assert!(before_recover.inputs.queue.is_empty());
    assert_eq!(before_recover.inputs.steer_queue, vec![input_id.clone()]);
    assert_eq!(before_recover.completion_waiters.input_count, 1);
    assert_eq!(before_recover.completion_waiters.waiter_count, 1);
    assert_eq!(before_recover.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        before_recover.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert_eq!(
        before_recover.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "pending wait carrier should track the same wait request id before recover"
    );
    assert_eq!(
        before_recover.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "wait_all should track the active operation before recover"
    );

    let runtime_id = runtime_id_for_session(&session_id);
    let report = crate::traits::RuntimeControlPlane::recover(&*adapter, &runtime_id)
        .await
        .expect("recover should preserve steered input and active wait_all");
    assert_eq!(report.inputs_recovered, 1);

    let after_recover = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after recover");
    assert_eq!(after_recover.control.phase, RuntimeState::Idle);
    assert!(after_recover.inputs.queue.is_empty());
    assert_eq!(after_recover.inputs.steer_queue, vec![input_id.clone()]);
    assert_eq!(after_recover.completion_waiters.input_count, 1);
    assert_eq!(after_recover.completion_waiters.waiter_count, 1);
    assert_eq!(after_recover.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        after_recover.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert_eq!(
        after_recover.ops.wait_request_id,
        Some(wait_request_id.clone()),
        "recover should preserve the authority-owned wait request"
    );
    assert!(after_recover.ops.pending_wait_present);
    assert_eq!(
        after_recover.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "recover should preserve request-id agreement across the wait carrier seam"
    );
    assert_eq!(
        after_recover.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "recover should preserve the tracked wait target"
    );

    registry
        .complete_operation(
            &operation_id,
            OperationResult {
                id: operation_id.clone(),
                content: "done".into(),
                is_error: false,
                duration_ms: 1,
                tokens_used: 0,
            },
        )
        .expect("operation should complete after recover");
    let wait_result = wait_future.await.expect("wait_all should still resolve");
    assert_eq!(wait_result.satisfied.wait_request_id, wait_request_id);
    assert_eq!(
        wait_result.satisfied.operation_ids,
        vec![operation_id.clone()]
    );

    let after_wait_all = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after wait_all settles");
    assert_eq!(after_wait_all.control.phase, RuntimeState::Idle);
    assert_eq!(after_wait_all.control.current_run_id, None);
    assert_eq!(after_wait_all.inputs.current_run_id, None);
    assert!(after_wait_all.inputs.queue.is_empty());
    assert_eq!(after_wait_all.inputs.steer_queue, vec![input_id.clone()]);
    assert_eq!(after_wait_all.completion_waiters.input_count, 1);
    assert_eq!(after_wait_all.completion_waiters.waiter_count, 1);
    assert_eq!(after_wait_all.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        after_wait_all.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert_eq!(after_wait_all.ops.wait_request_id, None);
    assert!(!after_wait_all.ops.pending_wait_present);
    assert_eq!(after_wait_all.ops.pending_wait_request_id, None);
    assert!(after_wait_all.ops.wait_operation_ids.is_empty());

    SessionServiceRuntimeExt::reset_runtime(&*adapter, &session_id)
        .await
        .expect("reset should terminate the preserved steered completion waiter at test end");
    match completion_handle.wait().await {
        CompletionOutcome::RuntimeTerminated(reason) => {
            assert_eq!(reason, "runtime reset");
        }
        other => panic!("expected runtime reset termination, got {other:?}"),
    }
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_recycle_preserves_steered_input_and_wait_all() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;
    let registry = adapter
        .ops_lifecycle_registry(&session_id)
        .await
        .expect("ops registry should exist for registered session");

    let input = Input::Prompt(crate::input::PromptInput::new(
        "recycle steered prompt",
        Some(
            meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                handling_mode: Some(meerkat_core::types::HandlingMode::Steer),
                ..Default::default()
            },
        ),
    ));
    let input_id = input.id().clone();
    let (_outcome, completion_handle) = adapter
        .accept_input_with_completion(&session_id, input)
        .await
        .expect("steered prompt should be accepted");
    let completion_handle =
        completion_handle.expect("steered prompt should register a completion waiter");

    let operation_id = OperationId::new();
    registry
        .register_operation(OperationSpec {
            id: operation_id.clone(),
            kind: OperationKind::BackgroundToolOp,
            owner_session_id: session_id.clone(),
            display_name: "recycle steered wait target".into(),
            source_label: "meerkat_machine_test".into(),
            child_session_id: None,
            expect_peer_channel: false,
        })
        .expect("operation should register");
    registry
        .provisioning_succeeded(&operation_id)
        .expect("operation should enter running");

    let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

    let before_recycle = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist before recycle");
    let wait_request_id = before_recycle
        .ops
        .wait_request_id
        .clone()
        .expect("wait_all should register an authority-owned wait request");
    assert_eq!(before_recycle.control.phase, RuntimeState::Idle);
    assert!(before_recycle.inputs.queue.is_empty());
    assert_eq!(before_recycle.inputs.steer_queue, vec![input_id.clone()]);
    assert_eq!(before_recycle.completion_waiters.input_count, 1);
    assert_eq!(before_recycle.completion_waiters.waiter_count, 1);
    assert_eq!(before_recycle.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        before_recycle.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert_eq!(
        before_recycle.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "pending wait carrier should track the same wait request id before recycle"
    );
    assert_eq!(
        before_recycle.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "wait_all should track the active operation before recycle"
    );

    let runtime_id = runtime_id_for_session(&session_id);
    let report = crate::traits::RuntimeControlPlane::recycle(&*adapter, &runtime_id)
        .await
        .expect("recycle should preserve steered input and active wait_all");
    assert_eq!(report.inputs_transferred, 1);

    let after_recycle = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after recycle");
    assert_eq!(after_recycle.control.phase, RuntimeState::Idle);
    assert!(after_recycle.inputs.queue.is_empty());
    assert_eq!(after_recycle.inputs.steer_queue, vec![input_id.clone()]);
    assert_eq!(after_recycle.completion_waiters.input_count, 1);
    assert_eq!(after_recycle.completion_waiters.waiter_count, 1);
    assert_eq!(after_recycle.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        after_recycle.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert_eq!(
        after_recycle.ops.wait_request_id,
        Some(wait_request_id.clone()),
        "recycle should preserve the authority-owned wait request"
    );
    assert!(after_recycle.ops.pending_wait_present);
    assert_eq!(
        after_recycle.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "recycle should preserve request-id agreement across the wait carrier seam"
    );
    assert_eq!(
        after_recycle.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "recycle should preserve the tracked wait target"
    );

    registry
        .complete_operation(
            &operation_id,
            OperationResult {
                id: operation_id.clone(),
                content: "done".into(),
                is_error: false,
                duration_ms: 1,
                tokens_used: 0,
            },
        )
        .expect("operation should complete after recycle");
    let wait_result = wait_future.await.expect("wait_all should still resolve");
    assert_eq!(wait_result.satisfied.wait_request_id, wait_request_id);
    assert_eq!(
        wait_result.satisfied.operation_ids,
        vec![operation_id.clone()]
    );

    let after_wait_all = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after wait_all settles");
    assert_eq!(after_wait_all.control.phase, RuntimeState::Idle);
    assert_eq!(after_wait_all.control.current_run_id, None);
    assert_eq!(after_wait_all.inputs.current_run_id, None);
    assert!(after_wait_all.inputs.queue.is_empty());
    assert_eq!(after_wait_all.inputs.steer_queue, vec![input_id.clone()]);
    assert_eq!(after_wait_all.completion_waiters.input_count, 1);
    assert_eq!(after_wait_all.completion_waiters.waiter_count, 1);
    assert_eq!(after_wait_all.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        after_wait_all.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert_eq!(after_wait_all.ops.wait_request_id, None);
    assert!(!after_wait_all.ops.pending_wait_present);
    assert_eq!(after_wait_all.ops.pending_wait_request_id, None);
    assert!(after_wait_all.ops.wait_operation_ids.is_empty());

    SessionServiceRuntimeExt::reset_runtime(&*adapter, &session_id)
        .await
        .expect("reset should terminate the preserved steered completion waiter at test end");
    match completion_handle.wait().await {
        CompletionOutcome::RuntimeTerminated(reason) => {
            assert_eq!(reason, "runtime reset");
        }
        other => panic!("expected runtime reset termination, got {other:?}"),
    }
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_preserves_wait_all_after_recycle() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;
    let registry = adapter
        .ops_lifecycle_registry(&session_id)
        .await
        .expect("ops registry should exist for registered session");

    let operation_id = OperationId::new();
    registry
        .register_operation(OperationSpec {
            id: operation_id.clone(),
            kind: OperationKind::BackgroundToolOp,
            owner_session_id: session_id.clone(),
            display_name: "recycle wait target".into(),
            source_label: "meerkat_machine_test".into(),
            child_session_id: None,
            expect_peer_channel: false,
        })
        .expect("operation should register");
    registry
        .provisioning_succeeded(&operation_id)
        .expect("operation should enter running");

    let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

    let before_recycle = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist before recycle");
    let wait_request_id = before_recycle
        .ops
        .wait_request_id
        .clone()
        .expect("wait_all should register an authority-owned wait request");
    assert!(before_recycle.ops.pending_wait_present);
    assert_eq!(
        before_recycle.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "pending wait carrier should track the same wait request id before recycle"
    );
    assert_eq!(
        before_recycle.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "wait_all should track the active operation before recycle"
    );

    let runtime_id = runtime_id_for_session(&session_id);
    let report = crate::traits::RuntimeControlPlane::recycle(&*adapter, &runtime_id)
        .await
        .expect("recycle should preserve the active wait_all carrier");
    assert_eq!(report.inputs_transferred, 0);

    let after_recycle = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after recycle");
    assert_eq!(
        after_recycle.ops.wait_request_id,
        Some(wait_request_id.clone()),
        "recycle should preserve the authority-owned wait request"
    );
    assert!(
        after_recycle.ops.pending_wait_present,
        "recycle should preserve the pending wait carrier while the operation remains active"
    );
    assert_eq!(
        after_recycle.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "recycle should preserve request-id agreement across the wait carrier seam"
    );
    assert_eq!(
        after_recycle.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "recycle should preserve the tracked wait targets"
    );

    registry
        .complete_operation(
            &operation_id,
            OperationResult {
                id: operation_id.clone(),
                content: "done".into(),
                is_error: false,
                duration_ms: 1,
                tokens_used: 0,
            },
        )
        .expect("operation should complete after recycle");
    let wait_result = wait_future.await.expect("wait_all should still resolve");
    assert_eq!(wait_result.satisfied.wait_request_id, wait_request_id);
    assert_eq!(
        wait_result.satisfied.operation_ids,
        vec![operation_id.clone()]
    );

    let settled = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after wait_all settles");
    assert_eq!(settled.ops.wait_request_id, None);
    assert!(!settled.ops.pending_wait_present);
    assert_eq!(settled.ops.pending_wait_request_id, None);
    assert!(settled.ops.wait_operation_ids.is_empty());
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_recycle_splits_completion_and_wait_all_lifetimes() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;
    let registry = adapter
        .ops_lifecycle_registry(&session_id)
        .await
        .expect("ops registry should exist for registered session");

    let input = make_prompt("recycle split lifetimes");
    let input_id = input.id().clone();
    let (_outcome, completion_handle) = adapter
        .accept_input_with_completion(&session_id, input)
        .await
        .expect("prompt should be accepted");
    let completion_handle =
        completion_handle.expect("queued prompt should register a completion waiter");

    let operation_id = OperationId::new();
    registry
        .register_operation(OperationSpec {
            id: operation_id.clone(),
            kind: OperationKind::BackgroundToolOp,
            owner_session_id: session_id.clone(),
            display_name: "recycle split wait target".into(),
            source_label: "meerkat_machine_test".into(),
            child_session_id: None,
            expect_peer_channel: false,
        })
        .expect("operation should register");
    registry
        .provisioning_succeeded(&operation_id)
        .expect("operation should enter running");

    let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

    let before_recycle = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist before recycle");
    let wait_request_id = before_recycle
        .ops
        .wait_request_id
        .clone()
        .expect("wait_all should register an authority-owned wait request");
    assert_eq!(before_recycle.control.phase, RuntimeState::Idle);
    assert_eq!(before_recycle.inputs.queue, vec![input_id.clone()]);
    assert_eq!(before_recycle.completion_waiters.input_count, 1);
    assert_eq!(before_recycle.completion_waiters.waiter_count, 1);
    assert_eq!(before_recycle.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        before_recycle.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert!(before_recycle.ops.pending_wait_present);
    assert_eq!(
        before_recycle.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "pending wait carrier should track the same wait request id before recycle"
    );
    assert_eq!(
        before_recycle.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "wait_all should track the active operation before recycle"
    );

    let runtime_id = runtime_id_for_session(&session_id);
    let report = crate::traits::RuntimeControlPlane::recycle(&*adapter, &runtime_id)
        .await
        .expect("recycle should preserve both queued input and active wait_all");
    assert_eq!(report.inputs_transferred, 1);

    let after_recycle = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after recycle");
    assert_eq!(after_recycle.control.phase, RuntimeState::Idle);
    assert_eq!(after_recycle.inputs.queue, vec![input_id.clone()]);
    assert_eq!(after_recycle.completion_waiters.input_count, 1);
    assert_eq!(after_recycle.completion_waiters.waiter_count, 1);
    assert_eq!(after_recycle.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        after_recycle.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert_eq!(
        after_recycle.ops.wait_request_id,
        Some(wait_request_id.clone()),
        "recycle should preserve the authority-owned wait request"
    );
    assert!(after_recycle.ops.pending_wait_present);
    assert_eq!(
        after_recycle.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "recycle should preserve request-id agreement across the wait carrier seam"
    );
    assert_eq!(
        after_recycle.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "recycle should preserve the tracked wait target"
    );

    registry
        .complete_operation(
            &operation_id,
            OperationResult {
                id: operation_id.clone(),
                content: "done".into(),
                is_error: false,
                duration_ms: 1,
                tokens_used: 0,
            },
        )
        .expect("operation should complete after recycle");
    let wait_result = wait_future.await.expect("wait_all should still resolve");
    assert_eq!(wait_result.satisfied.wait_request_id, wait_request_id);
    assert_eq!(
        wait_result.satisfied.operation_ids,
        vec![operation_id.clone()]
    );

    let after_wait_all = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after wait_all settles");
    assert_eq!(after_wait_all.control.phase, RuntimeState::Idle);
    assert_eq!(after_wait_all.inputs.queue, vec![input_id.clone()]);
    assert_eq!(after_wait_all.completion_waiters.input_count, 1);
    assert_eq!(after_wait_all.completion_waiters.waiter_count, 1);
    assert_eq!(after_wait_all.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        after_wait_all.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert_eq!(after_wait_all.ops.wait_request_id, None);
    assert!(!after_wait_all.ops.pending_wait_present);
    assert_eq!(after_wait_all.ops.pending_wait_request_id, None);
    assert!(after_wait_all.ops.wait_operation_ids.is_empty());

    SessionServiceRuntimeExt::reset_runtime(&*adapter, &session_id)
        .await
        .expect("reset should terminate the preserved completion waiter at test end");
    match completion_handle.wait().await {
        CompletionOutcome::RuntimeTerminated(reason) => {
            assert_eq!(reason, "runtime reset");
        }
        other => panic!("expected runtime reset termination, got {other:?}"),
    }
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_preserves_wait_all_after_recover_with_runtime_loop() {
    struct BlockingExecutor {
        apply_calls: Arc<AtomicUsize>,
        control_calls: Arc<AtomicUsize>,
        apply_started: Arc<Notify>,
        apply_finished: Arc<Notify>,
        allow_finish: Arc<Notify>,
    }

    #[async_trait::async_trait]
    impl CoreExecutor for BlockingExecutor {
        async fn apply(
            &mut self,
            run_id: RunId,
            primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            self.apply_calls.fetch_add(1, Ordering::SeqCst);
            self.apply_started.notify_waiters();
            self.allow_finish.notified().await;
            self.apply_finished.notify_waiters();

            Ok(CoreApplyOutput {
                receipt: RunBoundaryReceipt {
                    run_id,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                    sequence: 0,
                },
                session_snapshot: None,
                terminal: None,
            })
        }

        async fn control(&mut self, _command: RunControlCommand) -> Result<(), CoreExecutorError> {
            self.control_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    let apply_calls = Arc::new(AtomicUsize::new(0));
    let control_calls = Arc::new(AtomicUsize::new(0));
    let apply_started = Arc::new(Notify::new());
    let apply_finished = Arc::new(Notify::new());
    let allow_finish = Arc::new(Notify::new());

    adapter
        .register_session_with_executor(
            session_id.clone(),
            Box::new(BlockingExecutor {
                apply_calls: Arc::clone(&apply_calls),
                control_calls: Arc::clone(&control_calls),
                apply_started: Arc::clone(&apply_started),
                apply_finished: Arc::clone(&apply_finished),
                allow_finish: Arc::clone(&allow_finish),
            }),
        )
        .await;

    let outcome = adapter
        .accept_input_without_wake(
            &session_id,
            make_progress_input("recover-wait-all-with-loop"),
        )
        .await
        .expect("queued progress input should be accepted without waking the loop");
    assert!(outcome.is_accepted());

    let registry = adapter
        .ops_lifecycle_registry(&session_id)
        .await
        .expect("ops registry should exist for attached session");

    let operation_id = OperationId::new();
    registry
        .register_operation(OperationSpec {
            id: operation_id.clone(),
            kind: OperationKind::BackgroundToolOp,
            owner_session_id: session_id.clone(),
            display_name: "recover wait target with loop".into(),
            source_label: "meerkat_machine_test".into(),
            child_session_id: None,
            expect_peer_channel: false,
        })
        .expect("operation should register");
    registry
        .provisioning_succeeded(&operation_id)
        .expect("operation should enter running");

    let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

    let before_recover = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist before recover");
    let wait_request_id = before_recover
        .ops
        .wait_request_id
        .clone()
        .expect("wait_all should register an authority-owned wait request");
    assert_eq!(before_recover.control.phase, RuntimeState::Attached);
    assert!(before_recover.ops.pending_wait_present);
    assert_eq!(
        before_recover.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "pending wait carrier should track the same wait request id before recover"
    );
    assert_eq!(
        before_recover.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "wait_all should track the active operation before recover"
    );
    assert_eq!(
        apply_calls.load(Ordering::SeqCst),
        0,
        "queued attached-loop work should remain pending until recover wakes the loop"
    );
    assert_eq!(
        control_calls.load(Ordering::SeqCst),
        0,
        "recover should not yet have attempted executor control"
    );

    let runtime_id = runtime_id_for_session(&session_id);
    let report = crate::traits::RuntimeControlPlane::recover(&*adapter, &runtime_id)
        .await
        .expect("recover should preserve wait_all while replaying attached-loop work");
    assert_eq!(report.inputs_recovered, 1);

    tokio::time::timeout(Duration::from_secs(1), apply_started.notified())
        .await
        .expect("recover should wake the attached runtime loop to replay preserved work");

    let during_recover = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist while recover replay is in flight");
    assert_eq!(
        during_recover.control.phase,
        RuntimeState::Running,
        "post-#32 W6-J: during recover_with_runtime_loop, the runtime loop fires DSL `Prepare` which transitions lifecycle_phase to Running; the recover transition itself self-loops on Attached but the apply-in-flight binds to a run_id and sits at Running until Commit returns"
    );
    assert_eq!(
        during_recover.ops.wait_request_id,
        Some(wait_request_id.clone()),
        "recover should preserve the authority-owned wait request while replay is in flight"
    );
    assert!(during_recover.ops.pending_wait_present);
    assert_eq!(
        during_recover.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "recover should preserve request-id agreement across the wait carrier seam while replaying"
    );
    assert_eq!(
        during_recover.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "recover should preserve the tracked wait target while recovered work is replaying"
    );
    assert_eq!(
        apply_calls.load(Ordering::SeqCst),
        1,
        "recover should wake the attached loop exactly once for the recovered queued work"
    );
    assert_eq!(
        control_calls.load(Ordering::SeqCst),
        0,
        "recover should not route any out-of-band control command through the executor seam"
    );

    allow_finish.notify_waiters();
    tokio::time::timeout(Duration::from_secs(1), apply_finished.notified())
        .await
        .expect("attached loop should finish replaying the recovered queued work");

    let after_replay = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let snapshot = adapter
                .meerkat_machine_spine_snapshot(&session_id)
                .await
                .expect("snapshot should exist after attached-loop replay");
            if snapshot.control.phase != RuntimeState::Running {
                break snapshot;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("runtime should eventually leave Running once the recovered work finishes replaying");
    assert_eq!(
        after_replay.ops.wait_request_id,
        Some(wait_request_id.clone()),
        "wait_all should remain active after the attached loop finishes replaying recovered work"
    );
    assert!(after_replay.ops.pending_wait_present);
    assert_eq!(
        after_replay.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "request-id agreement should survive the return to Idle after replay"
    );
    assert_eq!(
        after_replay.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "the tracked wait target should remain present until the operation itself settles"
    );
    assert_eq!(
        after_replay.control.phase,
        RuntimeState::Attached,
        "recover should currently return to Attached once the recovered work finishes replaying"
    );
    assert!(
        after_replay.inputs.queue.is_empty(),
        "recover should not leave ordinary queued work behind once the attached runtime returns to Attached after replay"
    );
    assert!(
        after_replay.inputs.steer_queue.is_empty(),
        "recover should not leave steer-queued work behind once the attached runtime returns to Attached after replay"
    );

    registry
        .complete_operation(
            &operation_id,
            OperationResult {
                id: operation_id.clone(),
                content: "done".into(),
                is_error: false,
                duration_ms: 1,
                tokens_used: 0,
            },
        )
        .expect("operation should complete after recover+replay");
    let wait_result = wait_future.await.expect("wait_all should still resolve");
    assert_eq!(wait_result.satisfied.wait_request_id, wait_request_id);
    assert_eq!(
        wait_result.satisfied.operation_ids,
        vec![operation_id.clone()]
    );

    let settled = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after wait_all settles");
    assert_eq!(settled.control.phase, RuntimeState::Attached);
    assert_eq!(
        settled.control.current_run_id, None,
        "recover should not leave a settled control-side current-run binding behind even on attached runtimes"
    );
    assert_eq!(
        settled.inputs.current_run_id, None,
        "recover should not leave a settled ingress-side current-run binding behind even on attached runtimes"
    );
    assert!(
        settled.inputs.queue.is_empty(),
        "recover should keep the attached settled snapshot free of ordinary queued work after replay completes"
    );
    assert!(
        settled.inputs.steer_queue.is_empty(),
        "recover should keep the attached settled snapshot free of steer-queued work after replay completes"
    );
    assert_eq!(settled.ops.wait_request_id, None);
    assert!(!settled.ops.pending_wait_present);
    assert_eq!(settled.ops.pending_wait_request_id, None);
    assert!(settled.ops.wait_operation_ids.is_empty());
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_recover_with_runtime_loop_splits_completion_and_wait_all_lifetimes()
 {
    struct BlockingExecutor {
        apply_calls: Arc<AtomicUsize>,
        control_calls: Arc<AtomicUsize>,
        apply_started: Arc<Notify>,
        apply_finished: Arc<Notify>,
        allow_finish: Arc<Notify>,
    }

    #[async_trait::async_trait]
    impl CoreExecutor for BlockingExecutor {
        async fn apply(
            &mut self,
            run_id: RunId,
            primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            self.apply_calls.fetch_add(1, Ordering::SeqCst);
            self.apply_started.notify_waiters();
            self.allow_finish.notified().await;
            self.apply_finished.notify_waiters();

            Ok(CoreApplyOutput {
                receipt: RunBoundaryReceipt {
                    run_id,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                    sequence: 0,
                },
                session_snapshot: None,
                terminal: None,
            })
        }

        async fn control(&mut self, _command: RunControlCommand) -> Result<(), CoreExecutorError> {
            self.control_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    let apply_calls = Arc::new(AtomicUsize::new(0));
    let control_calls = Arc::new(AtomicUsize::new(0));
    let apply_started = Arc::new(Notify::new());
    let apply_finished = Arc::new(Notify::new());
    let allow_finish = Arc::new(Notify::new());

    adapter
        .register_session_with_executor(
            session_id.clone(),
            Box::new(BlockingExecutor {
                apply_calls: Arc::clone(&apply_calls),
                control_calls: Arc::clone(&control_calls),
                apply_started: Arc::clone(&apply_started),
                apply_finished: Arc::clone(&apply_finished),
                allow_finish: Arc::clone(&allow_finish),
            }),
        )
        .await;

    let input = make_progress_input("recover-with-loop-split-lifetimes");
    let input_id = input.id().clone();
    let (outcome, completion_handle) = adapter
        .accept_input_with_completion(&session_id, input)
        .await
        .expect("progress input should be accepted");
    assert!(outcome.is_accepted());
    let completion_handle =
        completion_handle.expect("queued progress input should expose a completion waiter");

    let registry = adapter
        .ops_lifecycle_registry(&session_id)
        .await
        .expect("ops registry should exist for attached session");

    let operation_id = OperationId::new();
    registry
        .register_operation(OperationSpec {
            id: operation_id.clone(),
            kind: OperationKind::BackgroundToolOp,
            owner_session_id: session_id.clone(),
            display_name: "recover split wait target".into(),
            source_label: "meerkat_machine_test".into(),
            child_session_id: None,
            expect_peer_channel: false,
        })
        .expect("operation should register");
    registry
        .provisioning_succeeded(&operation_id)
        .expect("operation should enter running");

    let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

    let before_recover = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist before recover");
    let wait_request_id = before_recover
        .ops
        .wait_request_id
        .clone()
        .expect("wait_all should register an authority-owned wait request");
    assert_eq!(before_recover.control.phase, RuntimeState::Attached);
    assert_eq!(before_recover.completion_waiters.input_count, 1);
    assert_eq!(before_recover.completion_waiters.waiter_count, 1);
    assert_eq!(before_recover.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        before_recover.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert!(before_recover.ops.pending_wait_present);
    assert_eq!(
        before_recover.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "pending wait carrier should track the same wait request id before recover"
    );
    assert_eq!(
        before_recover.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "wait_all should track the active operation before recover"
    );
    assert_eq!(
        apply_calls.load(Ordering::SeqCst),
        0,
        "queued attached-loop work should remain pending until recover wakes the loop"
    );
    assert_eq!(
        control_calls.load(Ordering::SeqCst),
        0,
        "recover should not yet have attempted executor control"
    );

    let runtime_id = runtime_id_for_session(&session_id);
    let report = crate::traits::RuntimeControlPlane::recover(&*adapter, &runtime_id)
        .await
        .expect("recover should split completion and wait_all lifetimes on attached runtimes");
    assert_eq!(report.inputs_recovered, 1);

    tokio::time::timeout(Duration::from_secs(1), apply_started.notified())
        .await
        .expect("recover should wake the attached runtime loop to replay recovered work");

    let during_recover = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist while recover replay is in flight");
    assert_eq!(
        during_recover.control.phase,
        RuntimeState::Running,
        "post-#32 W6-J: during recover_with_runtime_loop, the runtime loop fires DSL `Prepare` which transitions lifecycle_phase to Running; the recover transition itself self-loops on Attached but the apply-in-flight binds to a run_id and sits at Running until Commit returns"
    );
    assert_eq!(during_recover.completion_waiters.input_count, 1);
    assert_eq!(during_recover.completion_waiters.waiter_count, 1);
    assert_eq!(during_recover.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        during_recover.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert_eq!(
        during_recover.ops.wait_request_id,
        Some(wait_request_id.clone()),
        "recover should preserve the authority-owned wait request while replay is in flight"
    );
    assert!(during_recover.ops.pending_wait_present);
    assert_eq!(
        during_recover.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "recover should preserve request-id agreement across the wait carrier seam while replaying"
    );
    assert_eq!(
        during_recover.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "recover should preserve the tracked wait target while recovered work is replaying"
    );
    assert_eq!(
        apply_calls.load(Ordering::SeqCst),
        1,
        "recover should wake the attached loop exactly once for the recovered queued work"
    );
    assert_eq!(
        control_calls.load(Ordering::SeqCst),
        0,
        "recover should not route any out-of-band control command through the executor seam"
    );

    allow_finish.notify_waiters();
    tokio::time::timeout(Duration::from_secs(1), apply_finished.notified())
        .await
        .expect("attached loop should finish replaying the recovered queued work");

    match completion_handle.wait().await {
        CompletionOutcome::CompletedWithoutResult => {}
        other => panic!("expected recover+replay to complete queued work, got {other:?}"),
    }

    let after_replay = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let snapshot = adapter
                .meerkat_machine_spine_snapshot(&session_id)
                .await
                .expect("snapshot should exist after attached-loop replay");
            if snapshot.control.phase != RuntimeState::Running {
                break snapshot;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("runtime should eventually leave Running once the recovered work finishes replaying");
    assert_eq!(
        after_replay.control.phase,
        RuntimeState::Attached,
        "recover should currently return to Attached once the recovered work finishes replaying"
    );
    assert_eq!(after_replay.completion_waiters.input_count, 0);
    assert_eq!(after_replay.completion_waiters.waiter_count, 0);
    assert!(
        after_replay.completion_waiters.waiting_inputs.is_empty(),
        "recover should clear completion waiters once the recovered work finishes replaying"
    );
    assert_eq!(
        after_replay.ops.wait_request_id,
        Some(wait_request_id.clone()),
        "wait_all should remain active after the recovered work finishes replaying"
    );
    assert!(after_replay.ops.pending_wait_present);
    assert_eq!(
        after_replay.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "request-id agreement should survive the return to Attached after replay"
    );
    assert_eq!(
        after_replay.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "the tracked wait target should remain present until the operation itself settles"
    );

    registry
        .complete_operation(
            &operation_id,
            OperationResult {
                id: operation_id.clone(),
                content: "done".into(),
                is_error: false,
                duration_ms: 1,
                tokens_used: 0,
            },
        )
        .expect("operation should complete after recover+replay");
    let wait_result = wait_future.await.expect("wait_all should still resolve");
    assert_eq!(wait_result.satisfied.wait_request_id, wait_request_id);
    assert_eq!(
        wait_result.satisfied.operation_ids,
        vec![operation_id.clone()]
    );

    let settled = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after wait_all settles");
    assert_eq!(settled.control.phase, RuntimeState::Attached);
    assert_eq!(
        settled.control.current_run_id, None,
        "recycle should not leave a settled control-side current-run binding behind even on attached runtimes"
    );
    assert_eq!(
        settled.inputs.current_run_id, None,
        "recycle should not leave a settled ingress-side current-run binding behind even on attached runtimes"
    );
    assert_eq!(settled.ops.wait_request_id, None);
    assert!(!settled.ops.pending_wait_present);
    assert_eq!(settled.ops.pending_wait_request_id, None);
    assert!(settled.ops.wait_operation_ids.is_empty());
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_preserves_wait_all_after_recycle_with_runtime_loop() {
    struct BlockingExecutor {
        apply_calls: Arc<AtomicUsize>,
        control_calls: Arc<AtomicUsize>,
        apply_started: Arc<Notify>,
        apply_finished: Arc<Notify>,
        allow_finish: Arc<Notify>,
    }

    #[async_trait::async_trait]
    impl CoreExecutor for BlockingExecutor {
        async fn apply(
            &mut self,
            run_id: RunId,
            primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            self.apply_calls.fetch_add(1, Ordering::SeqCst);
            self.apply_started.notify_waiters();
            self.allow_finish.notified().await;
            self.apply_finished.notify_waiters();

            Ok(CoreApplyOutput {
                receipt: RunBoundaryReceipt {
                    run_id,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                    sequence: 0,
                },
                session_snapshot: None,
                terminal: None,
            })
        }

        async fn control(&mut self, _command: RunControlCommand) -> Result<(), CoreExecutorError> {
            self.control_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    let apply_calls = Arc::new(AtomicUsize::new(0));
    let control_calls = Arc::new(AtomicUsize::new(0));
    let apply_started = Arc::new(Notify::new());
    let apply_finished = Arc::new(Notify::new());
    let allow_finish = Arc::new(Notify::new());

    adapter
        .register_session_with_executor(
            session_id.clone(),
            Box::new(BlockingExecutor {
                apply_calls: Arc::clone(&apply_calls),
                control_calls: Arc::clone(&control_calls),
                apply_started: Arc::clone(&apply_started),
                apply_finished: Arc::clone(&apply_finished),
                allow_finish: Arc::clone(&allow_finish),
            }),
        )
        .await;

    let outcome = adapter
        .accept_input_without_wake(
            &session_id,
            make_progress_input("recycle-wait-all-with-loop"),
        )
        .await
        .expect("queued progress input should be accepted without waking the loop");
    assert!(outcome.is_accepted());

    let registry = adapter
        .ops_lifecycle_registry(&session_id)
        .await
        .expect("ops registry should exist for attached session");

    let operation_id = OperationId::new();
    registry
        .register_operation(OperationSpec {
            id: operation_id.clone(),
            kind: OperationKind::BackgroundToolOp,
            owner_session_id: session_id.clone(),
            display_name: "recycle wait target with loop".into(),
            source_label: "meerkat_machine_test".into(),
            child_session_id: None,
            expect_peer_channel: false,
        })
        .expect("operation should register");
    registry
        .provisioning_succeeded(&operation_id)
        .expect("operation should enter running");

    let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

    let before_recycle = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist before recycle");
    let wait_request_id = before_recycle
        .ops
        .wait_request_id
        .clone()
        .expect("wait_all should register an authority-owned wait request");
    assert_eq!(before_recycle.control.phase, RuntimeState::Attached);
    assert!(before_recycle.ops.pending_wait_present);
    assert_eq!(
        before_recycle.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "pending wait carrier should track the same wait request id before recycle"
    );
    assert_eq!(
        before_recycle.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "wait_all should track the active operation before recycle"
    );
    assert_eq!(
        apply_calls.load(Ordering::SeqCst),
        0,
        "queued attached-loop work should remain pending until recycle wakes the loop"
    );
    assert_eq!(
        control_calls.load(Ordering::SeqCst),
        0,
        "recycle should not yet have attempted executor control"
    );

    let runtime_id = runtime_id_for_session(&session_id);
    let report = crate::traits::RuntimeControlPlane::recycle(&*adapter, &runtime_id)
        .await
        .expect("recycle should preserve wait_all while requeueing attached-loop work");
    assert_eq!(report.inputs_transferred, 1);

    tokio::time::timeout(Duration::from_secs(1), apply_started.notified())
        .await
        .expect("recycle should wake the attached runtime loop to replay preserved work");

    let after_recycle = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after recycle wakes the loop");
    assert_eq!(
        after_recycle.control.phase,
        RuntimeState::Running,
        "post-#32 W6-J: recycle from Attached wakes the loop which fires DSL `Prepare` → Running; the apply-in-flight sits at Running until Commit returns (DSL is source of truth, now with run-lifecycle properly DSL-wired)"
    );
    assert_eq!(
        after_recycle.ops.wait_request_id,
        Some(wait_request_id.clone()),
        "recycle should preserve the authority-owned wait request while the attached loop is replaying preserved work"
    );
    assert!(after_recycle.ops.pending_wait_present);
    assert_eq!(
        after_recycle.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "recycle should preserve request-id agreement across the wait carrier seam while replaying"
    );
    assert_eq!(
        after_recycle.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "recycle should preserve the tracked wait target while preserved work is replaying"
    );
    assert_eq!(
        apply_calls.load(Ordering::SeqCst),
        1,
        "recycle should wake the attached loop exactly once for the preserved queued work"
    );
    assert_eq!(
        control_calls.load(Ordering::SeqCst),
        0,
        "recycle should not route any out-of-band control command through the executor seam"
    );

    allow_finish.notify_waiters();
    tokio::time::timeout(Duration::from_secs(1), apply_finished.notified())
        .await
        .expect("attached loop should finish replaying the preserved queued work");

    let after_replay = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let snapshot = adapter
                .meerkat_machine_spine_snapshot(&session_id)
                .await
                .expect("snapshot should exist after attached-loop replay");
            if snapshot.control.phase == RuntimeState::Attached {
                break snapshot;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("runtime should return to Attached once the preserved work finishes replaying");
    assert_eq!(
        after_replay.ops.wait_request_id,
        Some(wait_request_id.clone()),
        "wait_all should remain active after the attached loop finishes replaying preserved work"
    );
    assert!(after_replay.ops.pending_wait_present);
    assert_eq!(
        after_replay.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "request-id agreement should survive the return to Attached after replay"
    );
    assert_eq!(
        after_replay.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "the tracked wait target should remain present until the operation itself settles"
    );

    registry
        .complete_operation(
            &operation_id,
            OperationResult {
                id: operation_id.clone(),
                content: "done".into(),
                is_error: false,
                duration_ms: 1,
                tokens_used: 0,
            },
        )
        .expect("operation should complete after recycle+replay");
    let wait_result = wait_future.await.expect("wait_all should still resolve");
    assert_eq!(wait_result.satisfied.wait_request_id, wait_request_id);
    assert_eq!(
        wait_result.satisfied.operation_ids,
        vec![operation_id.clone()]
    );

    let settled = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after wait_all settles");
    assert_eq!(settled.control.phase, RuntimeState::Attached);
    assert_eq!(settled.ops.wait_request_id, None);
    assert!(!settled.ops.pending_wait_present);
    assert_eq!(settled.ops.pending_wait_request_id, None);
    assert!(settled.ops.wait_operation_ids.is_empty());
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_recycle_with_runtime_loop_splits_completion_and_wait_all_lifetimes()
 {
    struct BlockingExecutor {
        apply_calls: Arc<AtomicUsize>,
        control_calls: Arc<AtomicUsize>,
        apply_started: Arc<Notify>,
        apply_finished: Arc<Notify>,
        allow_finish: Arc<Notify>,
    }

    #[async_trait::async_trait]
    impl CoreExecutor for BlockingExecutor {
        async fn apply(
            &mut self,
            run_id: RunId,
            primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            self.apply_calls.fetch_add(1, Ordering::SeqCst);
            self.apply_started.notify_waiters();
            self.allow_finish.notified().await;
            self.apply_finished.notify_waiters();

            Ok(CoreApplyOutput {
                receipt: RunBoundaryReceipt {
                    run_id,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                    sequence: 0,
                },
                session_snapshot: None,
                terminal: None,
            })
        }

        async fn control(&mut self, _command: RunControlCommand) -> Result<(), CoreExecutorError> {
            self.control_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    let apply_calls = Arc::new(AtomicUsize::new(0));
    let control_calls = Arc::new(AtomicUsize::new(0));
    let apply_started = Arc::new(Notify::new());
    let apply_finished = Arc::new(Notify::new());
    let allow_finish = Arc::new(Notify::new());

    adapter
        .register_session_with_executor(
            session_id.clone(),
            Box::new(BlockingExecutor {
                apply_calls: Arc::clone(&apply_calls),
                control_calls: Arc::clone(&control_calls),
                apply_started: Arc::clone(&apply_started),
                apply_finished: Arc::clone(&apply_finished),
                allow_finish: Arc::clone(&allow_finish),
            }),
        )
        .await;

    let input = make_progress_input("recycle-with-loop-split-lifetimes");
    let input_id = input.id().clone();
    let (outcome, completion_handle) = adapter
        .accept_input_with_completion(&session_id, input)
        .await
        .expect("progress input should be accepted");
    assert!(outcome.is_accepted());
    let completion_handle =
        completion_handle.expect("queued progress input should expose a completion waiter");

    let registry = adapter
        .ops_lifecycle_registry(&session_id)
        .await
        .expect("ops registry should exist for attached session");

    let operation_id = OperationId::new();
    registry
        .register_operation(OperationSpec {
            id: operation_id.clone(),
            kind: OperationKind::BackgroundToolOp,
            owner_session_id: session_id.clone(),
            display_name: "recycle split wait target".into(),
            source_label: "meerkat_machine_test".into(),
            child_session_id: None,
            expect_peer_channel: false,
        })
        .expect("operation should register");
    registry
        .provisioning_succeeded(&operation_id)
        .expect("operation should enter running");

    let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

    let before_recycle = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist before recycle");
    let wait_request_id = before_recycle
        .ops
        .wait_request_id
        .clone()
        .expect("wait_all should register an authority-owned wait request");
    assert_eq!(before_recycle.control.phase, RuntimeState::Attached);
    assert_eq!(before_recycle.completion_waiters.input_count, 1);
    assert_eq!(before_recycle.completion_waiters.waiter_count, 1);
    assert_eq!(before_recycle.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        before_recycle.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert!(before_recycle.ops.pending_wait_present);
    assert_eq!(
        before_recycle.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "pending wait carrier should track the same wait request id before recycle"
    );
    assert_eq!(
        before_recycle.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "wait_all should track the active operation before recycle"
    );
    assert_eq!(
        apply_calls.load(Ordering::SeqCst),
        0,
        "queued attached-loop work should remain pending until recycle wakes the loop"
    );
    assert_eq!(
        control_calls.load(Ordering::SeqCst),
        0,
        "recycle should not yet have attempted executor control"
    );

    let runtime_id = runtime_id_for_session(&session_id);
    let report = crate::traits::RuntimeControlPlane::recycle(&*adapter, &runtime_id)
        .await
        .expect("recycle should split completion and wait_all lifetimes on attached runtimes");
    assert_eq!(report.inputs_transferred, 1);

    tokio::time::timeout(Duration::from_secs(1), apply_started.notified())
        .await
        .expect("recycle should wake the attached runtime loop to replay preserved work");

    let during_recycle = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist while recycle replay is in flight");
    assert_eq!(
        during_recycle.control.phase,
        RuntimeState::Running,
        "post-#32 W6-J: recycle from Attached starts the loop which fires DSL `Prepare` → Running; the apply-in-flight sits at Running during replay until Commit returns (DSL is source of truth, now with run-lifecycle properly DSL-wired)"
    );
    assert_eq!(during_recycle.completion_waiters.input_count, 1);
    assert_eq!(during_recycle.completion_waiters.waiter_count, 1);
    assert_eq!(during_recycle.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        during_recycle.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert_eq!(
        during_recycle.ops.wait_request_id,
        Some(wait_request_id.clone()),
        "recycle should preserve the authority-owned wait request while replay is in flight"
    );
    assert!(during_recycle.ops.pending_wait_present);
    assert_eq!(
        during_recycle.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "recycle should preserve request-id agreement across the wait carrier seam while replaying"
    );
    assert_eq!(
        during_recycle.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "recycle should preserve the tracked wait target while preserved work is replaying"
    );
    assert_eq!(
        apply_calls.load(Ordering::SeqCst),
        1,
        "recycle should wake the attached loop exactly once for the preserved queued work"
    );
    assert_eq!(
        control_calls.load(Ordering::SeqCst),
        0,
        "recycle should not route any out-of-band control command through the executor seam"
    );

    allow_finish.notify_waiters();
    tokio::time::timeout(Duration::from_secs(1), apply_finished.notified())
        .await
        .expect("attached loop should finish replaying the preserved queued work");

    match completion_handle.wait().await {
        CompletionOutcome::CompletedWithoutResult => {}
        other => panic!("expected recycle+replay to complete queued work, got {other:?}"),
    }

    let after_replay = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let snapshot = adapter
                .meerkat_machine_spine_snapshot(&session_id)
                .await
                .expect("snapshot should exist after attached-loop replay");
            if snapshot.control.phase == RuntimeState::Attached {
                break snapshot;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("runtime should return to Attached once the preserved work finishes replaying");
    assert_eq!(after_replay.completion_waiters.input_count, 0);
    assert_eq!(after_replay.completion_waiters.waiter_count, 0);
    assert!(
        after_replay.completion_waiters.waiting_inputs.is_empty(),
        "recycle should clear completion waiters once the preserved work finishes replaying"
    );
    assert!(
        after_replay.inputs.queue.is_empty(),
        "recycle should not leave ordinary queued work behind once the attached runtime returns to Attached after replay"
    );
    assert!(
        after_replay.inputs.steer_queue.is_empty(),
        "recycle should not leave steer-queued work behind once the attached runtime returns to Attached after replay"
    );
    assert_eq!(
        after_replay.ops.wait_request_id,
        Some(wait_request_id.clone()),
        "wait_all should remain active after the preserved work finishes replaying"
    );
    assert!(after_replay.ops.pending_wait_present);
    assert_eq!(
        after_replay.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "request-id agreement should survive the return to Attached after replay"
    );
    assert_eq!(
        after_replay.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "the tracked wait target should remain present until the operation itself settles"
    );

    registry
        .complete_operation(
            &operation_id,
            OperationResult {
                id: operation_id.clone(),
                content: "done".into(),
                is_error: false,
                duration_ms: 1,
                tokens_used: 0,
            },
        )
        .expect("operation should complete after recycle+replay");
    let wait_result = wait_future.await.expect("wait_all should still resolve");
    assert_eq!(wait_result.satisfied.wait_request_id, wait_request_id);
    assert_eq!(
        wait_result.satisfied.operation_ids,
        vec![operation_id.clone()]
    );

    let settled = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after wait_all settles");
    assert_eq!(settled.control.phase, RuntimeState::Attached);
    assert!(
        settled.inputs.queue.is_empty(),
        "recycle should keep the attached settled snapshot free of ordinary queued work after replay completes"
    );
    assert!(
        settled.inputs.steer_queue.is_empty(),
        "recycle should keep the attached settled snapshot free of steer-queued work after replay completes"
    );
    assert_eq!(settled.ops.wait_request_id, None);
    assert!(!settled.ops.pending_wait_present);
    assert_eq!(settled.ops.pending_wait_request_id, None);
    assert!(settled.ops.wait_operation_ids.is_empty());
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_preserves_wait_all_after_reset() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;
    let registry = adapter
        .ops_lifecycle_registry(&session_id)
        .await
        .expect("ops registry should exist for registered session");

    let operation_id = OperationId::new();
    registry
        .register_operation(OperationSpec {
            id: operation_id.clone(),
            kind: OperationKind::BackgroundToolOp,
            owner_session_id: session_id.clone(),
            display_name: "reset wait target".into(),
            source_label: "meerkat_machine_test".into(),
            child_session_id: None,
            expect_peer_channel: false,
        })
        .expect("operation should register");
    registry
        .provisioning_succeeded(&operation_id)
        .expect("operation should enter running");

    let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

    let before_reset = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist before reset");
    let wait_request_id = before_reset
        .ops
        .wait_request_id
        .clone()
        .expect("wait_all should register an authority-owned wait request");
    assert!(before_reset.ops.pending_wait_present);
    assert_eq!(
        before_reset.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "pending wait carrier should track the same wait request id before reset"
    );
    assert_eq!(
        before_reset.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "wait_all should track the active operation before reset"
    );

    SessionServiceRuntimeExt::reset_runtime(&*adapter, &session_id)
        .await
        .expect("reset should succeed while the runtime is idle");

    let after_reset = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after reset");
    assert_eq!(
        after_reset.ops.wait_request_id,
        Some(wait_request_id.clone()),
        "reset should preserve the authority-owned wait request"
    );
    assert!(
        after_reset.ops.pending_wait_present,
        "reset should preserve the pending wait carrier while the operation remains active"
    );
    assert_eq!(
        after_reset.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "reset should preserve request-id agreement across the wait carrier seam"
    );
    assert_eq!(
        after_reset.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "reset should preserve the tracked wait targets"
    );

    registry
        .complete_operation(
            &operation_id,
            OperationResult {
                id: operation_id.clone(),
                content: "done".into(),
                is_error: false,
                duration_ms: 1,
                tokens_used: 0,
            },
        )
        .expect("operation should complete after reset");
    let wait_result = wait_future.await.expect("wait_all should still resolve");
    assert_eq!(wait_result.satisfied.wait_request_id, wait_request_id);
    assert_eq!(
        wait_result.satisfied.operation_ids,
        vec![operation_id.clone()]
    );

    let settled = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after wait_all settles");
    assert_eq!(settled.ops.wait_request_id, None);
    assert!(!settled.ops.pending_wait_present);
    assert_eq!(settled.ops.pending_wait_request_id, None);
    assert!(settled.ops.wait_operation_ids.is_empty());
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_reset_clears_steered_waiter_and_queue_but_preserves_wait_all()
 {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;

    let input = Input::Prompt(crate::input::PromptInput::new(
        "reset steered prompt",
        Some(
            meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                handling_mode: Some(meerkat_core::types::HandlingMode::Steer),
                ..Default::default()
            },
        ),
    ));
    let input_id = input.id().clone();
    let (_outcome, completion_handle) = adapter
        .accept_input_with_completion(&session_id, input)
        .await
        .expect("steered prompt should be accepted");
    let completion_handle =
        completion_handle.expect("steered prompt should register a completion waiter");

    let registry = adapter
        .ops_lifecycle_registry(&session_id)
        .await
        .expect("ops registry should exist for registered session");

    let operation_id = OperationId::new();
    registry
        .register_operation(OperationSpec {
            id: operation_id.clone(),
            kind: OperationKind::BackgroundToolOp,
            owner_session_id: session_id.clone(),
            display_name: "reset steered wait target".into(),
            source_label: "meerkat_machine_test".into(),
            child_session_id: None,
            expect_peer_channel: false,
        })
        .expect("operation should register");
    registry
        .provisioning_succeeded(&operation_id)
        .expect("operation should enter running");

    let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

    let before_reset = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist before reset");
    let wait_request_id = before_reset
        .ops
        .wait_request_id
        .clone()
        .expect("wait_all should register an authority-owned wait request");
    assert_eq!(before_reset.control.phase, RuntimeState::Idle);
    assert!(before_reset.inputs.queue.is_empty());
    assert_eq!(before_reset.inputs.steer_queue, vec![input_id.clone()]);
    assert_eq!(before_reset.completion_waiters.input_count, 1);
    assert_eq!(before_reset.completion_waiters.waiter_count, 1);
    assert_eq!(before_reset.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        before_reset.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert!(before_reset.ops.pending_wait_present);
    assert_eq!(
        before_reset.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "pending wait carrier should track the same wait request id before reset"
    );
    assert_eq!(
        before_reset.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "wait_all should track the active operation before reset"
    );

    let report = SessionServiceRuntimeExt::reset_runtime(&*adapter, &session_id)
        .await
        .expect("reset should clear steered completion waiters while preserving wait_all");
    assert_eq!(report.inputs_abandoned, 1);

    match completion_handle.wait().await {
        CompletionOutcome::RuntimeTerminated(reason) => {
            assert_eq!(reason, "runtime reset");
        }
        other => {
            panic!("expected runtime reset termination for steered input, got {other:?}")
        }
    }

    let after_reset = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after reset");
    assert_eq!(after_reset.control.phase, RuntimeState::Idle);
    assert_eq!(after_reset.control.current_run_id, None);
    assert_eq!(after_reset.inputs.current_run_id, None);
    assert!(after_reset.inputs.queue.is_empty());
    assert!(
        after_reset.inputs.steer_queue.is_empty(),
        "reset should clear steered queued work immediately on the plain runtime path"
    );
    assert_eq!(after_reset.completion_waiters.input_count, 0);
    assert_eq!(after_reset.completion_waiters.waiter_count, 0);
    assert!(
        after_reset.completion_waiters.waiting_inputs.is_empty(),
        "reset should clear input-owned steered completion waiters immediately"
    );
    assert_eq!(
        after_reset.ops.wait_request_id,
        Some(wait_request_id.clone()),
        "reset should preserve the authority-owned wait request after steered completion waiters clear"
    );
    assert!(after_reset.ops.pending_wait_present);
    assert_eq!(
        after_reset.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "reset should preserve request-id agreement across the wait carrier seam"
    );
    assert_eq!(
        after_reset.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "reset should preserve the tracked wait target until the operation settles"
    );

    registry
        .complete_operation(
            &operation_id,
            OperationResult {
                id: operation_id.clone(),
                content: "done".into(),
                is_error: false,
                duration_ms: 1,
                tokens_used: 0,
            },
        )
        .expect("operation should complete after reset");
    let wait_result = wait_future.await.expect("wait_all should still resolve");
    assert_eq!(wait_result.satisfied.wait_request_id, wait_request_id);
    assert_eq!(
        wait_result.satisfied.operation_ids,
        vec![operation_id.clone()]
    );

    let settled = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after wait_all settles");
    assert_eq!(settled.control.phase, RuntimeState::Idle);
    assert_eq!(settled.control.current_run_id, None);
    assert_eq!(settled.inputs.current_run_id, None);
    assert!(settled.inputs.queue.is_empty());
    assert!(settled.inputs.steer_queue.is_empty());
    assert_eq!(settled.ops.wait_request_id, None);
    assert!(!settled.ops.pending_wait_present);
    assert_eq!(settled.ops.pending_wait_request_id, None);
    assert!(settled.ops.wait_operation_ids.is_empty());
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_reset_splits_completion_and_wait_all_lifetimes() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;

    let input = make_prompt("reset split lifetimes");
    let input_id = input.id().clone();
    let (_outcome, completion_handle) = adapter
        .accept_input_with_completion(&session_id, input)
        .await
        .expect("prompt should be accepted");
    let completion_handle =
        completion_handle.expect("queued prompt should register a completion waiter");

    let registry = adapter
        .ops_lifecycle_registry(&session_id)
        .await
        .expect("ops registry should exist for registered session");

    let operation_id = OperationId::new();
    registry
        .register_operation(OperationSpec {
            id: operation_id.clone(),
            kind: OperationKind::BackgroundToolOp,
            owner_session_id: session_id.clone(),
            display_name: "reset split wait target".into(),
            source_label: "meerkat_machine_test".into(),
            child_session_id: None,
            expect_peer_channel: false,
        })
        .expect("operation should register");
    registry
        .provisioning_succeeded(&operation_id)
        .expect("operation should enter running");

    let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

    let before_reset = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist before reset");
    let wait_request_id = before_reset
        .ops
        .wait_request_id
        .clone()
        .expect("wait_all should register an authority-owned wait request");
    assert_eq!(before_reset.control.phase, RuntimeState::Idle);
    assert_eq!(before_reset.completion_waiters.input_count, 1);
    assert_eq!(before_reset.completion_waiters.waiter_count, 1);
    assert_eq!(before_reset.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        before_reset.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert!(before_reset.ops.pending_wait_present);
    assert_eq!(
        before_reset.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "pending wait carrier should track the same wait request id before reset"
    );
    assert_eq!(
        before_reset.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "wait_all should track the active operation before reset"
    );

    let report = SessionServiceRuntimeExt::reset_runtime(&*adapter, &session_id)
        .await
        .expect("reset should split completion and wait_all lifetimes");
    assert_eq!(report.inputs_abandoned, 1);

    match completion_handle.wait().await {
        CompletionOutcome::RuntimeTerminated(reason) => {
            assert_eq!(reason, "runtime reset");
        }
        other => panic!("expected runtime reset termination, got {other:?}"),
    }

    let after_reset = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after reset");
    assert_eq!(after_reset.control.phase, RuntimeState::Idle);
    assert!(
        after_reset.inputs.queue.is_empty(),
        "reset should abandon ordinary queued work immediately on the plain runtime path"
    );
    assert!(
        after_reset.inputs.steer_queue.is_empty(),
        "reset should abandon steered queued work immediately on the plain runtime path"
    );
    assert_eq!(after_reset.completion_waiters.input_count, 0);
    assert_eq!(after_reset.completion_waiters.waiter_count, 0);
    assert!(
        after_reset.completion_waiters.waiting_inputs.is_empty(),
        "reset should clear input-owned completion waiters immediately"
    );
    assert_eq!(
        after_reset.ops.wait_request_id,
        Some(wait_request_id.clone()),
        "reset should preserve the authority-owned wait request after completion waiters clear"
    );
    assert!(after_reset.ops.pending_wait_present);
    assert_eq!(
        after_reset.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "reset should preserve request-id agreement across the wait carrier seam"
    );
    assert_eq!(
        after_reset.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "reset should preserve the tracked wait target until the operation settles"
    );

    registry
        .complete_operation(
            &operation_id,
            OperationResult {
                id: operation_id.clone(),
                content: "done".into(),
                is_error: false,
                duration_ms: 1,
                tokens_used: 0,
            },
        )
        .expect("operation should complete after reset");
    let wait_result = wait_future.await.expect("wait_all should still resolve");
    assert_eq!(wait_result.satisfied.wait_request_id, wait_request_id);
    assert_eq!(
        wait_result.satisfied.operation_ids,
        vec![operation_id.clone()]
    );

    let settled = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after wait_all settles");
    assert_eq!(settled.control.phase, RuntimeState::Idle);
    assert!(
        settled.inputs.queue.is_empty(),
        "reset should not reintroduce ordinary queued work once the plain runtime settles"
    );
    assert!(
        settled.inputs.steer_queue.is_empty(),
        "reset should not reintroduce steered queued work once the plain runtime settles"
    );
    assert_eq!(
        settled.control.current_run_id, None,
        "reset should not leave a settled control-side current-run binding behind"
    );
    assert_eq!(
        settled.inputs.current_run_id, None,
        "reset should not leave a settled ingress-side current-run binding behind"
    );
    assert_eq!(settled.ops.wait_request_id, None);
    assert!(!settled.ops.pending_wait_present);
    assert_eq!(settled.ops.pending_wait_request_id, None);
    assert!(settled.ops.wait_operation_ids.is_empty());
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_preserves_wait_all_after_reset_with_runtime_loop() {
    struct RecordingExecutor {
        apply_calls: Arc<AtomicUsize>,
        control_calls: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl CoreExecutor for RecordingExecutor {
        async fn apply(
            &mut self,
            run_id: RunId,
            primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            self.apply_calls.fetch_add(1, Ordering::SeqCst);
            Ok(CoreApplyOutput {
                receipt: RunBoundaryReceipt {
                    run_id,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                    sequence: 0,
                },
                session_snapshot: None,
                terminal: None,
            })
        }

        async fn control(&mut self, _command: RunControlCommand) -> Result<(), CoreExecutorError> {
            self.control_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    let apply_calls = Arc::new(AtomicUsize::new(0));
    let control_calls = Arc::new(AtomicUsize::new(0));

    adapter
        .register_session_with_executor(
            session_id.clone(),
            Box::new(RecordingExecutor {
                apply_calls: Arc::clone(&apply_calls),
                control_calls: Arc::clone(&control_calls),
            }),
        )
        .await;

    let outcome = adapter
        .accept_input_without_wake(&session_id, make_progress_input("reset-wait-all-with-loop"))
        .await
        .expect("queued progress input should be accepted without waking the loop");
    assert!(outcome.is_accepted());

    let registry = adapter
        .ops_lifecycle_registry(&session_id)
        .await
        .expect("ops registry should exist for attached session");

    let operation_id = OperationId::new();
    registry
        .register_operation(OperationSpec {
            id: operation_id.clone(),
            kind: OperationKind::BackgroundToolOp,
            owner_session_id: session_id.clone(),
            display_name: "reset wait target with loop".into(),
            source_label: "meerkat_machine_test".into(),
            child_session_id: None,
            expect_peer_channel: false,
        })
        .expect("operation should register");
    registry
        .provisioning_succeeded(&operation_id)
        .expect("operation should enter running");

    let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

    let before_reset = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist before reset");
    let wait_request_id = before_reset
        .ops
        .wait_request_id
        .clone()
        .expect("wait_all should register an authority-owned wait request");
    assert_eq!(before_reset.control.phase, RuntimeState::Attached);
    assert!(before_reset.ops.pending_wait_present);
    assert_eq!(
        before_reset.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "pending wait carrier should track the same wait request id before reset"
    );
    assert_eq!(
        before_reset.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "wait_all should track the active operation before reset"
    );
    assert_eq!(
        apply_calls.load(Ordering::SeqCst),
        0,
        "reset should be able to discard queued attached-loop work before apply runs"
    );
    assert_eq!(
        control_calls.load(Ordering::SeqCst),
        0,
        "reset has not yet attempted any executor control"
    );

    let report = SessionServiceRuntimeExt::reset_runtime(&*adapter, &session_id)
        .await
        .expect("reset should preserve wait_all while abandoning queued attached-loop work");
    assert_eq!(report.inputs_abandoned, 1);

    let after_reset = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after reset");
    assert_eq!(after_reset.control.phase, RuntimeState::Idle);
    assert!(
        after_reset.inputs.queue.is_empty(),
        "attached reset should abandon ordinary queued work immediately"
    );
    assert!(
        after_reset.inputs.steer_queue.is_empty(),
        "attached reset should abandon steered queued work immediately"
    );
    assert_eq!(
        after_reset.ops.wait_request_id,
        Some(wait_request_id.clone()),
        "reset should preserve the authority-owned wait request even with an attached loop"
    );
    assert!(
        after_reset.ops.pending_wait_present,
        "reset should preserve the pending wait carrier while the operation remains active"
    );
    assert_eq!(
        after_reset.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "reset should preserve request-id agreement across the wait carrier seam"
    );
    assert_eq!(
        after_reset.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "reset should preserve the tracked wait targets until the operation settles"
    );
    assert_eq!(
        apply_calls.load(Ordering::SeqCst),
        0,
        "reset should bypass queued attached-loop work entirely"
    );
    assert_eq!(
        control_calls.load(Ordering::SeqCst),
        0,
        "reset currently bypasses the executor control seam and does not deliver an out-of-band control command"
    );

    registry
        .complete_operation(
            &operation_id,
            OperationResult {
                id: operation_id.clone(),
                content: "done".into(),
                is_error: false,
                duration_ms: 1,
                tokens_used: 0,
            },
        )
        .expect("operation should complete after reset");
    let wait_result = wait_future.await.expect("wait_all should still resolve");
    assert_eq!(wait_result.satisfied.wait_request_id, wait_request_id);
    assert_eq!(
        wait_result.satisfied.operation_ids,
        vec![operation_id.clone()]
    );

    let settled = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after wait_all settles");
    assert_eq!(settled.control.phase, RuntimeState::Idle);
    assert!(
        settled.inputs.queue.is_empty(),
        "attached reset should not reintroduce ordinary queued work once the runtime settles"
    );
    assert!(
        settled.inputs.steer_queue.is_empty(),
        "attached reset should not reintroduce steered queued work once the runtime settles"
    );
    assert_eq!(
        settled.control.current_run_id, None,
        "reset should not leave a settled control-side current-run binding behind even on attached runtimes"
    );
    assert_eq!(
        settled.inputs.current_run_id, None,
        "reset should not leave a settled ingress-side current-run binding behind even on attached runtimes"
    );
    assert_eq!(settled.ops.wait_request_id, None);
    assert!(!settled.ops.pending_wait_present);
    assert_eq!(settled.ops.pending_wait_request_id, None);
    assert!(settled.ops.wait_operation_ids.is_empty());
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_reset_with_runtime_loop_splits_completion_and_wait_all_lifetimes()
 {
    struct RecordingExecutor {
        apply_calls: Arc<AtomicUsize>,
        control_calls: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl CoreExecutor for RecordingExecutor {
        async fn apply(
            &mut self,
            run_id: RunId,
            primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            self.apply_calls.fetch_add(1, Ordering::SeqCst);
            Ok(CoreApplyOutput {
                receipt: RunBoundaryReceipt {
                    run_id,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                    sequence: 0,
                },
                session_snapshot: None,
                terminal: None,
            })
        }

        async fn control(&mut self, _command: RunControlCommand) -> Result<(), CoreExecutorError> {
            self.control_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    let apply_calls = Arc::new(AtomicUsize::new(0));
    let control_calls = Arc::new(AtomicUsize::new(0));

    adapter
        .register_session_with_executor(
            session_id.clone(),
            Box::new(RecordingExecutor {
                apply_calls: Arc::clone(&apply_calls),
                control_calls: Arc::clone(&control_calls),
            }),
        )
        .await;

    let input = make_progress_input("reset-with-loop-split-lifetimes");
    let input_id = input.id().clone();
    let (outcome, completion_handle) = adapter
        .accept_input_with_completion(&session_id, input)
        .await
        .expect("progress input should be accepted");
    assert!(outcome.is_accepted());
    let completion_handle =
        completion_handle.expect("queued progress input should expose a completion waiter");

    let registry = adapter
        .ops_lifecycle_registry(&session_id)
        .await
        .expect("ops registry should exist for attached session");

    let operation_id = OperationId::new();
    registry
        .register_operation(OperationSpec {
            id: operation_id.clone(),
            kind: OperationKind::BackgroundToolOp,
            owner_session_id: session_id.clone(),
            display_name: "reset split-lifetime wait target".into(),
            source_label: "meerkat_machine_test".into(),
            child_session_id: None,
            expect_peer_channel: false,
        })
        .expect("operation should register");
    registry
        .provisioning_succeeded(&operation_id)
        .expect("operation should enter running");

    let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

    let before_reset = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist before reset");
    let wait_request_id = before_reset
        .ops
        .wait_request_id
        .clone()
        .expect("wait_all should register an authority-owned wait request");
    assert_eq!(before_reset.control.phase, RuntimeState::Attached);
    assert_eq!(before_reset.completion_waiters.input_count, 1);
    assert_eq!(before_reset.completion_waiters.waiter_count, 1);
    assert_eq!(before_reset.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        before_reset.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert_eq!(
        before_reset.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "pending wait carrier should track the same wait request id before reset"
    );
    assert_eq!(
        before_reset.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "wait_all should track the active operation before reset"
    );
    assert_eq!(
        apply_calls.load(Ordering::SeqCst),
        0,
        "reset should be able to discard queued attached-loop work before apply runs"
    );
    assert_eq!(
        control_calls.load(Ordering::SeqCst),
        0,
        "reset has not yet attempted any executor control"
    );

    let report = SessionServiceRuntimeExt::reset_runtime(&*adapter, &session_id)
        .await
        .expect("reset should preserve wait_all while abandoning queued attached-loop work");
    assert_eq!(report.inputs_abandoned, 1);

    let after_reset = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after reset");
    assert_eq!(after_reset.control.phase, RuntimeState::Idle);
    assert_eq!(after_reset.completion_waiters.input_count, 0);
    assert_eq!(after_reset.completion_waiters.waiter_count, 0);
    assert!(
        after_reset.completion_waiters.waiting_inputs.is_empty(),
        "reset should clear input-owned completion waiters immediately"
    );
    assert_eq!(
        after_reset.ops.wait_request_id,
        Some(wait_request_id.clone()),
        "reset should preserve the authority-owned wait request even after clearing input waiters"
    );
    assert!(after_reset.ops.pending_wait_present);
    assert_eq!(
        after_reset.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "reset should preserve request-id agreement across the wait carrier seam"
    );
    assert_eq!(
        after_reset.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "reset should preserve the tracked wait target until the operation settles"
    );
    assert_eq!(
        apply_calls.load(Ordering::SeqCst),
        0,
        "reset should bypass queued attached-loop work entirely"
    );
    assert_eq!(
        control_calls.load(Ordering::SeqCst),
        0,
        "reset currently bypasses the executor control seam and does not deliver an out-of-band control command"
    );

    match completion_handle.wait().await {
        CompletionOutcome::RuntimeTerminated(reason) => {
            assert_eq!(reason, "runtime reset");
        }
        other => {
            panic!("expected reset to terminate the completion waiter immediately, got {other:?}")
        }
    }

    registry
        .complete_operation(
            &operation_id,
            OperationResult {
                id: operation_id.clone(),
                content: "done".into(),
                is_error: false,
                duration_ms: 1,
                tokens_used: 0,
            },
        )
        .expect("operation should complete after reset");
    let wait_result = wait_future.await.expect("wait_all should still resolve");
    assert_eq!(wait_result.satisfied.wait_request_id, wait_request_id);
    assert_eq!(
        wait_result.satisfied.operation_ids,
        vec![operation_id.clone()]
    );

    let settled = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after wait_all settles");
    assert_eq!(settled.control.phase, RuntimeState::Idle);
    assert_eq!(settled.ops.wait_request_id, None);
    assert!(!settled.ops.pending_wait_present);
    assert_eq!(settled.ops.pending_wait_request_id, None);
    assert!(settled.ops.wait_operation_ids.is_empty());
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_preserves_wait_all_after_destroy() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;
    let registry = adapter
        .ops_lifecycle_registry(&session_id)
        .await
        .expect("ops registry should exist for registered session");

    let operation_id = OperationId::new();
    registry
        .register_operation(OperationSpec {
            id: operation_id.clone(),
            kind: OperationKind::BackgroundToolOp,
            owner_session_id: session_id.clone(),
            display_name: "destroy wait target".into(),
            source_label: "meerkat_machine_test".into(),
            child_session_id: None,
            expect_peer_channel: false,
        })
        .expect("operation should register");
    registry
        .provisioning_succeeded(&operation_id)
        .expect("operation should enter running");

    let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

    let before_destroy = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist before destroy");
    let wait_request_id = before_destroy
        .ops
        .wait_request_id
        .clone()
        .expect("wait_all should register an authority-owned wait request");
    assert!(before_destroy.ops.pending_wait_present);
    assert_eq!(
        before_destroy.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "pending wait carrier should track the same wait request id before destroy"
    );
    assert_eq!(
        before_destroy.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "wait_all should track the active operation before destroy"
    );

    let runtime_id = runtime_id_for_session(&session_id);
    let report = crate::traits::RuntimeControlPlane::destroy(&*adapter, &runtime_id)
        .await
        .expect("destroy should succeed for an idle runtime");
    assert_eq!(report.inputs_abandoned, 0);

    let after_destroy = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after destroy");
    assert_eq!(after_destroy.control.phase, RuntimeState::Destroyed);
    assert_eq!(
        after_destroy.ops.wait_request_id,
        Some(wait_request_id.clone()),
        "destroy currently preserves the authority-owned wait request"
    );
    assert!(
        after_destroy.ops.pending_wait_present,
        "destroy currently preserves the pending wait carrier while the operation remains active"
    );
    assert_eq!(
        after_destroy.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "destroy should preserve request-id agreement across the wait carrier seam"
    );
    assert_eq!(
        after_destroy.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "destroy should preserve the tracked wait targets until the operation settles"
    );

    registry
        .complete_operation(
            &operation_id,
            OperationResult {
                id: operation_id.clone(),
                content: "done".into(),
                is_error: false,
                duration_ms: 1,
                tokens_used: 0,
            },
        )
        .expect("operation should complete after destroy");
    let wait_result = wait_future.await.expect("wait_all should still resolve");
    assert_eq!(wait_result.satisfied.wait_request_id, wait_request_id);
    assert_eq!(
        wait_result.satisfied.operation_ids,
        vec![operation_id.clone()]
    );

    let settled = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after wait_all settles");
    assert_eq!(settled.control.phase, RuntimeState::Destroyed);
    assert_eq!(
        settled.control.current_run_id, None,
        "destroy should not leave a settled control-side current-run binding behind"
    );
    assert_eq!(
        settled.inputs.current_run_id, None,
        "destroy should not leave a settled ingress-side current-run binding behind"
    );
    assert_eq!(settled.ops.wait_request_id, None);
    assert!(!settled.ops.pending_wait_present);
    assert_eq!(settled.ops.pending_wait_request_id, None);
    assert!(settled.ops.wait_operation_ids.is_empty());
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_destroy_splits_completion_and_wait_all_lifetimes() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;

    let input = make_prompt("destroy split lifetimes");
    let input_id = input.id().clone();
    let (_outcome, completion_handle) = adapter
        .accept_input_with_completion(&session_id, input)
        .await
        .expect("prompt should be accepted");
    let completion_handle =
        completion_handle.expect("queued prompt should register a completion waiter");

    let registry = adapter
        .ops_lifecycle_registry(&session_id)
        .await
        .expect("ops registry should exist for registered session");

    let operation_id = OperationId::new();
    registry
        .register_operation(OperationSpec {
            id: operation_id.clone(),
            kind: OperationKind::BackgroundToolOp,
            owner_session_id: session_id.clone(),
            display_name: "destroy split wait target".into(),
            source_label: "meerkat_machine_test".into(),
            child_session_id: None,
            expect_peer_channel: false,
        })
        .expect("operation should register");
    registry
        .provisioning_succeeded(&operation_id)
        .expect("operation should enter running");

    let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

    let before_destroy = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist before destroy");
    let wait_request_id = before_destroy
        .ops
        .wait_request_id
        .clone()
        .expect("wait_all should register an authority-owned wait request");
    assert_eq!(before_destroy.control.phase, RuntimeState::Idle);
    assert_eq!(before_destroy.completion_waiters.input_count, 1);
    assert_eq!(before_destroy.completion_waiters.waiter_count, 1);
    assert_eq!(before_destroy.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        before_destroy.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert!(before_destroy.ops.pending_wait_present);
    assert_eq!(
        before_destroy.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "pending wait carrier should track the same wait request id before destroy"
    );
    assert_eq!(
        before_destroy.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "wait_all should track the active operation before destroy"
    );

    let runtime_id = runtime_id_for_session(&session_id);
    let report = crate::traits::RuntimeControlPlane::destroy(&*adapter, &runtime_id)
        .await
        .expect("destroy should split completion and wait_all lifetimes");
    assert_eq!(report.inputs_abandoned, 1);

    match completion_handle.wait().await {
        CompletionOutcome::RuntimeTerminated(reason) => {
            assert_eq!(reason, "runtime destroyed");
        }
        other => panic!("expected runtime destroyed termination, got {other:?}"),
    }

    let after_destroy = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after destroy");
    assert_eq!(after_destroy.control.phase, RuntimeState::Destroyed);
    assert_eq!(after_destroy.completion_waiters.input_count, 0);
    assert_eq!(after_destroy.completion_waiters.waiter_count, 0);
    assert!(
        after_destroy.completion_waiters.waiting_inputs.is_empty(),
        "destroy should clear input-owned completion waiters immediately"
    );
    assert_eq!(
        after_destroy.ops.wait_request_id,
        Some(wait_request_id.clone()),
        "destroy should preserve the authority-owned wait request after completion waiters clear"
    );
    assert!(after_destroy.ops.pending_wait_present);
    assert_eq!(
        after_destroy.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "destroy should preserve request-id agreement across the wait carrier seam"
    );
    assert_eq!(
        after_destroy.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "destroy should preserve the tracked wait target until the operation settles"
    );

    registry
        .complete_operation(
            &operation_id,
            OperationResult {
                id: operation_id.clone(),
                content: "done".into(),
                is_error: false,
                duration_ms: 1,
                tokens_used: 0,
            },
        )
        .expect("operation should complete after destroy");
    let wait_result = wait_future.await.expect("wait_all should still resolve");
    assert_eq!(wait_result.satisfied.wait_request_id, wait_request_id);
    assert_eq!(
        wait_result.satisfied.operation_ids,
        vec![operation_id.clone()]
    );

    let settled = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after wait_all settles");
    assert_eq!(settled.control.phase, RuntimeState::Destroyed);
    assert_eq!(
        settled.control.current_run_id, None,
        "destroy should not leave a settled control-side current-run binding behind even on attached runtimes"
    );
    assert_eq!(
        settled.inputs.current_run_id, None,
        "destroy should not leave a settled ingress-side current-run binding behind even on attached runtimes"
    );
    assert_eq!(settled.ops.wait_request_id, None);
    assert!(!settled.ops.pending_wait_present);
    assert_eq!(settled.ops.pending_wait_request_id, None);
    assert!(settled.ops.wait_operation_ids.is_empty());
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_preserves_wait_all_after_destroy_with_runtime_loop() {
    struct RecordingExecutor {
        apply_calls: Arc<AtomicUsize>,
        control_calls: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl CoreExecutor for RecordingExecutor {
        async fn apply(
            &mut self,
            run_id: RunId,
            primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            self.apply_calls.fetch_add(1, Ordering::SeqCst);
            Ok(CoreApplyOutput {
                receipt: RunBoundaryReceipt {
                    run_id,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                    sequence: 0,
                },
                session_snapshot: None,
                terminal: None,
            })
        }

        async fn control(&mut self, _command: RunControlCommand) -> Result<(), CoreExecutorError> {
            self.control_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    let apply_calls = Arc::new(AtomicUsize::new(0));
    let control_calls = Arc::new(AtomicUsize::new(0));

    adapter
        .register_session_with_executor(
            session_id.clone(),
            Box::new(RecordingExecutor {
                apply_calls: Arc::clone(&apply_calls),
                control_calls: Arc::clone(&control_calls),
            }),
        )
        .await;

    let outcome = adapter
        .accept_input_without_wake(
            &session_id,
            make_progress_input("destroy-wait-all-with-loop"),
        )
        .await
        .expect("queued progress input should be accepted without waking the loop");
    assert!(outcome.is_accepted());

    let registry = adapter
        .ops_lifecycle_registry(&session_id)
        .await
        .expect("ops registry should exist for attached session");

    let operation_id = OperationId::new();
    registry
        .register_operation(OperationSpec {
            id: operation_id.clone(),
            kind: OperationKind::BackgroundToolOp,
            owner_session_id: session_id.clone(),
            display_name: "destroy wait target with loop".into(),
            source_label: "meerkat_machine_test".into(),
            child_session_id: None,
            expect_peer_channel: false,
        })
        .expect("operation should register");
    registry
        .provisioning_succeeded(&operation_id)
        .expect("operation should enter running");

    let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

    let before_destroy = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist before destroy");
    let wait_request_id = before_destroy
        .ops
        .wait_request_id
        .clone()
        .expect("wait_all should register an authority-owned wait request");
    assert_eq!(before_destroy.control.phase, RuntimeState::Attached);
    assert!(before_destroy.ops.pending_wait_present);
    assert_eq!(
        before_destroy.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "pending wait carrier should track the same wait request id before destroy"
    );
    assert_eq!(
        before_destroy.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "wait_all should track the active operation before destroy"
    );
    assert_eq!(
        apply_calls.load(Ordering::SeqCst),
        0,
        "destroy should be able to abandon queued attached-loop work before apply runs"
    );
    assert_eq!(
        control_calls.load(Ordering::SeqCst),
        0,
        "destroy has not yet attempted any executor control"
    );

    let runtime_id = runtime_id_for_session(&session_id);
    let report = crate::traits::RuntimeControlPlane::destroy(&*adapter, &runtime_id)
        .await
        .expect("destroy should preserve wait_all while abandoning queued attached-loop work");
    assert_eq!(report.inputs_abandoned, 1);

    let after_destroy = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after destroy");
    assert_eq!(after_destroy.control.phase, RuntimeState::Destroyed);
    assert_eq!(
        after_destroy.ops.wait_request_id,
        Some(wait_request_id.clone()),
        "destroy should preserve the authority-owned wait request even with an attached loop"
    );
    assert!(
        after_destroy.ops.pending_wait_present,
        "destroy should preserve the pending wait carrier while the operation remains active"
    );
    assert_eq!(
        after_destroy.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "destroy should preserve request-id agreement across the wait carrier seam"
    );
    assert_eq!(
        after_destroy.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "destroy should preserve the tracked wait targets until the operation settles"
    );
    assert_eq!(
        apply_calls.load(Ordering::SeqCst),
        0,
        "destroy should bypass queued attached-loop work entirely"
    );
    assert_eq!(
        control_calls.load(Ordering::SeqCst),
        0,
        "destroy currently bypasses the executor control seam and does not deliver an out-of-band control command"
    );

    registry
        .complete_operation(
            &operation_id,
            OperationResult {
                id: operation_id.clone(),
                content: "done".into(),
                is_error: false,
                duration_ms: 1,
                tokens_used: 0,
            },
        )
        .expect("operation should complete");

    let waited = wait_future
        .await
        .expect("wait_all should resolve after completion");
    assert_eq!(waited.satisfied.wait_request_id, wait_request_id);
    assert_eq!(waited.satisfied.operation_ids, vec![operation_id]);

    let settled = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after wait_all settles");
    assert_eq!(settled.control.phase, RuntimeState::Destroyed);
    assert_eq!(settled.ops.wait_request_id, None);
    assert!(!settled.ops.pending_wait_present);
    assert_eq!(settled.ops.pending_wait_request_id, None);
    assert!(settled.ops.wait_operation_ids.is_empty());
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_destroy_with_runtime_loop_splits_completion_and_wait_all_lifetimes()
 {
    struct RecordingExecutor {
        apply_calls: Arc<AtomicUsize>,
        control_calls: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl CoreExecutor for RecordingExecutor {
        async fn apply(
            &mut self,
            run_id: RunId,
            primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            self.apply_calls.fetch_add(1, Ordering::SeqCst);
            Ok(CoreApplyOutput {
                receipt: RunBoundaryReceipt {
                    run_id,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                    sequence: 0,
                },
                session_snapshot: None,
                terminal: None,
            })
        }

        async fn control(&mut self, _command: RunControlCommand) -> Result<(), CoreExecutorError> {
            self.control_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    let apply_calls = Arc::new(AtomicUsize::new(0));
    let control_calls = Arc::new(AtomicUsize::new(0));

    adapter
        .register_session_with_executor(
            session_id.clone(),
            Box::new(RecordingExecutor {
                apply_calls: Arc::clone(&apply_calls),
                control_calls: Arc::clone(&control_calls),
            }),
        )
        .await;

    let input = make_progress_input("destroy-with-loop-split-lifetimes");
    let input_id = input.id().clone();
    let outcome = adapter
        .accept_input_without_wake(&session_id, input)
        .await
        .expect("queued progress input should be accepted without waking the loop");
    assert!(outcome.is_accepted());

    let completion_handle = {
        let completions = {
            let sessions = adapter.sessions.read().await;
            sessions
                .get(&session_id)
                .expect("attached session should exist")
                .completions
                .clone()
        };
        let mut completions = completions.lock().await;
        completions.register(input_id.clone())
    };

    let registry = adapter
        .ops_lifecycle_registry(&session_id)
        .await
        .expect("ops registry should exist for attached session");

    let operation_id = OperationId::new();
    registry
        .register_operation(OperationSpec {
            id: operation_id.clone(),
            kind: OperationKind::BackgroundToolOp,
            owner_session_id: session_id.clone(),
            display_name: "destroy split wait target".into(),
            source_label: "meerkat_machine_test".into(),
            child_session_id: None,
            expect_peer_channel: false,
        })
        .expect("operation should register");
    registry
        .provisioning_succeeded(&operation_id)
        .expect("operation should enter running");

    let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

    let before_destroy = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist before destroy");
    let wait_request_id = before_destroy
        .ops
        .wait_request_id
        .clone()
        .expect("wait_all should register an authority-owned wait request");
    assert_eq!(before_destroy.control.phase, RuntimeState::Attached);
    assert_eq!(before_destroy.completion_waiters.input_count, 1);
    assert_eq!(before_destroy.completion_waiters.waiter_count, 1);
    assert_eq!(before_destroy.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        before_destroy.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert!(before_destroy.ops.pending_wait_present);
    assert_eq!(
        before_destroy.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "pending wait carrier should track the same wait request id before destroy"
    );
    assert_eq!(
        before_destroy.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "wait_all should track the active operation before destroy"
    );

    let runtime_id = runtime_id_for_session(&session_id);
    let report = crate::traits::RuntimeControlPlane::destroy(&*adapter, &runtime_id)
        .await
        .expect("destroy should split completion and wait_all lifetimes on attached runtimes");
    assert_eq!(report.inputs_abandoned, 1);

    match completion_handle.wait().await {
        CompletionOutcome::RuntimeTerminated(reason) => {
            assert_eq!(reason, "runtime destroyed");
        }
        other => panic!("expected runtime destroyed termination, got {other:?}"),
    }

    let after_destroy = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after destroy");
    assert_eq!(after_destroy.control.phase, RuntimeState::Destroyed);
    assert_eq!(after_destroy.completion_waiters.input_count, 0);
    assert_eq!(after_destroy.completion_waiters.waiter_count, 0);
    assert!(
        after_destroy.completion_waiters.waiting_inputs.is_empty(),
        "destroy should clear input-owned completion waiters immediately"
    );
    assert_eq!(
        after_destroy.ops.wait_request_id,
        Some(wait_request_id.clone()),
        "destroy should preserve the authority-owned wait request after completion waiters clear"
    );
    assert!(after_destroy.ops.pending_wait_present);
    assert_eq!(
        after_destroy.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "destroy should preserve request-id agreement across the wait carrier seam"
    );
    assert_eq!(
        after_destroy.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "destroy should preserve the tracked wait target until the operation settles"
    );
    assert_eq!(
        apply_calls.load(Ordering::SeqCst),
        0,
        "destroy should bypass queued attached-loop work entirely"
    );
    assert_eq!(
        control_calls.load(Ordering::SeqCst),
        0,
        "destroy currently bypasses the executor control seam and does not deliver an out-of-band control command"
    );

    registry
        .complete_operation(
            &operation_id,
            OperationResult {
                id: operation_id.clone(),
                content: "done".into(),
                is_error: false,
                duration_ms: 1,
                tokens_used: 0,
            },
        )
        .expect("operation should complete after destroy");
    let wait_result = wait_future.await.expect("wait_all should still resolve");
    assert_eq!(wait_result.satisfied.wait_request_id, wait_request_id);
    assert_eq!(
        wait_result.satisfied.operation_ids,
        vec![operation_id.clone()]
    );

    let settled = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after wait_all settles");
    assert_eq!(settled.control.phase, RuntimeState::Destroyed);
    assert_eq!(settled.ops.wait_request_id, None);
    assert!(!settled.ops.pending_wait_present);
    assert_eq!(settled.ops.pending_wait_request_id, None);
    assert!(settled.ops.wait_operation_ids.is_empty());
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_preserves_wait_all_after_stop_runtime_executor() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;
    let registry = adapter
        .ops_lifecycle_registry(&session_id)
        .await
        .expect("ops registry should exist for registered session");

    let operation_id = OperationId::new();
    registry
        .register_operation(OperationSpec {
            id: operation_id.clone(),
            kind: OperationKind::BackgroundToolOp,
            owner_session_id: session_id.clone(),
            display_name: "stop wait target".into(),
            source_label: "meerkat_machine_test".into(),
            child_session_id: None,
            expect_peer_channel: false,
        })
        .expect("operation should register");
    registry
        .provisioning_succeeded(&operation_id)
        .expect("operation should enter running");

    let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

    let before_stop = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist before stop");
    let wait_request_id = before_stop
        .ops
        .wait_request_id
        .clone()
        .expect("wait_all should register an authority-owned wait request");
    assert!(before_stop.ops.pending_wait_present);
    assert_eq!(
        before_stop.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "pending wait carrier should track the same wait request id before stop"
    );
    assert_eq!(
        before_stop.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "wait_all should track the active operation before stop"
    );

    adapter
        .stop_runtime_executor(
            &session_id,
            RunControlCommand::StopRuntimeExecutor {
                reason: "stop wait_all test".into(),
            },
        )
        .await
        .expect("stop should preserve the active wait_all carrier");

    let after_stop = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let snapshot = adapter
                .meerkat_machine_spine_snapshot(&session_id)
                .await
                .expect("snapshot should exist after stop");
            if snapshot.control.phase == RuntimeState::Stopped {
                break snapshot;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("attached stop should eventually publish the Stopped phase");
    assert_eq!(
        after_stop.ops.wait_request_id,
        Some(wait_request_id.clone()),
        "stop currently preserves the authority-owned wait request"
    );
    assert!(
        after_stop.ops.pending_wait_present,
        "stop currently preserves the pending wait carrier while the operation remains active"
    );
    assert_eq!(
        after_stop.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "stop should preserve request-id agreement across the wait carrier seam"
    );
    assert_eq!(
        after_stop.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "stop should preserve the tracked wait targets until the operation settles"
    );

    registry
        .complete_operation(
            &operation_id,
            OperationResult {
                id: operation_id.clone(),
                content: "done".into(),
                is_error: false,
                duration_ms: 1,
                tokens_used: 0,
            },
        )
        .expect("operation should complete after stop");
    let wait_result = wait_future.await.expect("wait_all should still resolve");
    assert_eq!(wait_result.satisfied.wait_request_id, wait_request_id);
    assert_eq!(
        wait_result.satisfied.operation_ids,
        vec![operation_id.clone()]
    );

    let settled = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after wait_all settles");
    assert_eq!(settled.control.phase, RuntimeState::Stopped);
    assert_eq!(
        settled.control.current_run_id, None,
        "stop should not leave a settled control-side current-run binding behind"
    );
    assert_eq!(
        settled.inputs.current_run_id, None,
        "stop should not leave a settled ingress-side current-run binding behind"
    );
    assert_eq!(settled.ops.wait_request_id, None);
    assert!(!settled.ops.pending_wait_present);
    assert_eq!(settled.ops.pending_wait_request_id, None);
    assert!(settled.ops.wait_operation_ids.is_empty());
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_stop_runtime_executor_clears_steered_waiter_and_queue_but_preserves_wait_all()
 {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;

    let input = Input::Prompt(crate::input::PromptInput::new(
        "stop steered prompt",
        Some(
            meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                handling_mode: Some(meerkat_core::types::HandlingMode::Steer),
                ..Default::default()
            },
        ),
    ));
    let input_id = input.id().clone();
    let (_outcome, completion_handle) = adapter
        .accept_input_with_completion(&session_id, input)
        .await
        .expect("steered prompt should be accepted");
    let completion_handle =
        completion_handle.expect("steered prompt should register a completion waiter");

    let registry = adapter
        .ops_lifecycle_registry(&session_id)
        .await
        .expect("ops registry should exist for registered session");

    let operation_id = OperationId::new();
    registry
        .register_operation(OperationSpec {
            id: operation_id.clone(),
            kind: OperationKind::BackgroundToolOp,
            owner_session_id: session_id.clone(),
            display_name: "stop steered wait target".into(),
            source_label: "meerkat_machine_test".into(),
            child_session_id: None,
            expect_peer_channel: false,
        })
        .expect("operation should register");
    registry
        .provisioning_succeeded(&operation_id)
        .expect("operation should enter running");

    let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

    let before_stop = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist before stop");
    let wait_request_id = before_stop
        .ops
        .wait_request_id
        .clone()
        .expect("wait_all should register an authority-owned wait request");
    assert_eq!(before_stop.control.phase, RuntimeState::Idle);
    assert!(before_stop.inputs.queue.is_empty());
    assert_eq!(before_stop.inputs.steer_queue, vec![input_id.clone()]);
    assert_eq!(before_stop.completion_waiters.input_count, 1);
    assert_eq!(before_stop.completion_waiters.waiter_count, 1);
    assert_eq!(before_stop.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        before_stop.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert!(before_stop.ops.pending_wait_present);
    assert_eq!(
        before_stop.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "pending wait carrier should track the same wait request id before stop"
    );
    assert_eq!(
        before_stop.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "wait_all should track the active operation before stop"
    );

    adapter
        .stop_runtime_executor(
            &session_id,
            RunControlCommand::StopRuntimeExecutor {
                reason: "stop steered split lifetimes".into(),
            },
        )
        .await
        .expect("stop should clear steered completion waiters while preserving wait_all");

    match completion_handle.wait().await {
        CompletionOutcome::RuntimeTerminated(reason) => {
            assert_eq!(reason, "runtime stopped");
        }
        other => {
            panic!("expected runtime stopped termination for steered input, got {other:?}")
        }
    }

    let after_stop = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after stop");
    assert_eq!(after_stop.control.phase, RuntimeState::Stopped);
    assert_eq!(after_stop.control.current_run_id, None);
    assert_eq!(after_stop.inputs.current_run_id, None);
    assert!(after_stop.inputs.queue.is_empty());
    assert!(
        after_stop.inputs.steer_queue.is_empty(),
        "stop should clear steered queued work immediately on the plain runtime path"
    );
    assert_eq!(after_stop.completion_waiters.input_count, 0);
    assert_eq!(after_stop.completion_waiters.waiter_count, 0);
    assert!(
        after_stop.completion_waiters.waiting_inputs.is_empty(),
        "stop should clear input-owned steered completion waiters immediately"
    );
    assert_eq!(
        after_stop.ops.wait_request_id,
        Some(wait_request_id.clone()),
        "stop should preserve the authority-owned wait request after steered completion waiters clear"
    );
    assert!(after_stop.ops.pending_wait_present);
    assert_eq!(
        after_stop.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "stop should preserve request-id agreement across the wait carrier seam"
    );
    assert_eq!(
        after_stop.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "stop should preserve the tracked wait target until the operation settles"
    );

    registry
        .complete_operation(
            &operation_id,
            OperationResult {
                id: operation_id.clone(),
                content: "done".into(),
                is_error: false,
                duration_ms: 1,
                tokens_used: 0,
            },
        )
        .expect("operation should complete after stop");
    let wait_result = wait_future.await.expect("wait_all should still resolve");
    assert_eq!(wait_result.satisfied.wait_request_id, wait_request_id);
    assert_eq!(
        wait_result.satisfied.operation_ids,
        vec![operation_id.clone()]
    );

    let settled = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after wait_all settles");
    assert_eq!(settled.control.phase, RuntimeState::Stopped);
    assert_eq!(settled.control.current_run_id, None);
    assert_eq!(settled.inputs.current_run_id, None);
    assert!(settled.inputs.queue.is_empty());
    assert!(settled.inputs.steer_queue.is_empty());
    assert_eq!(settled.ops.wait_request_id, None);
    assert!(!settled.ops.pending_wait_present);
    assert_eq!(settled.ops.pending_wait_request_id, None);
    assert!(settled.ops.wait_operation_ids.is_empty());
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_stop_runtime_executor_splits_completion_and_wait_all_lifetimes()
 {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;

    let input = make_prompt("stop split lifetimes");
    let input_id = input.id().clone();
    let (_outcome, completion_handle) = adapter
        .accept_input_with_completion(&session_id, input)
        .await
        .expect("prompt should be accepted");
    let completion_handle =
        completion_handle.expect("queued prompt should register a completion waiter");

    let registry = adapter
        .ops_lifecycle_registry(&session_id)
        .await
        .expect("ops registry should exist for registered session");

    let operation_id = OperationId::new();
    registry
        .register_operation(OperationSpec {
            id: operation_id.clone(),
            kind: OperationKind::BackgroundToolOp,
            owner_session_id: session_id.clone(),
            display_name: "stop split wait target".into(),
            source_label: "meerkat_machine_test".into(),
            child_session_id: None,
            expect_peer_channel: false,
        })
        .expect("operation should register");
    registry
        .provisioning_succeeded(&operation_id)
        .expect("operation should enter running");

    let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

    let before_stop = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist before stop");
    let wait_request_id = before_stop
        .ops
        .wait_request_id
        .clone()
        .expect("wait_all should register an authority-owned wait request");
    assert_eq!(before_stop.control.phase, RuntimeState::Idle);
    assert_eq!(before_stop.completion_waiters.input_count, 1);
    assert_eq!(before_stop.completion_waiters.waiter_count, 1);
    assert_eq!(before_stop.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        before_stop.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert!(before_stop.ops.pending_wait_present);
    assert_eq!(
        before_stop.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "pending wait carrier should track the same wait request id before stop"
    );
    assert_eq!(
        before_stop.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "wait_all should track the active operation before stop"
    );

    adapter
        .stop_runtime_executor(
            &session_id,
            RunControlCommand::StopRuntimeExecutor {
                reason: "stop split lifetimes".into(),
            },
        )
        .await
        .expect("stop should split completion and wait_all lifetimes");

    match completion_handle.wait().await {
        CompletionOutcome::RuntimeTerminated(reason) => {
            assert_eq!(reason, "runtime stopped");
        }
        other => panic!("expected runtime stopped termination, got {other:?}"),
    }

    let after_stop = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after stop");
    assert_eq!(after_stop.control.phase, RuntimeState::Stopped);
    assert!(
        after_stop.inputs.queue.is_empty(),
        "stop should abandon ordinary queued work immediately on the plain runtime path"
    );
    assert!(
        after_stop.inputs.steer_queue.is_empty(),
        "stop should abandon steered queued work immediately on the plain runtime path"
    );
    assert_eq!(after_stop.completion_waiters.input_count, 0);
    assert_eq!(after_stop.completion_waiters.waiter_count, 0);
    assert!(
        after_stop.completion_waiters.waiting_inputs.is_empty(),
        "stop should clear input-owned completion waiters immediately"
    );
    assert_eq!(
        after_stop.ops.wait_request_id,
        Some(wait_request_id.clone()),
        "stop should preserve the authority-owned wait request after completion waiters clear"
    );
    assert!(after_stop.ops.pending_wait_present);
    assert_eq!(
        after_stop.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "stop should preserve request-id agreement across the wait carrier seam"
    );
    assert_eq!(
        after_stop.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "stop should preserve the tracked wait target until the operation settles"
    );

    registry
        .complete_operation(
            &operation_id,
            OperationResult {
                id: operation_id.clone(),
                content: "done".into(),
                is_error: false,
                duration_ms: 1,
                tokens_used: 0,
            },
        )
        .expect("operation should complete after stop");
    let wait_result = wait_future.await.expect("wait_all should still resolve");
    assert_eq!(wait_result.satisfied.wait_request_id, wait_request_id);
    assert_eq!(
        wait_result.satisfied.operation_ids,
        vec![operation_id.clone()]
    );

    let settled = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after wait_all settles");
    assert_eq!(settled.control.phase, RuntimeState::Stopped);
    assert!(
        settled.inputs.queue.is_empty(),
        "stop should not reintroduce ordinary queued work once the plain runtime settles"
    );
    assert!(
        settled.inputs.steer_queue.is_empty(),
        "stop should not reintroduce steered queued work once the plain runtime settles"
    );
    assert_eq!(
        settled.control.current_run_id, None,
        "stop should not leave a settled control-side current-run binding behind even on attached runtimes"
    );
    assert_eq!(
        settled.inputs.current_run_id, None,
        "stop should not leave a settled ingress-side current-run binding behind even on attached runtimes"
    );
    assert_eq!(settled.ops.wait_request_id, None);
    assert!(!settled.ops.pending_wait_present);
    assert_eq!(settled.ops.pending_wait_request_id, None);
    assert!(settled.ops.wait_operation_ids.is_empty());
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_preserves_wait_all_after_stop_runtime_executor_with_runtime_loop()
 {
    struct RecordingExecutor {
        apply_calls: Arc<AtomicUsize>,
        stop_calls: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl CoreExecutor for RecordingExecutor {
        async fn apply(
            &mut self,
            run_id: RunId,
            primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            self.apply_calls.fetch_add(1, Ordering::SeqCst);
            Ok(CoreApplyOutput {
                receipt: RunBoundaryReceipt {
                    run_id,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                    sequence: 0,
                },
                session_snapshot: None,
                terminal: None,
            })
        }

        async fn control(&mut self, command: RunControlCommand) -> Result<(), CoreExecutorError> {
            if matches!(command, RunControlCommand::StopRuntimeExecutor { .. }) {
                self.stop_calls.fetch_add(1, Ordering::SeqCst);
            }
            Ok(())
        }
    }

    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    let apply_calls = Arc::new(AtomicUsize::new(0));
    let stop_calls = Arc::new(AtomicUsize::new(0));

    adapter
        .register_session_with_executor(
            session_id.clone(),
            Box::new(RecordingExecutor {
                apply_calls: Arc::clone(&apply_calls),
                stop_calls: Arc::clone(&stop_calls),
            }),
        )
        .await;

    let outcome = adapter
        .accept_input_without_wake(&session_id, make_progress_input("stop-wait-all-with-loop"))
        .await
        .expect("queued progress input should be accepted without waking the loop");
    assert!(outcome.is_accepted());

    let registry = adapter
        .ops_lifecycle_registry(&session_id)
        .await
        .expect("ops registry should exist for attached session");

    let operation_id = OperationId::new();
    registry
        .register_operation(OperationSpec {
            id: operation_id.clone(),
            kind: OperationKind::BackgroundToolOp,
            owner_session_id: session_id.clone(),
            display_name: "stop wait target with loop".into(),
            source_label: "meerkat_machine_test".into(),
            child_session_id: None,
            expect_peer_channel: false,
        })
        .expect("operation should register");
    registry
        .provisioning_succeeded(&operation_id)
        .expect("operation should enter running");

    let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

    let before_stop = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist before stop");
    let wait_request_id = before_stop
        .ops
        .wait_request_id
        .clone()
        .expect("wait_all should register an authority-owned wait request");
    assert_eq!(before_stop.control.phase, RuntimeState::Attached);
    assert!(before_stop.ops.pending_wait_present);
    assert_eq!(
        before_stop.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "pending wait carrier should track the same wait request id before stop"
    );
    assert_eq!(
        before_stop.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "wait_all should track the active operation before stop"
    );
    assert_eq!(
        apply_calls.load(Ordering::SeqCst),
        0,
        "stop should be able to preempt queued attached-loop work before apply runs"
    );

    adapter
        .stop_runtime_executor(
            &session_id,
            RunControlCommand::StopRuntimeExecutor {
                reason: "stop attached-loop wait_all".into(),
            },
        )
        .await
        .expect("stop should preserve the active wait_all carrier through the live control seam");

    let after_stop = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let snapshot = adapter
                .meerkat_machine_spine_snapshot(&session_id)
                .await
                .expect("snapshot should exist after stop");
            if snapshot.control.phase == RuntimeState::Stopped {
                break snapshot;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("attached stop should eventually publish the Stopped phase");
    assert_eq!(
        after_stop.ops.wait_request_id,
        Some(wait_request_id.clone()),
        "stop should preserve the authority-owned wait request on attached runtimes"
    );
    assert!(after_stop.ops.pending_wait_present);
    assert_eq!(
        after_stop.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "stop should preserve request-id agreement across the wait carrier seam"
    );
    assert_eq!(
        after_stop.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "stop should preserve the tracked wait target until the operation settles"
    );
    assert_eq!(
        apply_calls.load(Ordering::SeqCst),
        0,
        "stop should beat queued ordinary work on an attached runtime loop"
    );
    assert_eq!(
        stop_calls.load(Ordering::SeqCst),
        1,
        "the attached executor should observe exactly one stop-runtime-executor control"
    );

    registry
        .complete_operation(
            &operation_id,
            OperationResult {
                id: operation_id.clone(),
                content: "done".into(),
                is_error: false,
                duration_ms: 1,
                tokens_used: 0,
            },
        )
        .expect("operation should complete after stop");
    let wait_result = wait_future.await.expect("wait_all should still resolve");
    assert_eq!(wait_result.satisfied.wait_request_id, wait_request_id);
    assert_eq!(
        wait_result.satisfied.operation_ids,
        vec![operation_id.clone()]
    );

    let settled = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after wait_all settles");
    assert_eq!(settled.control.phase, RuntimeState::Stopped);
    assert_eq!(settled.ops.wait_request_id, None);
    assert!(!settled.ops.pending_wait_present);
    assert_eq!(settled.ops.pending_wait_request_id, None);
    assert!(settled.ops.wait_operation_ids.is_empty());
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_stop_runtime_executor_with_runtime_loop_splits_completion_and_wait_all_lifetimes()
 {
    struct RecordingExecutor {
        apply_calls: Arc<AtomicUsize>,
        stop_calls: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl CoreExecutor for RecordingExecutor {
        async fn apply(
            &mut self,
            run_id: RunId,
            primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            self.apply_calls.fetch_add(1, Ordering::SeqCst);
            Ok(CoreApplyOutput {
                receipt: RunBoundaryReceipt {
                    run_id,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                    sequence: 0,
                },
                session_snapshot: None,
                terminal: None,
            })
        }

        async fn control(&mut self, command: RunControlCommand) -> Result<(), CoreExecutorError> {
            if matches!(command, RunControlCommand::StopRuntimeExecutor { .. }) {
                self.stop_calls.fetch_add(1, Ordering::SeqCst);
            }
            Ok(())
        }
    }

    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    let apply_calls = Arc::new(AtomicUsize::new(0));
    let stop_calls = Arc::new(AtomicUsize::new(0));

    adapter
        .register_session_with_executor(
            session_id.clone(),
            Box::new(RecordingExecutor {
                apply_calls: Arc::clone(&apply_calls),
                stop_calls: Arc::clone(&stop_calls),
            }),
        )
        .await;

    let input = make_progress_input("stop-with-loop-split-lifetimes");
    let input_id = input.id().clone();
    let outcome = adapter
        .accept_input_without_wake(&session_id, input)
        .await
        .expect("queued progress input should be accepted without waking the loop");
    assert!(outcome.is_accepted());

    let completion_handle = {
        let completions = {
            let sessions = adapter.sessions.read().await;
            sessions
                .get(&session_id)
                .expect("attached session should exist")
                .completions
                .clone()
        };
        let mut completions = completions.lock().await;
        completions.register(input_id.clone())
    };

    let registry = adapter
        .ops_lifecycle_registry(&session_id)
        .await
        .expect("ops registry should exist for attached session");

    let operation_id = OperationId::new();
    registry
        .register_operation(OperationSpec {
            id: operation_id.clone(),
            kind: OperationKind::BackgroundToolOp,
            owner_session_id: session_id.clone(),
            display_name: "stop split wait target".into(),
            source_label: "meerkat_machine_test".into(),
            child_session_id: None,
            expect_peer_channel: false,
        })
        .expect("operation should register");
    registry
        .provisioning_succeeded(&operation_id)
        .expect("operation should enter running");

    let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

    let before_stop = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist before stop");
    let wait_request_id = before_stop
        .ops
        .wait_request_id
        .clone()
        .expect("wait_all should register an authority-owned wait request");
    assert_eq!(before_stop.control.phase, RuntimeState::Attached);
    assert_eq!(before_stop.completion_waiters.input_count, 1);
    assert_eq!(before_stop.completion_waiters.waiter_count, 1);
    assert_eq!(before_stop.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        before_stop.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert!(before_stop.ops.pending_wait_present);
    assert_eq!(
        before_stop.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "pending wait carrier should track the same wait request id before stop"
    );
    assert_eq!(
        before_stop.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "wait_all should track the active operation before stop"
    );

    adapter
        .stop_runtime_executor(
            &session_id,
            RunControlCommand::StopRuntimeExecutor {
                reason: "stop attached-loop split lifetimes".into(),
            },
        )
        .await
        .expect("stop should split completion and wait_all lifetimes on attached runtimes");

    match completion_handle.wait().await {
        CompletionOutcome::RuntimeTerminated(reason) => {
            assert_eq!(reason, "runtime stopped");
        }
        other => panic!("expected runtime stopped termination, got {other:?}"),
    }

    let after_stop = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let snapshot = adapter
                .meerkat_machine_spine_snapshot(&session_id)
                .await
                .expect("snapshot should exist after stop");
            if snapshot.control.phase == RuntimeState::Stopped {
                break snapshot;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("attached stop should eventually publish the Stopped phase");
    assert!(
        after_stop.inputs.queue.is_empty(),
        "attached stop should abandon ordinary queued work immediately"
    );
    assert!(
        after_stop.inputs.steer_queue.is_empty(),
        "attached stop should abandon steered queued work immediately"
    );
    assert_eq!(after_stop.completion_waiters.input_count, 0);
    assert_eq!(after_stop.completion_waiters.waiter_count, 0);
    assert!(
        after_stop.completion_waiters.waiting_inputs.is_empty(),
        "stop should clear input-owned completion waiters immediately"
    );
    assert_eq!(
        after_stop.ops.wait_request_id,
        Some(wait_request_id.clone()),
        "stop should preserve the authority-owned wait request after completion waiters clear"
    );
    assert!(after_stop.ops.pending_wait_present);
    assert_eq!(
        after_stop.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "stop should preserve request-id agreement across the wait carrier seam"
    );
    assert_eq!(
        after_stop.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "stop should preserve the tracked wait target until the operation settles"
    );
    assert_eq!(
        apply_calls.load(Ordering::SeqCst),
        0,
        "stop should beat queued ordinary work on an attached runtime loop"
    );
    assert_eq!(
        stop_calls.load(Ordering::SeqCst),
        1,
        "the attached executor should observe exactly one stop-runtime-executor control"
    );

    registry
        .complete_operation(
            &operation_id,
            OperationResult {
                id: operation_id.clone(),
                content: "done".into(),
                is_error: false,
                duration_ms: 1,
                tokens_used: 0,
            },
        )
        .expect("operation should complete after stop");
    let wait_result = wait_future.await.expect("wait_all should still resolve");
    assert_eq!(wait_result.satisfied.wait_request_id, wait_request_id);
    assert_eq!(
        wait_result.satisfied.operation_ids,
        vec![operation_id.clone()]
    );

    let settled = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after wait_all settles");
    assert_eq!(settled.control.phase, RuntimeState::Stopped);
    assert!(
        settled.inputs.queue.is_empty(),
        "attached stop should not reintroduce ordinary queued work once the runtime settles"
    );
    assert!(
        settled.inputs.steer_queue.is_empty(),
        "attached stop should not reintroduce steered queued work once the runtime settles"
    );
    assert_eq!(settled.ops.wait_request_id, None);
    assert!(!settled.ops.pending_wait_present);
    assert_eq!(settled.ops.pending_wait_request_id, None);
    assert!(settled.ops.wait_operation_ids.is_empty());
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_preserves_wait_all_after_retire() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;
    let registry = adapter
        .ops_lifecycle_registry(&session_id)
        .await
        .expect("ops registry should exist for registered session");

    let operation_id = OperationId::new();
    registry
        .register_operation(OperationSpec {
            id: operation_id.clone(),
            kind: OperationKind::BackgroundToolOp,
            owner_session_id: session_id.clone(),
            display_name: "retire wait target".into(),
            source_label: "meerkat_machine_test".into(),
            child_session_id: None,
            expect_peer_channel: false,
        })
        .expect("operation should register");
    registry
        .provisioning_succeeded(&operation_id)
        .expect("operation should enter running");

    let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

    let before_retire = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist before retire");
    let wait_request_id = before_retire
        .ops
        .wait_request_id
        .clone()
        .expect("wait_all should register an authority-owned wait request");
    assert!(before_retire.ops.pending_wait_present);
    assert_eq!(
        before_retire.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "pending wait carrier should track the same wait request id before retire"
    );
    assert_eq!(
        before_retire.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "wait_all should track the active operation before retire"
    );

    let runtime_id = runtime_id_for_session(&session_id);
    let report = crate::traits::RuntimeControlPlane::retire(&*adapter, &runtime_id)
        .await
        .expect("retire should preserve the active wait_all carrier");
    assert_eq!(report.inputs_abandoned, 0);
    assert_eq!(report.inputs_pending_drain, 0);

    let after_retire = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after retire");
    assert_eq!(after_retire.control.phase, RuntimeState::Retired);
    assert_eq!(
        after_retire.ops.wait_request_id,
        Some(wait_request_id.clone()),
        "retire currently preserves the authority-owned wait request"
    );
    assert!(
        after_retire.ops.pending_wait_present,
        "retire currently preserves the pending wait carrier while the operation remains active"
    );
    assert_eq!(
        after_retire.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "retire should preserve request-id agreement across the wait carrier seam"
    );
    assert_eq!(
        after_retire.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "retire should preserve the tracked wait targets until the operation settles"
    );

    registry
        .complete_operation(
            &operation_id,
            OperationResult {
                id: operation_id.clone(),
                content: "done".into(),
                is_error: false,
                duration_ms: 1,
                tokens_used: 0,
            },
        )
        .expect("operation should complete after retire");
    let wait_result = wait_future.await.expect("wait_all should still resolve");
    assert_eq!(wait_result.satisfied.wait_request_id, wait_request_id);
    assert_eq!(
        wait_result.satisfied.operation_ids,
        vec![operation_id.clone()]
    );

    let settled = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after wait_all settles");
    assert_eq!(settled.control.phase, RuntimeState::Retired);
    assert_eq!(settled.ops.wait_request_id, None);
    assert!(!settled.ops.pending_wait_present);
    assert_eq!(settled.ops.pending_wait_request_id, None);
    assert!(settled.ops.wait_operation_ids.is_empty());
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_retire_splits_completion_and_wait_all_lifetimes() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;

    let input = make_prompt("retire split lifetimes");
    let input_id = input.id().clone();
    let (_outcome, completion_handle) = adapter
        .accept_input_with_completion(&session_id, input)
        .await
        .expect("prompt should be accepted");
    let completion_handle =
        completion_handle.expect("queued prompt should register a completion waiter");

    let registry = adapter
        .ops_lifecycle_registry(&session_id)
        .await
        .expect("ops registry should exist for registered session");

    let operation_id = OperationId::new();
    registry
        .register_operation(OperationSpec {
            id: operation_id.clone(),
            kind: OperationKind::BackgroundToolOp,
            owner_session_id: session_id.clone(),
            display_name: "retire split wait target".into(),
            source_label: "meerkat_machine_test".into(),
            child_session_id: None,
            expect_peer_channel: false,
        })
        .expect("operation should register");
    registry
        .provisioning_succeeded(&operation_id)
        .expect("operation should enter running");

    let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

    let before_retire = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist before retire");
    let wait_request_id = before_retire
        .ops
        .wait_request_id
        .clone()
        .expect("wait_all should register an authority-owned wait request");
    assert_eq!(before_retire.control.phase, RuntimeState::Idle);
    assert_eq!(before_retire.completion_waiters.input_count, 1);
    assert_eq!(before_retire.completion_waiters.waiter_count, 1);
    assert_eq!(before_retire.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        before_retire.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert!(before_retire.ops.pending_wait_present);
    assert_eq!(
        before_retire.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "pending wait carrier should track the same wait request id before retire"
    );
    assert_eq!(
        before_retire.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "wait_all should track the active operation before retire"
    );

    let runtime_id = runtime_id_for_session(&session_id);
    let report = crate::traits::RuntimeControlPlane::retire(&*adapter, &runtime_id)
        .await
        .expect("retire should split completion and wait_all lifetimes");
    assert_eq!(report.inputs_abandoned, 1);
    assert_eq!(report.inputs_pending_drain, 0);

    match completion_handle.wait().await {
        CompletionOutcome::RuntimeTerminated(reason) => {
            assert_eq!(reason, "retired without runtime loop");
        }
        other => panic!("expected retire termination, got {other:?}"),
    }

    let after_retire = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after retire");
    assert_eq!(after_retire.control.phase, RuntimeState::Retired);
    assert_eq!(
        after_retire.control.current_run_id, None,
        "retire should not leave a settled current-run binding behind once the runtime is actually Retired"
    );
    assert_eq!(
        after_retire.inputs.current_run_id, None,
        "retire should clear ingress-side current-run binding once the runtime is actually Retired"
    );
    assert_eq!(after_retire.completion_waiters.input_count, 0);
    assert_eq!(after_retire.completion_waiters.waiter_count, 0);
    assert!(
        after_retire.completion_waiters.waiting_inputs.is_empty(),
        "retire should clear input-owned completion waiters immediately when no runtime loop can drain"
    );
    assert_eq!(
        after_retire.ops.wait_request_id,
        Some(wait_request_id.clone()),
        "retire should preserve the authority-owned wait request after completion waiters clear"
    );
    assert!(after_retire.ops.pending_wait_present);
    assert_eq!(
        after_retire.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "retire should preserve request-id agreement across the wait carrier seam"
    );
    assert_eq!(
        after_retire.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "retire should preserve the tracked wait target until the operation settles"
    );

    registry
        .complete_operation(
            &operation_id,
            OperationResult {
                id: operation_id.clone(),
                content: "done".into(),
                is_error: false,
                duration_ms: 1,
                tokens_used: 0,
            },
        )
        .expect("operation should complete after retire");
    let wait_result = wait_future.await.expect("wait_all should still resolve");
    assert_eq!(wait_result.satisfied.wait_request_id, wait_request_id);
    assert_eq!(
        wait_result.satisfied.operation_ids,
        vec![operation_id.clone()]
    );

    let settled = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after wait_all settles");
    assert_eq!(settled.control.phase, RuntimeState::Retired);
    assert_eq!(settled.ops.wait_request_id, None);
    assert!(!settled.ops.pending_wait_present);
    assert_eq!(settled.ops.pending_wait_request_id, None);
    assert!(settled.ops.wait_operation_ids.is_empty());
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_retire_clears_steered_waiter_and_steer_queue() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;

    let input = Input::Prompt(crate::input::PromptInput::new(
        "retire steered prompt",
        Some(
            meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                handling_mode: Some(meerkat_core::types::HandlingMode::Steer),
                ..Default::default()
            },
        ),
    ));
    let input_id = input.id().clone();
    let (_outcome, completion_handle) = adapter
        .accept_input_with_completion(&session_id, input)
        .await
        .expect("steered prompt should be accepted");
    let completion_handle =
        completion_handle.expect("steered prompt should register a completion waiter");

    let registry = adapter
        .ops_lifecycle_registry(&session_id)
        .await
        .expect("ops registry should exist for registered session");

    let operation_id = OperationId::new();
    registry
        .register_operation(OperationSpec {
            id: operation_id.clone(),
            kind: OperationKind::BackgroundToolOp,
            owner_session_id: session_id.clone(),
            display_name: "retire steered wait target".into(),
            source_label: "meerkat_machine_test".into(),
            child_session_id: None,
            expect_peer_channel: false,
        })
        .expect("operation should register");
    registry
        .provisioning_succeeded(&operation_id)
        .expect("operation should enter running");

    let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

    let before_retire = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist before retire");
    let wait_request_id = before_retire
        .ops
        .wait_request_id
        .clone()
        .expect("wait_all should register an authority-owned wait request");
    assert_eq!(before_retire.control.phase, RuntimeState::Idle);
    assert!(before_retire.inputs.queue.is_empty());
    assert_eq!(before_retire.inputs.steer_queue, vec![input_id.clone()]);
    assert_eq!(before_retire.completion_waiters.input_count, 1);
    assert_eq!(before_retire.completion_waiters.waiter_count, 1);
    assert_eq!(before_retire.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        before_retire.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert!(before_retire.ops.pending_wait_present);
    assert_eq!(
        before_retire.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "pending wait carrier should track the same wait request id before retire"
    );
    assert_eq!(
        before_retire.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "wait_all should track the active operation before retire"
    );

    let runtime_id = runtime_id_for_session(&session_id);
    let report = crate::traits::RuntimeControlPlane::retire(&*adapter, &runtime_id)
        .await
        .expect("retire should terminate the steered completion waiter while preserving wait_all");
    assert_eq!(report.inputs_abandoned, 1);
    assert_eq!(report.inputs_pending_drain, 0);

    match completion_handle.wait().await {
        CompletionOutcome::RuntimeTerminated(reason) => {
            assert_eq!(reason, "retired without runtime loop");
        }
        other => panic!("expected retire termination for steered input, got {other:?}"),
    }

    let after_retire = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after retire");
    assert_eq!(after_retire.control.phase, RuntimeState::Retired);
    assert_eq!(after_retire.control.current_run_id, None);
    assert_eq!(after_retire.inputs.current_run_id, None);
    assert!(after_retire.inputs.queue.is_empty());
    assert!(
        after_retire.inputs.steer_queue.is_empty(),
        "retire should clear steered queued visibility once ledger-owned abandonment is reconciled back into ingress"
    );
    assert_eq!(after_retire.completion_waiters.input_count, 0);
    assert_eq!(after_retire.completion_waiters.waiter_count, 0);
    assert!(
        after_retire.completion_waiters.waiting_inputs.is_empty(),
        "retire should clear input-owned steered completion waiters immediately"
    );
    assert_eq!(
        after_retire.ops.wait_request_id,
        Some(wait_request_id.clone()),
        "retire should preserve the authority-owned wait request after steered completion waiters clear"
    );
    assert!(after_retire.ops.pending_wait_present);
    assert_eq!(
        after_retire.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "retire should preserve request-id agreement across the wait carrier seam"
    );
    assert_eq!(
        after_retire.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "retire should preserve the tracked wait target until the operation settles"
    );

    registry
        .complete_operation(
            &operation_id,
            OperationResult {
                id: operation_id.clone(),
                content: "done".into(),
                is_error: false,
                duration_ms: 1,
                tokens_used: 0,
            },
        )
        .expect("operation should complete after retire");
    let wait_result = wait_future.await.expect("wait_all should still resolve");
    assert_eq!(wait_result.satisfied.wait_request_id, wait_request_id);
    assert_eq!(
        wait_result.satisfied.operation_ids,
        vec![operation_id.clone()]
    );

    let settled = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after wait_all settles");
    assert_eq!(settled.control.phase, RuntimeState::Retired);
    assert_eq!(settled.ops.wait_request_id, None);
    assert!(!settled.ops.pending_wait_present);
    assert_eq!(settled.ops.pending_wait_request_id, None);
    assert!(settled.ops.wait_operation_ids.is_empty());
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_preserves_wait_all_after_retire_with_runtime_loop() {
    struct BlockingExecutor {
        apply_calls: Arc<AtomicUsize>,
        apply_started: Arc<Notify>,
        apply_finished: Arc<Notify>,
        allow_finish: Arc<Notify>,
    }

    #[async_trait::async_trait]
    impl CoreExecutor for BlockingExecutor {
        async fn apply(
            &mut self,
            run_id: RunId,
            primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            self.apply_calls.fetch_add(1, Ordering::SeqCst);
            self.apply_started.notify_waiters();
            self.allow_finish.notified().await;
            self.apply_finished.notify_waiters();

            Ok(CoreApplyOutput {
                receipt: RunBoundaryReceipt {
                    run_id,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                    sequence: 0,
                },
                session_snapshot: None,
                terminal: None,
            })
        }

        async fn control(&mut self, _command: RunControlCommand) -> Result<(), CoreExecutorError> {
            Ok(())
        }
    }

    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    let apply_calls = Arc::new(AtomicUsize::new(0));
    let apply_started = Arc::new(Notify::new());
    let apply_finished = Arc::new(Notify::new());
    let allow_finish = Arc::new(Notify::new());

    adapter
        .register_session_with_executor(
            session_id.clone(),
            Box::new(BlockingExecutor {
                apply_calls: Arc::clone(&apply_calls),
                apply_started: Arc::clone(&apply_started),
                apply_finished: Arc::clone(&apply_finished),
                allow_finish: Arc::clone(&allow_finish),
            }),
        )
        .await;

    let input = make_progress_input("retire-wait-all-with-loop");
    let outcome = adapter
        .accept_input_without_wake(&session_id, input)
        .await
        .expect("queued progress input should be accepted without waking the loop");
    assert!(outcome.is_accepted());

    let registry = adapter
        .ops_lifecycle_registry(&session_id)
        .await
        .expect("ops registry should exist for attached session");

    let operation_id = OperationId::new();
    registry
        .register_operation(OperationSpec {
            id: operation_id.clone(),
            kind: OperationKind::BackgroundToolOp,
            owner_session_id: session_id.clone(),
            display_name: "retire wait target with loop".into(),
            source_label: "meerkat_machine_test".into(),
            child_session_id: None,
            expect_peer_channel: false,
        })
        .expect("operation should register");
    registry
        .provisioning_succeeded(&operation_id)
        .expect("operation should enter running");

    let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

    let before_retire = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist before retire");
    let wait_request_id = before_retire
        .ops
        .wait_request_id
        .clone()
        .expect("wait_all should register an authority-owned wait request");
    assert_eq!(before_retire.control.phase, RuntimeState::Attached);
    assert!(before_retire.ops.pending_wait_present);
    assert_eq!(
        before_retire.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "pending wait carrier should track the same wait request id before retire"
    );
    assert_eq!(
        before_retire.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "wait_all should track the active operation before retire"
    );
    assert_eq!(
        apply_calls.load(Ordering::SeqCst),
        0,
        "queued attached-loop work should remain pending until retire wakes the loop"
    );

    let runtime_id = runtime_id_for_session(&session_id);
    let report = crate::traits::RuntimeControlPlane::retire(&*adapter, &runtime_id)
        .await
        .expect("retire should preserve wait_all while the live loop drains queued work");
    assert_eq!(report.inputs_abandoned, 0);
    assert_eq!(report.inputs_pending_drain, 1);

    tokio::time::timeout(Duration::from_secs(1), apply_started.notified())
        .await
        .expect("retire should wake the attached runtime loop to drain queued work");

    let after_retire = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after retire wakes the loop");
    assert_eq!(
        after_retire.control.phase,
        RuntimeState::Retired,
        "post-`e5c5ecaf3` DSL-authoritative: retire holds lifecycle_phase at Retired through the drain window (Retire transition goes to Retired unconditionally); DSL is source of truth, not control_projection cache"
    );
    assert_eq!(
        after_retire.ops.wait_request_id,
        Some(wait_request_id.clone()),
        "retire should preserve the authority-owned wait request while the attached loop is draining"
    );
    assert!(after_retire.ops.pending_wait_present);
    assert_eq!(
        after_retire.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "retire should preserve request-id agreement across the wait carrier seam while draining"
    );
    assert_eq!(
        after_retire.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "retire should preserve the tracked wait target while queued work is draining"
    );
    assert_eq!(
        apply_calls.load(Ordering::SeqCst),
        1,
        "retire should wake the attached loop exactly once for the preserved queued work"
    );

    allow_finish.notify_waiters();
    tokio::time::timeout(Duration::from_secs(1), apply_finished.notified())
        .await
        .expect("attached loop should finish draining the preserved queued work");

    let after_drain = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let snapshot = adapter
                .meerkat_machine_spine_snapshot(&session_id)
                .await
                .expect("snapshot should exist after attached-loop drain");
            if snapshot.control.phase == RuntimeState::Retired {
                break snapshot;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("runtime should return to Retired once the preserved work finishes draining");

    assert_eq!(
        after_drain.control.current_run_id, None,
        "retire should not leave a settled control-side current-run binding once the attached loop returns to Retired"
    );
    assert_eq!(
        after_drain.inputs.current_run_id, None,
        "retire should not leave a settled ingress-side current-run binding once the attached loop returns to Retired"
    );
    assert_eq!(
        after_drain.ops.wait_request_id,
        Some(wait_request_id.clone()),
        "wait_all should remain active after the attached loop finishes draining preserved work"
    );
    assert!(after_drain.ops.pending_wait_present);
    assert_eq!(
        after_drain.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "request-id agreement should survive the return to Retired after drain"
    );
    assert_eq!(
        after_drain.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "the tracked wait target should remain present until the operation itself settles"
    );
    assert!(
        after_drain.inputs.queue.is_empty(),
        "retire should not leave ordinary queued work behind once the attached runtime returns to Retired"
    );
    assert!(
        after_drain.inputs.steer_queue.is_empty(),
        "retire should not leave steer-queued work behind once the attached runtime returns to Retired"
    );

    registry
        .complete_operation(
            &operation_id,
            OperationResult {
                id: operation_id.clone(),
                content: "done".into(),
                is_error: false,
                duration_ms: 1,
                tokens_used: 0,
            },
        )
        .expect("operation should complete after retire+drain");
    let wait_result = wait_future.await.expect("wait_all should still resolve");
    assert_eq!(wait_result.satisfied.wait_request_id, wait_request_id);
    assert_eq!(
        wait_result.satisfied.operation_ids,
        vec![operation_id.clone()]
    );

    let settled = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after wait_all settles");
    assert_eq!(settled.control.phase, RuntimeState::Retired);
    assert!(
        settled.inputs.queue.is_empty(),
        "retire should keep the attached settled Retired snapshot free of ordinary queued work"
    );
    assert!(
        settled.inputs.steer_queue.is_empty(),
        "retire should keep the attached settled Retired snapshot free of steer-queued work"
    );
    assert_eq!(settled.ops.wait_request_id, None);
    assert!(!settled.ops.pending_wait_present);
    assert_eq!(settled.ops.pending_wait_request_id, None);
    assert!(settled.ops.wait_operation_ids.is_empty());
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_retire_with_runtime_loop_splits_completion_and_wait_all_lifetimes()
 {
    struct BlockingExecutor {
        apply_calls: Arc<AtomicUsize>,
        control_calls: Arc<AtomicUsize>,
        apply_started: Arc<Notify>,
        apply_finished: Arc<Notify>,
        allow_finish: Arc<Notify>,
    }

    #[async_trait::async_trait]
    impl CoreExecutor for BlockingExecutor {
        async fn apply(
            &mut self,
            run_id: RunId,
            primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            self.apply_calls.fetch_add(1, Ordering::SeqCst);
            self.apply_started.notify_waiters();
            self.allow_finish.notified().await;
            self.apply_finished.notify_waiters();

            Ok(CoreApplyOutput {
                receipt: RunBoundaryReceipt {
                    run_id,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                    sequence: 0,
                },
                session_snapshot: None,
                terminal: None,
            })
        }

        async fn control(&mut self, _command: RunControlCommand) -> Result<(), CoreExecutorError> {
            self.control_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    let apply_calls = Arc::new(AtomicUsize::new(0));
    let control_calls = Arc::new(AtomicUsize::new(0));
    let apply_started = Arc::new(Notify::new());
    let apply_finished = Arc::new(Notify::new());
    let allow_finish = Arc::new(Notify::new());

    adapter
        .register_session_with_executor(
            session_id.clone(),
            Box::new(BlockingExecutor {
                apply_calls: Arc::clone(&apply_calls),
                control_calls: Arc::clone(&control_calls),
                apply_started: Arc::clone(&apply_started),
                apply_finished: Arc::clone(&apply_finished),
                allow_finish: Arc::clone(&allow_finish),
            }),
        )
        .await;

    let input = make_progress_input("retire-with-loop-split-lifetimes");
    let input_id = input.id().clone();
    let (outcome, completion_handle) = adapter
        .accept_input_with_completion(&session_id, input)
        .await
        .expect("progress input should be accepted");
    assert!(outcome.is_accepted());
    let completion_handle =
        completion_handle.expect("queued progress input should expose a completion waiter");

    let registry = adapter
        .ops_lifecycle_registry(&session_id)
        .await
        .expect("ops registry should exist for attached session");

    let operation_id = OperationId::new();
    registry
        .register_operation(OperationSpec {
            id: operation_id.clone(),
            kind: OperationKind::BackgroundToolOp,
            owner_session_id: session_id.clone(),
            display_name: "retire split-lifetime wait target".into(),
            source_label: "meerkat_machine_test".into(),
            child_session_id: None,
            expect_peer_channel: false,
        })
        .expect("operation should register");
    registry
        .provisioning_succeeded(&operation_id)
        .expect("operation should enter running");

    let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

    let before_retire = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist before retire");
    let wait_request_id = before_retire
        .ops
        .wait_request_id
        .clone()
        .expect("wait_all should register an authority-owned wait request");
    assert_eq!(before_retire.control.phase, RuntimeState::Attached);
    assert_eq!(before_retire.completion_waiters.input_count, 1);
    assert_eq!(before_retire.completion_waiters.waiter_count, 1);
    assert_eq!(before_retire.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        before_retire.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert_eq!(
        before_retire.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "pending wait carrier should track the same wait request id before retire"
    );
    assert_eq!(
        before_retire.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "wait_all should track the active operation before retire"
    );
    assert_eq!(
        apply_calls.load(Ordering::SeqCst),
        0,
        "queued attached-loop work should remain pending until retire wakes the loop"
    );
    assert_eq!(
        control_calls.load(Ordering::SeqCst),
        0,
        "retire should not yet have attempted executor control"
    );

    let runtime_id = runtime_id_for_session(&session_id);
    let report = crate::traits::RuntimeControlPlane::retire(&*adapter, &runtime_id)
        .await
        .expect("retire should preserve queued work and wait_all while the live loop drains");
    assert_eq!(report.inputs_abandoned, 0);
    assert_eq!(report.inputs_pending_drain, 1);

    tokio::time::timeout(Duration::from_secs(1), apply_started.notified())
        .await
        .expect("retire should wake the attached runtime loop to drain queued work");

    let during_retire = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after retire wakes the loop");
    assert_eq!(
        during_retire.control.phase,
        RuntimeState::Retired,
        "post-`e5c5ecaf3` DSL-authoritative: retire holds lifecycle_phase at Retired through the drain window (Retire transition goes to Retired unconditionally); DSL is source of truth, not control_projection cache"
    );
    assert_eq!(during_retire.completion_waiters.input_count, 1);
    assert_eq!(during_retire.completion_waiters.waiter_count, 1);
    assert_eq!(during_retire.completion_waiters.waiting_inputs.len(), 1);
    assert_eq!(
        during_retire.completion_waiters.waiting_inputs[0].input_id,
        input_id
    );
    assert_eq!(
        during_retire.ops.wait_request_id,
        Some(wait_request_id.clone()),
        "retire should preserve the authority-owned wait request while the attached loop is draining"
    );
    assert!(during_retire.ops.pending_wait_present);
    assert_eq!(
        during_retire.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "retire should preserve request-id agreement across the wait carrier seam while draining"
    );
    assert_eq!(
        during_retire.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "retire should preserve the tracked wait target while queued work is draining"
    );
    assert_eq!(
        apply_calls.load(Ordering::SeqCst),
        1,
        "retire should wake the attached loop exactly once for the preserved queued work"
    );
    assert_eq!(
        control_calls.load(Ordering::SeqCst),
        0,
        "retire should not route any out-of-band control command through the executor seam"
    );

    allow_finish.notify_waiters();
    tokio::time::timeout(Duration::from_secs(1), apply_finished.notified())
        .await
        .expect("attached loop should finish draining the preserved queued work");

    match completion_handle.wait().await {
        CompletionOutcome::CompletedWithoutResult => {}
        other => panic!(
            "expected retire+drain to complete queued work while wait_all remains live, got {other:?}"
        ),
    }

    let after_drain = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let snapshot = adapter
                .meerkat_machine_spine_snapshot(&session_id)
                .await
                .expect("snapshot should exist after attached-loop drain");
            if snapshot.control.phase == RuntimeState::Retired {
                break snapshot;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("runtime should return to Retired once the preserved work finishes draining");
    assert!(
        after_drain.inputs.queue.is_empty(),
        "retire with a runtime loop should not leave ordinary queued work behind once the runtime returns to Retired"
    );
    assert!(
        after_drain.inputs.steer_queue.is_empty(),
        "retire with a runtime loop should not leave steer-queued work behind once the runtime returns to Retired"
    );
    assert_eq!(after_drain.completion_waiters.input_count, 0);
    assert_eq!(after_drain.completion_waiters.waiter_count, 0);
    assert!(
        after_drain.completion_waiters.waiting_inputs.is_empty(),
        "completion waiters should clear once retire-drained work completes"
    );
    assert_eq!(
        after_drain.ops.wait_request_id,
        Some(wait_request_id.clone()),
        "wait_all should remain active after input-owned completion waiters clear"
    );
    assert!(after_drain.ops.pending_wait_present);
    assert_eq!(
        after_drain.ops.pending_wait_request_id,
        Some(wait_request_id.clone()),
        "request-id agreement should survive the return to Retired after drain"
    );
    assert_eq!(
        after_drain.ops.wait_operation_ids,
        vec![operation_id.clone()],
        "the tracked wait target should remain present until the operation itself settles"
    );

    registry
        .complete_operation(
            &operation_id,
            OperationResult {
                id: operation_id.clone(),
                content: "done".into(),
                is_error: false,
                duration_ms: 1,
                tokens_used: 0,
            },
        )
        .expect("operation should complete after retire+drain");
    let wait_result = wait_future.await.expect("wait_all should still resolve");
    assert_eq!(wait_result.satisfied.wait_request_id, wait_request_id);
    assert_eq!(
        wait_result.satisfied.operation_ids,
        vec![operation_id.clone()]
    );

    let settled = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after wait_all settles");
    assert_eq!(settled.control.phase, RuntimeState::Retired);
    assert_eq!(settled.ops.wait_request_id, None);
    assert!(!settled.ops.pending_wait_present);
    assert_eq!(settled.ops.pending_wait_request_id, None);
    assert!(settled.ops.wait_operation_ids.is_empty());
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_tracks_comms_drain_state() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    let comms_runtime: Arc<dyn CommsRuntime> = Arc::new(FakeDrainRuntime::idle());

    spawn_test_comms_drain(
        &adapter,
        &session_id,
        CommsDrainMode::PersistentHost,
        comms_runtime,
        Duration::from_secs(60),
    )
    .await;

    let snapshot = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist for registered session");

    assert!(snapshot.drain.slot_present);
    assert_eq!(snapshot.drain.phase, Some(CommsDrainPhase::Running));
    assert_eq!(snapshot.drain.mode, Some(CommsDrainMode::PersistentHost));
    assert!(snapshot.drain.handle_present);
}

#[tokio::test]
async fn meerkat_machine_spine_snapshot_tracks_stopped_comms_drain_state() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    let comms_runtime: Arc<dyn CommsRuntime> = Arc::new(FakeDrainRuntime::idle());

    let spawned = adapter
        .maybe_spawn_comms_drain(&session_id, true, Some(comms_runtime))
        .await;
    assert!(
        !spawned,
        "unregistered session should not spawn a comms drain"
    );

    adapter.register_session(session_id.clone()).await;

    let spawned = adapter
        .maybe_spawn_comms_drain(
            &session_id,
            true,
            Some(Arc::new(FakeDrainRuntime::idle()) as Arc<dyn CommsRuntime>),
        )
        .await;
    assert!(spawned, "registered session should spawn a comms drain");

    adapter.abort_comms_drain(&session_id).await;

    let snapshot = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist for registered session");

    assert!(snapshot.drain.slot_present);
    assert_eq!(snapshot.drain.phase, Some(CommsDrainPhase::Stopped));
    assert_eq!(snapshot.drain.mode, Some(CommsDrainMode::PersistentHost));
    assert!(!snapshot.drain.handle_present);
}

// ---------------------------------------------------------------
// A1: Session command guards (TLA+ DestroyedShapeInvariant,
//     RunningHasActiveRunInvariant)
// ---------------------------------------------------------------

#[tokio::test]
async fn register_session_rejects_destroyed_session() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;

    // Transition to Destroyed via the control-plane destroy path.
    let runtime_id = runtime_id_for_session(&session_id);
    crate::traits::RuntimeControlPlane::destroy(&*adapter, &runtime_id)
        .await
        .expect("destroy should succeed");

    // Second register must be rejected — DestroyedShapeInvariant forbids
    // resurrecting a destroyed binding.
    let err = adapter
        .execute_meerkat_machine_command(
            None,
            MeerkatMachineCommand::RegisterSession {
                session_id: session_id.clone(),
            },
        )
        .await
        .expect_err("register should reject a destroyed session");
    assert!(
        matches!(
            err,
            MeerkatMachineCommandError::Driver(RuntimeDriverError::Destroyed)
        ),
        "expected Destroyed, got {err:?}"
    );
}

#[tokio::test]
async fn unregister_session_rejects_unknown_session() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();

    // Unregister on a session that was never registered must return an error.
    let err = adapter
        .execute_meerkat_machine_command(
            None,
            MeerkatMachineCommand::UnregisterSession {
                session_id: session_id.clone(),
            },
        )
        .await
        .expect_err("unregister should reject an unknown session");
    assert!(
        matches!(
            err,
            MeerkatMachineCommandError::Driver(RuntimeDriverError::NotReady { .. })
        ),
        "expected NotReady, got {err:?}"
    );
}

#[tokio::test]
async fn interrupt_current_run_rejects_destroyed_session() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;

    let runtime_id = runtime_id_for_session(&session_id);
    crate::traits::RuntimeControlPlane::destroy(&*adapter, &runtime_id)
        .await
        .expect("destroy should succeed");

    let err = adapter
        .interrupt_current_run(&session_id)
        .await
        .expect_err("interrupt should reject a destroyed session");
    assert!(
        matches!(err, RuntimeDriverError::Destroyed),
        "expected Destroyed, got {err:?}"
    );
}

#[tokio::test]
async fn cancel_after_boundary_rejects_destroyed_session() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;

    let runtime_id = runtime_id_for_session(&session_id);
    crate::traits::RuntimeControlPlane::destroy(&*adapter, &runtime_id)
        .await
        .expect("destroy should succeed");

    let err = adapter
        .cancel_after_boundary(&session_id)
        .await
        .expect_err("cancel_after_boundary should reject a destroyed session");
    assert!(
        matches!(err, RuntimeDriverError::Destroyed),
        "expected Destroyed, got {err:?}"
    );
}

#[tokio::test]
async fn stop_runtime_executor_rejects_destroyed_session() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;

    let runtime_id = runtime_id_for_session(&session_id);
    crate::traits::RuntimeControlPlane::destroy(&*adapter, &runtime_id)
        .await
        .expect("destroy should succeed");

    let err = adapter
        .stop_runtime_executor(
            &session_id,
            RunControlCommand::StopRuntimeExecutor {
                reason: "test".to_string(),
            },
        )
        .await
        .expect_err("stop_runtime_executor should reject a destroyed session");
    assert!(
        matches!(err, RuntimeDriverError::Destroyed),
        "expected Destroyed, got {err:?}"
    );
}

// ---------------------------------------------------------------
// A2: Drain command guards (TLA+ DrainBindingInvariant,
//     DrainModeInvariant)
// ---------------------------------------------------------------

#[tokio::test]
async fn set_peer_ingress_context_rejects_unknown_session() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();

    let spawned = adapter
        .update_peer_ingress_context(&session_id, true, None)
        .await;
    assert!(
        !spawned,
        "update_peer_ingress_context should not spawn for unknown session"
    );
}

#[tokio::test]
async fn set_peer_ingress_context_rejects_destroyed_session() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;

    let runtime_id = runtime_id_for_session(&session_id);
    crate::traits::RuntimeControlPlane::destroy(adapter.as_ref(), &runtime_id)
        .await
        .expect("destroy should succeed");

    let spawned = adapter
        .update_peer_ingress_context(&session_id, true, None)
        .await;
    assert!(
        !spawned,
        "update_peer_ingress_context should not spawn for destroyed session"
    );
}

#[tokio::test]
async fn notify_drain_exited_rejects_unknown_session() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();

    // Should not panic — the guard silently rejects.
    adapter
        .notify_comms_drain_exited(&session_id, DrainExitReason::Dismissed)
        .await;
}

#[tokio::test]
async fn notify_drain_exited_rejects_destroyed_session() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;

    let runtime_id = runtime_id_for_session(&session_id);
    crate::traits::RuntimeControlPlane::destroy(adapter.as_ref(), &runtime_id)
        .await
        .expect("destroy should succeed");

    // Should not panic — the guard silently rejects.
    adapter
        .notify_comms_drain_exited(&session_id, DrainExitReason::Dismissed)
        .await;
}

#[tokio::test]
async fn abort_comms_drain_tolerates_unknown_session() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();

    // Guard rejects unknown session but caller swallows the error.
    adapter.abort_comms_drain(&session_id).await;
}

#[tokio::test]
async fn wait_comms_drain_tolerates_unknown_session() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();

    // Guard rejects unknown session but caller swallows the error.
    adapter.wait_comms_drain(&session_id).await;
}

// ---------------------------------------------------------------
// A3: Control command guards (TLA+ ActiveRunPhaseInvariant,
//     LiveBindingLifecycleInvariant, AdmitQueuedInput precondition)
// ---------------------------------------------------------------

#[tokio::test]
async fn ingest_rejects_retired_session() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;

    let runtime_id = runtime_id_for_session(&session_id);
    crate::traits::RuntimeControlPlane::retire(&*adapter, &runtime_id)
        .await
        .expect("retire should succeed");

    let input = make_prompt("should be rejected");
    let err = crate::traits::RuntimeControlPlane::ingest(&*adapter, &runtime_id, input)
        .await
        .expect_err("ingest should reject a retired session");
    assert!(
        matches!(
            err,
            RuntimeControlPlaneError::InvalidState {
                state: RuntimeState::Retired
            }
        ),
        "expected InvalidState(Retired), got {err:?}"
    );
}

#[tokio::test]
async fn ingest_rejects_stopped_session() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;

    // Stop the session by driving it through retire → stop.
    let runtime_id = runtime_id_for_session(&session_id);
    adapter
        .stop_runtime_executor(
            &session_id,
            RunControlCommand::StopRuntimeExecutor {
                reason: "test stop".to_string(),
            },
        )
        .await
        .expect("stop should succeed");

    let input = make_prompt("should be rejected");
    let err = crate::traits::RuntimeControlPlane::ingest(&*adapter, &runtime_id, input)
        .await
        .expect_err("ingest should reject a stopped session");
    assert!(
        matches!(
            err,
            RuntimeControlPlaneError::InvalidState {
                state: RuntimeState::Stopped
            }
        ),
        "expected InvalidState(Stopped), got {err:?}"
    );
}

#[tokio::test]
async fn retire_rejection_from_stopped_surfaces_dsl_authority() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();

    // Retire legality is DSL-owned; the Idle -> Retired path is exercised
    // above, so this anchors an incompatible Stopped phase.
    adapter.register_session(session_id.clone()).await;

    // First stop
    adapter
        .stop_runtime_executor(
            &session_id,
            RunControlCommand::StopRuntimeExecutor {
                reason: "test".to_string(),
            },
        )
        .await
        .expect("stop should succeed");

    let runtime_id = runtime_id_for_session(&session_id);
    let err = crate::traits::RuntimeControlPlane::retire(&*adapter, &runtime_id)
        .await
        .expect_err("retire should reject a stopped session");
    assert!(
        matches!(
            err,
            RuntimeControlPlaneError::Internal(ref reason)
                if reason.contains("DSL authority (Retire)")
                    && reason.contains("Stopped")
                    && reason.contains("Retire")
        ),
        "expected DSL authority rejection for Retire from Stopped, got {err:?}"
    );
}

// ---------------------------------------------------------------
// A4: Ingress command guards (TLA+ WaitingInputsInvariant)
// ---------------------------------------------------------------

#[tokio::test]
async fn accept_input_with_completion_rejects_retired_session() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;

    let runtime_id = runtime_id_for_session(&session_id);
    crate::traits::RuntimeControlPlane::retire(&*adapter, &runtime_id)
        .await
        .expect("retire should succeed");

    let input = make_prompt("should be rejected");
    let result = adapter
        .accept_input_with_completion(&session_id, input)
        .await;
    match result {
        Err(RuntimeDriverError::NotReady {
            state: RuntimeState::Retired,
        }) => {}
        Err(other) => panic!("expected NotReady(Retired), got {other:?}"),
        Ok(_) => panic!("accept_input_with_completion should reject retired session"),
    }
}

#[tokio::test]
async fn accept_input_with_completion_rejects_destroyed_session() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;

    let runtime_id = runtime_id_for_session(&session_id);
    crate::traits::RuntimeControlPlane::destroy(&*adapter, &runtime_id)
        .await
        .expect("destroy should succeed");

    let input = make_prompt("should be rejected");
    let result = adapter
        .accept_input_with_completion(&session_id, input)
        .await;
    match result {
        Err(RuntimeDriverError::Destroyed) => {}
        Err(other) => panic!("expected Destroyed, got {other:?}"),
        Ok(_) => panic!("accept_input_with_completion should reject destroyed session"),
    }
}

// ---------------------------------------------------------------
// A5: Legacy run command guards (TLA+ RunningHasActiveRunInvariant)
// ---------------------------------------------------------------

async fn prepare_legacy_run_for_authority_test(
    adapter: &MeerkatMachine,
    session_id: &SessionId,
    text: &str,
) -> MeerkatMachineRunPrepared {
    let result = adapter
        .execute_meerkat_machine_command(
            None,
            MeerkatMachineCommand::Prepare {
                session_id: session_id.clone(),
                input: make_prompt(text),
            },
        )
        .await
        .expect("legacy run prepare should succeed");
    match result {
        MeerkatMachineCommandResult::Prepared(prepared) => prepared,
        other => panic!("unexpected prepare result: {other:?}"),
    }
}

fn legacy_run_test_output(run_id: RunId, input_id: InputId) -> CoreApplyOutput {
    CoreApplyOutput {
        receipt: RunBoundaryReceipt {
            run_id,
            boundary: RunApplyBoundary::RunStart,
            contributing_input_ids: vec![input_id],
            conversation_digest: None,
            message_count: 0,
            sequence: 0,
        },
        session_snapshot: None,
        terminal: None,
    }
}

#[tokio::test]
async fn legacy_run_prepare_rejects_retired_session() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;

    let runtime_id = runtime_id_for_session(&session_id);
    crate::traits::RuntimeControlPlane::retire(&*adapter, &runtime_id)
        .await
        .expect("retire should succeed");

    let input = make_prompt("should be rejected");
    let err = adapter
        .accept_input_and_run::<(), _, _>(&session_id, input, |_run_id, _prim| async {
            panic!("executor should not be called on retired session");
        })
        .await
        .expect_err("legacy run should reject a retired session");
    assert!(
        matches!(
            err,
            RuntimeDriverError::NotReady {
                state: RuntimeState::Retired
            }
        ),
        "expected NotReady(Retired), got {err:?}"
    );
}

#[tokio::test]
async fn legacy_run_prepare_rejects_destroyed_session() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;

    let runtime_id = runtime_id_for_session(&session_id);
    crate::traits::RuntimeControlPlane::destroy(&*adapter, &runtime_id)
        .await
        .expect("destroy should succeed");

    let input = make_prompt("should be rejected");
    let err = adapter
        .accept_input_and_run::<(), _, _>(&session_id, input, |_run_id, _prim| async {
            panic!("executor should not be called on destroyed session");
        })
        .await
        .expect_err("legacy run should reject a destroyed session");
    assert!(
        matches!(err, RuntimeDriverError::Destroyed),
        "expected Destroyed, got {err:?}"
    );
}

#[tokio::test]
async fn legacy_run_commit_rejection_preserves_registered_running_session() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    adapter.register_session(session_id.clone()).await;

    let prepared =
        prepare_legacy_run_for_authority_test(&adapter, &session_id, "commit rejection").await;
    let rejected_run_id = RunId::new();
    let result = adapter
        .execute_meerkat_machine_command(
            None,
            MeerkatMachineCommand::Commit {
                session_id: session_id.clone(),
                input_id: prepared.input_id.clone(),
                run_id: rejected_run_id.clone(),
                output: legacy_run_test_output(rejected_run_id, prepared.input_id.clone()),
            },
        )
        .await;

    assert!(
        result.is_err(),
        "commit with a non-active run id should be rejected"
    );
    assert!(
        adapter.contains_session(&session_id).await,
        "typed commit rejection must not unregister the session"
    );
    assert_eq!(
        adapter.runtime_state(&session_id).await.unwrap(),
        RuntimeState::Running,
        "rejected commit must leave canonical DSL lifecycle on the active run"
    );
    let input_state = adapter
        .input_state(&session_id, &prepared.input_id)
        .await
        .expect("input state read should succeed")
        .expect("prepared input should remain visible");
    assert_eq!(
        input_state.seed.phase,
        crate::input_state::InputLifecycleState::Staged,
        "rejected commit must not fake input completion"
    );
}

#[tokio::test]
async fn legacy_run_commit_mismatched_input_rejection_preserves_active_run() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    adapter.register_session(session_id.clone()).await;

    let prepared =
        prepare_legacy_run_for_authority_test(&adapter, &session_id, "commit input mismatch").await;
    let wrong_input_id = InputId::new();
    let result = adapter
        .execute_meerkat_machine_command(
            None,
            MeerkatMachineCommand::Commit {
                session_id: session_id.clone(),
                input_id: wrong_input_id,
                run_id: prepared.run_id.clone(),
                output: legacy_run_test_output(prepared.run_id.clone(), prepared.input_id.clone()),
            },
        )
        .await;

    assert!(result.is_err(), "mismatched commit input should reject");
    assert!(
        adapter.contains_session(&session_id).await,
        "typed commit rejection must not unregister the session"
    );
    assert_eq!(
        adapter.runtime_state(&session_id).await.unwrap(),
        RuntimeState::Running,
        "malformed commit must preserve the active runtime run"
    );
    let input_state = adapter
        .input_state(&session_id, &prepared.input_id)
        .await
        .expect("input state read should succeed")
        .expect("prepared input should remain visible");
    assert_eq!(
        input_state.seed.phase,
        crate::input_state::InputLifecycleState::Staged,
        "malformed commit must not leave the contributor pending consumption"
    );

    adapter
        .execute_meerkat_machine_command(
            None,
            MeerkatMachineCommand::Fail {
                session_id: session_id.clone(),
                run_id: prepared.run_id.clone(),
                error: "unwind malformed commit".to_string(),
            },
        )
        .await
        .expect("preserved active run should still be terminalizable");
    assert_eq!(
        adapter.runtime_state(&session_id).await.unwrap(),
        RuntimeState::Idle
    );
}

#[tokio::test]
async fn legacy_run_fail_rejection_preserves_registered_running_session() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    adapter.register_session(session_id.clone()).await;

    let prepared =
        prepare_legacy_run_for_authority_test(&adapter, &session_id, "fail rejection").await;
    let result = adapter
        .execute_meerkat_machine_command(
            None,
            MeerkatMachineCommand::Fail {
                session_id: session_id.clone(),
                run_id: RunId::new(),
                error: "reject the wrong run".to_string(),
            },
        )
        .await;

    assert!(
        result.is_err(),
        "fail with a non-active run id should be rejected"
    );
    assert!(
        adapter.contains_session(&session_id).await,
        "typed fail rejection must not unregister the session"
    );
    assert_eq!(
        adapter.runtime_state(&session_id).await.unwrap(),
        RuntimeState::Running,
        "rejected fail must leave canonical DSL lifecycle on the active run"
    );
    let input_state = adapter
        .input_state(&session_id, &prepared.input_id)
        .await
        .expect("input state read should succeed")
        .expect("prepared input should remain visible");
    assert_eq!(
        input_state.seed.phase,
        crate::input_state::InputLifecycleState::Staged,
        "rejected fail must not roll back the active input from shell policy"
    );
}

#[tokio::test]
async fn legacy_run_fail_terminalizes_through_machine_authority() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    adapter.register_session(session_id.clone()).await;

    let prepared =
        prepare_legacy_run_for_authority_test(&adapter, &session_id, "fail terminalization").await;
    adapter
        .execute_meerkat_machine_command(
            None,
            MeerkatMachineCommand::Fail {
                session_id: session_id.clone(),
                run_id: prepared.run_id.clone(),
                error: "executor failed".to_string(),
            },
        )
        .await
        .expect("active fail should terminalize the run");

    assert!(adapter.contains_session(&session_id).await);
    assert_eq!(
        adapter.runtime_state(&session_id).await.unwrap(),
        RuntimeState::Idle
    );
    let input_state = adapter
        .input_state(&session_id, &prepared.input_id)
        .await
        .expect("input state read should succeed")
        .expect("prepared input should remain visible");
    assert_eq!(
        input_state.seed.phase,
        crate::input_state::InputLifecycleState::Queued,
        "machine fail should roll the recoverable input back for retry"
    );
}

async fn staged_batch_commit_driver(
    first: Input,
    second: Input,
) -> (SharedDriver, RunId, Vec<InputId>) {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    adapter.register_session(session_id.clone()).await;

    let first = match adapter
        .accept_input(&session_id, first)
        .await
        .expect("first input should queue")
    {
        AcceptOutcome::Accepted { input_id, .. } => input_id,
        other => panic!("expected accepted first input, got {other:?}"),
    };
    let second = match adapter
        .accept_input(&session_id, second)
        .await
        .expect("second input should queue")
    {
        AcceptOutcome::Accepted { input_id, .. } => input_id,
        other => panic!("expected accepted second input, got {other:?}"),
    };
    let staged_ids = vec![first, second];
    let driver = {
        let sessions = adapter.sessions.read().await;
        Arc::clone(
            &sessions
                .get(&session_id)
                .expect("registered session should exist")
                .driver,
        )
    };
    let run_id = RunId::new();
    prepare_runtime_loop_batch_start(&driver, run_id.clone(), &staged_ids)
        .await
        .expect("batch prepare should stage both inputs");
    (driver, run_id, staged_ids)
}

fn batch_receipt(run_id: RunId, contributing_input_ids: Vec<InputId>) -> RunBoundaryReceipt {
    RunBoundaryReceipt {
        run_id,
        boundary: RunApplyBoundary::RunStart,
        contributing_input_ids,
        conversation_digest: None,
        message_count: 0,
        sequence: 0,
    }
}

async fn assert_commit_rejection_preserved_staged_batch(
    driver: &SharedDriver,
    run_id: &RunId,
    staged_ids: &[InputId],
) {
    let entry = driver.lock().await;
    assert_eq!(
        entry.runtime_state(),
        RuntimeState::Running,
        "rejected commit must preserve the active run"
    );
    assert_eq!(
        entry.current_run_id(),
        Some(run_id.clone()),
        "rejected commit must preserve the active run id"
    );
    for input_id in staged_ids {
        assert_eq!(
            entry.input_phase(input_id),
            Some(crate::input_state::InputLifecycleState::Staged),
            "rejected commit must leave every contributor staged"
        );
    }
}

#[tokio::test]
async fn legacy_run_commit_rejects_receipt_run_id_mismatch_before_mutation() {
    let (driver, run_id, staged_ids) =
        staged_batch_commit_driver(make_prompt("first"), make_prompt("second")).await;

    let result = commit_runtime_loop_run(
        &driver,
        run_id.clone(),
        staged_ids.clone(),
        batch_receipt(RunId::new(), staged_ids.clone()),
        None,
    )
    .await;

    assert!(
        result.is_err(),
        "receipt for a different run id must reject before commit mutation"
    );
    assert_commit_rejection_preserved_staged_batch(&driver, &run_id, &staged_ids).await;
}

#[tokio::test]
async fn legacy_run_commit_rejects_reordered_receipt_contributors_before_mutation() {
    let (driver, run_id, staged_ids) =
        staged_batch_commit_driver(make_prompt("first"), make_prompt("second")).await;
    let mut receipt_contributors = staged_ids.clone();
    receipt_contributors.reverse();

    let result = commit_runtime_loop_run(
        &driver,
        run_id.clone(),
        staged_ids.clone(),
        batch_receipt(run_id.clone(), receipt_contributors),
        None,
    )
    .await;

    assert!(
        result.is_err(),
        "receipt contributors must exactly match consumed input ids"
    );
    assert_commit_rejection_preserved_staged_batch(&driver, &run_id, &staged_ids).await;
}

// ---------------------------------------------------------------
// A6: Deep invariant region validation via spine snapshot
// ---------------------------------------------------------------

#[tokio::test]
async fn spine_invariants_hold_after_register() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    adapter.register_session(session_id.clone()).await;

    let snapshot = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist");
    snapshot
        .validate_spine_invariants()
        .expect("all invariants should hold after register");
}

#[tokio::test]
async fn spine_invariants_hold_after_queued_input() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    adapter.register_session(session_id.clone()).await;

    let input = make_prompt("queued input");
    let (_outcome, _handle) = adapter
        .accept_input_with_completion(&session_id, input)
        .await
        .expect("prompt should be accepted");

    let snapshot = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist");
    snapshot
        .validate_spine_invariants()
        .expect("all invariants should hold after queued input");
}

#[tokio::test]
async fn spine_invariants_hold_after_destroy() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    adapter.register_session(session_id.clone()).await;

    let input = make_prompt("will be destroyed");
    let (_outcome, _handle) = adapter
        .accept_input_with_completion(&session_id, input)
        .await
        .expect("prompt should be accepted");

    let runtime_id = runtime_id_for_session(&session_id);
    crate::traits::RuntimeControlPlane::destroy(&*adapter, &runtime_id)
        .await
        .expect("destroy should succeed");

    let snapshot = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after destroy");
    snapshot
        .validate_spine_invariants()
        .expect("all invariants should hold after destroy");
}

#[tokio::test]
async fn spine_invariants_hold_after_retire() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    adapter.register_session(session_id.clone()).await;

    let input = make_prompt("will be retired");
    let (_outcome, _handle) = adapter
        .accept_input_with_completion(&session_id, input)
        .await
        .expect("prompt should be accepted");

    let runtime_id = runtime_id_for_session(&session_id);
    crate::traits::RuntimeControlPlane::retire(&*adapter, &runtime_id)
        .await
        .expect("retire should succeed");

    let snapshot = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after retire");
    snapshot
        .validate_spine_invariants()
        .expect("all invariants should hold after retire");
}

#[tokio::test]
async fn spine_invariants_hold_after_reset() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    adapter.register_session(session_id.clone()).await;

    let input = make_prompt("will be reset");
    let (_outcome, _handle) = adapter
        .accept_input_with_completion(&session_id, input)
        .await
        .expect("prompt should be accepted");

    let runtime_id = runtime_id_for_session(&session_id);
    crate::traits::RuntimeControlPlane::reset(&*adapter, &runtime_id)
        .await
        .expect("reset should succeed");

    let snapshot = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after reset");
    snapshot
        .validate_spine_invariants()
        .expect("all invariants should hold after reset");
}

#[tokio::test]
async fn spine_invariants_hold_after_recycle() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    adapter.register_session(session_id.clone()).await;

    let input = make_prompt("will be recycled");
    let (_outcome, _handle) = adapter
        .accept_input_with_completion(&session_id, input)
        .await
        .expect("prompt should be accepted");

    let runtime_id = runtime_id_for_session(&session_id);
    crate::traits::RuntimeControlPlane::recycle(&*adapter, &runtime_id)
        .await
        .expect("recycle should succeed");

    let snapshot = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist after recycle");
    snapshot
        .validate_spine_invariants()
        .expect("all invariants should hold after recycle");
}

#[tokio::test]
async fn spine_invariants_hold_after_steered_input() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    adapter.register_session(session_id.clone()).await;

    let steered = Input::Prompt(crate::input::PromptInput::new(
        "steered input",
        Some(
            meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                handling_mode: Some(meerkat_core::types::HandlingMode::Steer),
                ..Default::default()
            },
        ),
    ));
    let (_outcome, _handle) = adapter
        .accept_input_with_completion(&session_id, steered)
        .await
        .expect("steered prompt should be accepted");

    let snapshot = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist");
    snapshot
        .validate_spine_invariants()
        .expect("all invariants should hold after steered input");
}

// ---------------------------------------------------------------
// A7: PublishCommittedVisibleSet dispatch guards
// (TLA+ VisibleSurfacesMatchAppliedStateInvariant)
// ---------------------------------------------------------------

#[tokio::test]
async fn publish_committed_visible_set_succeeds_for_registered_session() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    let bindings = adapter
        .prepare_bindings(session_id.clone())
        .await
        .expect("bindings should prepare");

    let state = meerkat_core::SessionToolVisibilityState {
        active_revision: 1,
        staged_revision: 1,
        ..Default::default()
    };
    let result = adapter
        .publish_committed_visible_set(&session_id, state.clone())
        .await;
    let published = result.expect("publish should succeed for registered session");
    assert_eq!(
        published.active_revision, state.active_revision,
        "returned state should match the submitted state"
    );
    assert_eq!(
        bindings
            .tool_visibility_owner
            .visibility_state()
            .expect("owner state should be readable"),
        state,
        "publish must replace the machine-owned visibility state"
    );
}

#[tokio::test]
async fn stage_persistent_filter_updates_machine_owned_visibility_state() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    let bindings = adapter
        .prepare_bindings(session_id.clone())
        .await
        .expect("bindings should prepare");
    let filter = meerkat_core::ToolFilter::Deny(["secret".to_string()].into_iter().collect());
    let witnesses = [(
        "secret".to_string(),
        meerkat_core::ToolVisibilityWitness {
            stable_owner_key: Some("callback:test".to_string()),
            last_seen_provenance: None,
        },
    )]
    .into_iter()
    .collect();

    let revision = adapter
        .stage_persistent_filter(&session_id, filter.clone(), witnesses)
        .await
        .expect("stage should succeed");
    let state = bindings
        .tool_visibility_owner
        .visibility_state()
        .expect("owner state should be readable");
    assert_eq!(state.staged_filter, filter);
    assert_eq!(state.staged_revision, revision.0);
    assert_eq!(state.active_revision, 0);
}

#[tokio::test]
async fn request_deferred_tools_updates_machine_owned_visibility_state() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    let bindings = adapter
        .prepare_bindings(session_id.clone())
        .await
        .expect("bindings should prepare");
    let names = ["deferred_tool".to_string()].into_iter().collect();
    let witnesses = [(
        "deferred_tool".to_string(),
        meerkat_core::ToolVisibilityWitness {
            stable_owner_key: Some("callback:test".to_string()),
            last_seen_provenance: None,
        },
    )]
    .into_iter()
    .collect();

    let revision = adapter
        .request_deferred_tools(&session_id, names, witnesses)
        .await
        .expect("request should succeed");
    let state = bindings
        .tool_visibility_owner
        .visibility_state()
        .expect("owner state should be readable");
    assert!(
        state
            .staged_requested_deferred_names
            .contains("deferred_tool"),
        "requested deferred tools must be staged on the machine-owned state"
    );
    assert_eq!(state.staged_revision, revision.0);
}

#[tokio::test]
async fn request_deferred_tools_records_typed_authority_in_dsl_state() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    let _bindings = adapter
        .prepare_bindings(session_id.clone())
        .await
        .expect("bindings should prepare");
    let names = ["deferred_tool".to_string()].into_iter().collect();
    let witness = meerkat_core::ToolVisibilityWitness {
        stable_owner_key: Some("callback:test".to_string()),
        last_seen_provenance: Some(meerkat_core::ToolProvenance {
            kind: meerkat_core::ToolSourceKind::Callback,
            source_id: "test".into(),
        }),
    };

    adapter
        .request_deferred_tools(
            &session_id,
            names,
            [("deferred_tool".to_string(), witness.clone())]
                .into_iter()
                .collect(),
        )
        .await
        .expect("request should succeed");

    let sessions = adapter.sessions.read().await;
    let entry = sessions.get(&session_id).expect("session should exist");
    let authority = entry
        .dsl_authority
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    assert_eq!(
        authority
            .state
            .staged_deferred_authorities
            .get("deferred_tool"),
        Some(&crate::meerkat_machine::dsl::ToolVisibilityWitness::from(
            &witness
        )),
        "the DSL authority must carry stable owner and provenance, not only the staged name"
    );
    assert_eq!(
        authority.state.staged_deferred_names,
        ["deferred_tool".to_string()].into_iter().collect(),
        "staged names are retained only as the routing projection"
    );
}

#[tokio::test]
async fn request_deferred_tools_scopes_dsl_authority_to_requested_names() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    let bindings = adapter
        .prepare_bindings(session_id.clone())
        .await
        .expect("bindings should prepare");
    let first_witness = meerkat_core::ToolVisibilityWitness {
        stable_owner_key: Some("callback:first".to_string()),
        last_seen_provenance: Some(meerkat_core::ToolProvenance {
            kind: meerkat_core::ToolSourceKind::Callback,
            source_id: "first".into(),
        }),
    };
    adapter
        .request_deferred_tools(
            &session_id,
            ["first_tool".to_string()].into_iter().collect(),
            [("first_tool".to_string(), first_witness)]
                .into_iter()
                .collect(),
        )
        .await
        .expect("first request should succeed");
    bindings
        .tool_visibility_owner
        .stage_requested_deferred_names(BTreeSet::new())
        .expect("empty legacy staging should clear staged routing names");

    let second_witness = meerkat_core::ToolVisibilityWitness {
        stable_owner_key: Some("callback:second".to_string()),
        last_seen_provenance: Some(meerkat_core::ToolProvenance {
            kind: meerkat_core::ToolSourceKind::Callback,
            source_id: "second".into(),
        }),
    };
    adapter
        .request_deferred_tools(
            &session_id,
            ["second_tool".to_string()].into_iter().collect(),
            [("second_tool".to_string(), second_witness.clone())]
                .into_iter()
                .collect(),
        )
        .await
        .expect("stale witnesses outside staged names must not poison DSL authority");

    let sessions = adapter.sessions.read().await;
    let entry = sessions.get(&session_id).expect("session should exist");
    let authority = entry
        .dsl_authority
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    assert_eq!(
        authority.state.staged_deferred_authorities,
        [(
            "second_tool".to_string(),
            crate::meerkat_machine::dsl::ToolVisibilityWitness::from(&second_witness),
        )]
        .into_iter()
        .collect(),
        "DSL authority must remain name-scoped to the admitted staged routing set"
    );
}

#[tokio::test]
async fn request_deferred_tools_requires_machine_visible_witnesses() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    let bindings = adapter
        .prepare_bindings(session_id.clone())
        .await
        .expect("bindings should prepare");
    let names = ["deferred_tool".to_string()].into_iter().collect();

    let err = adapter
        .request_deferred_tools(&session_id, names, Default::default())
        .await
        .expect_err("missing deferred-tool witnesses should fail");

    assert!(
        err.to_string().contains("deferred_tool"),
        "missing-witness error should name the requested tool: {err}"
    );
    let state = bindings
        .tool_visibility_owner
        .visibility_state()
        .expect("owner state should be readable");
    assert!(
        state.staged_requested_deferred_names.is_empty(),
        "failed witness validation must not stage names"
    );

    let names = ["deferred_tool".to_string()].into_iter().collect();
    let err = adapter
        .request_deferred_tools(
            &session_id,
            names,
            [(
                "deferred_tool".to_string(),
                meerkat_core::ToolVisibilityWitness::default(),
            )]
            .into_iter()
            .collect(),
        )
        .await
        .expect_err("empty deferred-tool witnesses should fail");

    assert!(
        err.to_string().contains("deferred_tool"),
        "empty-witness error should name the requested tool: {err}"
    );
    let state = bindings
        .tool_visibility_owner
        .visibility_state()
        .expect("owner state should be readable");
    assert!(
        state.staged_requested_deferred_names.is_empty(),
        "failed empty-witness validation must not stage names"
    );
}

fn registered_dsl_authority_for_visibility_tests() -> mm_dsl::MeerkatMachineAuthority {
    let mut authority = mm_dsl::MeerkatMachineAuthority::new();
    authority
        .apply_signal(mm_dsl::MeerkatMachineSignal::Initialize)
        .expect("initialize DSL authority");
    mm_dsl::MeerkatMachineMutator::apply(
        &mut authority,
        mm_dsl::MeerkatMachineInput::RegisterSession {
            session_id: mm_dsl::SessionId("session-1".to_string()),
        },
    )
    .expect("register session");
    authority
}

#[test]
fn request_deferred_tools_rejects_empty_dsl_authority_witness() {
    let mut authority = registered_dsl_authority_for_visibility_tests();
    let witnesses = [(
        "deferred_tool".to_string(),
        mm_dsl::ToolVisibilityWitness::from(&meerkat_core::ToolVisibilityWitness::default()),
    )]
    .into_iter()
    .collect();
    let err = mm_dsl::MeerkatMachineMutator::apply(
        &mut authority,
        mm_dsl::MeerkatMachineInput::RequestDeferredTools {
            names: ["deferred_tool".to_string()].into_iter().collect(),
            witnesses,
        },
    )
    .expect_err("machine authority must reject empty/default deferred-tool witness");

    assert!(
        matches!(
            err,
            mm_dsl::MeerkatMachineTransitionError::GuardRejected { .. }
        ),
        "empty witness should be rejected by a DSL guard: {err:?}"
    );
    assert!(
        authority.state.staged_deferred_names.is_empty(),
        "failed DSL admission must not stage routing names"
    );
}

#[test]
fn request_deferred_tools_accepts_provenance_only_dsl_authority_witness() {
    let mut authority = registered_dsl_authority_for_visibility_tests();
    let witness = mm_dsl::ToolVisibilityWitness::from(&meerkat_core::ToolVisibilityWitness {
        stable_owner_key: None,
        last_seen_provenance: Some(meerkat_core::ToolProvenance {
            kind: meerkat_core::ToolSourceKind::Callback,
            source_id: "test".into(),
        }),
    });
    let witnesses = [("deferred_tool".to_string(), witness.clone())]
        .into_iter()
        .collect();

    mm_dsl::MeerkatMachineMutator::apply(
        &mut authority,
        mm_dsl::MeerkatMachineInput::RequestDeferredTools {
            names: ["deferred_tool".to_string()].into_iter().collect(),
            witnesses,
        },
    )
    .expect("provenance witness should carry DSL admission authority");

    assert_eq!(
        authority
            .state
            .staged_deferred_authorities
            .get("deferred_tool"),
        Some(&witness),
        "provenance-only witness must be retained as the typed admission authority"
    );
}

#[tokio::test]
async fn machine_owned_visibility_owner_promotes_staged_state_at_boundary() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    let bindings = adapter
        .prepare_bindings(session_id.clone())
        .await
        .expect("bindings should prepare");
    let filter = meerkat_core::ToolFilter::Deny(["secret".to_string()].into_iter().collect());

    adapter
        .stage_persistent_filter(&session_id, filter.clone(), Default::default())
        .await
        .expect("stage should succeed");
    let promoted = bindings
        .tool_visibility_owner
        .boundary_applied()
        .expect("boundary promotion should succeed");
    assert_eq!(promoted.active_filter, filter);
    assert_eq!(promoted.active_filter, promoted.staged_filter);
    assert_eq!(promoted.active_revision, promoted.staged_revision);
}

#[tokio::test]
async fn machine_owned_visibility_owner_promotes_deferred_authority_at_boundary() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    let bindings = adapter
        .prepare_bindings(session_id.clone())
        .await
        .expect("bindings should prepare");
    let witness = meerkat_core::ToolVisibilityWitness {
        stable_owner_key: Some("callback:test".to_string()),
        last_seen_provenance: Some(meerkat_core::ToolProvenance {
            kind: meerkat_core::ToolSourceKind::Callback,
            source_id: "test".into(),
        }),
    };

    adapter
        .request_deferred_tools(
            &session_id,
            ["deferred_tool".to_string()].into_iter().collect(),
            [("deferred_tool".to_string(), witness.clone())]
                .into_iter()
                .collect(),
        )
        .await
        .expect("request should succeed");
    let promoted = bindings
        .tool_visibility_owner
        .boundary_applied()
        .expect("boundary promotion should succeed");
    assert!(
        promoted
            .active_requested_deferred_names
            .contains("deferred_tool"),
        "owner state should promote requested deferred tool visibility"
    );

    let sessions = adapter.sessions.read().await;
    let entry = sessions.get(&session_id).expect("session should exist");
    let authority = entry
        .dsl_authority
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    assert_eq!(
        authority
            .state
            .active_deferred_authorities
            .get("deferred_tool"),
        Some(&crate::meerkat_machine::dsl::ToolVisibilityWitness::from(
            &witness
        )),
        "deferred visibility admission should promote the same typed authority as the owner state"
    );
}

#[tokio::test]
async fn replace_visibility_state_rejects_deferred_names_without_authority() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    let bindings = adapter
        .prepare_bindings(session_id.clone())
        .await
        .expect("bindings should prepare");
    let replacement = meerkat_core::SessionToolVisibilityState {
        staged_requested_deferred_names: ["deferred_tool".to_string()].into_iter().collect(),
        staged_revision: 1,
        ..Default::default()
    };

    let err = bindings
        .tool_visibility_owner
        .replace_visibility_state(replacement)
        .expect_err("replacement must not install deferred names without typed authority");

    assert!(
        err.to_string().contains("SyncVisibilityRevisions"),
        "rejection should come from the DSL authority sync: {err}"
    );
    let state = bindings
        .tool_visibility_owner
        .visibility_state()
        .expect("owner state should still be readable");
    assert!(
        state.staged_requested_deferred_names.is_empty(),
        "failed authority sync must not install staged routing names"
    );
}

#[tokio::test]
async fn replace_visibility_state_rejects_deferred_names_with_empty_authority() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    let bindings = adapter
        .prepare_bindings(session_id.clone())
        .await
        .expect("bindings should prepare");
    let replacement = meerkat_core::SessionToolVisibilityState {
        staged_requested_deferred_names: ["deferred_tool".to_string()].into_iter().collect(),
        requested_witnesses: [(
            "deferred_tool".to_string(),
            meerkat_core::ToolVisibilityWitness::default(),
        )]
        .into_iter()
        .collect(),
        staged_revision: 1,
        ..Default::default()
    };

    let err = bindings
        .tool_visibility_owner
        .replace_visibility_state(replacement)
        .expect_err("replacement must not install deferred names with empty typed authority");

    assert!(
        err.to_string().contains("SyncVisibilityRevisions"),
        "rejection should come from the DSL authority sync: {err}"
    );
    let state = bindings
        .tool_visibility_owner
        .visibility_state()
        .expect("owner state should still be readable");
    assert!(
        state.staged_requested_deferred_names.is_empty(),
        "failed empty-authority sync must not install staged routing names"
    );
}

#[tokio::test]
async fn publish_committed_visible_set_rejects_unknown_session() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();

    let state = meerkat_core::SessionToolVisibilityState::default();
    let err = adapter
        .publish_committed_visible_set(&session_id, state)
        .await
        .expect_err("publish should reject an unknown session");
    assert!(
        matches!(err, RuntimeDriverError::NotReady { .. }),
        "expected NotReady, got {err:?}"
    );
}

#[tokio::test]
async fn publish_committed_visible_set_rejects_destroyed_session() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    adapter.register_session(session_id.clone()).await;

    let runtime_id = runtime_id_for_session(&session_id);
    crate::traits::RuntimeControlPlane::destroy(&*adapter, &runtime_id)
        .await
        .expect("destroy should succeed");

    let state = meerkat_core::SessionToolVisibilityState::default();
    let err = adapter
        .publish_committed_visible_set(&session_id, state)
        .await
        .expect_err("publish should reject a destroyed session");
    assert!(
        matches!(err, RuntimeDriverError::Destroyed),
        "expected Destroyed, got {err:?}"
    );
}

#[tokio::test]
async fn publish_committed_visible_set_rejects_stale_active_revision() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    adapter.register_session(session_id.clone()).await;

    // VisibleSurfacesMatchAppliedStateInvariant: active_revision must not
    // lag behind staged_revision.
    let state = meerkat_core::SessionToolVisibilityState {
        active_revision: 1,
        staged_revision: 3,
        ..Default::default()
    };
    let err = adapter
        .publish_committed_visible_set(&session_id, state)
        .await
        .expect_err("publish should reject stale active revision");
    assert!(
        matches!(err, RuntimeDriverError::ValidationFailed { .. }),
        "expected ValidationFailed, got {err:?}"
    );
}

#[tokio::test]
async fn publish_committed_visible_set_rejects_equal_revisions_with_divergent_filters() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    adapter.register_session(session_id.clone()).await;

    let state = meerkat_core::SessionToolVisibilityState {
        active_filter: meerkat_core::ToolFilter::All,
        staged_filter: meerkat_core::ToolFilter::Deny(["secret".to_string()].into_iter().collect()),
        active_revision: 3,
        staged_revision: 3,
        ..Default::default()
    };
    let err = adapter
        .publish_committed_visible_set(&session_id, state)
        .await
        .expect_err("publish should reject equal revisions with divergent filters");
    assert!(
        matches!(err, RuntimeDriverError::ValidationFailed { .. }),
        "expected ValidationFailed, got {err:?}"
    );
}

#[tokio::test]
async fn publish_committed_visible_set_rejects_active_requested_names_outside_staged() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    adapter.register_session(session_id.clone()).await;

    let state = meerkat_core::SessionToolVisibilityState {
        active_requested_deferred_names: ["probe_tool".to_string()].into_iter().collect(),
        staged_requested_deferred_names: BTreeSet::new(),
        active_revision: 4,
        staged_revision: 3,
        ..Default::default()
    };
    let err = adapter
        .publish_committed_visible_set(&session_id, state)
        .await
        .expect_err("publish should reject active requested names outside staged names");
    assert!(
        matches!(err, RuntimeDriverError::ValidationFailed { .. }),
        "expected ValidationFailed, got {err:?}"
    );
}

#[tokio::test]
async fn publish_committed_visible_set_accepts_active_ahead_of_staged() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    let bindings = adapter
        .prepare_bindings(session_id.clone())
        .await
        .expect("bindings should prepare");

    // active_revision > staged_revision is valid — the active set has
    // advanced past the last staged projection.
    let state = runtime_parity_publish_state_with_revisions(5, 3);
    let result = adapter
        .publish_committed_visible_set(&session_id, state.clone())
        .await;
    let published = result.expect("publish should succeed when active_revision >= staged_revision");
    assert_eq!(
        published, state,
        "publish should preserve the supplied state"
    );
    assert_eq!(
        bindings
            .tool_visibility_owner
            .visibility_state()
            .expect("owner state should be readable"),
        state,
        "publish must persist active-ahead visibility state exactly"
    );
}

#[tokio::test]
async fn modeled_stage_persistent_filter_matches_runtime_after_active_ahead_reconfigure() {
    let schema = modeled_meerkat_kernel::schema();
    let fixture = build_runtime_parity_fixture(RuntimeParityPhase::Attached).await;
    install_runtime_parity_reconfigure_host(&fixture.adapter);
    SessionServiceRuntimeExt::reconfigure_session_llm_identity(
        fixture.adapter.as_ref(),
        &fixture.session_id,
        SessionLlmReconfigureRequest {
            model: Some("gpt-5.2".to_string()),
            provider: Some("openai".to_string()),
            provider_params: None,
            clear_provider_params: false,
            connection_ref: None,
            clear_connection_ref: false,
        },
    )
    .await
    .expect("reconfigure should succeed for attached fixture");

    let before = runtime_parity_snapshot_summary(&fixture.adapter, &fixture.session_id)
        .await
        .expect("pre-stage snapshot should exist");
    assert!(
        !before
            .formal_available_fields
            .contains_key("active_visibility_revision"),
        "top-level machine should no longer mirror active visibility revision",
    );
    assert!(
        !before
            .formal_available_fields
            .contains_key("staged_visibility_revision"),
        "top-level machine should no longer mirror staged visibility revision",
    );

    let input = runtime_modeled_kernel_input(
        &schema,
        &before,
        RuntimeParityProbeInput::StagePersistentFilter,
    )
    .expect("modeled stage input should build");

    let revision = fixture
        .adapter
        .stage_persistent_filter(
            &fixture.session_id,
            meerkat_core::ToolFilter::Deny(["probe_tool".to_string()].into_iter().collect()),
            runtime_parity_witnesses(),
        )
        .await
        .expect("stage should succeed after active-ahead reconfigure");
    assert_eq!(
        revision.0, 2,
        "stage should advance from max(active, staged)"
    );

    let after = runtime_parity_snapshot_summary(&fixture.adapter, &fixture.session_id)
        .await
        .expect("post-stage snapshot should exist");
    assert_modeled_meerkat_transition_matches_runtime_after(&schema, &before, &input, &after);
    fixture.cleanup().await;
}

#[tokio::test]
async fn modeled_request_deferred_tools_matches_runtime_after_active_ahead_reconfigure() {
    let schema = modeled_meerkat_kernel::schema();
    let fixture = build_runtime_parity_fixture(RuntimeParityPhase::Attached).await;
    install_runtime_parity_reconfigure_host(&fixture.adapter);
    SessionServiceRuntimeExt::reconfigure_session_llm_identity(
        fixture.adapter.as_ref(),
        &fixture.session_id,
        SessionLlmReconfigureRequest {
            model: Some("gpt-5.2".to_string()),
            provider: Some("openai".to_string()),
            provider_params: None,
            clear_provider_params: false,
            connection_ref: None,
            clear_connection_ref: false,
        },
    )
    .await
    .expect("reconfigure should succeed for attached fixture");

    let before = runtime_parity_snapshot_summary(&fixture.adapter, &fixture.session_id)
        .await
        .expect("pre-request snapshot should exist");
    let input = runtime_modeled_kernel_input(
        &schema,
        &before,
        RuntimeParityProbeInput::RequestDeferredTools,
    )
    .expect("modeled request input should build");

    let revision = fixture
        .adapter
        .request_deferred_tools(
            &fixture.session_id,
            ["probe_tool".to_string()].into_iter().collect(),
            runtime_parity_witnesses(),
        )
        .await
        .expect("request should succeed after active-ahead reconfigure");
    assert_eq!(
        revision.0, 2,
        "request should advance from max(active, staged)"
    );

    let after = runtime_parity_snapshot_summary(&fixture.adapter, &fixture.session_id)
        .await
        .expect("post-request snapshot should exist");
    assert_modeled_meerkat_transition_matches_runtime_after(&schema, &before, &input, &after);
    fixture.cleanup().await;
}

#[tokio::test]
async fn modeled_publish_matches_runtime_for_active_ahead_state() {
    let schema = modeled_meerkat_kernel::schema();
    let fixture = build_runtime_parity_fixture(RuntimeParityPhase::Idle).await;
    let before = runtime_parity_snapshot_summary(&fixture.adapter, &fixture.session_id)
        .await
        .expect("pre-publish snapshot should exist");
    let input = runtime_modeled_publish_input(5, 3);

    fixture
        .adapter
        .publish_committed_visible_set(
            &fixture.session_id,
            runtime_parity_publish_state_with_revisions(5, 3),
        )
        .await
        .expect("publish should accept active-ahead state");

    let after = runtime_parity_snapshot_summary(&fixture.adapter, &fixture.session_id)
        .await
        .expect("post-publish snapshot should exist");
    assert_modeled_meerkat_transition_matches_runtime_after(&schema, &before, &input, &after);
    fixture.cleanup().await;
}

#[derive(Clone)]
struct TestLlmReconfigureHost {
    current_identity: Arc<std::sync::Mutex<meerkat_core::SessionLlmIdentity>>,
    current_visibility_state: Arc<std::sync::Mutex<meerkat_core::SessionToolVisibilityState>>,
    target_identity: meerkat_core::SessionLlmIdentity,
    current_capability_surface: Option<SessionLlmCapabilitySurface>,
    target_capability_surface: SessionLlmCapabilitySurface,
    base_tool_names: std::collections::BTreeSet<String>,
    fail_persist: bool,
}

#[async_trait::async_trait]
impl SessionLlmReconfigureHost for TestLlmReconfigureHost {
    async fn hydrate_session_llm_state(
        &self,
        _session_id: &SessionId,
    ) -> Result<HydratedSessionLlmState, RuntimeDriverError> {
        Ok(HydratedSessionLlmState {
            current_identity: self
                .current_identity
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone(),
            current_visibility_state: self
                .current_visibility_state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone(),
            current_capability_surface: self.current_capability_surface.clone(),
            capability_surface_status: if self.current_capability_surface.is_some() {
                SessionLlmCapabilitySurfaceStatus::Resolved
            } else {
                SessionLlmCapabilitySurfaceStatus::Unresolved
            },
            base_tool_names: self.base_tool_names.clone(),
        })
    }

    async fn resolve_target_session_llm_identity(
        &self,
        request: &SessionLlmReconfigureRequest,
        _current_identity: &meerkat_core::SessionLlmIdentity,
    ) -> Result<crate::ResolvedSessionLlmReconfigure, RuntimeDriverError> {
        if request.provider.is_some() && request.model.is_none() {
            return Err(RuntimeDriverError::ValidationFailed {
                reason: "provider override requires model on an existing session".to_string(),
            });
        }
        Ok(crate::ResolvedSessionLlmReconfigure {
            target_identity: self.target_identity.clone(),
            target_capability_surface: self.target_capability_surface.clone(),
        })
    }

    async fn apply_live_session_llm_identity(
        &self,
        _session_id: &SessionId,
        identity: &meerkat_core::SessionLlmIdentity,
    ) -> Result<(), RuntimeDriverError> {
        *self
            .current_identity
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = identity.clone();
        Ok(())
    }

    async fn apply_live_session_tool_visibility_state(
        &self,
        _session_id: &SessionId,
        visibility_state: Option<meerkat_core::SessionToolVisibilityState>,
    ) -> Result<(), RuntimeDriverError> {
        *self
            .current_visibility_state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            visibility_state.unwrap_or_default();
        Ok(())
    }

    async fn persist_live_session(
        &self,
        _session_id: &SessionId,
    ) -> Result<(), RuntimeDriverError> {
        if self.fail_persist {
            Err(RuntimeDriverError::Internal(
                "injected persist failure".to_string(),
            ))
        } else {
            Ok(())
        }
    }

    async fn discard_live_session(
        &self,
        _session_id: &SessionId,
    ) -> Result<(), RuntimeDriverError> {
        Ok(())
    }
}

fn test_llm_capability_surface(image_tool_results: bool) -> SessionLlmCapabilitySurface {
    SessionLlmCapabilitySurface {
        supports_temperature: true,
        supports_thinking: true,
        supports_reasoning: true,
        inline_video: false,
        vision: true,
        image_tool_results,
        supports_web_search: true,
        realtime: false,
        call_timeout_secs: Some(60),
    }
}

fn test_llm_capability_surface_realtime() -> SessionLlmCapabilitySurface {
    SessionLlmCapabilitySurface {
        supports_temperature: true,
        supports_thinking: false,
        supports_reasoning: false,
        inline_video: false,
        vision: false,
        image_tool_results: false,
        supports_web_search: false,
        realtime: true,
        call_timeout_secs: None,
    }
}

#[tokio::test]
async fn reconfigure_session_llm_identity_rejects_idle_session() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    adapter.register_session(session_id.clone()).await;
    adapter.set_session_llm_reconfigure_host(Arc::new(TestLlmReconfigureHost {
        current_identity: Arc::new(std::sync::Mutex::new(meerkat_core::SessionLlmIdentity {
            model: "claude-sonnet-4-5".to_string(),
            provider: meerkat_core::Provider::Anthropic,
            self_hosted_server_id: None,
            provider_params: None,
            connection_ref: None,
        })),
        current_visibility_state: Arc::new(std::sync::Mutex::new(Default::default())),
        target_identity: meerkat_core::SessionLlmIdentity {
            model: "gpt-5.2".to_string(),
            provider: meerkat_core::Provider::OpenAI,
            self_hosted_server_id: None,
            provider_params: None,
            connection_ref: None,
        },
        current_capability_surface: Some(test_llm_capability_surface(true)),
        target_capability_surface: test_llm_capability_surface(false),
        base_tool_names: [meerkat_core::VIEW_IMAGE_TOOL_NAME.to_string()]
            .into_iter()
            .collect(),
        fail_persist: false,
    }));

    let err = adapter
        .reconfigure_session_llm_identity(
            &session_id,
            SessionLlmReconfigureRequest {
                model: Some("gpt-5.2".to_string()),
                provider: Some("openai".to_string()),
                provider_params: None,
                clear_provider_params: false,
                connection_ref: None,
                clear_connection_ref: false,
            },
        )
        .await
        .expect_err("idle session should reject live reconfiguration");
    assert!(
        matches!(
            err,
            RuntimeDriverError::NotReady {
                state: RuntimeState::Idle
            }
        ),
        "expected Idle rejection, got {err:?}"
    );
}

#[tokio::test]
async fn reconfigure_session_llm_identity_updates_machine_owned_visibility_on_attached_session() {
    struct NoopExecutor;

    #[async_trait::async_trait]
    impl CoreExecutor for NoopExecutor {
        async fn apply(
            &mut self,
            run_id: RunId,
            primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            Ok(CoreApplyOutput {
                receipt: RunBoundaryReceipt {
                    run_id,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                    sequence: 0,
                },
                session_snapshot: None,
                terminal: None,
            })
        }

        async fn control(&mut self, _command: RunControlCommand) -> Result<(), CoreExecutorError> {
            Ok(())
        }
    }

    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    let bindings = adapter
        .prepare_bindings(session_id.clone())
        .await
        .expect("bindings should prepare");
    adapter
        .ensure_session_with_executor(session_id.clone(), Box::new(NoopExecutor))
        .await;

    let current_identity = Arc::new(std::sync::Mutex::new(meerkat_core::SessionLlmIdentity {
        model: "claude-sonnet-4-5".to_string(),
        provider: meerkat_core::Provider::Anthropic,
        self_hosted_server_id: None,
        provider_params: None,
        connection_ref: None,
    }));
    let current_visibility_state = Arc::new(std::sync::Mutex::new(
        meerkat_core::SessionToolVisibilityState::default(),
    ));
    adapter.set_session_llm_reconfigure_host(Arc::new(TestLlmReconfigureHost {
        current_identity: Arc::clone(&current_identity),
        current_visibility_state: Arc::clone(&current_visibility_state),
        target_identity: meerkat_core::SessionLlmIdentity {
            model: "gpt-5.2".to_string(),
            provider: meerkat_core::Provider::OpenAI,
            self_hosted_server_id: None,
            provider_params: Some(serde_json::json!({ "reasoning_effort": "high" })),
            connection_ref: None,
        },
        current_capability_surface: Some(test_llm_capability_surface(true)),
        target_capability_surface: test_llm_capability_surface(false),
        base_tool_names: [meerkat_core::VIEW_IMAGE_TOOL_NAME.to_string()]
            .into_iter()
            .collect(),
        fail_persist: false,
    }));

    let report = adapter
        .reconfigure_session_llm_identity(
            &session_id,
            SessionLlmReconfigureRequest {
                model: Some("gpt-5.2".to_string()),
                provider: Some("openai".to_string()),
                provider_params: Some(serde_json::json!({ "reasoning_effort": "high" })),
                clear_provider_params: false,
                connection_ref: None,
                clear_connection_ref: false,
            },
        )
        .await
        .expect("attached session should reconfigure");

    assert_eq!(report.previous_identity.model, "claude-sonnet-4-5");
    assert_eq!(report.new_identity.model, "gpt-5.2");
    assert!(
        report.tool_visibility_delta.committed_visible_set_changed,
        "capability-owned view_image removal should change the committed visible set"
    );
    let owner_state = bindings
        .tool_visibility_owner
        .visibility_state()
        .expect("owner state should be readable");
    assert_eq!(
        owner_state.capability_base_filter,
        meerkat_core::capability_base_filter_for_image_tool_results(false)
    );
    assert_eq!(
        owner_state.active_revision, 1,
        "committed visibility revision should advance when the visible set changes"
    );
}

#[tokio::test]
async fn reconfigure_session_llm_identity_succeeds_while_running() {
    struct BlockingExecutor {
        apply_started: Arc<Notify>,
        allow_finish: Arc<Notify>,
    }

    #[async_trait::async_trait]
    impl CoreExecutor for BlockingExecutor {
        async fn apply(
            &mut self,
            run_id: RunId,
            primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            self.apply_started.notify_waiters();
            self.allow_finish.notified().await;
            Ok(CoreApplyOutput {
                receipt: RunBoundaryReceipt {
                    run_id,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                    sequence: 0,
                },
                session_snapshot: None,
                terminal: None,
            })
        }

        async fn control(&mut self, _command: RunControlCommand) -> Result<(), CoreExecutorError> {
            Ok(())
        }
    }

    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    let bindings = adapter
        .prepare_bindings(session_id.clone())
        .await
        .expect("bindings should prepare");
    let apply_started = Arc::new(Notify::new());
    let allow_finish = Arc::new(Notify::new());
    adapter
        .register_session_with_executor(
            session_id.clone(),
            Box::new(BlockingExecutor {
                apply_started: Arc::clone(&apply_started),
                allow_finish: Arc::clone(&allow_finish),
            }),
        )
        .await;

    let current_identity = Arc::new(std::sync::Mutex::new(meerkat_core::SessionLlmIdentity {
        model: "claude-sonnet-4-5".to_string(),
        provider: meerkat_core::Provider::Anthropic,
        self_hosted_server_id: None,
        provider_params: None,
        connection_ref: None,
    }));
    let current_visibility_state = Arc::new(std::sync::Mutex::new(
        meerkat_core::SessionToolVisibilityState::default(),
    ));
    adapter.set_session_llm_reconfigure_host(Arc::new(TestLlmReconfigureHost {
        current_identity: Arc::clone(&current_identity),
        current_visibility_state: Arc::clone(&current_visibility_state),
        target_identity: meerkat_core::SessionLlmIdentity {
            model: "gpt-5.2".to_string(),
            provider: meerkat_core::Provider::OpenAI,
            self_hosted_server_id: None,
            provider_params: None,
            connection_ref: None,
        },
        current_capability_surface: Some(test_llm_capability_surface(true)),
        target_capability_surface: test_llm_capability_surface(false),
        base_tool_names: [meerkat_core::VIEW_IMAGE_TOOL_NAME.to_string()]
            .into_iter()
            .collect(),
        fail_persist: false,
    }));

    let (outcome, completion_handle) = adapter
        .accept_input_with_completion(&session_id, make_prompt("hold running"))
        .await
        .expect("running input should be accepted");
    assert!(outcome.is_accepted(), "running input should be accepted");
    let completion_handle = completion_handle.expect("running input should yield completion");

    tokio::time::timeout(Duration::from_secs(1), apply_started.notified())
        .await
        .expect("executor should enter apply");

    let phase_while_running = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should exist while running")
        .control
        .phase;
    assert_eq!(
        phase_while_running,
        RuntimeState::Running,
        "post-#32 W6-J: DSL `Prepare` fires from `machine_begin_run` during the apply-in-flight, transitioning lifecycle_phase to Running; the reconfigure path enters while the loop holds a run binding, so the DSL-visible phase is Running"
    );

    let report = adapter
        .reconfigure_session_llm_identity(
            &session_id,
            SessionLlmReconfigureRequest {
                model: Some("gpt-5.2".to_string()),
                provider: Some("openai".to_string()),
                provider_params: None,
                clear_provider_params: false,
                connection_ref: None,
                clear_connection_ref: false,
            },
        )
        .await
        .expect("running session should reconfigure");
    assert_eq!(report.previous_identity.model, "claude-sonnet-4-5");
    assert_eq!(report.new_identity.model, "gpt-5.2");

    let owner_state = bindings
        .tool_visibility_owner
        .visibility_state()
        .expect("owner state should be readable while running");
    assert_eq!(
        owner_state.capability_base_filter,
        meerkat_core::capability_base_filter_for_image_tool_results(false)
    );

    let phase_after_reconfigure = adapter
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .expect("snapshot should still exist after running reconfigure")
        .control
        .phase;
    assert_eq!(
        phase_after_reconfigure,
        RuntimeState::Running,
        "post-#32 W6-J: reconfigure is a per-phase self-loop — the DSL lifecycle_phase stays at Running while the executor applies (Prepare fired at loop-start, Commit fires at completion)"
    );

    allow_finish.notify_waiters();
    let completion = completion_handle.wait().await;
    assert!(
        matches!(completion, CompletionOutcome::CompletedWithoutResult),
        "running turn should still complete normally after reconfigure: {completion:?}"
    );
}

#[tokio::test]
async fn reconfigure_session_llm_identity_rolls_back_on_persist_failure() {
    struct NoopExecutor;

    #[async_trait::async_trait]
    impl CoreExecutor for NoopExecutor {
        async fn apply(
            &mut self,
            run_id: RunId,
            primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            Ok(CoreApplyOutput {
                receipt: RunBoundaryReceipt {
                    run_id,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                    sequence: 0,
                },
                session_snapshot: None,
                terminal: None,
            })
        }

        async fn control(&mut self, _command: RunControlCommand) -> Result<(), CoreExecutorError> {
            Ok(())
        }
    }

    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    let bindings = adapter
        .prepare_bindings(session_id.clone())
        .await
        .expect("bindings should prepare");
    adapter
        .ensure_session_with_executor(session_id.clone(), Box::new(NoopExecutor))
        .await;

    let current_identity = Arc::new(std::sync::Mutex::new(meerkat_core::SessionLlmIdentity {
        model: "claude-sonnet-4-5".to_string(),
        provider: meerkat_core::Provider::Anthropic,
        self_hosted_server_id: None,
        provider_params: None,
        connection_ref: None,
    }));
    let current_visibility_state = Arc::new(std::sync::Mutex::new(
        meerkat_core::SessionToolVisibilityState::default(),
    ));
    adapter.set_session_llm_reconfigure_host(Arc::new(TestLlmReconfigureHost {
        current_identity: Arc::clone(&current_identity),
        current_visibility_state: Arc::clone(&current_visibility_state),
        target_identity: meerkat_core::SessionLlmIdentity {
            model: "gpt-5.2".to_string(),
            provider: meerkat_core::Provider::OpenAI,
            self_hosted_server_id: None,
            provider_params: None,
            connection_ref: None,
        },
        current_capability_surface: Some(test_llm_capability_surface(true)),
        target_capability_surface: test_llm_capability_surface(false),
        base_tool_names: [meerkat_core::VIEW_IMAGE_TOOL_NAME.to_string()]
            .into_iter()
            .collect(),
        fail_persist: true,
    }));

    let err = adapter
        .reconfigure_session_llm_identity(
            &session_id,
            SessionLlmReconfigureRequest {
                model: Some("gpt-5.2".to_string()),
                provider: Some("openai".to_string()),
                provider_params: None,
                clear_provider_params: false,
                connection_ref: None,
                clear_connection_ref: false,
            },
        )
        .await
        .expect_err("persist failure should abort the transition");
    assert!(
        matches!(err, RuntimeDriverError::Internal(ref message) if message.contains("injected persist failure")),
        "expected injected persist failure, got {err:?}"
    );
    assert_eq!(
        current_identity
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .model,
        "claude-sonnet-4-5",
        "live identity should roll back to the previous value"
    );
    assert_eq!(
        bindings
            .tool_visibility_owner
            .visibility_state()
            .expect("owner state should be readable")
            .capability_base_filter,
        meerkat_core::ToolFilter::All,
        "machine-owned visibility should roll back with the failed transition"
    );
}

#[tokio::test]
async fn reconfigure_session_llm_identity_discards_live_session_when_rollback_fails() {
    struct NoopExecutor;

    #[async_trait::async_trait]
    impl CoreExecutor for NoopExecutor {
        async fn apply(
            &mut self,
            run_id: RunId,
            primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            Ok(CoreApplyOutput {
                receipt: RunBoundaryReceipt {
                    run_id,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                    sequence: 0,
                },
                session_snapshot: None,
                terminal: None,
            })
        }

        async fn control(&mut self, _command: RunControlCommand) -> Result<(), CoreExecutorError> {
            Ok(())
        }
    }

    struct RollbackFailingHost {
        current_identity: Arc<std::sync::Mutex<meerkat_core::SessionLlmIdentity>>,
        current_visibility_state: Arc<std::sync::Mutex<meerkat_core::SessionToolVisibilityState>>,
        identity_apply_calls: AtomicUsize,
        discarded: AtomicBool,
    }

    #[async_trait::async_trait]
    impl SessionLlmReconfigureHost for RollbackFailingHost {
        async fn hydrate_session_llm_state(
            &self,
            _session_id: &SessionId,
        ) -> Result<HydratedSessionLlmState, RuntimeDriverError> {
            Ok(HydratedSessionLlmState {
                current_identity: self
                    .current_identity
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone(),
                current_visibility_state: self
                    .current_visibility_state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone(),
                current_capability_surface: Some(test_llm_capability_surface(true)),
                capability_surface_status: SessionLlmCapabilitySurfaceStatus::Resolved,
                base_tool_names: [meerkat_core::VIEW_IMAGE_TOOL_NAME.to_string()]
                    .into_iter()
                    .collect(),
            })
        }

        async fn resolve_target_session_llm_identity(
            &self,
            _request: &SessionLlmReconfigureRequest,
            _current_identity: &meerkat_core::SessionLlmIdentity,
        ) -> Result<crate::ResolvedSessionLlmReconfigure, RuntimeDriverError> {
            Ok(crate::ResolvedSessionLlmReconfigure {
                target_identity: meerkat_core::SessionLlmIdentity {
                    model: "gpt-5.2".to_string(),
                    provider: meerkat_core::Provider::OpenAI,
                    self_hosted_server_id: None,
                    provider_params: None,
                    connection_ref: None,
                },
                target_capability_surface: test_llm_capability_surface(false),
            })
        }

        async fn apply_live_session_llm_identity(
            &self,
            _session_id: &SessionId,
            identity: &meerkat_core::SessionLlmIdentity,
        ) -> Result<(), RuntimeDriverError> {
            let call = self.identity_apply_calls.fetch_add(1, Ordering::SeqCst);
            if call >= 1 {
                return Err(RuntimeDriverError::Internal(
                    "injected rollback identity failure".to_string(),
                ));
            }
            *self
                .current_identity
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = identity.clone();
            Ok(())
        }

        async fn apply_live_session_tool_visibility_state(
            &self,
            _session_id: &SessionId,
            visibility_state: Option<meerkat_core::SessionToolVisibilityState>,
        ) -> Result<(), RuntimeDriverError> {
            *self
                .current_visibility_state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) =
                visibility_state.unwrap_or_default();
            Ok(())
        }

        async fn persist_live_session(
            &self,
            _session_id: &SessionId,
        ) -> Result<(), RuntimeDriverError> {
            Err(RuntimeDriverError::Internal(
                "injected persist failure".to_string(),
            ))
        }

        async fn discard_live_session(
            &self,
            _session_id: &SessionId,
        ) -> Result<(), RuntimeDriverError> {
            self.discarded.store(true, Ordering::SeqCst);
            Ok(())
        }
    }

    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    let bindings = adapter
        .prepare_bindings(session_id.clone())
        .await
        .expect("bindings should prepare");
    adapter
        .ensure_session_with_executor(session_id.clone(), Box::new(NoopExecutor))
        .await;

    let host = Arc::new(RollbackFailingHost {
        current_identity: Arc::new(std::sync::Mutex::new(meerkat_core::SessionLlmIdentity {
            model: "claude-sonnet-4-5".to_string(),
            provider: meerkat_core::Provider::Anthropic,
            self_hosted_server_id: None,
            provider_params: None,
            connection_ref: None,
        })),
        current_visibility_state: Arc::new(std::sync::Mutex::new(
            meerkat_core::SessionToolVisibilityState::default(),
        )),
        identity_apply_calls: AtomicUsize::new(0),
        discarded: AtomicBool::new(false),
    });
    adapter.set_session_llm_reconfigure_host(host.clone());

    let err = adapter
        .reconfigure_session_llm_identity(
            &session_id,
            SessionLlmReconfigureRequest {
                model: Some("gpt-5.2".to_string()),
                provider: Some("openai".to_string()),
                provider_params: None,
                clear_provider_params: false,
                connection_ref: None,
                clear_connection_ref: false,
            },
        )
        .await
        .expect_err("rollback failure should abort and discard");
    assert!(
        matches!(err, RuntimeDriverError::Internal(ref message) if message.contains("failed to rollback live llm reconfiguration")),
        "expected structured rollback failure, got {err:?}"
    );
    assert!(
        host.discarded.load(Ordering::SeqCst),
        "rollback failure should discard the live session"
    );
    assert_eq!(
        bindings
            .tool_visibility_owner
            .visibility_state()
            .expect("owner state should remain readable")
            .capability_base_filter,
        meerkat_core::ToolFilter::All,
        "machine-owned visibility should clear back to unresolved/default state on discard"
    );
}

#[tokio::test]
async fn reconfigure_live_topology_drives_running_session_to_boundary_and_rebinds() {
    struct BlockingExecutor {
        apply_started: Arc<Notify>,
        allow_finish: Arc<Notify>,
        boundary_cancel_calls: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl CoreExecutor for BlockingExecutor {
        async fn apply(
            &mut self,
            run_id: RunId,
            primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            self.apply_started.notify_waiters();
            self.allow_finish.notified().await;
            Ok(CoreApplyOutput {
                receipt: RunBoundaryReceipt {
                    run_id,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                    sequence: 0,
                },
                session_snapshot: None,
                terminal: None,
            })
        }

        async fn control(&mut self, command: RunControlCommand) -> Result<(), CoreExecutorError> {
            if matches!(command, RunControlCommand::CancelAfterBoundary { .. }) {
                self.boundary_cancel_calls.fetch_add(1, Ordering::SeqCst);
            }
            Ok(())
        }
    }

    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    let apply_started = Arc::new(Notify::new());
    let allow_finish = Arc::new(Notify::new());
    let boundary_cancel_calls = Arc::new(AtomicUsize::new(0));

    adapter
        .prepare_bindings(session_id.clone())
        .await
        .expect("bindings should prepare");
    adapter
        .register_session_with_executor(
            session_id.clone(),
            Box::new(BlockingExecutor {
                apply_started: Arc::clone(&apply_started),
                allow_finish: Arc::clone(&allow_finish),
                boundary_cancel_calls: Arc::clone(&boundary_cancel_calls),
            }),
        )
        .await;

    let current_identity = Arc::new(std::sync::Mutex::new(meerkat_core::SessionLlmIdentity {
        model: "gpt-realtime".to_string(),
        provider: meerkat_core::Provider::OpenAI,
        self_hosted_server_id: None,
        provider_params: None,
        connection_ref: None,
    }));
    let current_visibility_state = Arc::new(std::sync::Mutex::new(
        meerkat_core::SessionToolVisibilityState::default(),
    ));
    // Both current and target are realtime-capable: the test verifies that
    // realtime survives a model-to-model reconfigure. Previously this targeted
    // gpt-5.2 (no realtime capability) which relied on the silent-fallback
    // behavior that D5 deleted; post-D5 the reconfigure must stay within
    // realtime-capable models.
    adapter.set_session_llm_reconfigure_host(Arc::new(TestLlmReconfigureHost {
        current_identity: Arc::clone(&current_identity),
        current_visibility_state: Arc::clone(&current_visibility_state),
        target_identity: meerkat_core::SessionLlmIdentity {
            model: "gpt-realtime".to_string(),
            provider: meerkat_core::Provider::OpenAI,
            self_hosted_server_id: None,
            provider_params: None,
            connection_ref: None,
        },
        current_capability_surface: Some(test_llm_capability_surface_realtime()),
        target_capability_surface: test_llm_capability_surface_realtime(),
        base_tool_names: [meerkat_core::VIEW_IMAGE_TOOL_NAME.to_string()]
            .into_iter()
            .collect(),
        fail_persist: false,
    }));
    adapter
        .project_realtime_attachment_intent(&session_id, true)
        .await
        .expect("intent projection should succeed");
    let authority = adapter
        .attach_live(&session_id)
        .await
        .expect("attach should mint authority");
    adapter
        .publish_realtime_attachment_signal(
            authority.clone(),
            crate::RealtimeAttachmentStatus::BindingReady,
        )
        .await
        .expect("binding-ready signal should be accepted");

    let (outcome, completion_handle) = adapter
        .accept_input_with_completion(&session_id, make_prompt("hold topology change"))
        .await
        .expect("running input should be accepted");
    assert!(outcome.is_accepted());
    let completion_handle = completion_handle.expect("running input should expose completion");
    tokio::time::timeout(Duration::from_secs(1), apply_started.notified())
        .await
        .expect("executor should enter apply");

    let expected_previous_epoch = authority.authority_epoch;
    let task = {
        let adapter = Arc::clone(&adapter);
        let authority = authority.clone();
        tokio::spawn(async move {
            adapter
                .reconfigure_live_topology(
                    authority,
                    SessionLlmReconfigureRequest {
                        model: Some("gpt-realtime".to_string()),
                        provider: Some("openai".to_string()),
                        provider_params: None,
                        clear_provider_params: false,
                        connection_ref: None,
                        clear_connection_ref: false,
                    },
                )
                .await
        })
    };

    allow_finish.notify_waiters();
    let completion = completion_handle.wait().await;
    assert!(
        matches!(completion, CompletionOutcome::CompletedWithoutResult),
        "running input should complete before topology rebind settles: {completion:?}"
    );

    let new_authority = task
        .await
        .expect("topology task should join")
        .expect("topology reconfigure should succeed");
    assert_ne!(
        new_authority.authority_epoch, expected_previous_epoch,
        "topology success should mint fresh authority"
    );
    assert_eq!(
        boundary_cancel_calls.load(Ordering::SeqCst),
        1,
        "running topology change should drive cancel_after_boundary before detach"
    );

    let status = <MeerkatMachine as SessionServiceRuntimeExt>::realtime_attachment_status(
        adapter.as_ref(),
        &session_id,
    )
    .await
    .expect("status should remain readable");
    assert_eq!(status, crate::RealtimeAttachmentStatus::BindingNotReady);
    assert_eq!(
        current_identity
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .model,
        "gpt-realtime",
        "topology success should install the target identity"
    );
}

#[tokio::test]
async fn reconfigure_live_topology_failure_before_detach_restores_prior_binding() {
    struct ResolveFailingHost;

    #[async_trait::async_trait]
    impl SessionLlmReconfigureHost for ResolveFailingHost {
        async fn hydrate_session_llm_state(
            &self,
            _session_id: &SessionId,
        ) -> Result<HydratedSessionLlmState, RuntimeDriverError> {
            Ok(HydratedSessionLlmState {
                current_identity: meerkat_core::SessionLlmIdentity {
                    model: "claude-sonnet-4-5".to_string(),
                    provider: meerkat_core::Provider::Anthropic,
                    self_hosted_server_id: None,
                    provider_params: None,
                    connection_ref: None,
                },
                current_visibility_state: meerkat_core::SessionToolVisibilityState::default(),
                current_capability_surface: Some(test_llm_capability_surface(true)),
                capability_surface_status: SessionLlmCapabilitySurfaceStatus::Resolved,
                base_tool_names: [meerkat_core::VIEW_IMAGE_TOOL_NAME.to_string()]
                    .into_iter()
                    .collect(),
            })
        }

        async fn resolve_target_session_llm_identity(
            &self,
            _request: &SessionLlmReconfigureRequest,
            _current_identity: &meerkat_core::SessionLlmIdentity,
        ) -> Result<crate::ResolvedSessionLlmReconfigure, RuntimeDriverError> {
            Err(RuntimeDriverError::ValidationFailed {
                reason: "injected pre-detach resolution failure".to_string(),
            })
        }

        async fn apply_live_session_llm_identity(
            &self,
            _session_id: &SessionId,
            _identity: &meerkat_core::SessionLlmIdentity,
        ) -> Result<(), RuntimeDriverError> {
            Ok(())
        }

        async fn apply_live_session_tool_visibility_state(
            &self,
            _session_id: &SessionId,
            _visibility_state: Option<meerkat_core::SessionToolVisibilityState>,
        ) -> Result<(), RuntimeDriverError> {
            Ok(())
        }

        async fn persist_live_session(
            &self,
            _session_id: &SessionId,
        ) -> Result<(), RuntimeDriverError> {
            Ok(())
        }

        async fn discard_live_session(
            &self,
            _session_id: &SessionId,
        ) -> Result<(), RuntimeDriverError> {
            Ok(())
        }
    }

    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    adapter
        .register_session_with_executor(session_id.clone(), Box::new(RuntimeParityNoopExecutor))
        .await;
    adapter.set_session_llm_reconfigure_host(Arc::new(ResolveFailingHost));
    adapter
        .project_realtime_attachment_intent(&session_id, true)
        .await
        .expect("intent projection should succeed");
    let authority = adapter
        .attach_live(&session_id)
        .await
        .expect("attach should mint authority");
    adapter
        .publish_realtime_attachment_signal(
            authority.clone(),
            crate::RealtimeAttachmentStatus::BindingReady,
        )
        .await
        .expect("binding-ready signal should be accepted");

    let err = adapter
        .reconfigure_live_topology(
            authority.clone(),
            SessionLlmReconfigureRequest {
                model: Some("gpt-5.2".to_string()),
                provider: Some("openai".to_string()),
                provider_params: None,
                clear_provider_params: false,
                connection_ref: None,
                clear_connection_ref: false,
            },
        )
        .await
        .expect_err("pre-detach failure should abort without tearing down binding");
    assert!(
        matches!(err, RuntimeDriverError::ValidationFailed { .. }),
        "expected ValidationFailed, got {err:?}"
    );

    adapter
        .publish_realtime_attachment_signal(
            authority,
            crate::RealtimeAttachmentStatus::BindingReady,
        )
        .await
        .expect("old authority should remain valid after pre-detach failure");
    let status = <MeerkatMachine as SessionServiceRuntimeExt>::realtime_attachment_status(
        &adapter,
        &session_id,
    )
    .await
    .expect("status should remain readable");
    assert_eq!(status, crate::RealtimeAttachmentStatus::BindingReady);
}

#[tokio::test]
async fn reconfigure_live_topology_failure_after_detach_discards_and_requires_reattach() {
    struct PersistFailingTopologyHost {
        discarded: AtomicBool,
    }

    #[async_trait::async_trait]
    impl SessionLlmReconfigureHost for PersistFailingTopologyHost {
        async fn hydrate_session_llm_state(
            &self,
            _session_id: &SessionId,
        ) -> Result<HydratedSessionLlmState, RuntimeDriverError> {
            Ok(HydratedSessionLlmState {
                current_identity: meerkat_core::SessionLlmIdentity {
                    model: "claude-sonnet-4-5".to_string(),
                    provider: meerkat_core::Provider::Anthropic,
                    self_hosted_server_id: None,
                    provider_params: None,
                    connection_ref: None,
                },
                current_visibility_state: meerkat_core::SessionToolVisibilityState::default(),
                current_capability_surface: Some(test_llm_capability_surface(true)),
                capability_surface_status: SessionLlmCapabilitySurfaceStatus::Resolved,
                base_tool_names: [meerkat_core::VIEW_IMAGE_TOOL_NAME.to_string()]
                    .into_iter()
                    .collect(),
            })
        }

        async fn resolve_target_session_llm_identity(
            &self,
            _request: &SessionLlmReconfigureRequest,
            _current_identity: &meerkat_core::SessionLlmIdentity,
        ) -> Result<crate::ResolvedSessionLlmReconfigure, RuntimeDriverError> {
            Ok(crate::ResolvedSessionLlmReconfigure {
                target_identity: meerkat_core::SessionLlmIdentity {
                    model: "gpt-5.2".to_string(),
                    provider: meerkat_core::Provider::OpenAI,
                    self_hosted_server_id: None,
                    provider_params: None,
                    connection_ref: None,
                },
                target_capability_surface: test_llm_capability_surface(false),
            })
        }

        async fn apply_live_session_llm_identity(
            &self,
            _session_id: &SessionId,
            _identity: &meerkat_core::SessionLlmIdentity,
        ) -> Result<(), RuntimeDriverError> {
            Ok(())
        }

        async fn apply_live_session_tool_visibility_state(
            &self,
            _session_id: &SessionId,
            _visibility_state: Option<meerkat_core::SessionToolVisibilityState>,
        ) -> Result<(), RuntimeDriverError> {
            Ok(())
        }

        async fn persist_live_session(
            &self,
            _session_id: &SessionId,
        ) -> Result<(), RuntimeDriverError> {
            Err(RuntimeDriverError::Internal(
                "injected post-detach persist failure".to_string(),
            ))
        }

        async fn discard_live_session(
            &self,
            _session_id: &SessionId,
        ) -> Result<(), RuntimeDriverError> {
            self.discarded.store(true, Ordering::SeqCst);
            Ok(())
        }
    }

    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    adapter
        .register_session_with_executor(session_id.clone(), Box::new(RuntimeParityNoopExecutor))
        .await;
    let host = Arc::new(PersistFailingTopologyHost {
        discarded: AtomicBool::new(false),
    });
    adapter.set_session_llm_reconfigure_host(host.clone());
    adapter
        .project_realtime_attachment_intent(&session_id, true)
        .await
        .expect("intent projection should succeed");
    let authority = adapter
        .attach_live(&session_id)
        .await
        .expect("attach should mint authority");
    adapter
        .publish_realtime_attachment_signal(
            authority.clone(),
            crate::RealtimeAttachmentStatus::BindingReady,
        )
        .await
        .expect("binding-ready signal should be accepted");

    let err = adapter
        .reconfigure_live_topology(
            authority,
            SessionLlmReconfigureRequest {
                model: Some("gpt-5.2".to_string()),
                provider: Some("openai".to_string()),
                provider_params: None,
                clear_provider_params: false,
                connection_ref: None,
                clear_connection_ref: false,
            },
        )
        .await
        .expect_err("post-detach failure should discard and require reattach");
    assert!(
        matches!(err, RuntimeDriverError::Internal(ref message) if message.contains("failed after detach")),
        "expected post-detach failure, got {err:?}"
    );
    assert!(
        host.discarded.load(Ordering::SeqCst),
        "post-detach failure should discard the live session"
    );
    let status = <MeerkatMachine as SessionServiceRuntimeExt>::realtime_attachment_status(
        &adapter,
        &session_id,
    )
    .await
    .expect("status should remain readable");
    assert_eq!(status, crate::RealtimeAttachmentStatus::ReattachRequired);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum RuntimeParityClassification {
    SameSurface,
    DifferentSurface,
    LeftOnly,
    RightOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum RuntimeParityPhase {
    Idle,
    Attached,
    Running,
    Retired,
    Stopped,
}

impl RuntimeParityPhase {
    fn schema_name(self) -> &'static str {
        match self {
            Self::Idle => "Idle",
            Self::Attached => "Attached",
            Self::Running => "Running",
            Self::Retired => "Retired",
            Self::Stopped => "Stopped",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum RuntimeParityOutcomeKind {
    Ok,
    Err,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct RuntimeParitySnapshotSummary {
    phase: String,
    current_run_present: bool,
    formal_session_id: Option<String>,
    formal_active_runtime_id: Option<String>,
    formal_current_run_id: Option<String>,
    pre_run_phase: Option<String>,
    attachment_live: bool,
    queue_len: usize,
    steer_queue_len: usize,
    current_run_contributor_count: usize,
    admitted_input_count: usize,
    post_admission_signal: String,
    ledger_input_count: usize,
    ledger_non_terminal_count: usize,
    ledger_accepted_count: usize,
    ledger_queued_count: usize,
    ledger_staged_count: usize,
    ledger_applied_count: usize,
    ledger_applied_pending_consumption_count: usize,
    ledger_consumed_count: usize,
    ledger_superseded_count: usize,
    ledger_coalesced_count: usize,
    ledger_abandoned_count: usize,
    wait_request_present: bool,
    drain_slot_present: bool,
    drain_phase: Option<String>,
    formal_available_fields: std::collections::BTreeMap<String, String>,
    formal_unavailable_fields: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RuntimeParityObservableSurface {
    outcome_kind: RuntimeParityOutcomeKind,
    result_summary: String,
    after: Option<RuntimeParitySnapshotSummary>,
}

#[derive(Debug, Clone, Serialize)]
struct RuntimeParityInvocationReport {
    phase: String,
    setup_tags: Vec<String>,
    before: Option<RuntimeParitySnapshotSummary>,
    outcome_kind: RuntimeParityOutcomeKind,
    result_summary: String,
    after: Option<RuntimeParitySnapshotSummary>,
}

impl RuntimeParityInvocationReport {
    fn observable_surface(&self) -> RuntimeParityObservableSurface {
        RuntimeParityObservableSurface {
            outcome_kind: self.outcome_kind,
            result_summary: self.result_summary.clone(),
            after: self
                .after
                .as_ref()
                .map(runtime_parity_normalize_observable_snapshot),
        }
    }
}

fn runtime_parity_normalize_observable_snapshot(
    snapshot: &RuntimeParitySnapshotSummary,
) -> RuntimeParitySnapshotSummary {
    let mut normalized = snapshot.clone();
    normalized.formal_session_id = Some("\"<session-id>\"".to_string());
    normalized.formal_active_runtime_id = Some("\"<runtime-id>\"".to_string());
    normalized.formal_current_run_id = normalized
        .formal_current_run_id
        .as_ref()
        .map(|_| "\"<run-id>\"".to_string());
    normalized
}

fn assert_runtime_parity_identity_stability(
    probe: RuntimeParityProbeInput,
    before: Option<&RuntimeParitySnapshotSummary>,
    after: Option<&RuntimeParitySnapshotSummary>,
) {
    let (Some(before), Some(after)) = (before, after) else {
        return;
    };

    assert_eq!(
        before.formal_session_id, after.formal_session_id,
        "runtime parity probe {probe:?} should keep formal session_id stable"
    );
    assert_eq!(
        before.formal_active_runtime_id, after.formal_active_runtime_id,
        "runtime parity probe {probe:?} should keep formal active_runtime_id stable"
    );
    if before.formal_current_run_id.is_some() && after.formal_current_run_id.is_some() {
        assert_eq!(
            before.formal_current_run_id, after.formal_current_run_id,
            "runtime parity probe {probe:?} should not churn current_run_id while a run remains bound"
        );
    }
}

#[derive(Debug, Clone, Serialize)]
struct RuntimeParitySchemaTransitionSummary {
    transition: String,
    to_phase: String,
    binding_names: Vec<String>,
    guard_names: Vec<String>,
    update_count: usize,
    update_signatures: Vec<String>,
    effect_variants: Vec<String>,
}

#[derive(Debug, Clone)]
struct RuntimeParitySchemaRow {
    input_variant: String,
    classification: RuntimeParityClassification,
    left: Vec<RuntimeParitySchemaTransitionSummary>,
    right: Vec<RuntimeParitySchemaTransitionSummary>,
}

#[derive(Debug, Serialize)]
struct RuntimeParityProbeReport {
    schema_classification: RuntimeParityClassification,
    runtime_classification: RuntimeParityClassification,
    agrees_with_schema: bool,
    schema_left: RuntimeModeledStateSchemaReport,
    schema_right: RuntimeModeledStateSchemaReport,
    left: RuntimeParityInvocationReport,
    right: RuntimeParityInvocationReport,
}

#[derive(Debug, Serialize)]
struct RuntimeParityRowReport {
    input_variant: String,
    probe_required: bool,
    static_schema_classification: RuntimeParityClassification,
    static_schema_left: Vec<RuntimeParitySchemaTransitionSummary>,
    static_schema_right: Vec<RuntimeParitySchemaTransitionSummary>,
    probe: Option<RuntimeParityProbeReport>,
    note: Option<String>,
}

#[derive(Debug, Default, Serialize)]
struct RuntimeParityPairSummary {
    interesting_rows: usize,
    probed_rows: usize,
    aligned_rows: usize,
    mismatched_rows: usize,
    unprobed_rows: usize,
    surface_only_unprobed_rows: usize,
}

#[derive(Debug, Serialize)]
struct RuntimeParityPairReport {
    left_phase: String,
    right_phase: String,
    summary: RuntimeParityPairSummary,
    rows: Vec<RuntimeParityRowReport>,
}

#[derive(Debug, Default, Serialize)]
struct RuntimeParityAuditSummary {
    pair_count: usize,
    interesting_rows: usize,
    probed_rows: usize,
    aligned_rows: usize,
    mismatched_rows: usize,
    unprobed_rows: usize,
    surface_only_unprobed_rows: usize,
}

#[derive(Debug, Serialize)]
struct RuntimeParityAuditReport {
    machine: String,
    generated_at: String,
    summary: RuntimeParityAuditSummary,
    pairs: Vec<RuntimeParityPairReport>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum RuntimeModeledStateOutcomeKind {
    Ok,
    Err,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct RuntimeModeledStateSummary {
    phase: String,
    formal_fields: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize)]
struct RuntimeModeledStateRuntimeReport {
    phase: String,
    outcome_kind: RuntimeModeledStateOutcomeKind,
    before: Option<RuntimeModeledStateSummary>,
    after: Option<RuntimeModeledStateSummary>,
    result_summary: String,
    surface_summary: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct RuntimeModeledStateSchemaReport {
    outcome_kind: RuntimeModeledStateOutcomeKind,
    after: Option<RuntimeModeledStateSummary>,
    detail: String,
    result_summary: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct RuntimeModeledStateRowReport {
    phase: String,
    input_variant: String,
    aligned: bool,
    differing_keys: Vec<String>,
    runtime: RuntimeModeledStateRuntimeReport,
    schema: RuntimeModeledStateSchemaReport,
}

#[derive(Debug, Default, Serialize)]
struct RuntimeModeledStateAuditSummary {
    row_count: usize,
    aligned_rows: usize,
    mismatched_rows: usize,
    unprobed_rows: usize,
}

#[derive(Debug, Serialize)]
struct RuntimeModeledStateAuditReport {
    machine: String,
    generated_at: String,
    summary: RuntimeModeledStateAuditSummary,
    rows: Vec<RuntimeModeledStateRowReport>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RuntimeParityProbeInput {
    RegisterSession,
    UnregisterSession,
    EnsureSessionWithExecutor,
    SetSilentIntents,
    ReconfigureSessionLlmIdentity,
    ContainsSession,
    SessionHasExecutor,
    SessionHasComms,
    OpsLifecycleRegistry,
    PrepareBindings,
    InputState,
    ListActiveInputs,
    SetPeerIngressContext,
    NotifyDrainExited,
    InterruptCurrentRun,
    CancelAfterBoundary,
    StagePersistentFilter,
    RequestDeferredTools,
    PublishCommittedVisibleSet,
    AbortAll,
    Abort,
    Wait,
    Ingest,
    PublishEvent,
    Recover,
    Retire,
    Recycle,
    RuntimeState,
    LoadBoundaryReceipt,
    AcceptWithCompletion,
    AcceptWithoutWake,
    Prepare,
    Commit,
    Fail,
    Reset,
    StopRuntimeExecutor,
    Destroy,
}

struct RuntimeParityFixture {
    adapter: Arc<MeerkatMachine>,
    session_id: SessionId,
    runtime_id: LogicalRuntimeId,
    running_release: Option<Arc<Notify>>,
    running_completion: Option<crate::completion::CompletionHandle>,
    prepared_input_id: Option<InputId>,
    prepared_run_id: Option<RunId>,
}

impl RuntimeParityFixture {
    async fn cleanup(mut self) {
        if let Some(release) = self.running_release.take() {
            release.notify_waiters();
        }
        if let Some(handle) = self.running_completion.take() {
            let _ = tokio::time::timeout(Duration::from_millis(250), handle.wait()).await;
        }
        let _ = self
            .adapter
            .stop_runtime_executor(
                &self.session_id,
                RunControlCommand::StopRuntimeExecutor {
                    reason: "runtime parity cleanup".to_string(),
                },
            )
            .await;
        let _ =
            crate::traits::RuntimeControlPlane::destroy(self.adapter.as_ref(), &self.runtime_id)
                .await;
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

struct RuntimeParityNoopExecutor;

#[async_trait::async_trait]
impl CoreExecutor for RuntimeParityNoopExecutor {
    async fn apply(
        &mut self,
        run_id: RunId,
        primitive: RunPrimitive,
    ) -> Result<CoreApplyOutput, CoreExecutorError> {
        Ok(CoreApplyOutput {
            receipt: RunBoundaryReceipt {
                run_id,
                boundary: RunApplyBoundary::RunStart,
                contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                conversation_digest: None,
                message_count: 0,
                sequence: 0,
            },
            session_snapshot: None,
            terminal: None,
        })
    }

    async fn control(&mut self, _command: RunControlCommand) -> Result<(), CoreExecutorError> {
        Ok(())
    }
}

struct RuntimeParityBlockingExecutor {
    apply_started: Arc<Notify>,
    allow_finish: Arc<Notify>,
}

#[async_trait::async_trait]
impl CoreExecutor for RuntimeParityBlockingExecutor {
    async fn apply(
        &mut self,
        run_id: RunId,
        primitive: RunPrimitive,
    ) -> Result<CoreApplyOutput, CoreExecutorError> {
        self.apply_started.notify_waiters();
        self.allow_finish.notified().await;
        Ok(CoreApplyOutput {
            receipt: RunBoundaryReceipt {
                run_id,
                boundary: RunApplyBoundary::RunStart,
                contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                conversation_digest: None,
                message_count: 0,
                sequence: 0,
            },
            session_snapshot: None,
            terminal: None,
        })
    }

    async fn control(&mut self, _command: RunControlCommand) -> Result<(), CoreExecutorError> {
        Ok(())
    }
}

fn runtime_parity_report_path() -> PathBuf {
    std::env::temp_dir().join("meerkat-runtime-phase-parity.json")
}

fn runtime_parity_full_report_path() -> PathBuf {
    std::env::temp_dir().join("meerkat-runtime-phase-parity-full.json")
}

fn runtime_modeled_state_report_path() -> PathBuf {
    std::env::temp_dir().join("meerkat-runtime-modeled-state-parity.json")
}

fn runtime_parity_target_pairs() -> &'static [(RuntimeParityPhase, RuntimeParityPhase)] {
    &[
        (RuntimeParityPhase::Attached, RuntimeParityPhase::Idle),
        (RuntimeParityPhase::Attached, RuntimeParityPhase::Running),
        (RuntimeParityPhase::Running, RuntimeParityPhase::Stopped),
        (RuntimeParityPhase::Running, RuntimeParityPhase::Retired),
        (RuntimeParityPhase::Idle, RuntimeParityPhase::Retired),
        (RuntimeParityPhase::Idle, RuntimeParityPhase::Stopped),
        (RuntimeParityPhase::Attached, RuntimeParityPhase::Retired),
        (RuntimeParityPhase::Attached, RuntimeParityPhase::Stopped),
        (RuntimeParityPhase::Idle, RuntimeParityPhase::Running),
        (RuntimeParityPhase::Retired, RuntimeParityPhase::Stopped),
    ]
}

fn runtime_parity_probe_for_input_variant(input_variant: &str) -> Option<RuntimeParityProbeInput> {
    match input_variant {
        "RegisterSession" => Some(RuntimeParityProbeInput::RegisterSession),
        "UnregisterSession" => Some(RuntimeParityProbeInput::UnregisterSession),
        "EnsureSessionWithExecutor" => Some(RuntimeParityProbeInput::EnsureSessionWithExecutor),
        "SetSilentIntents" => Some(RuntimeParityProbeInput::SetSilentIntents),
        "ReconfigureSessionLlmIdentity" => {
            Some(RuntimeParityProbeInput::ReconfigureSessionLlmIdentity)
        }
        "ContainsSession" => Some(RuntimeParityProbeInput::ContainsSession),
        "SessionHasExecutor" => Some(RuntimeParityProbeInput::SessionHasExecutor),
        "SessionHasComms" => Some(RuntimeParityProbeInput::SessionHasComms),
        "OpsLifecycleRegistry" => Some(RuntimeParityProbeInput::OpsLifecycleRegistry),
        "PrepareBindings" => Some(RuntimeParityProbeInput::PrepareBindings),
        "InputState" => Some(RuntimeParityProbeInput::InputState),
        "ListActiveInputs" => Some(RuntimeParityProbeInput::ListActiveInputs),
        "SetPeerIngressContext" => Some(RuntimeParityProbeInput::SetPeerIngressContext),
        "NotifyDrainExited" => Some(RuntimeParityProbeInput::NotifyDrainExited),
        "InterruptCurrentRun" => Some(RuntimeParityProbeInput::InterruptCurrentRun),
        "CancelAfterBoundary" => Some(RuntimeParityProbeInput::CancelAfterBoundary),
        "StagePersistentFilter" => Some(RuntimeParityProbeInput::StagePersistentFilter),
        "RequestDeferredTools" => Some(RuntimeParityProbeInput::RequestDeferredTools),
        "PublishCommittedVisibleSet" => Some(RuntimeParityProbeInput::PublishCommittedVisibleSet),
        "AbortAll" => Some(RuntimeParityProbeInput::AbortAll),
        "Abort" => Some(RuntimeParityProbeInput::Abort),
        "Wait" => Some(RuntimeParityProbeInput::Wait),
        "Ingest" => Some(RuntimeParityProbeInput::Ingest),
        "PublishEvent" => Some(RuntimeParityProbeInput::PublishEvent),
        "Recover" => Some(RuntimeParityProbeInput::Recover),
        "Retire" => Some(RuntimeParityProbeInput::Retire),
        "Recycle" => Some(RuntimeParityProbeInput::Recycle),
        "RuntimeState" => Some(RuntimeParityProbeInput::RuntimeState),
        "LoadBoundaryReceipt" => Some(RuntimeParityProbeInput::LoadBoundaryReceipt),
        "AcceptWithCompletion" => Some(RuntimeParityProbeInput::AcceptWithCompletion),
        "AcceptWithoutWake" => Some(RuntimeParityProbeInput::AcceptWithoutWake),
        "Prepare" => Some(RuntimeParityProbeInput::Prepare),
        "Commit" => Some(RuntimeParityProbeInput::Commit),
        "Fail" => Some(RuntimeParityProbeInput::Fail),
        "Reset" => Some(RuntimeParityProbeInput::Reset),
        "StopRuntimeExecutor" => Some(RuntimeParityProbeInput::StopRuntimeExecutor),
        "Destroy" => Some(RuntimeParityProbeInput::Destroy),
        _ => None,
    }
}

fn runtime_parity_state_label(state: RuntimeState) -> String {
    format!("{state:?}")
}

fn runtime_parity_drain_phase_label(phase: CommsDrainPhase) -> String {
    format!("{phase:?}")
}

fn normalize_runtime_parity_formal_fields(
    mut fields: std::collections::BTreeMap<String, String>,
) -> std::collections::BTreeMap<String, String> {
    if let Some(value) = fields.get_mut("session_id") {
        *value = "\"<session-id>\"".to_string();
    }
    if let Some(value) = fields.get_mut("active_runtime_id") {
        *value = "\"<runtime-id>\"".to_string();
    }
    if let Some(value) = fields.get_mut("current_run_id")
        && value != "null"
    {
        *value = "\"<run-id>\"".to_string();
    }
    for value in fields.values_mut() {
        *value = runtime_modeled_normalize_formal_string(value);
    }
    fields
}

fn runtime_parity_formal_identity_field(
    fields: &std::collections::BTreeMap<String, String>,
    key: &str,
) -> Option<String> {
    match fields.get(key).map(String::as_str) {
        Some("null") | None => None,
        Some(value) => Some(value.to_string()),
    }
}

fn runtime_modeled_normalize_json_value(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Array(items) => serde_json::Value::Array(
            items
                .into_iter()
                .map(runtime_modeled_normalize_json_value)
                .collect(),
        ),
        serde_json::Value::Object(entries) => {
            let mut normalized = serde_json::Map::new();
            let mut keys: Vec<_> = entries.into_iter().collect();
            keys.sort_by(|(left, _), (right, _)| left.cmp(right));
            for (key, value) in keys {
                normalized.insert(key, runtime_modeled_normalize_json_value(value));
            }
            serde_json::Value::Object(normalized)
        }
        other => other,
    }
}

fn runtime_modeled_normalize_formal_string(raw: &str) -> String {
    serde_json::from_str(raw)
        .map(runtime_modeled_normalize_json_value)
        .and_then(|value| serde_json::to_string(&value))
        .unwrap_or_else(|_| raw.to_string())
}

fn runtime_modeled_default_kernel_value(ty: &TypeRef) -> KernelValue {
    match ty {
        TypeRef::Bool => KernelValue::Bool(false),
        TypeRef::U32 | TypeRef::U64 => KernelValue::U64(0),
        TypeRef::String => KernelValue::String(String::new()),
        TypeRef::Named(name) => {
            if let Some(value) = runtime_modeled_default_string_enum_named_value(name) {
                value
            } else {
                runtime_modeled_named_value(
                    name,
                    if name.as_str() == "ToolVisibilityWitness" {
                        KernelValue::Map(BTreeMap::new())
                    } else if runtime_modeled_named_type_is_u64(name.as_str()) {
                        KernelValue::U64(0)
                    } else {
                        KernelValue::String(String::new())
                    },
                )
            }
        }
        TypeRef::Enum(name) => {
            runtime_modeled_default_string_enum_variant(name).unwrap_or_else(|| {
                KernelValue::NamedVariant {
                    enum_name: name.clone(),
                    variant: meerkat_machine_schema::identity::EnumVariantId::parse("_")
                        .expect("valid placeholder slug"),
                }
            })
        }
        TypeRef::Option(_) => KernelValue::None,
        TypeRef::Set(_) => KernelValue::Set(BTreeSet::new()),
        TypeRef::Seq(_) => KernelValue::Seq(Vec::new()),
        TypeRef::Map(_, _) => KernelValue::Map(BTreeMap::new()),
    }
}

fn runtime_modeled_named_type_is_u64(name: &str) -> bool {
    matches!(
        name,
        "BoundarySequence" | "TurnNumber" | "FenceToken" | "Generation"
    )
}

fn runtime_modeled_string_enum_variants(
    name: &meerkat_machine_schema::identity::NamedTypeId,
) -> Option<Vec<meerkat_machine_schema::identity::EnumVariantId>> {
    let schema = modeled_meerkat_kernel::schema();
    let binding = schema.named_type_binding(name)?;
    match &binding.rust {
        meerkat_machine_schema::identity::RustTypeAtom::StringEnum { variants } => {
            Some(variants.clone())
        }
        _ => None,
    }
}

fn runtime_modeled_string_enum_named_value_from_raw(
    name: &meerkat_machine_schema::identity::NamedTypeId,
    raw: &str,
) -> Option<KernelValue> {
    let enum_name = meerkat_machine_schema::identity::EnumTypeId::parse(name.as_str()).ok()?;
    runtime_modeled_string_enum_variants(name)?
        .into_iter()
        .find(|variant| variant.as_str() == raw)
        .map(|variant| KernelValue::NamedVariant { enum_name, variant })
}

fn runtime_modeled_default_string_enum_named_value(
    name: &meerkat_machine_schema::identity::NamedTypeId,
) -> Option<KernelValue> {
    let enum_name = meerkat_machine_schema::identity::EnumTypeId::parse(name.as_str()).ok()?;
    runtime_modeled_string_enum_variants(name)?
        .into_iter()
        .next()
        .map(|variant| KernelValue::NamedVariant { enum_name, variant })
}

fn runtime_modeled_default_string_enum_variant(
    name: &meerkat_machine_schema::identity::EnumTypeId,
) -> Option<KernelValue> {
    let named_type = meerkat_machine_schema::identity::NamedTypeId::parse(name.as_str()).ok()?;
    runtime_modeled_string_enum_variants(&named_type)?
        .into_iter()
        .next()
        .map(|variant| KernelValue::NamedVariant {
            enum_name: name.clone(),
            variant,
        })
}

fn runtime_modeled_string_enum_named_value_from_json(
    name: &meerkat_machine_schema::identity::NamedTypeId,
    value: &serde_json::Value,
) -> Option<KernelValue> {
    runtime_modeled_string_enum_named_value_from_raw(name, value.as_str()?)
}

fn runtime_modeled_string_enum_named_value(
    name: &meerkat_machine_schema::identity::NamedTypeId,
    value: &KernelValue,
) -> Option<KernelValue> {
    match value {
        KernelValue::NamedVariant { enum_name, variant } if enum_name.as_str() == name.as_str() => {
            runtime_modeled_string_enum_variants(name)?
                .iter()
                .any(|allowed| allowed == variant)
                .then(|| value.clone())
        }
        KernelValue::Named { type_name, value } if type_name == name => {
            runtime_modeled_string_enum_named_value(name, value)
        }
        KernelValue::String(raw) => serde_json::from_str::<String>(raw)
            .ok()
            .or_else(|| Some(raw.clone()))
            .and_then(|raw| runtime_modeled_string_enum_named_value_from_raw(name, &raw)),
        _ => None,
    }
}

fn runtime_modeled_named_value(
    type_name: &meerkat_machine_schema::identity::NamedTypeId,
    value: KernelValue,
) -> KernelValue {
    if matches!(
        &value,
        KernelValue::Named {
            type_name: existing,
            ..
        } if existing == type_name
    ) {
        return value;
    }

    if let Some(value) = runtime_modeled_string_enum_named_value(type_name, &value) {
        return value;
    }

    let value = if type_name.as_str() == "ToolVisibilityWitness" {
        runtime_modeled_tool_visibility_witness_inner(value)
    } else if runtime_modeled_named_type_is_u64(type_name.as_str()) {
        match value {
            KernelValue::U64(value) => KernelValue::U64(value),
            KernelValue::String(value) => KernelValue::U64(
                serde_json::from_str::<u64>(&value)
                    .ok()
                    .or_else(|| value.parse::<u64>().ok())
                    .unwrap_or_default(),
            ),
            _ => KernelValue::U64(0),
        }
    } else {
        match value {
            KernelValue::String(value) => KernelValue::String(value),
            KernelValue::U64(value) => KernelValue::String(value.to_string()),
            other => KernelValue::String(
                serde_json::to_string(&runtime_modeled_json_from_kernel_value(&other))
                    .unwrap_or_default(),
            ),
        }
    };

    KernelValue::Named {
        type_name: type_name.clone(),
        value: Box::new(value),
    }
}

fn runtime_modeled_coerce_value_to_type(ty: &TypeRef, value: KernelValue) -> KernelValue {
    match ty {
        TypeRef::Named(name) => runtime_modeled_named_value(name, value),
        TypeRef::Option(inner) => match value {
            KernelValue::None => KernelValue::None,
            KernelValue::Map(mut entries)
                if entries.len() == 1
                    && entries.contains_key(&KernelValue::String("value".to_string())) =>
            {
                let key = KernelValue::String("value".to_string());
                let inner_value = entries.remove(&key).unwrap_or(KernelValue::None);
                runtime_modeled_option_some(runtime_modeled_coerce_value_to_type(
                    inner,
                    inner_value,
                ))
            }
            other => {
                runtime_modeled_option_some(runtime_modeled_coerce_value_to_type(inner, other))
            }
        },
        TypeRef::Set(inner) => match value {
            KernelValue::Set(items) => KernelValue::Set(
                items
                    .into_iter()
                    .map(|item| runtime_modeled_coerce_value_to_type(inner, item))
                    .collect(),
            ),
            other => other,
        },
        TypeRef::Seq(inner) => match value {
            KernelValue::Seq(items) => KernelValue::Seq(
                items
                    .into_iter()
                    .map(|item| runtime_modeled_coerce_value_to_type(inner, item))
                    .collect(),
            ),
            other => other,
        },
        TypeRef::Map(key_ty, value_ty) => match value {
            KernelValue::Map(entries) => KernelValue::Map(
                entries
                    .into_iter()
                    .map(|(key, value)| {
                        (
                            runtime_modeled_coerce_value_to_type(key_ty, key),
                            runtime_modeled_coerce_value_to_type(value_ty, value),
                        )
                    })
                    .collect(),
            ),
            other => other,
        },
        _ => value,
    }
}

fn runtime_modeled_option_some(value: KernelValue) -> KernelValue {
    KernelValue::Map(BTreeMap::from([(
        KernelValue::String("value".to_string()),
        value,
    )]))
}

fn runtime_modeled_json_value_from_raw(raw: &str) -> serde_json::Value {
    serde_json::from_str(raw).unwrap_or_else(|_| serde_json::Value::String(raw.to_string()))
}

fn runtime_modeled_kernel_value_from_json(ty: &TypeRef, value: &serde_json::Value) -> KernelValue {
    match ty {
        TypeRef::Bool => KernelValue::Bool(value.as_bool().unwrap_or(false)),
        TypeRef::U32 | TypeRef::U64 => KernelValue::U64(value.as_u64().unwrap_or(0)),
        TypeRef::String => KernelValue::String(
            value
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| serde_json::to_string(value).unwrap_or_default()),
        ),
        TypeRef::Named(name)
            if matches!(
                name.as_str(),
                "BoundarySequence" | "TurnNumber" | "FenceToken" | "Generation"
            ) =>
        {
            KernelValue::Named {
                type_name: name.clone(),
                value: Box::new(KernelValue::U64(value.as_u64().unwrap_or(0))),
            }
        }
        TypeRef::Named(name) if name.as_str() == "ToolVisibilityWitness" => KernelValue::Named {
            type_name: name.clone(),
            value: Box::new(runtime_modeled_tool_visibility_witness_inner_from_json(
                value,
            )),
        },
        TypeRef::Named(name) => runtime_modeled_string_enum_named_value_from_json(name, value)
            .unwrap_or_else(|| KernelValue::Named {
                type_name: name.clone(),
                value: Box::new(KernelValue::String(
                    serde_json::to_string(value).unwrap_or_else(|_| "null".into()),
                )),
            }),
        TypeRef::Enum(name) => KernelValue::NamedVariant {
            enum_name: name.clone(),
            variant: meerkat_machine_schema::identity::EnumVariantId::parse(
                value.as_str().unwrap_or("_"),
            )
            .unwrap_or_else(|_| {
                meerkat_machine_schema::identity::EnumVariantId::parse("_")
                    .expect("valid placeholder slug")
            }),
        },
        TypeRef::Option(inner) => {
            if value.is_null() {
                KernelValue::None
            } else {
                runtime_modeled_option_some(runtime_modeled_kernel_value_from_json(inner, value))
            }
        }
        TypeRef::Set(inner) => {
            let values = value
                .as_array()
                .map(|items| {
                    items
                        .iter()
                        .map(|item| runtime_modeled_kernel_value_from_json(inner, item))
                        .collect()
                })
                .unwrap_or_default();
            KernelValue::Set(values)
        }
        TypeRef::Seq(inner) => KernelValue::Seq(
            value
                .as_array()
                .map(|items| {
                    items
                        .iter()
                        .map(|item| runtime_modeled_kernel_value_from_json(inner, item))
                        .collect()
                })
                .unwrap_or_default(),
        ),
        TypeRef::Map(key_ty, value_ty) => {
            let mut entries = BTreeMap::new();
            if let Some(object) = value.as_object() {
                for (key, item) in object {
                    let key_value = runtime_modeled_kernel_value_from_json(
                        key_ty,
                        &serde_json::Value::String(key.clone()),
                    );
                    let value_value = runtime_modeled_kernel_value_from_json(value_ty, item);
                    entries.insert(key_value, value_value);
                }
            }
            KernelValue::Map(entries)
        }
    }
}

fn runtime_modeled_kernel_value_from_raw(ty: &TypeRef, raw: &str) -> KernelValue {
    runtime_modeled_kernel_value_from_json(ty, &runtime_modeled_json_value_from_raw(raw))
}

fn runtime_modeled_json_from_kernel_value(value: &KernelValue) -> serde_json::Value {
    match value {
        KernelValue::Bool(value) => serde_json::Value::Bool(*value),
        KernelValue::U64(value) => serde_json::Value::Number(serde_json::Number::from(*value)),
        KernelValue::String(value) => {
            serde_json::from_str(value).unwrap_or_else(|_| serde_json::Value::String(value.clone()))
        }
        KernelValue::Named { value, .. } => runtime_modeled_json_from_kernel_value(value),
        KernelValue::NamedVariant { variant, .. } => {
            serde_json::Value::String(variant.as_str().to_string())
        }
        KernelValue::Seq(items) => serde_json::Value::Array(
            items
                .iter()
                .map(runtime_modeled_json_from_kernel_value)
                .collect(),
        ),
        KernelValue::Set(items) => serde_json::Value::Array(
            items
                .iter()
                .map(runtime_modeled_json_from_kernel_value)
                .collect(),
        ),
        KernelValue::Map(entries)
            if entries.len() == 1
                && entries.contains_key(&KernelValue::String("value".to_string())) =>
        {
            runtime_modeled_json_from_kernel_value(
                entries
                    .get(&KernelValue::String("value".to_string()))
                    .expect("value key present"),
            )
        }
        KernelValue::Map(entries) => {
            let mut object = serde_json::Map::new();
            for (key, value) in entries {
                let key_json = runtime_modeled_json_from_kernel_value(key);
                let key_string = key_json
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| serde_json::to_string(&key_json).unwrap_or_default());
                object.insert(key_string, runtime_modeled_json_from_kernel_value(value));
            }
            serde_json::Value::Object(object)
        }
        KernelValue::None => serde_json::Value::Null,
    }
}

fn runtime_modeled_formal_string_from_kernel_value(value: &KernelValue) -> String {
    runtime_modeled_normalize_formal_string(
        &serde_json::to_string(&runtime_modeled_json_from_kernel_value(value))
            .unwrap_or_else(|_| "null".into()),
    )
}

fn runtime_modeled_summary_from_runtime_snapshot(
    snapshot: Option<&RuntimeParitySnapshotSummary>,
) -> Option<RuntimeModeledStateSummary> {
    snapshot.map(|snapshot| RuntimeModeledStateSummary {
        phase: snapshot.phase.clone(),
        formal_fields: snapshot.formal_available_fields.clone(),
    })
}

fn runtime_modeled_session_value(before: &RuntimeParitySnapshotSummary) -> String {
    before
        .formal_available_fields
        .get("session_id")
        .cloned()
        .unwrap_or_else(|| "\"<session-id>\"".to_string())
}

fn runtime_modeled_runtime_value(before: &RuntimeParitySnapshotSummary) -> String {
    before
        .formal_available_fields
        .get("active_runtime_id")
        .cloned()
        .unwrap_or_else(|| "\"<runtime-id>\"".to_string())
}

fn runtime_modeled_named_string(value: String) -> KernelValue {
    KernelValue::String(value)
}

fn runtime_modeled_string_set(values: &[&str]) -> KernelValue {
    KernelValue::Set(
        values
            .iter()
            .map(|value| KernelValue::String((*value).to_string()))
            .collect(),
    )
}

fn runtime_modeled_tool_source_kind_label(kind: &meerkat_core::ToolSourceKind) -> &'static str {
    match kind {
        meerkat_core::ToolSourceKind::Builtin => "Builtin",
        meerkat_core::ToolSourceKind::Shell => "Shell",
        meerkat_core::ToolSourceKind::Comms => "Comms",
        meerkat_core::ToolSourceKind::Memory => "Memory",
        meerkat_core::ToolSourceKind::Schedule => "Schedule",
        meerkat_core::ToolSourceKind::Mob => "Mob",
        meerkat_core::ToolSourceKind::MobTasks => "MobTasks",
        meerkat_core::ToolSourceKind::Callback => "Callback",
        meerkat_core::ToolSourceKind::Mcp => "Mcp",
        meerkat_core::ToolSourceKind::RustBundle => "RustBundle",
    }
}

fn runtime_modeled_tool_provenance_inner(provenance: &meerkat_core::ToolProvenance) -> KernelValue {
    KernelValue::Map(BTreeMap::from([
        (
            KernelValue::String("kind".to_string()),
            KernelValue::String(runtime_modeled_tool_source_kind_label(&provenance.kind).into()),
        ),
        (
            KernelValue::String("source_id".to_string()),
            KernelValue::String(provenance.source_id.to_string()),
        ),
    ]))
}

fn runtime_modeled_tool_visibility_witness_inner_from_domain(
    witness: &meerkat_core::ToolVisibilityWitness,
) -> KernelValue {
    let mut fields = BTreeMap::new();
    if let Some(stable_owner_key) = &witness.stable_owner_key {
        fields.insert(
            KernelValue::String("stable_owner_key".to_string()),
            KernelValue::String(stable_owner_key.clone()),
        );
    }
    if let Some(last_seen_provenance) = &witness.last_seen_provenance {
        fields.insert(
            KernelValue::String("last_seen_provenance".to_string()),
            runtime_modeled_tool_provenance_inner(last_seen_provenance),
        );
    }
    KernelValue::Map(fields)
}

fn runtime_modeled_tool_visibility_witness_inner_from_json(
    value: &serde_json::Value,
) -> KernelValue {
    serde_json::from_value::<meerkat_core::ToolVisibilityWitness>(value.clone())
        .map(|witness| runtime_modeled_tool_visibility_witness_inner_from_domain(&witness))
        .unwrap_or_else(|_| KernelValue::Map(BTreeMap::new()))
}

fn runtime_modeled_tool_visibility_witness_inner(value: KernelValue) -> KernelValue {
    match value {
        KernelValue::Named { type_name, value }
            if type_name.as_str() == "ToolVisibilityWitness" =>
        {
            *value
        }
        KernelValue::Map(_) => value,
        KernelValue::String(raw) => {
            serde_json::from_str::<meerkat_core::ToolVisibilityWitness>(&raw)
                .map(|witness| runtime_modeled_tool_visibility_witness_inner_from_domain(&witness))
                .unwrap_or_else(|_| KernelValue::Map(BTreeMap::new()))
        }
        _ => KernelValue::Map(BTreeMap::new()),
    }
}

fn runtime_modeled_witness_map() -> KernelValue {
    let mut entries = BTreeMap::new();
    for (name, witness) in runtime_parity_witnesses() {
        entries.insert(
            KernelValue::String(name),
            runtime_modeled_tool_visibility_witness_inner_from_domain(&witness),
        );
    }
    KernelValue::Map(entries)
}

fn runtime_modeled_input_id_value() -> KernelValue {
    KernelValue::String("\"<input-id>\"".to_string())
}

fn modeled_input_variant(slug: &str) -> meerkat_machine_schema::identity::InputVariantId {
    meerkat_machine_schema::identity::InputVariantId::parse(slug).expect("input variant slug")
}

fn modeled_field_id(slug: &str) -> meerkat_machine_schema::identity::FieldId {
    meerkat_machine_schema::identity::FieldId::parse(slug).expect("field slug")
}

fn modeled_kernel_input(
    variant: &str,
    fields: impl IntoIterator<Item = (&'static str, KernelValue)>,
) -> KernelInput {
    let schema = modeled_meerkat_kernel::schema();
    let input_variant = schema
        .inputs
        .variant_named(variant)
        .expect("input variant should exist in modeled schema");
    KernelInput {
        variant: modeled_input_variant(variant),
        fields: fields
            .into_iter()
            .map(|(name, value)| {
                let field = input_variant
                    .fields
                    .iter()
                    .find(|field| field.name.as_str() == name)
                    .expect("field should exist in modeled input variant");
                (
                    modeled_field_id(name),
                    runtime_modeled_coerce_value_to_type(&field.ty, value),
                )
            })
            .collect(),
    }
}

fn runtime_modeled_run_id_value() -> KernelValue {
    KernelValue::String("\"<run-id>\"".to_string())
}

fn runtime_modeled_kernel_state(
    schema: &MachineSchema,
    before: &RuntimeParitySnapshotSummary,
) -> KernelState {
    let mut fields = BTreeMap::new();
    for field in &schema.state.fields {
        let value = before
            .formal_available_fields
            .get(field.name.as_str())
            .map(|raw| runtime_modeled_kernel_value_from_raw(&field.ty, raw))
            .unwrap_or_else(|| match field.name.as_str() {
                "active_fence_token" => runtime_modeled_option_some(KernelValue::U64(0)),
                _ => runtime_modeled_default_kernel_value(&field.ty),
            });
        fields.insert(
            field.name.clone(),
            runtime_modeled_coerce_value_to_type(&field.ty, value),
        );
    }
    KernelState {
        phase: meerkat_machine_schema::identity::PhaseId::parse(before.phase.as_str())
            .expect("phase slug"),
        fields,
    }
}

fn runtime_modeled_kernel_input(
    schema: &MachineSchema,
    before: &RuntimeParitySnapshotSummary,
    probe: RuntimeParityProbeInput,
) -> Result<KernelInput, String> {
    let variant_name = runtime_parity_probe_variant_name(probe).to_string();
    let input_variant = schema
        .inputs
        .variant_named(&variant_name)
        .map_err(|err| err.to_string())?;
    let variant = meerkat_machine_schema::identity::InputVariantId::parse(variant_name.as_str())
        .map_err(|err| err.to_string())?;
    let session_value = runtime_modeled_session_value(before);
    let runtime_value = runtime_modeled_runtime_value(before);
    let mut fields = BTreeMap::new();

    for field in &input_variant.fields {
        let value = match field.name.as_str() {
            "session_id" => runtime_modeled_named_string(session_value.clone()),
            "runtime_id" | "agent_runtime_id" => {
                runtime_modeled_named_string(runtime_value.clone())
            }
            "previous_identity" => runtime_modeled_named_string(
                serde_json::to_string(&meerkat_core::SessionLlmIdentity {
                    model: "claude-sonnet-4-5".to_string(),
                    provider: meerkat_core::Provider::Anthropic,
                    self_hosted_server_id: None,
                    provider_params: None,
                    connection_ref: None,
                })
                .unwrap_or_else(|_| "\"<previous-identity>\"".into()),
            ),
            "previous_visibility_state" => runtime_modeled_named_string(
                serde_json::to_string(&meerkat_core::SessionToolVisibilityState::default())
                    .unwrap_or_else(|_| "\"<previous-visibility-state>\"".into()),
            ),
            "previous_capability_surface" => {
                runtime_modeled_option_some(runtime_modeled_named_string(
                    serde_json::to_string(&test_llm_capability_surface(true))
                        .unwrap_or_else(|_| "\"<previous-capability-surface>\"".into()),
                ))
            }
            "previous_capability_surface_status" => {
                runtime_modeled_named_string("\"resolved\"".to_string())
            }
            "target_identity" => runtime_modeled_named_string(
                serde_json::to_string(&meerkat_core::SessionLlmIdentity {
                    model: "gpt-5.2".to_string(),
                    provider: meerkat_core::Provider::OpenAI,
                    self_hosted_server_id: None,
                    provider_params: None,
                    connection_ref: None,
                })
                .unwrap_or_else(|_| "\"<target-identity>\"".into()),
            ),
            "target_capability_surface" => runtime_modeled_named_string(
                serde_json::to_string(&test_llm_capability_surface(false))
                    .unwrap_or_else(|_| "\"<target-capability-surface>\"".into()),
            ),
            "next_visibility_state" => runtime_modeled_named_string(
                serde_json::to_string(&meerkat_core::SessionToolVisibilityState {
                    capability_base_filter: meerkat_core::ToolFilter::Deny(
                        [meerkat_core::VIEW_IMAGE_TOOL_NAME.to_string()]
                            .into_iter()
                            .collect(),
                    ),
                    active_revision: 1,
                    ..Default::default()
                })
                .unwrap_or_else(|_| "\"<next-visibility-state>\"".into()),
            ),
            "next_capability_base_filter" => runtime_modeled_named_string(
                serde_json::to_string(&meerkat_core::ToolFilter::Deny(
                    [meerkat_core::VIEW_IMAGE_TOOL_NAME.to_string()]
                        .into_iter()
                        .collect(),
                ))
                .unwrap_or_else(|_| "\"<next-capability-base-filter>\"".into()),
            ),
            "next_active_visibility_revision" => KernelValue::U64(1),
            "tool_visibility_delta" => {
                runtime_modeled_named_string("\"<tool-visibility-delta>\"".to_string())
            }
            "fence_token" | "generation" => runtime_modeled_default_kernel_value(&field.ty),
            "model" => KernelValue::String("gpt-5.2".to_string()),
            "provider" => KernelValue::String("openai".to_string()),
            "provider_params" => KernelValue::None,
            "intents" => runtime_modeled_string_set(&["mob.peer_added", "probe.intent"]),
            "filter" => KernelValue::String(
                serde_json::to_string(&meerkat_core::ToolFilter::Deny(
                    ["probe_tool".to_string()].into_iter().collect(),
                ))
                .unwrap_or_else(|_| "\"<tool-filter>\"".into()),
            ),
            "active_filter" | "staged_filter" => KernelValue::String(
                serde_json::to_string(&meerkat_core::ToolFilter::All)
                    .unwrap_or_else(|_| "\"<tool-filter>\"".into()),
            ),
            "witnesses" => runtime_modeled_witness_map(),
            "names" => runtime_modeled_string_set(&["probe_tool"]),
            "active_requested_deferred_names" | "staged_requested_deferred_names" => {
                runtime_modeled_string_set(&[])
            }
            "active_deferred_authorities" | "staged_deferred_authorities" => {
                KernelValue::Map(BTreeMap::new())
            }
            "active_visibility_revision" => KernelValue::U64(1),
            "staged_visibility_revision" => KernelValue::U64(1),
            "keep_alive" => KernelValue::Bool(true),
            "reason" => KernelValue::String("Dismissed".to_string()),
            "input_id" => runtime_modeled_input_id_value(),
            "run_id" => runtime_modeled_run_id_value(),
            "sequence" => KernelValue::U64(0),
            "kind" => KernelValue::String("runtime_created".to_string()),
            _ => runtime_modeled_default_kernel_value(&field.ty),
        };
        fields.insert(
            field.name.clone(),
            runtime_modeled_coerce_value_to_type(&field.ty, value),
        );
    }

    Ok(KernelInput { variant, fields })
}

fn runtime_parity_probe_variant_name(probe: RuntimeParityProbeInput) -> &'static str {
    match probe {
        RuntimeParityProbeInput::RegisterSession => "RegisterSession",
        RuntimeParityProbeInput::UnregisterSession => "UnregisterSession",
        RuntimeParityProbeInput::EnsureSessionWithExecutor => "EnsureSessionWithExecutor",
        RuntimeParityProbeInput::SetSilentIntents => "SetSilentIntents",
        RuntimeParityProbeInput::ReconfigureSessionLlmIdentity => "ReconfigureSessionLlmIdentity",
        RuntimeParityProbeInput::ContainsSession => "ContainsSession",
        RuntimeParityProbeInput::SessionHasExecutor => "SessionHasExecutor",
        RuntimeParityProbeInput::SessionHasComms => "SessionHasComms",
        RuntimeParityProbeInput::OpsLifecycleRegistry => "OpsLifecycleRegistry",
        RuntimeParityProbeInput::PrepareBindings => "PrepareBindings",
        RuntimeParityProbeInput::InputState => "InputState",
        RuntimeParityProbeInput::ListActiveInputs => "ListActiveInputs",
        RuntimeParityProbeInput::SetPeerIngressContext => "SetPeerIngressContext",
        RuntimeParityProbeInput::NotifyDrainExited => "NotifyDrainExited",
        RuntimeParityProbeInput::InterruptCurrentRun => "InterruptCurrentRun",
        RuntimeParityProbeInput::CancelAfterBoundary => "CancelAfterBoundary",
        RuntimeParityProbeInput::StagePersistentFilter => "StagePersistentFilter",
        RuntimeParityProbeInput::RequestDeferredTools => "RequestDeferredTools",
        RuntimeParityProbeInput::PublishCommittedVisibleSet => "PublishCommittedVisibleSet",
        RuntimeParityProbeInput::AbortAll => "AbortAll",
        RuntimeParityProbeInput::Abort => "Abort",
        RuntimeParityProbeInput::Wait => "Wait",
        RuntimeParityProbeInput::Ingest => "Ingest",
        RuntimeParityProbeInput::PublishEvent => "PublishEvent",
        RuntimeParityProbeInput::Recover => "Recover",
        RuntimeParityProbeInput::Retire => "Retire",
        RuntimeParityProbeInput::Recycle => "Recycle",
        RuntimeParityProbeInput::RuntimeState => "RuntimeState",
        RuntimeParityProbeInput::LoadBoundaryReceipt => "LoadBoundaryReceipt",
        RuntimeParityProbeInput::AcceptWithCompletion => "AcceptWithCompletion",
        RuntimeParityProbeInput::AcceptWithoutWake => "AcceptWithoutWake",
        RuntimeParityProbeInput::Prepare => "Prepare",
        RuntimeParityProbeInput::Commit => "Commit",
        RuntimeParityProbeInput::Fail => "Fail",
        RuntimeParityProbeInput::Reset => "Reset",
        RuntimeParityProbeInput::StopRuntimeExecutor => "StopRuntimeExecutor",
        RuntimeParityProbeInput::Destroy => "Destroy",
    }
}

fn runtime_modeled_summary_from_kernel_state(
    schema: &MachineSchema,
    state: &KernelState,
    runtime_reference: &RuntimeParitySnapshotSummary,
) -> Option<RuntimeModeledStateSummary> {
    let session_id_field =
        meerkat_machine_schema::identity::FieldId::parse("session_id").expect("session_id slug");
    if state
        .fields
        .get(&session_id_field)
        .is_some_and(|value| matches!(value, KernelValue::None))
    {
        return None;
    }

    let formal_fields = schema
        .state
        .fields
        .iter()
        .filter(|field| {
            runtime_reference
                .formal_available_fields
                .contains_key(field.name.as_str())
        })
        .map(|field| {
            let value = state
                .fields
                .get(&field.name)
                .map(runtime_modeled_formal_string_from_kernel_value)
                .unwrap_or_else(|| "null".to_string());
            (field.name.as_str().to_string(), value)
        })
        .collect();

    Some(RuntimeModeledStateSummary {
        phase: state.phase.as_str().to_string(),
        formal_fields,
    })
}

fn runtime_modeled_differing_keys(
    runtime_after: &Option<RuntimeModeledStateSummary>,
    schema_after: &Option<RuntimeModeledStateSummary>,
) -> Vec<String> {
    let mut keys = BTreeSet::new();
    if runtime_after.as_ref().map(|summary| summary.phase.as_str())
        != schema_after.as_ref().map(|summary| summary.phase.as_str())
    {
        keys.insert("phase".to_string());
    }

    let runtime_fields = runtime_after
        .as_ref()
        .map(|summary| &summary.formal_fields)
        .cloned()
        .unwrap_or_default();
    let schema_fields = schema_after
        .as_ref()
        .map(|summary| &summary.formal_fields)
        .cloned()
        .unwrap_or_default();

    for key in runtime_fields
        .keys()
        .chain(schema_fields.keys())
        .collect::<BTreeSet<_>>()
    {
        if runtime_fields.get(key) != schema_fields.get(key) {
            keys.insert(key.clone());
        }
    }

    keys.into_iter().collect()
}

fn assert_modeled_meerkat_transition_matches_runtime_after(
    schema: &MachineSchema,
    before: &RuntimeParitySnapshotSummary,
    input: &KernelInput,
    runtime_after: &RuntimeParitySnapshotSummary,
) {
    let outcome = GeneratedMachineKernel::new(modeled_meerkat_kernel::schema())
        .transition(&runtime_modeled_kernel_state(schema, before), input)
        .expect("modeled transition should succeed");
    let schema_after =
        runtime_modeled_summary_from_kernel_state(schema, &outcome.next_state, before)
            .expect("modeled transition should produce a schema summary");
    let runtime_after = runtime_modeled_summary_from_runtime_snapshot(Some(runtime_after))
        .expect("runtime transition should produce a runtime summary");
    assert_eq!(
        runtime_after.phase, schema_after.phase,
        "modeled phase should match runtime phase after {}",
        input.variant
    );
    assert_eq!(
        runtime_after.formal_fields, schema_after.formal_fields,
        "modeled formal fields should match runtime fields after {}",
        input.variant
    );
}

fn assert_modeled_meerkat_post_admission_signal_matches_runtime(
    schema: &MachineSchema,
    before: &RuntimeParitySnapshotSummary,
    input: &KernelInput,
    runtime_signal: crate::driver::ephemeral::PostAdmissionSignal,
) {
    let outcome = GeneratedMachineKernel::new(modeled_meerkat_kernel::schema())
        .transition(&runtime_modeled_kernel_state(schema, before), input)
        .expect("modeled transition should succeed");
    let modeled_signal = runtime_modeled_post_admission_signal_from_effects(&outcome.effects);
    assert_eq!(
        format!("{runtime_signal:?}"),
        modeled_signal,
        "modeled post-admission signal should match runtime after {}",
        input.variant
    );
}

fn runtime_modeled_publish_input(
    active_visibility_revision: u64,
    staged_visibility_revision: u64,
) -> KernelInput {
    modeled_kernel_input(
        "PublishCommittedVisibleSet",
        [
            (
                "active_filter",
                KernelValue::String(
                    serde_json::to_string(&meerkat_core::ToolFilter::All)
                        .unwrap_or_else(|_| "\"<tool-filter>\"".into()),
                ),
            ),
            (
                "staged_filter",
                KernelValue::String(
                    serde_json::to_string(&meerkat_core::ToolFilter::All)
                        .unwrap_or_else(|_| "\"<tool-filter>\"".into()),
                ),
            ),
            (
                "active_requested_deferred_names",
                runtime_modeled_string_set(&[]),
            ),
            (
                "staged_requested_deferred_names",
                runtime_modeled_string_set(&[]),
            ),
            (
                "active_deferred_authorities",
                KernelValue::Map(BTreeMap::new()),
            ),
            (
                "staged_deferred_authorities",
                KernelValue::Map(BTreeMap::new()),
            ),
            (
                "active_visibility_revision",
                KernelValue::U64(active_visibility_revision),
            ),
            (
                "staged_visibility_revision",
                KernelValue::U64(staged_visibility_revision),
            ),
        ],
    )
}

fn runtime_parity_witnesses() -> BTreeMap<String, meerkat_core::ToolVisibilityWitness> {
    [(
        "probe_tool".to_string(),
        meerkat_core::ToolVisibilityWitness {
            stable_owner_key: Some("callback:runtime-parity".to_string()),
            last_seen_provenance: None,
        },
    )]
    .into_iter()
    .collect()
}

fn runtime_parity_steered_prompt(text: &str) -> Input {
    Input::Prompt(crate::input::PromptInput::new(
        text,
        Some(
            meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                handling_mode: Some(meerkat_core::types::HandlingMode::Steer),
                ..Default::default()
            },
        ),
    ))
}

fn runtime_parity_prompt(text: &str) -> Input {
    make_prompt(text)
}

fn runtime_parity_peer_message(text: &str) -> Input {
    Input::Peer(crate::input::PeerInput {
        header: crate::input::InputHeader {
            id: InputId::new(),
            timestamp: Utc::now(),
            source: crate::input::InputOrigin::Peer {
                peer_id: "runtime-parity".into(),
                display_identity: None,
                runtime_id: None,
            },
            durability: crate::input::InputDurability::Durable,
            visibility: crate::input::InputVisibility::default(),
            idempotency_key: None,
            supersession_key: None,
            correlation_id: None,
        },
        convention: Some(crate::input::PeerConvention::Message),
        body: text.into(),
        payload: None,
        blocks: None,
        handling_mode: None,
    })
}

fn runtime_parity_publish_state_with_revisions(
    active_revision: u64,
    staged_revision: u64,
) -> meerkat_core::SessionToolVisibilityState {
    meerkat_core::SessionToolVisibilityState {
        active_revision,
        staged_revision,
        ..Default::default()
    }
}

fn runtime_parity_publish_state() -> meerkat_core::SessionToolVisibilityState {
    runtime_parity_publish_state_with_revisions(1, 1)
}

fn runtime_parity_input_id() -> InputId {
    InputId::new()
}

fn runtime_parity_silent_intents() -> Vec<String> {
    vec!["mob.peer_added".to_string(), "probe.intent".to_string()]
}

fn runtime_parity_event(
    runtime_id: &LogicalRuntimeId,
) -> crate::runtime_event::RuntimeEventEnvelope {
    crate::runtime_event::RuntimeEventEnvelope {
        id: crate::identifiers::RuntimeEventId::new(),
        timestamp: Utc::now(),
        runtime_id: runtime_id.clone(),
        event: crate::runtime_event::RuntimeEvent::Topology(
            crate::runtime_event::RuntimeTopologyEvent::RuntimeCreated {
                runtime_id: runtime_id.clone(),
            },
        ),
        causation_id: None,
        correlation_id: None,
    }
}

#[tokio::test]
async fn modeled_meerkat_accept_with_completion_attached_steer_matches_runtime() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    let apply_started = Arc::new(Notify::new());
    let allow_finish = Arc::new(Notify::new());

    adapter
        .prepare_bindings(session_id.clone())
        .await
        .expect("bindings should prepare for attached steer modeling");
    adapter
        .ensure_session_with_executor(
            session_id.clone(),
            Box::new(RuntimeParityBlockingExecutor {
                apply_started: Arc::clone(&apply_started),
                allow_finish: Arc::clone(&allow_finish),
            }),
        )
        .await;
    wait_for_runtime_parity_phase(&adapter, &session_id, RuntimeState::Attached).await;

    let before = runtime_parity_snapshot_summary(&adapter, &session_id)
        .await
        .expect("attached steer test should capture a pre-state snapshot");
    let result = adapter
        .execute_meerkat_machine_command(
            None,
            MeerkatMachineCommand::AcceptWithCompletion {
                session_id: session_id.clone(),
                input: runtime_parity_steered_prompt("modeled attached steer"),
            },
        )
        .await
        .expect("attached steered input should be accepted");
    let (outcome, completion_handle, admission_signal) = match result {
        MeerkatMachineCommandResult::AcceptWithCompletion {
            outcome,
            handle,
            admission_signal,
        } => (outcome, handle, admission_signal),
        other => panic!("unexpected attached steer result: {other:?}"),
    };
    assert!(outcome.is_accepted());
    let completion_handle =
        completion_handle.expect("attached steered input should expose a completion waiter");

    tokio::time::timeout(Duration::from_secs(1), apply_started.notified())
        .await
        .expect("attached steered input should request immediate processing");

    let after = runtime_parity_snapshot_summary(&adapter, &session_id)
        .await
        .expect("attached steer test should capture an active run snapshot");
    let schema = modeled_meerkat_kernel::schema();
    let input = modeled_kernel_input(
        "AcceptWithCompletion",
        [
            ("input_id", runtime_modeled_input_id_value()),
            ("request_immediate_processing", KernelValue::Bool(true)),
            ("interrupt_yielding", KernelValue::Bool(false)),
            ("wake_if_idle", KernelValue::Bool(false)),
        ],
    );
    let accept_outcome = GeneratedMachineKernel::new(modeled_meerkat_kernel::schema())
        .transition(&runtime_modeled_kernel_state(&schema, &before), &input)
        .expect("modeled AcceptWithCompletion should succeed");
    let prepare_input = modeled_kernel_input(
        "Prepare",
        [
            (
                "session_id",
                runtime_modeled_named_string(runtime_modeled_session_value(&before)),
            ),
            ("run_id", runtime_modeled_run_id_value()),
        ],
    );
    let prepare_outcome = GeneratedMachineKernel::new(modeled_meerkat_kernel::schema())
        .transition(&accept_outcome.next_state, &prepare_input)
        .expect("modeled Prepare should succeed after attached AcceptWithCompletion");
    let schema_after =
        runtime_modeled_summary_from_kernel_state(&schema, &prepare_outcome.next_state, &before)
            .expect("modeled AcceptWithCompletion+Prepare should produce a schema summary");
    let runtime_after = runtime_modeled_summary_from_runtime_snapshot(Some(&after))
        .expect("runtime attached steer should produce a runtime summary");
    assert_eq!(
        runtime_after.phase, schema_after.phase,
        "modeled phase should match runtime phase after AcceptWithCompletion+Prepare"
    );
    assert_eq!(
        runtime_after.formal_fields, schema_after.formal_fields,
        "modeled formal fields should match runtime fields after AcceptWithCompletion+Prepare"
    );
    assert_modeled_meerkat_post_admission_signal_matches_runtime(
        &schema,
        &before,
        &input,
        admission_signal,
    );

    allow_finish.notify_waiters();
    let _ = tokio::time::timeout(Duration::from_secs(1), completion_handle.wait()).await;
}

#[tokio::test]
async fn modeled_meerkat_accept_with_completion_idle_queue_signal_matches_runtime() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;
    wait_for_runtime_parity_phase(&adapter, &session_id, RuntimeState::Idle).await;

    let before = runtime_parity_snapshot_summary(&adapter, &session_id)
        .await
        .expect("idle queue test should capture a pre-state snapshot");
    let result = adapter
        .execute_meerkat_machine_command(
            None,
            MeerkatMachineCommand::AcceptWithCompletion {
                session_id: session_id.clone(),
                input: runtime_parity_prompt("modeled idle queued admission"),
            },
        )
        .await
        .expect("idle queued input should be accepted");
    let (outcome, completion_handle, admission_signal) = match result {
        MeerkatMachineCommandResult::AcceptWithCompletion {
            outcome,
            handle,
            admission_signal,
        } => (outcome, handle, admission_signal),
        other => panic!("unexpected idle queued result: {other:?}"),
    };
    assert!(outcome.is_accepted());
    let completion_handle =
        completion_handle.expect("queued idle input should expose a completion waiter");

    let after = runtime_parity_snapshot_summary(&adapter, &session_id)
        .await
        .expect("idle queue test should capture a post-state snapshot");
    let schema = modeled_meerkat_kernel::schema();
    let input = modeled_kernel_input(
        "AcceptWithCompletion",
        [
            ("input_id", runtime_modeled_input_id_value()),
            ("request_immediate_processing", KernelValue::Bool(false)),
            ("interrupt_yielding", KernelValue::Bool(false)),
            ("wake_if_idle", KernelValue::Bool(false)),
        ],
    );
    assert_modeled_meerkat_transition_matches_runtime_after(&schema, &before, &input, &after);
    assert_modeled_meerkat_post_admission_signal_matches_runtime(
        &schema,
        &before,
        &input,
        admission_signal,
    );

    crate::traits::RuntimeControlPlane::destroy(
        adapter.as_ref(),
        &runtime_id_for_session(&session_id),
    )
    .await
    .expect("idle queue test should destroy runtime cleanly");
    match completion_handle.wait().await {
        CompletionOutcome::RuntimeTerminated(reason) => {
            assert_eq!(reason, "runtime destroyed");
        }
        other => panic!("expected runtime destroyed termination, got {other:?}"),
    }
}

#[tokio::test]
async fn modeled_meerkat_set_silent_intents_matches_runtime() {
    let fixture = build_runtime_parity_fixture(RuntimeParityPhase::Idle).await;
    let before = runtime_parity_snapshot_summary(&fixture.adapter, &fixture.session_id)
        .await
        .expect("set silent intents test should capture a pre-state snapshot");

    let result = fixture
        .adapter
        .execute_meerkat_machine_command(
            None,
            MeerkatMachineCommand::SetSilentIntents {
                session_id: fixture.session_id.clone(),
                intents: runtime_parity_silent_intents(),
            },
        )
        .await
        .expect("set silent intents should succeed");
    assert!(matches!(result, MeerkatMachineCommandResult::Unit));

    let after = runtime_parity_snapshot_summary(&fixture.adapter, &fixture.session_id)
        .await
        .expect("set silent intents test should capture a post-state snapshot");
    let schema = modeled_meerkat_kernel::schema();
    let input =
        runtime_modeled_kernel_input(&schema, &before, RuntimeParityProbeInput::SetSilentIntents)
            .expect("modeled set silent intents input should build");
    assert_modeled_meerkat_transition_matches_runtime_after(&schema, &before, &input, &after);

    fixture.cleanup().await;
}

#[tokio::test]
async fn meerkat_reset_clears_silent_intent_overrides() {
    let fixture = build_runtime_parity_fixture(RuntimeParityPhase::Idle).await;
    fixture
        .adapter
        .set_session_silent_intents(&fixture.session_id, runtime_parity_silent_intents())
        .await;

    let runtime_id = runtime_id_for_session(&fixture.session_id);
    let result = fixture
        .adapter
        .execute_meerkat_machine_command(
            None,
            MeerkatMachineCommand::Reset {
                runtime_id: runtime_id.clone(),
            },
        )
        .await
        .expect("reset should succeed");
    assert!(matches!(
        result,
        MeerkatMachineCommandResult::ResetReport(_)
    ));

    let snapshot = fixture
        .adapter
        .meerkat_machine_spine_snapshot(&fixture.session_id)
        .await
        .expect("snapshot should exist after reset");
    assert!(
        snapshot.inputs.silent_intent_overrides.is_empty(),
        "reset should clear ingress-side silent intent overrides"
    );

    fixture.cleanup().await;
}

#[tokio::test]
async fn meerkat_destroy_clears_silent_intent_overrides() {
    let fixture = build_runtime_parity_fixture(RuntimeParityPhase::Idle).await;
    fixture
        .adapter
        .set_session_silent_intents(&fixture.session_id, runtime_parity_silent_intents())
        .await;

    let runtime_id = runtime_id_for_session(&fixture.session_id);
    let result = fixture
        .adapter
        .execute_meerkat_machine_command(
            None,
            MeerkatMachineCommand::Destroy {
                runtime_id: runtime_id.clone(),
            },
        )
        .await
        .expect("destroy should succeed");
    assert!(matches!(
        result,
        MeerkatMachineCommandResult::DestroyReport(_)
    ));

    let snapshot = fixture
        .adapter
        .meerkat_machine_spine_snapshot(&fixture.session_id)
        .await
        .expect("snapshot should exist after destroy");
    assert!(
        snapshot.inputs.silent_intent_overrides.is_empty(),
        "destroy should clear ingress-side silent intent overrides"
    );

    fixture.cleanup().await;
}

#[tokio::test]
async fn meerkat_stop_runtime_executor_clears_silent_intent_overrides() {
    let fixture = build_runtime_parity_fixture(RuntimeParityPhase::Idle).await;
    fixture
        .adapter
        .set_session_silent_intents(&fixture.session_id, runtime_parity_silent_intents())
        .await;

    fixture
        .adapter
        .stop_runtime_executor(
            &fixture.session_id,
            RunControlCommand::StopRuntimeExecutor {
                reason: "clear silent intents".into(),
            },
        )
        .await
        .expect("stop runtime executor should succeed");

    wait_for_runtime_parity_phase(&fixture.adapter, &fixture.session_id, RuntimeState::Stopped)
        .await;
    let snapshot = fixture
        .adapter
        .meerkat_machine_spine_snapshot(&fixture.session_id)
        .await
        .expect("snapshot should exist after stop");
    assert!(
        snapshot.inputs.silent_intent_overrides.is_empty(),
        "stop should clear ingress-side silent intent overrides"
    );

    fixture.cleanup().await;
}

#[tokio::test]
async fn modeled_meerkat_accept_with_completion_running_steer_signal_matches_runtime() {
    let fixture = build_runtime_parity_fixture(RuntimeParityPhase::Running).await;
    let before = runtime_parity_snapshot_summary(&fixture.adapter, &fixture.session_id)
        .await
        .expect("running steer test should capture a pre-state snapshot");

    let result = fixture
        .adapter
        .execute_meerkat_machine_command(
            None,
            MeerkatMachineCommand::AcceptWithCompletion {
                session_id: fixture.session_id.clone(),
                input: runtime_parity_steered_prompt("modeled running steer admission"),
            },
        )
        .await
        .expect("running steered input should be accepted");
    let (outcome, completion_handle, admission_signal) = match result {
        MeerkatMachineCommandResult::AcceptWithCompletion {
            outcome,
            handle,
            admission_signal,
        } => (outcome, handle, admission_signal),
        other => panic!("unexpected running steer result: {other:?}"),
    };
    assert!(outcome.is_accepted());
    let completion_handle =
        completion_handle.expect("running steered input should expose a completion waiter");

    let after = runtime_parity_snapshot_summary(&fixture.adapter, &fixture.session_id)
        .await
        .expect("running steer test should capture a post-state snapshot");
    let schema = modeled_meerkat_kernel::schema();
    let input = modeled_kernel_input(
        "AcceptWithCompletion",
        [
            ("input_id", runtime_modeled_input_id_value()),
            ("request_immediate_processing", KernelValue::Bool(true)),
            ("interrupt_yielding", KernelValue::Bool(false)),
            ("wake_if_idle", KernelValue::Bool(false)),
        ],
    );
    assert_modeled_meerkat_transition_matches_runtime_after(&schema, &before, &input, &after);
    assert_modeled_meerkat_post_admission_signal_matches_runtime(
        &schema,
        &before,
        &input,
        admission_signal,
    );

    drop(completion_handle);
    fixture.cleanup().await;
}

#[tokio::test]
async fn prepare_runtime_loop_batch_start_unwinds_run_state_when_staging_rejects() {
    let runtime_id = LogicalRuntimeId::new("prepare-unwind");
    let driver: SharedDriver = Arc::new(tokio::sync::Mutex::new(DriverEntry::Ephemeral(
        EphemeralRuntimeDriver::new(runtime_id),
    )));

    let accepted_input_id = {
        let mut entry = driver.lock().await;
        let outcome = entry
            .as_driver_mut()
            .accept_input(make_prompt("queued"))
            .await
            .expect("accept should queue input");
        match outcome {
            AcceptOutcome::Accepted { input_id, .. } => input_id,
            other => panic!("expected accepted input, got {other:?}"),
        }
    };

    let err = prepare_runtime_loop_batch_start(&driver, RunId::new(), &[InputId::new()])
        .await
        .expect_err("staging an unknown input should fail and unwind");
    assert!(
        err.to_string()
            .contains("failed to stage accepted input batch")
            || err
                .to_string()
                .contains("stage drain snapshot requires queued contributors"),
        "unexpected helper error: {err}"
    );

    let entry = driver.lock().await;
    let DriverEntry::Ephemeral(driver) = &*entry else {
        panic!("test uses ephemeral driver");
    };
    assert_eq!(
        driver.runtime_state(),
        RuntimeState::Idle,
        "helper should unwind the started run back to idle"
    );
    assert!(
        driver.current_run_id().is_none(),
        "helper should clear the transient run id on staging failure"
    );
    assert!(
        driver.input_state(&accepted_input_id).is_some(),
        "accepted input should still be present after unwind"
    );
    assert_eq!(
        driver.input_phase(&accepted_input_id),
        Some(crate::input_state::InputLifecycleState::Queued),
        "staging failure should leave the queued input untouched"
    );
}

#[tokio::test]
async fn modeled_meerkat_accept_with_completion_running_interrupt_signal_matches_runtime() {
    let fixture = build_runtime_parity_fixture(RuntimeParityPhase::Running).await;
    let before = runtime_parity_snapshot_summary(&fixture.adapter, &fixture.session_id)
        .await
        .expect("running interrupt test should capture a pre-state snapshot");

    let result = fixture
        .adapter
        .execute_meerkat_machine_command(
            None,
            MeerkatMachineCommand::AcceptWithCompletion {
                session_id: fixture.session_id.clone(),
                input: runtime_parity_peer_message("modeled running interrupt admission"),
            },
        )
        .await
        .expect("running peer input should be accepted");
    let (outcome, completion_handle, admission_signal) = match result {
        MeerkatMachineCommandResult::AcceptWithCompletion {
            outcome,
            handle,
            admission_signal,
        } => (outcome, handle, admission_signal),
        other => panic!("unexpected running interrupt result: {other:?}"),
    };
    assert!(outcome.is_accepted());
    let completion_handle =
        completion_handle.expect("running interrupt input should expose a completion waiter");

    let after = runtime_parity_snapshot_summary(&fixture.adapter, &fixture.session_id)
        .await
        .expect("running interrupt test should capture a post-state snapshot");
    let schema = modeled_meerkat_kernel::schema();
    let input = modeled_kernel_input(
        "AcceptWithCompletion",
        [
            ("input_id", runtime_modeled_input_id_value()),
            ("request_immediate_processing", KernelValue::Bool(false)),
            ("interrupt_yielding", KernelValue::Bool(true)),
            ("wake_if_idle", KernelValue::Bool(false)),
        ],
    );
    assert_modeled_meerkat_transition_matches_runtime_after(&schema, &before, &input, &after);
    assert_modeled_meerkat_post_admission_signal_matches_runtime(
        &schema,
        &before,
        &input,
        admission_signal,
    );

    drop(completion_handle);
    fixture.cleanup().await;
}

fn install_runtime_parity_reconfigure_host(adapter: &Arc<MeerkatMachine>) {
    adapter.set_session_llm_reconfigure_host(Arc::new(TestLlmReconfigureHost {
        current_identity: Arc::new(std::sync::Mutex::new(meerkat_core::SessionLlmIdentity {
            model: "claude-sonnet-4-5".to_string(),
            provider: meerkat_core::Provider::Anthropic,
            self_hosted_server_id: None,
            provider_params: None,
            connection_ref: None,
        })),
        current_visibility_state: Arc::new(std::sync::Mutex::new(Default::default())),
        target_identity: meerkat_core::SessionLlmIdentity {
            model: "gpt-5.2".to_string(),
            provider: meerkat_core::Provider::OpenAI,
            self_hosted_server_id: None,
            provider_params: None,
            connection_ref: None,
        },
        current_capability_surface: Some(test_llm_capability_surface(true)),
        target_capability_surface: test_llm_capability_surface(false),
        base_tool_names: [meerkat_core::VIEW_IMAGE_TOOL_NAME.to_string()]
            .into_iter()
            .collect(),
        fail_persist: false,
    }));
}

async fn runtime_parity_snapshot_summary(
    adapter: &Arc<MeerkatMachine>,
    session_id: &SessionId,
) -> Option<RuntimeParitySnapshotSummary> {
    adapter
        .meerkat_machine_spine_snapshot(session_id)
        .await
        .map(|snapshot| {
            let raw_formal_fields = snapshot.formal_state.available_fields.clone();
            RuntimeParitySnapshotSummary {
                phase: runtime_parity_state_label(snapshot.control.phase),
                current_run_present: snapshot.control.current_run_id.is_some(),
                formal_session_id: runtime_parity_formal_identity_field(
                    &raw_formal_fields,
                    "session_id",
                ),
                formal_active_runtime_id: runtime_parity_formal_identity_field(
                    &raw_formal_fields,
                    "active_runtime_id",
                ),
                formal_current_run_id: runtime_parity_formal_identity_field(
                    &raw_formal_fields,
                    "current_run_id",
                ),
                pre_run_phase: snapshot
                    .control
                    .pre_run_phase
                    .map(runtime_parity_state_label),
                attachment_live: snapshot.binding.attachment_live,
                queue_len: snapshot.inputs.queue.len(),
                steer_queue_len: snapshot.inputs.steer_queue.len(),
                current_run_contributor_count: snapshot.inputs.current_run_contributors.len(),
                admitted_input_count: snapshot.inputs.admission_order.len(),
                post_admission_signal: snapshot.inputs.post_admission_signal,
                ledger_input_count: snapshot.ledger.input_count,
                ledger_non_terminal_count: snapshot.ledger.non_terminal_count,
                ledger_accepted_count: snapshot.ledger.accepted_count,
                ledger_queued_count: snapshot.ledger.queued_count,
                ledger_staged_count: snapshot.ledger.staged_count,
                ledger_applied_count: snapshot.ledger.applied_count,
                ledger_applied_pending_consumption_count: snapshot
                    .ledger
                    .applied_pending_consumption_count,
                ledger_consumed_count: snapshot.ledger.consumed_count,
                ledger_superseded_count: snapshot.ledger.superseded_count,
                ledger_coalesced_count: snapshot.ledger.coalesced_count,
                ledger_abandoned_count: snapshot.ledger.abandoned_count,
                wait_request_present: snapshot.ops.wait_request_id.is_some(),
                drain_slot_present: snapshot.drain.slot_present,
                drain_phase: snapshot.drain.phase.map(runtime_parity_drain_phase_label),
                formal_available_fields: normalize_runtime_parity_formal_fields(
                    snapshot.formal_state.available_fields,
                ),
                formal_unavailable_fields: snapshot.formal_state.unavailable_fields,
            }
        })
}

async fn wait_for_runtime_parity_phase(
    adapter: &Arc<MeerkatMachine>,
    session_id: &SessionId,
    expected: RuntimeState,
) {
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            match adapter.meerkat_machine_spine_snapshot(session_id).await {
                Some(snapshot) if snapshot.control.phase == expected => break,
                _ => tokio::time::sleep(Duration::from_millis(5)).await,
            }
        }
    })
    .await
    .expect("runtime phase transition should complete");
}

async fn build_runtime_parity_fixture(phase: RuntimeParityPhase) -> RuntimeParityFixture {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    let runtime_id = runtime_id_for_session(&session_id);

    match phase {
        RuntimeParityPhase::Idle => {
            adapter.register_session(session_id.clone()).await;
            wait_for_runtime_parity_phase(&adapter, &session_id, RuntimeState::Idle).await;
            RuntimeParityFixture {
                adapter,
                session_id,
                runtime_id,
                running_release: None,
                running_completion: None,
                prepared_input_id: None,
                prepared_run_id: None,
            }
        }
        RuntimeParityPhase::Attached => {
            adapter
                .prepare_bindings(session_id.clone())
                .await
                .expect("bindings should prepare for attached fixture");
            adapter
                .ensure_session_with_executor(
                    session_id.clone(),
                    Box::new(RuntimeParityNoopExecutor),
                )
                .await;
            wait_for_runtime_parity_phase(&adapter, &session_id, RuntimeState::Attached).await;
            RuntimeParityFixture {
                adapter,
                session_id,
                runtime_id,
                running_release: None,
                running_completion: None,
                prepared_input_id: None,
                prepared_run_id: None,
            }
        }
        RuntimeParityPhase::Running => {
            let apply_started = Arc::new(Notify::new());
            let allow_finish = Arc::new(Notify::new());
            adapter
                .prepare_bindings(session_id.clone())
                .await
                .expect("bindings should prepare for running fixture");
            adapter
                .ensure_session_with_executor(
                    session_id.clone(),
                    Box::new(RuntimeParityBlockingExecutor {
                        apply_started: Arc::clone(&apply_started),
                        allow_finish: Arc::clone(&allow_finish),
                    }),
                )
                .await;
            let (outcome, completion) = adapter
                .accept_input_with_completion(
                    &session_id,
                    runtime_parity_steered_prompt("runtime parity running fixture"),
                )
                .await
                .expect("running fixture should accept the steered prompt");
            assert!(
                outcome.is_accepted(),
                "running fixture prompt should be accepted"
            );
            let completion =
                completion.expect("running fixture prompt should expose a completion handle");
            tokio::time::timeout(Duration::from_secs(1), apply_started.notified())
                .await
                .expect("running fixture executor should enter apply");
            wait_for_runtime_parity_phase(&adapter, &session_id, RuntimeState::Running).await;
            RuntimeParityFixture {
                adapter,
                session_id,
                runtime_id,
                running_release: Some(allow_finish),
                running_completion: Some(completion),
                prepared_input_id: None,
                prepared_run_id: None,
            }
        }
        RuntimeParityPhase::Retired => {
            adapter.register_session(session_id.clone()).await;
            adapter
                .execute_meerkat_machine_command(
                    Some(Arc::clone(&adapter)),
                    MeerkatMachineCommand::Retire {
                        runtime_id: runtime_id.clone(),
                    },
                )
                .await
                .expect("retired fixture should retire");
            wait_for_runtime_parity_phase(&adapter, &session_id, RuntimeState::Retired).await;
            RuntimeParityFixture {
                adapter,
                session_id,
                runtime_id,
                running_release: None,
                running_completion: None,
                prepared_input_id: None,
                prepared_run_id: None,
            }
        }
        RuntimeParityPhase::Stopped => {
            adapter.register_session(session_id.clone()).await;
            adapter
                .stop_runtime_executor(
                    &session_id,
                    RunControlCommand::StopRuntimeExecutor {
                        reason: "runtime parity stopped fixture".to_string(),
                    },
                )
                .await
                .expect("stopped fixture should stop");
            wait_for_runtime_parity_phase(&adapter, &session_id, RuntimeState::Stopped).await;
            RuntimeParityFixture {
                adapter,
                session_id,
                runtime_id,
                running_release: None,
                running_completion: None,
                prepared_input_id: None,
                prepared_run_id: None,
            }
        }
    }
}

async fn prepare_runtime_parity_probe(
    phase: RuntimeParityPhase,
    fixture: &mut RuntimeParityFixture,
    probe: RuntimeParityProbeInput,
    setup_tags: &mut Vec<String>,
) {
    if matches!(
        probe,
        RuntimeParityProbeInput::ReconfigureSessionLlmIdentity
    ) {
        install_runtime_parity_reconfigure_host(&fixture.adapter);
        setup_tags.push("llm_reconfigure_host".to_string());
    }

    if matches!(probe, RuntimeParityProbeInput::NotifyDrainExited) {
        let comms_runtime: Arc<dyn CommsRuntime> = Arc::new(FakeDrainRuntime::idle());
        let spawned = fixture
            .adapter
            .update_peer_ingress_context(&fixture.session_id, true, Some(comms_runtime))
            .await;
        setup_tags.push(format!("drain_primed:{spawned}"));
        if spawned {
            wait_for_phase(
                &fixture.adapter,
                &fixture.session_id,
                CommsDrainPhase::Running,
            )
            .await;
        }
    }

    if phase == RuntimeParityPhase::Running
        && matches!(
            probe,
            RuntimeParityProbeInput::Commit | RuntimeParityProbeInput::Fail
        )
        && fixture.prepared_run_id.is_none()
    {
        let prepared = fixture
            .adapter
            .execute_meerkat_machine_command(
                None,
                MeerkatMachineCommand::Prepare {
                    session_id: fixture.session_id.clone(),
                    input: runtime_parity_prompt("runtime parity prepared run"),
                },
            )
            .await
            .expect("runtime parity should prepare a running fixture for commit/fail");
        let prepared = match prepared {
            MeerkatMachineCommandResult::Prepared(prepared) => prepared,
            other => panic!("unexpected runtime parity prepare result for commit/fail: {other:?}"),
        };
        fixture.prepared_input_id = Some(prepared.input_id);
        fixture.prepared_run_id = Some(prepared.run_id);
        wait_for_runtime_parity_phase(&fixture.adapter, &fixture.session_id, RuntimeState::Running)
            .await;
        setup_tags.push("legacy_run_prepared".to_string());
    }
}

fn runtime_parity_probe_command(
    fixture: &RuntimeParityFixture,
    probe: RuntimeParityProbeInput,
) -> MeerkatMachineCommand {
    match probe {
        RuntimeParityProbeInput::RegisterSession => MeerkatMachineCommand::RegisterSession {
            session_id: fixture.session_id.clone(),
        },
        RuntimeParityProbeInput::UnregisterSession => MeerkatMachineCommand::UnregisterSession {
            session_id: fixture.session_id.clone(),
        },
        RuntimeParityProbeInput::EnsureSessionWithExecutor => {
            MeerkatMachineCommand::EnsureSessionWithExecutor {
                session_id: fixture.session_id.clone(),
                executor: Box::new(RuntimeParityNoopExecutor),
            }
        }
        RuntimeParityProbeInput::SetSilentIntents => MeerkatMachineCommand::SetSilentIntents {
            session_id: fixture.session_id.clone(),
            intents: runtime_parity_silent_intents(),
        },
        RuntimeParityProbeInput::ReconfigureSessionLlmIdentity => {
            unreachable!("reconfigure parity probes use the public helper path")
        }
        RuntimeParityProbeInput::ContainsSession => MeerkatMachineCommand::ContainsSession {
            session_id: fixture.session_id.clone(),
        },
        RuntimeParityProbeInput::SessionHasExecutor => MeerkatMachineCommand::SessionHasExecutor {
            session_id: fixture.session_id.clone(),
        },
        RuntimeParityProbeInput::SessionHasComms => MeerkatMachineCommand::SessionHasComms {
            session_id: fixture.session_id.clone(),
        },
        RuntimeParityProbeInput::OpsLifecycleRegistry => {
            MeerkatMachineCommand::OpsLifecycleRegistry {
                session_id: fixture.session_id.clone(),
            }
        }
        RuntimeParityProbeInput::PrepareBindings => MeerkatMachineCommand::PrepareBindings {
            session_id: fixture.session_id.clone(),
        },
        RuntimeParityProbeInput::InputState => MeerkatMachineCommand::InputState {
            session_id: fixture.session_id.clone(),
            input_id: runtime_parity_input_id(),
        },
        RuntimeParityProbeInput::ListActiveInputs => MeerkatMachineCommand::ListActiveInputs {
            session_id: fixture.session_id.clone(),
        },
        RuntimeParityProbeInput::SetPeerIngressContext => {
            let comms_runtime: Arc<dyn CommsRuntime> = Arc::new(FakeDrainRuntime::idle());
            MeerkatMachineCommand::SetPeerIngressContext {
                session_id: fixture.session_id.clone(),
                keep_alive: true,
                comms_runtime: Some(comms_runtime),
                mob_id: None,
            }
        }
        RuntimeParityProbeInput::NotifyDrainExited => MeerkatMachineCommand::NotifyDrainExited {
            session_id: fixture.session_id.clone(),
            reason: DrainExitReason::Dismissed,
        },
        RuntimeParityProbeInput::InterruptCurrentRun => {
            MeerkatMachineCommand::InterruptCurrentRun {
                session_id: fixture.session_id.clone(),
            }
        }
        RuntimeParityProbeInput::CancelAfterBoundary => {
            MeerkatMachineCommand::CancelAfterBoundary {
                session_id: fixture.session_id.clone(),
            }
        }
        RuntimeParityProbeInput::StagePersistentFilter => {
            MeerkatMachineCommand::StagePersistentFilter {
                session_id: fixture.session_id.clone(),
                filter: meerkat_core::ToolFilter::Deny(
                    ["probe_tool".to_string()].into_iter().collect(),
                ),
                witnesses: runtime_parity_witnesses(),
            }
        }
        RuntimeParityProbeInput::RequestDeferredTools => {
            MeerkatMachineCommand::RequestDeferredTools {
                session_id: fixture.session_id.clone(),
                names: ["probe_tool".to_string()].into_iter().collect(),
                witnesses: runtime_parity_witnesses(),
            }
        }
        RuntimeParityProbeInput::PublishCommittedVisibleSet => {
            MeerkatMachineCommand::PublishCommittedVisibleSet {
                session_id: fixture.session_id.clone(),
                visibility_state: Box::new(runtime_parity_publish_state()),
            }
        }
        RuntimeParityProbeInput::AbortAll => MeerkatMachineCommand::AbortAll,
        RuntimeParityProbeInput::Abort => MeerkatMachineCommand::Abort {
            session_id: fixture.session_id.clone(),
        },
        RuntimeParityProbeInput::Wait => MeerkatMachineCommand::Wait {
            session_id: fixture.session_id.clone(),
        },
        RuntimeParityProbeInput::Ingest => MeerkatMachineCommand::Ingest {
            runtime_id: fixture.runtime_id.clone(),
            input: runtime_parity_prompt("runtime parity ingest"),
        },
        RuntimeParityProbeInput::PublishEvent => MeerkatMachineCommand::PublishEvent {
            event: runtime_parity_event(&fixture.runtime_id),
        },
        RuntimeParityProbeInput::Recover => MeerkatMachineCommand::Recover {
            runtime_id: fixture.runtime_id.clone(),
        },
        RuntimeParityProbeInput::Retire => MeerkatMachineCommand::Retire {
            runtime_id: fixture.runtime_id.clone(),
        },
        RuntimeParityProbeInput::Recycle => MeerkatMachineCommand::Recycle {
            runtime_id: fixture.runtime_id.clone(),
        },
        RuntimeParityProbeInput::RuntimeState => MeerkatMachineCommand::RuntimeState {
            runtime_id: fixture.runtime_id.clone(),
        },
        RuntimeParityProbeInput::LoadBoundaryReceipt => {
            MeerkatMachineCommand::LoadBoundaryReceipt {
                runtime_id: fixture.runtime_id.clone(),
                run_id: fixture.prepared_run_id.clone().unwrap_or_default(),
                sequence: 0,
            }
        }
        RuntimeParityProbeInput::AcceptWithCompletion => {
            MeerkatMachineCommand::AcceptWithCompletion {
                session_id: fixture.session_id.clone(),
                input: runtime_parity_prompt("runtime parity accept with completion"),
            }
        }
        RuntimeParityProbeInput::AcceptWithoutWake => MeerkatMachineCommand::AcceptWithoutWake {
            session_id: fixture.session_id.clone(),
            input: runtime_parity_prompt("runtime parity accept without wake"),
        },
        RuntimeParityProbeInput::Prepare => MeerkatMachineCommand::Prepare {
            session_id: fixture.session_id.clone(),
            input: runtime_parity_prompt("runtime parity prepare"),
        },
        RuntimeParityProbeInput::Commit => {
            let input_id = fixture.prepared_input_id.clone().unwrap_or_default();
            let run_id = fixture.prepared_run_id.clone().unwrap_or_default();
            MeerkatMachineCommand::Commit {
                session_id: fixture.session_id.clone(),
                input_id: input_id.clone(),
                run_id: run_id.clone(),
                output: CoreApplyOutput {
                    receipt: RunBoundaryReceipt {
                        run_id,
                        boundary: RunApplyBoundary::RunStart,
                        contributing_input_ids: vec![input_id],
                        conversation_digest: None,
                        message_count: 0,
                        sequence: 0,
                    },
                    session_snapshot: None,
                    terminal: None,
                },
            }
        }
        RuntimeParityProbeInput::Fail => MeerkatMachineCommand::Fail {
            session_id: fixture.session_id.clone(),
            run_id: fixture.prepared_run_id.clone().unwrap_or_default(),
            error: "runtime parity failure".to_string(),
        },
        RuntimeParityProbeInput::Reset => MeerkatMachineCommand::Reset {
            runtime_id: fixture.runtime_id.clone(),
        },
        RuntimeParityProbeInput::StopRuntimeExecutor => {
            MeerkatMachineCommand::StopRuntimeExecutor {
                session_id: fixture.session_id.clone(),
                command: RunControlCommand::StopRuntimeExecutor {
                    reason: "runtime parity probe".to_string(),
                },
            }
        }
        RuntimeParityProbeInput::Destroy => MeerkatMachineCommand::Destroy {
            runtime_id: fixture.runtime_id.clone(),
        },
    }
}

fn summarize_runtime_parity_command_result(result: &MeerkatMachineCommandResult) -> String {
    match result {
        MeerkatMachineCommandResult::AcceptOutcome(outcome) => {
            format!("accept_outcome:{outcome:?}")
        }
        MeerkatMachineCommandResult::AcceptWithCompletion {
            outcome,
            handle,
            admission_signal,
        } => {
            format!(
                "accept_with_completion:{outcome:?}:handle={}:signal={admission_signal:?}",
                handle.is_some()
            )
        }
        MeerkatMachineCommandResult::Unit => "unit".to_string(),
        MeerkatMachineCommandResult::Bool(value) => format!("bool:{value}"),
        MeerkatMachineCommandResult::Spawned(value) => format!("spawned:{value}"),
        MeerkatMachineCommandResult::OpsLifecycleRegistry(registry) => {
            format!("ops_registry:{}", registry.is_some())
        }
        MeerkatMachineCommandResult::Bindings(_) => "bindings".to_string(),
        MeerkatMachineCommandResult::InputState(state) => {
            format!("input_state_present:{}", state.is_some())
        }
        MeerkatMachineCommandResult::ActiveInputs(inputs) => {
            format!("active_inputs:{}", inputs.len())
        }
        MeerkatMachineCommandResult::LlmReconfigured(report) => format!(
            "llm_reconfigured:{}->{}",
            report.previous_identity.model, report.new_identity.model
        ),
        MeerkatMachineCommandResult::VisibilityRevision(revision) => {
            format!("visibility_revision:{}", revision.0)
        }
        MeerkatMachineCommandResult::VisibilityPublished(state) => format!(
            "visibility_published:active={},staged={}",
            state.active_revision, state.staged_revision
        ),
        MeerkatMachineCommandResult::RetireReport(report) => format!(
            "retire:abandoned={},pending_drain={}",
            report.inputs_abandoned, report.inputs_pending_drain
        ),
        MeerkatMachineCommandResult::RecycleReport(report) => {
            format!("recycle:transferred={}", report.inputs_transferred)
        }
        MeerkatMachineCommandResult::ResetReport(report) => {
            format!("reset:abandoned={}", report.inputs_abandoned)
        }
        MeerkatMachineCommandResult::RecoveryReport(report) => format!(
            "recover:recovered={},abandoned={},requeued={}",
            report.inputs_recovered, report.inputs_abandoned, report.inputs_requeued
        ),
        MeerkatMachineCommandResult::DestroyReport(report) => {
            format!("destroy:abandoned={}", report.inputs_abandoned)
        }
        MeerkatMachineCommandResult::RuntimeState(state) => {
            format!("runtime_state:{}", runtime_parity_state_label(*state))
        }
        MeerkatMachineCommandResult::BoundaryReceipt(receipt) => {
            format!("boundary_receipt:{}", receipt.is_some())
        }
        MeerkatMachineCommandResult::RealtimeAttachmentStatus(status) => {
            format!("realtime_attachment_status:{status:?}")
        }
        MeerkatMachineCommandResult::RealtimeChannelStatus(status) => {
            format!("realtime_channel_status:{status:?}")
        }
        MeerkatMachineCommandResult::SessionModelRoutingStatus(status) => {
            format!(
                "session_model_routing_status:{}->{}",
                status.baseline_model, status.effective_model
            )
        }
        MeerkatMachineCommandResult::SwitchTurnControlResult(result) => {
            format!("switch_turn_control_result:{result:?}")
        }
        MeerkatMachineCommandResult::ImageOperationRoutingResult(result) => {
            format!("image_operation_routing_result:{result:?}")
        }
        MeerkatMachineCommandResult::ImageOperationPhase(phase) => {
            format!("image_operation_phase:{phase:?}")
        }
        MeerkatMachineCommandResult::Prepared(_) => "prepared".to_string(),
    }
}

fn summarize_runtime_parity_driver_error(error: &RuntimeDriverError) -> String {
    match error {
        RuntimeDriverError::NotReady { state } => {
            format!("not_ready:{}", runtime_parity_state_label(*state))
        }
        RuntimeDriverError::ValidationFailed { reason } => {
            format!("validation_failed:{reason}")
        }
        RuntimeDriverError::Destroyed => "destroyed".to_string(),
        RuntimeDriverError::Internal(reason) => format!("internal:{reason}"),
    }
}

fn summarize_runtime_parity_control_error(error: &RuntimeControlPlaneError) -> String {
    match error {
        RuntimeControlPlaneError::NotFound(runtime_id) => format!("not_found:{runtime_id}"),
        RuntimeControlPlaneError::InvalidState { state } => {
            format!("invalid_state:{}", runtime_parity_state_label(*state))
        }
        RuntimeControlPlaneError::StoreError(reason) => format!("store_error:{reason}"),
        RuntimeControlPlaneError::Internal(reason) => format!("internal:{reason}"),
    }
}

fn summarize_runtime_parity_command_error(error: &MeerkatMachineCommandError) -> String {
    match error {
        MeerkatMachineCommandError::Driver(error) => {
            format!("driver:{}", summarize_runtime_parity_driver_error(error))
        }
        MeerkatMachineCommandError::Control(error) => {
            format!("control:{}", summarize_runtime_parity_control_error(error))
        }
    }
}

async fn execute_runtime_parity_probe(
    phase: RuntimeParityPhase,
    probe: RuntimeParityProbeInput,
) -> RuntimeParityInvocationReport {
    let base_phase = if phase == RuntimeParityPhase::Running
        && matches!(
            probe,
            RuntimeParityProbeInput::Commit | RuntimeParityProbeInput::Fail
        ) {
        RuntimeParityPhase::Idle
    } else {
        phase
    };
    let mut fixture = build_runtime_parity_fixture(base_phase).await;
    let mut setup_tags = Vec::new();
    prepare_runtime_parity_probe(phase, &mut fixture, probe, &mut setup_tags).await;
    let before = runtime_parity_snapshot_summary(&fixture.adapter, &fixture.session_id).await;
    let result = if matches!(
        probe,
        RuntimeParityProbeInput::ReconfigureSessionLlmIdentity
    ) {
        SessionServiceRuntimeExt::reconfigure_session_llm_identity(
            fixture.adapter.as_ref(),
            &fixture.session_id,
            SessionLlmReconfigureRequest {
                model: Some("gpt-5.2".to_string()),
                provider: Some("openai".to_string()),
                provider_params: None,
                clear_provider_params: false,
                connection_ref: None,
                clear_connection_ref: false,
            },
        )
        .await
        .map(MeerkatMachineCommandResult::LlmReconfigured)
        .map_err(MeerkatMachineCommandError::from)
    } else {
        fixture
            .adapter
            .execute_meerkat_machine_command(
                Some(Arc::clone(&fixture.adapter)),
                runtime_parity_probe_command(&fixture, probe),
            )
            .await
    };
    let after = runtime_parity_snapshot_summary(&fixture.adapter, &fixture.session_id).await;
    assert_runtime_parity_identity_stability(probe, before.as_ref(), after.as_ref());
    let (outcome_kind, result_summary) = match &result {
        Ok(result) => (
            RuntimeParityOutcomeKind::Ok,
            summarize_runtime_parity_command_result(result),
        ),
        Err(error) => (
            RuntimeParityOutcomeKind::Err,
            summarize_runtime_parity_command_error(error),
        ),
    };
    fixture.cleanup().await;

    RuntimeParityInvocationReport {
        phase: phase.schema_name().to_string(),
        setup_tags,
        before,
        outcome_kind,
        result_summary,
        after,
    }
}

fn runtime_modeled_schema_report(
    schema: &MachineSchema,
    runtime: &RuntimeParityInvocationReport,
    probe: RuntimeParityProbeInput,
) -> RuntimeModeledStateSchemaReport {
    let Some(before) = runtime.before.as_ref() else {
        return RuntimeModeledStateSchemaReport {
            outcome_kind: RuntimeModeledStateOutcomeKind::Err,
            after: None,
            detail: "missing runtime pre-state".to_string(),
            result_summary: None,
        };
    };

    let state = runtime_modeled_kernel_state(schema, before);
    let input = match runtime_modeled_kernel_input(schema, before, probe) {
        Ok(input) => input,
        Err(detail) => {
            return RuntimeModeledStateSchemaReport {
                outcome_kind: RuntimeModeledStateOutcomeKind::Err,
                after: None,
                detail,
                result_summary: None,
            };
        }
    };

    match GeneratedMachineKernel::new(modeled_meerkat_kernel::schema()).transition(&state, &input) {
        Ok(outcome) => {
            let result_summary = runtime_modeled_schema_result_summary(before, probe, &outcome);
            RuntimeModeledStateSchemaReport {
                outcome_kind: RuntimeModeledStateOutcomeKind::Ok,
                after: runtime_modeled_summary_from_kernel_state(
                    schema,
                    &outcome.next_state,
                    before,
                ),
                detail: outcome.transition.to_string(),
                result_summary,
            }
        }
        Err(error) => RuntimeModeledStateSchemaReport {
            outcome_kind: RuntimeModeledStateOutcomeKind::Err,
            after: Some(RuntimeModeledStateSummary {
                phase: before.phase.clone(),
                formal_fields: before.formal_available_fields.clone(),
            }),
            detail: runtime_modeled_transition_refusal_detail(&error),
            result_summary: None,
        },
    }
}

fn runtime_modeled_post_admission_signal_from_effects(effects: &[KernelEffect]) -> String {
    let signal_field = modeled_field_id("signal");
    effects
        .iter()
        .find(|effect| effect.variant.as_str() == "PostAdmissionSignal")
        .and_then(|effect| effect.fields.get(&signal_field))
        .and_then(|value| match value {
            KernelValue::NamedVariant { variant, .. } => Some(variant.as_str().to_string()),
            _ => None,
        })
        .unwrap_or_else(|| "None".to_string())
}

fn runtime_modeled_schema_result_summary(
    before: &RuntimeParitySnapshotSummary,
    probe: RuntimeParityProbeInput,
    outcome: &TransitionOutcome,
) -> Option<String> {
    match probe {
        RuntimeParityProbeInput::AcceptWithCompletion => Some(format!(
            "admission_signal:{}",
            runtime_modeled_post_admission_signal_from_effects(&outcome.effects)
        )),
        RuntimeParityProbeInput::AcceptWithoutWake => Some("admission_signal:None".to_string()),
        // These control-plane reports are exact functions of the lower-level
        // input ledger carrier, not the top-level phase machine alone.
        RuntimeParityProbeInput::Destroy => Some(format!(
            "destroy:abandoned={}",
            before.ledger_non_terminal_count
        )),
        RuntimeParityProbeInput::Reset => Some(format!(
            "reset:abandoned={}",
            before.ledger_non_terminal_count
        )),
        RuntimeParityProbeInput::Recycle => Some(format!(
            "recycle:transferred={}",
            before.ledger_non_terminal_count
        )),
        _ => None,
    }
}

fn runtime_modeled_transition_refusal_detail(error: &TransitionRefusal) -> String {
    match error {
        TransitionRefusal::UnknownInputVariant { variant, .. } => {
            format!("unknown_input:{variant}")
        }
        TransitionRefusal::UnknownSignalVariant { variant, .. } => {
            format!("unknown_signal:{variant}")
        }
        TransitionRefusal::InvalidInputPayload { reason, .. } => {
            format!("invalid_input:{reason}")
        }
        TransitionRefusal::InvalidSignalPayload { reason, .. } => {
            format!("invalid_signal:{reason}")
        }
        TransitionRefusal::NoMatchingTransition { phase, trigger, .. } => {
            format!("no_match:{phase}:{trigger}")
        }
        TransitionRefusal::AmbiguousTransition {
            phase,
            trigger,
            transitions,
            ..
        } => format!("ambiguous:{phase}:{trigger}:{transitions:?}"),
        TransitionRefusal::EvaluationError {
            transition, reason, ..
        } => format!("evaluation:{transition}:{reason}"),
    }
}

fn runtime_modeled_runtime_report(
    runtime: &RuntimeParityInvocationReport,
    probe: RuntimeParityProbeInput,
) -> RuntimeModeledStateRuntimeReport {
    let before = runtime_modeled_summary_from_runtime_snapshot(runtime.before.as_ref());
    let after =
        runtime_modeled_summary_from_runtime_snapshot(runtime.after.as_ref()).or_else(|| {
            (runtime.outcome_kind == RuntimeParityOutcomeKind::Err)
                .then(|| before.clone())
                .flatten()
        });

    RuntimeModeledStateRuntimeReport {
        phase: runtime.phase.clone(),
        outcome_kind: match runtime.outcome_kind {
            RuntimeParityOutcomeKind::Ok => RuntimeModeledStateOutcomeKind::Ok,
            RuntimeParityOutcomeKind::Err => RuntimeModeledStateOutcomeKind::Err,
        },
        before,
        after,
        result_summary: runtime.result_summary.clone(),
        surface_summary: runtime_modeled_runtime_surface_summary(runtime, probe),
    }
}

fn runtime_modeled_runtime_surface_summary(
    runtime: &RuntimeParityInvocationReport,
    probe: RuntimeParityProbeInput,
) -> Option<String> {
    if runtime.outcome_kind != RuntimeParityOutcomeKind::Ok {
        return None;
    }

    match probe {
        RuntimeParityProbeInput::AcceptWithCompletion => Some(format!(
            "admission_signal:{}",
            runtime
                .result_summary
                .rsplit("signal=")
                .next()
                .unwrap_or("None")
        )),
        RuntimeParityProbeInput::AcceptWithoutWake => Some("admission_signal:None".to_string()),
        RuntimeParityProbeInput::Destroy
        | RuntimeParityProbeInput::Reset
        | RuntimeParityProbeInput::Recycle => Some(runtime.result_summary.clone()),
        _ => None,
    }
}

fn classify_runtime_parity_probe_pair(
    left: &RuntimeParityInvocationReport,
    right: &RuntimeParityInvocationReport,
) -> RuntimeParityClassification {
    match (left.outcome_kind, right.outcome_kind) {
        (RuntimeParityOutcomeKind::Ok, RuntimeParityOutcomeKind::Err) => {
            RuntimeParityClassification::LeftOnly
        }
        (RuntimeParityOutcomeKind::Err, RuntimeParityOutcomeKind::Ok) => {
            RuntimeParityClassification::RightOnly
        }
        _ if left.observable_surface() == right.observable_surface() => {
            RuntimeParityClassification::SameSurface
        }
        _ => RuntimeParityClassification::DifferentSurface,
    }
}

fn classify_runtime_modeled_schema_pair(
    left: &RuntimeModeledStateSchemaReport,
    right: &RuntimeModeledStateSchemaReport,
) -> RuntimeParityClassification {
    match (left.outcome_kind, right.outcome_kind) {
        (RuntimeModeledStateOutcomeKind::Ok, RuntimeModeledStateOutcomeKind::Err) => {
            RuntimeParityClassification::LeftOnly
        }
        (RuntimeModeledStateOutcomeKind::Err, RuntimeModeledStateOutcomeKind::Ok) => {
            RuntimeParityClassification::RightOnly
        }
        (RuntimeModeledStateOutcomeKind::Ok, RuntimeModeledStateOutcomeKind::Ok)
            if left.after == right.after && left.result_summary == right.result_summary =>
        {
            RuntimeParityClassification::SameSurface
        }
        (RuntimeModeledStateOutcomeKind::Err, RuntimeModeledStateOutcomeKind::Err)
            if left.after == right.after && left.detail == right.detail =>
        {
            RuntimeParityClassification::SameSurface
        }
        _ => RuntimeParityClassification::DifferentSurface,
    }
}

async fn probe_runtime_parity_row(
    modeled_schema: &MachineSchema,
    left_phase: RuntimeParityPhase,
    right_phase: RuntimeParityPhase,
    probe: RuntimeParityProbeInput,
) -> RuntimeParityProbeReport {
    let left = execute_runtime_parity_probe(left_phase, probe).await;
    let right = execute_runtime_parity_probe(right_phase, probe).await;
    let schema_left = runtime_modeled_schema_report(modeled_schema, &left, probe);
    let schema_right = runtime_modeled_schema_report(modeled_schema, &right, probe);
    let schema_classification = classify_runtime_modeled_schema_pair(&schema_left, &schema_right);
    let runtime_classification = classify_runtime_parity_probe_pair(&left, &right);

    RuntimeParityProbeReport {
        schema_classification,
        runtime_classification,
        agrees_with_schema: runtime_classification == schema_classification,
        schema_left,
        schema_right,
        left,
        right,
    }
}

fn runtime_parity_schema_transition_summaries_for_phase_input(
    schema: &MachineSchema,
    phase: &str,
    input_variant: &str,
) -> Vec<RuntimeParitySchemaTransitionSummary> {
    let mut summaries = schema
        .transitions
        .iter()
        .filter(|transition| {
            matches!(
                &transition.on,
                meerkat_machine_schema::TriggerMatch::Input { variant, .. }
                    if variant.as_str() == input_variant
            ) && transition.from.iter().any(|from| from.as_str() == phase)
        })
        .map(|transition| RuntimeParitySchemaTransitionSummary {
            transition: transition.name.to_string(),
            to_phase: transition.to.to_string(),
            binding_names: transition
                .on
                .bindings()
                .iter()
                .map(|b| b.as_str().to_string())
                .collect(),
            guard_names: transition
                .guards
                .iter()
                .map(|guard| guard.name.clone())
                .collect(),
            update_count: transition.updates.len(),
            update_signatures: transition
                .updates
                .iter()
                .map(|update| format!("{update:?}"))
                .collect(),
            effect_variants: transition
                .emit
                .iter()
                .map(|effect| effect.variant.as_str().to_string())
                .collect(),
        })
        .collect::<Vec<_>>();
    summaries.sort_by(|left, right| left.transition.cmp(&right.transition));
    summaries
}

fn classify_runtime_parity_schema_row(
    left: &[RuntimeParitySchemaTransitionSummary],
    right: &[RuntimeParitySchemaTransitionSummary],
) -> RuntimeParityClassification {
    if left.is_empty() && !right.is_empty() {
        return RuntimeParityClassification::RightOnly;
    }
    if !left.is_empty() && right.is_empty() {
        return RuntimeParityClassification::LeftOnly;
    }

    let left_surface = left
        .iter()
        .map(|summary| {
            (
                summary.to_phase.clone(),
                summary.binding_names.clone(),
                summary.guard_names.clone(),
                summary.update_count,
                summary.update_signatures.clone(),
                summary.effect_variants.clone(),
            )
        })
        .collect::<BTreeSet<_>>();
    let right_surface = right
        .iter()
        .map(|summary| {
            (
                summary.to_phase.clone(),
                summary.binding_names.clone(),
                summary.guard_names.clone(),
                summary.update_count,
                summary.update_signatures.clone(),
                summary.effect_variants.clone(),
            )
        })
        .collect::<BTreeSet<_>>();

    if left_surface == right_surface {
        RuntimeParityClassification::SameSurface
    } else {
        RuntimeParityClassification::DifferentSurface
    }
}

fn runtime_parity_schema_rows_for_pair(
    schema: &MachineSchema,
    left_phase: RuntimeParityPhase,
    right_phase: RuntimeParityPhase,
) -> Vec<RuntimeParitySchemaRow> {
    let mut rows = Vec::new();

    for input_variant in &schema.inputs.variants {
        let left = runtime_parity_schema_transition_summaries_for_phase_input(
            schema,
            left_phase.schema_name(),
            input_variant.name.as_str(),
        );
        let right = runtime_parity_schema_transition_summaries_for_phase_input(
            schema,
            right_phase.schema_name(),
            input_variant.name.as_str(),
        );
        if left.is_empty() && right.is_empty() {
            continue;
        }

        rows.push(RuntimeParitySchemaRow {
            input_variant: input_variant.name.as_str().to_string(),
            classification: classify_runtime_parity_schema_row(&left, &right),
            left,
            right,
        });
    }

    rows
}

async fn build_runtime_parity_pair_report(
    static_schema: &MachineSchema,
    modeled_schema: &MachineSchema,
    left_phase: RuntimeParityPhase,
    right_phase: RuntimeParityPhase,
    include_same_surface_rows: bool,
) -> RuntimeParityPairReport {
    let mut rows = Vec::new();
    let surface_only_inputs = static_schema
        .surface_only_inputs
        .iter()
        .map(|v| v.as_str())
        .collect::<BTreeSet<_>>();

    for schema_row in runtime_parity_schema_rows_for_pair(static_schema, left_phase, right_phase) {
        let probe_required = !surface_only_inputs.contains(schema_row.input_variant.as_str());
        let probe =
            runtime_parity_probe_for_input_variant(&schema_row.input_variant).map(|probe_input| {
                Box::pin(probe_runtime_parity_row(
                    modeled_schema,
                    left_phase,
                    right_phase,
                    probe_input,
                ))
            });
        let probe = match probe {
            Some(probe) => Some(probe.await),
            None => None,
        };
        let effective_schema_classification = probe
            .as_ref()
            .map(|probe| probe.schema_classification)
            .unwrap_or(schema_row.classification);
        if !include_same_surface_rows
            && effective_schema_classification == RuntimeParityClassification::SameSurface
        {
            continue;
        }
        let note = match &probe {
            Some(probe) if probe.schema_classification != schema_row.classification => {
                Some(format!(
                    "static schema classified {:?}, simulated schema classified {:?}",
                    schema_row.classification, probe.schema_classification
                ))
            }
            Some(_) => None,
            None if probe_required => Some(
                "required runtime probe missing; non-surface-only omissions fail closed"
                    .to_string(),
            ),
            None => Some("surface-only input: runtime probe not required".to_string()),
        };

        rows.push(RuntimeParityRowReport {
            input_variant: schema_row.input_variant,
            probe_required,
            static_schema_classification: schema_row.classification,
            static_schema_left: schema_row.left,
            static_schema_right: schema_row.right,
            probe,
            note,
        });
    }

    let summary = rows.iter().fold(
        RuntimeParityPairSummary {
            interesting_rows: rows.len(),
            ..Default::default()
        },
        |mut summary, row| {
            match &row.probe {
                Some(probe) => {
                    summary.probed_rows += 1;
                    if probe.agrees_with_schema {
                        summary.aligned_rows += 1;
                    } else {
                        summary.mismatched_rows += 1;
                    }
                }
                None if row.probe_required => summary.unprobed_rows += 1,
                None => summary.surface_only_unprobed_rows += 1,
            }
            summary
        },
    );

    RuntimeParityPairReport {
        left_phase: left_phase.schema_name().to_string(),
        right_phase: right_phase.schema_name().to_string(),
        summary,
        rows,
    }
}

async fn write_runtime_parity_audit_report(
    include_same_surface_rows: bool,
    path: PathBuf,
) -> RuntimeParityAuditReport {
    let static_schema = schema_meerkat_machine();
    let modeled_schema = modeled_meerkat_kernel::schema();
    let mut pairs = Vec::new();

    for &(left_phase, right_phase) in runtime_parity_target_pairs() {
        pairs.push(
            build_runtime_parity_pair_report(
                &static_schema,
                &modeled_schema,
                left_phase,
                right_phase,
                include_same_surface_rows,
            )
            .await,
        );
    }

    let summary = pairs.iter().fold(
        RuntimeParityAuditSummary {
            pair_count: pairs.len(),
            ..Default::default()
        },
        |mut summary, pair| {
            summary.interesting_rows += pair.summary.interesting_rows;
            summary.probed_rows += pair.summary.probed_rows;
            summary.aligned_rows += pair.summary.aligned_rows;
            summary.mismatched_rows += pair.summary.mismatched_rows;
            summary.unprobed_rows += pair.summary.unprobed_rows;
            summary.surface_only_unprobed_rows += pair.summary.surface_only_unprobed_rows;
            summary
        },
    );

    let report = RuntimeParityAuditReport {
        machine: "MeerkatMachine".to_string(),
        generated_at: Utc::now().to_rfc3339(),
        summary,
        pairs,
    };

    std::fs::write(
        &path,
        serde_json::to_vec_pretty(&report).expect("serialize runtime parity report"),
    )
    .expect("write runtime parity report");
    assert_eq!(
        report.summary.unprobed_rows,
        0,
        "MeerkatMachine runtime parity report has required non-surface-only inputs without probes; report written to {}",
        path.display()
    );

    report
}

async fn write_runtime_modeled_state_audit_report(path: PathBuf) -> RuntimeModeledStateAuditReport {
    let schema = modeled_meerkat_kernel::schema();
    let surface_only_inputs = schema
        .surface_only_inputs
        .iter()
        .map(|v| v.as_str())
        .collect::<BTreeSet<_>>();
    let mut rows = Vec::new();

    for phase in [
        RuntimeParityPhase::Idle,
        RuntimeParityPhase::Attached,
        RuntimeParityPhase::Running,
        RuntimeParityPhase::Retired,
        RuntimeParityPhase::Stopped,
    ] {
        for input_variant in &schema.inputs.variants {
            if surface_only_inputs.contains(input_variant.name.as_str()) {
                continue;
            }
            let Some(probe) = runtime_parity_probe_for_input_variant(input_variant.name.as_str())
            else {
                rows.push(RuntimeModeledStateRowReport {
                    phase: phase.schema_name().to_string(),
                    input_variant: input_variant.name.as_str().to_string(),
                    aligned: false,
                    differing_keys: vec!["unprobed".to_string()],
                    runtime: RuntimeModeledStateRuntimeReport {
                        phase: phase.schema_name().to_string(),
                        outcome_kind: RuntimeModeledStateOutcomeKind::Err,
                        before: None,
                        after: None,
                        result_summary:
                            "required runtime probe missing; non-surface-only omissions fail closed"
                                .to_string(),
                        surface_summary: None,
                    },
                    schema: RuntimeModeledStateSchemaReport {
                        outcome_kind: RuntimeModeledStateOutcomeKind::Err,
                        after: None,
                        detail:
                            "required runtime probe missing; non-surface-only omissions fail closed"
                                .to_string(),
                        result_summary: None,
                    },
                });
                continue;
            };

            let runtime = execute_runtime_parity_probe(phase, probe).await;
            let schema_report = runtime_modeled_schema_report(&schema, &runtime, probe);
            let runtime_report = runtime_modeled_runtime_report(&runtime, probe);
            let differing_keys =
                runtime_modeled_differing_keys(&runtime_report.after, &schema_report.after);
            let aligned = runtime_report.outcome_kind == schema_report.outcome_kind
                && differing_keys.is_empty()
                && runtime_report.surface_summary == schema_report.result_summary;

            rows.push(RuntimeModeledStateRowReport {
                phase: phase.schema_name().to_string(),
                input_variant: input_variant.name.as_str().to_string(),
                aligned,
                differing_keys,
                runtime: runtime_report,
                schema: schema_report,
            });
        }
    }

    let summary = rows.iter().fold(
        RuntimeModeledStateAuditSummary::default(),
        |mut summary, row| {
            summary.row_count += 1;
            if row.runtime.result_summary
                == "required runtime probe missing; non-surface-only omissions fail closed"
            {
                summary.unprobed_rows += 1;
            } else if row.aligned {
                summary.aligned_rows += 1;
            } else {
                summary.mismatched_rows += 1;
            }
            summary
        },
    );

    let report = RuntimeModeledStateAuditReport {
        machine: "MeerkatMachine".to_string(),
        generated_at: Utc::now().to_rfc3339(),
        summary,
        rows,
    };

    std::fs::write(
        &path,
        serde_json::to_vec_pretty(&report).expect("serialize modeled-state audit report"),
    )
    .expect("write modeled-state audit report");
    assert_eq!(
        report.summary.unprobed_rows,
        0,
        "MeerkatMachine modeled-state parity report has required non-surface-only inputs without probes; report written to {}",
        path.display()
    );

    report
}

// ---------------------------------------------------------------------------
// Per-session mutation gate tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn retire_from_retired_is_backed_by_dsl_idempotent_transition() {
    let schema = schema_meerkat_machine();
    let has_retired_self_loop = schema.transitions.iter().any(|transition| {
        transition.on.variant_str() == "Retire"
            && transition
                .from
                .iter()
                .any(|phase| phase.as_str() == "Retired")
            && transition.to.as_str() == "Retired"
    });
    assert!(
        has_retired_self_loop,
        "Retire idempotence must be represented by the MeerkatMachine DSL"
    );

    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    adapter.register_session(session_id.clone()).await;

    let runtime_id = runtime_id_for_session(&session_id);
    crate::traits::RuntimeControlPlane::retire(adapter.as_ref(), &runtime_id)
        .await
        .expect("initial retire should succeed");

    let report = crate::traits::RuntimeControlPlane::retire(adapter.as_ref(), &runtime_id)
        .await
        .expect("retire from Retired should succeed idempotently through DSL");
    assert_eq!(report.inputs_abandoned, 0);
    assert_eq!(report.inputs_pending_drain, 0);

    let state = crate::traits::RuntimeControlPlane::runtime_state(adapter.as_ref(), &runtime_id)
        .await
        .expect("runtime state should remain readable");
    assert_eq!(state, RuntimeState::Retired);
}

#[tokio::test]
async fn reset_from_running_surfaces_dsl_rejection() {
    let fixture = build_runtime_parity_fixture(RuntimeParityPhase::Running).await;

    let result = fixture
        .adapter
        .execute_meerkat_machine_command(
            Some(Arc::clone(&fixture.adapter)),
            MeerkatMachineCommand::Reset {
                runtime_id: fixture.runtime_id.clone(),
            },
        )
        .await;

    fixture.cleanup().await;

    let err = result.expect_err("reset from Running should reject through DSL");
    assert!(
        matches!(
            err,
            MeerkatMachineCommandError::Control(RuntimeControlPlaneError::Internal(ref reason))
                if reason.contains("DSL authority (Reset)")
                    && reason.contains("Running")
                    && reason.contains("Reset")
        ),
        "expected DSL authority rejection for Reset from Running, got {err:?}"
    );
}

#[tokio::test]
async fn destroy_from_bound_initializing_is_backed_by_dsl_guard() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    let runtime_id = runtime_id_for_session(&session_id);

    adapter
        .prepare_bindings(session_id.clone())
        .await
        .expect("bindings should establish active runtime identity");

    let driver = {
        let sessions = adapter.sessions.read().await;
        Arc::clone(
            &sessions
                .get(&session_id)
                .expect("prepared session should exist")
                .driver,
        )
    };
    {
        let mut entry = driver.lock().await;
        let DriverEntry::Ephemeral(driver) = &mut *entry else {
            panic!("test uses ephemeral driver");
        };
        driver.force_runtime_authority(RuntimeState::Initializing, None, None);
        driver.sync_control_projection_from_dsl_authority();
    }

    let report = crate::traits::RuntimeControlPlane::destroy(adapter.as_ref(), &runtime_id)
        .await
        .expect("bound Initializing destroy should follow the DSL DestroyInitializing guard");
    assert_eq!(report.inputs_abandoned, 0);

    let state = crate::traits::RuntimeControlPlane::runtime_state(adapter.as_ref(), &runtime_id)
        .await
        .expect("runtime state should remain readable after destroy");
    assert_eq!(state, RuntimeState::Destroyed);
}

/// Two concurrent Retire commands on the same session must serialize: the
/// first stages the mutating DSL transition, and the second reaches the
/// DSL-authoritative Retired self-loop and completes idempotently.
#[tokio::test]
async fn concurrent_retire_serializes_via_mutation_gate() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    adapter.register_session(session_id.clone()).await;

    // Accept an input so the session has queued work.
    let outcome = <MeerkatMachine as SessionServiceRuntimeExt>::accept_input(
        &adapter,
        &session_id,
        make_prompt("concurrent-retire-gate"),
    )
    .await
    .expect("accept should succeed");
    assert!(matches!(outcome, AcceptOutcome::Accepted { .. }));

    // Launch two concurrent Retire commands on the same session.
    let adapter_a = adapter.clone();
    let sid_a = session_id.clone();
    let retire_a = tokio::spawn(async move {
        <MeerkatMachine as SessionServiceRuntimeExt>::retire_runtime(&adapter_a, &sid_a).await
    });
    let adapter_b = adapter.clone();
    let sid_b = session_id.clone();
    let retire_b = tokio::spawn(async move {
        <MeerkatMachine as SessionServiceRuntimeExt>::retire_runtime(&adapter_b, &sid_b).await
    });

    let (result_a, result_b) = tokio::join!(retire_a, retire_b);
    let result_a = result_a.expect("task a should not panic");
    let result_b = result_b.expect("task b should not panic");

    // Both command calls should succeed. The mutation gate still matters:
    // it ensures the mutating Retire and idempotent Retire self-loop are
    // observed in sequence.
    let successes = [&result_a, &result_b].iter().filter(|r| r.is_ok()).count();
    let failures = [&result_a, &result_b].iter().filter(|r| r.is_err()).count();

    assert_eq!(
        successes, 2,
        "both concurrent Retire commands should succeed idempotently, got: a={result_a:?}, b={result_b:?}"
    );
    assert_eq!(
        failures, 0,
        "no concurrent Retire command should fail idempotence, got: a={result_a:?}, b={result_b:?}"
    );

    // Verify the session is in Retired state after both commands complete.
    let state = <MeerkatMachine as SessionServiceRuntimeExt>::runtime_state(&adapter, &session_id)
        .await
        .expect("runtime state should be readable");
    assert_eq!(
        state,
        RuntimeState::Retired,
        "session should be Retired after serialized concurrent retires"
    );
}

/// Once the DSL-authoritative session phase is Retired, another retire should
/// succeed through the DSL-owned idempotent Retire self-loop.
#[tokio::test]
async fn retire_runtime_is_idempotent_from_retired() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    adapter.register_session(session_id.clone()).await;

    // Transition: Idle → Retired (no runtime loop, so inputs are abandoned)
    let _ = <MeerkatMachine as SessionServiceRuntimeExt>::retire_runtime(&adapter, &session_id)
        .await
        .expect("retire should succeed");

    let state_after_retire =
        <MeerkatMachine as SessionServiceRuntimeExt>::runtime_state(&adapter, &session_id)
            .await
            .expect("runtime state after retire");
    assert_eq!(state_after_retire, RuntimeState::Retired);

    // Attempt Reset from Retired (valid transition) → Idle
    let _ = <MeerkatMachine as SessionServiceRuntimeExt>::reset_runtime(&adapter, &session_id)
        .await
        .expect("reset from Retired should succeed");

    let state_after_reset =
        <MeerkatMachine as SessionServiceRuntimeExt>::runtime_state(&adapter, &session_id)
            .await
            .expect("runtime state after reset");
    assert_eq!(state_after_reset, RuntimeState::Idle);

    // First Retire → should succeed again.
    let _ = <MeerkatMachine as SessionServiceRuntimeExt>::retire_runtime(&adapter, &session_id)
        .await
        .expect("second retire should succeed");

    let state = <MeerkatMachine as SessionServiceRuntimeExt>::runtime_state(&adapter, &session_id)
        .await
        .expect("runtime state");
    assert_eq!(state, RuntimeState::Retired);

    // Attempt a second Retire from Retired. The command path should be
    // idempotent through the DSL Retire self-loop.
    let report =
        <MeerkatMachine as SessionServiceRuntimeExt>::retire_runtime(&adapter, &session_id)
            .await
            .expect("retire from Retired should succeed idempotently");
    assert_eq!(report.inputs_abandoned, 0);
    assert_eq!(report.inputs_pending_drain, 0);

    // Verify state is still Retired.
    let state_after_idempotent =
        <MeerkatMachine as SessionServiceRuntimeExt>::runtime_state(&adapter, &session_id)
            .await
            .expect("runtime state after idempotent retire");
    assert_eq!(
        state_after_idempotent,
        RuntimeState::Retired,
        "DSL state should be unchanged after an idempotent Retire command"
    );
}

/// The mutation gate must be per-session: commands on different sessions
/// should not block each other. Verify that two concurrent Retire commands
/// on DIFFERENT sessions both succeed.
#[tokio::test]
async fn mutation_gate_is_per_session() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let sid_a = SessionId::new();
    let sid_b = SessionId::new();
    adapter.register_session(sid_a.clone()).await;
    adapter.register_session(sid_b.clone()).await;

    let adapter_a = adapter.clone();
    let sa = sid_a.clone();
    let retire_a = tokio::spawn(async move {
        <MeerkatMachine as SessionServiceRuntimeExt>::retire_runtime(&adapter_a, &sa).await
    });
    let adapter_b = adapter.clone();
    let sb = sid_b.clone();
    let retire_b = tokio::spawn(async move {
        <MeerkatMachine as SessionServiceRuntimeExt>::retire_runtime(&adapter_b, &sb).await
    });

    let (result_a, result_b) = tokio::join!(retire_a, retire_b);
    let result_a = result_a.expect("task a should not panic");
    let result_b = result_b.expect("task b should not panic");

    assert!(
        result_a.is_ok(),
        "Retire on session A should succeed: {result_a:?}"
    );
    assert!(
        result_b.is_ok(),
        "Retire on session B should succeed: {result_b:?}"
    );
}

// =====================================================================
// W2-G: Peer-ingress transport capability ownership
// =====================================================================

#[tokio::test]
async fn peer_ingress_owner_starts_unattached() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    adapter.register_session(session_id.clone()).await;

    let owner = adapter.peer_ingress_owner(&session_id).await;
    assert!(
        matches!(owner, crate::meerkat_machine::PeerIngressOwner::Unattached),
        "freshly registered session should have Unattached peer-ingress owner, got {owner:?}"
    );
}

#[tokio::test]
async fn peer_ingress_owner_unknown_session_is_unattached() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();

    let owner = adapter.peer_ingress_owner(&session_id).await;
    assert!(
        matches!(owner, crate::meerkat_machine::PeerIngressOwner::Unattached),
        "unknown session should read as Unattached, got {owner:?}"
    );
}

#[tokio::test]
async fn attach_session_ingress_transitions_owner() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    adapter.register_session(session_id.clone()).await;

    let comms_runtime: Arc<dyn CommsRuntime> = Arc::new(FakeDrainRuntime::idle());
    adapter
        .update_peer_ingress_context(&session_id, true, Some(Arc::clone(&comms_runtime)))
        .await;

    let owner = adapter.peer_ingress_owner(&session_id).await;
    let expected_id = crate::meerkat_machine::dsl::CommsRuntimeId::from_runtime(&comms_runtime);
    match owner {
        crate::meerkat_machine::PeerIngressOwner::SessionOwned { comms_runtime_id } => {
            assert_eq!(
                comms_runtime_id, expected_id,
                "session-owned drain should carry the exact comms runtime id"
            );
        }
        other => panic!("expected SessionOwned, got {other:?}"),
    }
}

#[tokio::test]
async fn attach_mob_ingress_transitions_owner() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    adapter.register_session(session_id.clone()).await;

    let comms_runtime: Arc<dyn CommsRuntime> = Arc::new(FakeDrainRuntime::idle());
    let mob_id = crate::meerkat_machine::dsl::MobId::from("mob-w2g-test");
    adapter
        .maybe_spawn_mob_comms_drain(&session_id, Arc::clone(&comms_runtime), mob_id.clone())
        .await;

    let owner = adapter.peer_ingress_owner(&session_id).await;
    let expected_id = crate::meerkat_machine::dsl::CommsRuntimeId::from_runtime(&comms_runtime);
    match owner {
        crate::meerkat_machine::PeerIngressOwner::MobOwned {
            comms_runtime_id,
            mob_id: actual_mob_id,
        } => {
            assert_eq!(comms_runtime_id, expected_id);
            assert_eq!(actual_mob_id, mob_id);
        }
        other => panic!("expected MobOwned, got {other:?}"),
    }
}

#[tokio::test]
async fn mob_owned_drain_rejects_silent_session_downgrade() {
    // The spec's regression class: once a mob has claimed peer-ingress
    // ownership, a later session-runtime attach must not silently swap the
    // comms runtime out from under the mob. The command must surface the DSL
    // rejection and stop before the mechanical drain slot can rebind.
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    adapter.register_session(session_id.clone()).await;

    // Mob claims ownership with a specific comms runtime instance.
    let mob_comms: Arc<dyn CommsRuntime> = Arc::new(FakeDrainRuntime::idle());
    let mob_id = crate::meerkat_machine::dsl::MobId::from("mob-w2g-nodowngrade");
    assert!(
        adapter
            .maybe_spawn_mob_comms_drain(&session_id, Arc::clone(&mob_comms), mob_id.clone())
            .await,
        "initial mob-owned drain should spawn"
    );
    let expected_id = crate::meerkat_machine::dsl::CommsRuntimeId::from_runtime(&mob_comms);

    let phase_before = current_phase(&adapter, &session_id).await;
    let session_comms: Arc<dyn CommsRuntime> = Arc::new(FakeDrainRuntime::idle());
    let downgrade = adapter
        .execute_meerkat_machine_command(
            Some(Arc::clone(&adapter)),
            MeerkatMachineCommand::SetPeerIngressContext {
                session_id: session_id.clone(),
                keep_alive: true,
                comms_runtime: Some(Arc::clone(&session_comms)),
                mob_id: None,
            },
        )
        .await;
    assert!(
        matches!(
            downgrade,
            Err(MeerkatMachineCommandError::Driver(
                RuntimeDriverError::ValidationFailed { .. }
            ))
        ),
        "mob-owned downgrade must surface a DSL validation failure, got {downgrade:?}"
    );
    assert_eq!(
        current_phase(&adapter, &session_id).await,
        phase_before,
        "mechanical drain slot must not be rebound after DSL rejection"
    );

    // Owner must remain `MobOwned` with the original comms runtime id.
    let owner = adapter.peer_ingress_owner(&session_id).await;
    match owner {
        crate::meerkat_machine::PeerIngressOwner::MobOwned {
            comms_runtime_id,
            mob_id: actual_mob_id,
        } => {
            assert_eq!(
                comms_runtime_id, expected_id,
                "mob-owned comms runtime id must be unchanged after session swap attempt"
            );
            assert_eq!(actual_mob_id, mob_id);
        }
        other => panic!("expected MobOwned to survive downgrade attempt, got {other:?}"),
    }
}

#[tokio::test]
async fn attach_session_ingress_exact_reassertion_is_idempotent() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    adapter.register_session(session_id.clone()).await;

    let comms_runtime: Arc<dyn CommsRuntime> = Arc::new(FakeDrainRuntime::idle());
    assert!(
        adapter
            .update_peer_ingress_context(&session_id, true, Some(Arc::clone(&comms_runtime)))
            .await,
        "first session-owned attach should spawn"
    );

    let reassert = adapter
        .execute_meerkat_machine_command(
            Some(Arc::clone(&adapter)),
            MeerkatMachineCommand::SetPeerIngressContext {
                session_id: session_id.clone(),
                keep_alive: true,
                comms_runtime: Some(Arc::clone(&comms_runtime)),
                mob_id: None,
            },
        )
        .await
        .expect("exact session-owned reassertion should be accepted");
    assert!(
        matches!(reassert, MeerkatMachineCommandResult::Spawned(false)),
        "idempotent reassertion should not spawn or rebind, got {reassert:?}"
    );

    let owner = adapter.peer_ingress_owner(&session_id).await;
    let expected_id = crate::meerkat_machine::dsl::CommsRuntimeId::from_runtime(&comms_runtime);
    assert!(
        matches!(
            owner,
            crate::meerkat_machine::PeerIngressOwner::SessionOwned { ref comms_runtime_id }
                if *comms_runtime_id == expected_id
        ),
        "session owner should remain bound to the original runtime, got {owner:?}"
    );
}

#[tokio::test]
async fn attach_mob_ingress_exact_reassertion_is_idempotent() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    adapter.register_session(session_id.clone()).await;

    let comms_runtime: Arc<dyn CommsRuntime> = Arc::new(FakeDrainRuntime::idle());
    let mob_id = crate::meerkat_machine::dsl::MobId::from("mob-w2g-idempotent");
    assert!(
        adapter
            .maybe_spawn_mob_comms_drain(&session_id, Arc::clone(&comms_runtime), mob_id.clone())
            .await,
        "first mob-owned attach should spawn"
    );

    let reassert = adapter
        .execute_meerkat_machine_command(
            Some(Arc::clone(&adapter)),
            MeerkatMachineCommand::SetPeerIngressContext {
                session_id: session_id.clone(),
                keep_alive: true,
                comms_runtime: Some(Arc::clone(&comms_runtime)),
                mob_id: Some(mob_id.clone()),
            },
        )
        .await
        .expect("exact mob-owned reassertion should be accepted");
    assert!(
        matches!(reassert, MeerkatMachineCommandResult::Spawned(false)),
        "idempotent mob reassertion should not spawn or rebind, got {reassert:?}"
    );

    let owner = adapter.peer_ingress_owner(&session_id).await;
    let expected_id = crate::meerkat_machine::dsl::CommsRuntimeId::from_runtime(&comms_runtime);
    assert!(
        matches!(
            owner,
            crate::meerkat_machine::PeerIngressOwner::MobOwned {
                ref comms_runtime_id,
                mob_id: ref actual_mob_id,
            } if *comms_runtime_id == expected_id && *actual_mob_id == mob_id
        ),
        "mob owner should remain bound to the original runtime and mob, got {owner:?}"
    );
}

#[tokio::test]
async fn attach_mob_ingress_rejects_conflicting_mob_rebind() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    adapter.register_session(session_id.clone()).await;

    let comms_runtime: Arc<dyn CommsRuntime> = Arc::new(FakeDrainRuntime::idle());
    let mob_id = crate::meerkat_machine::dsl::MobId::from("mob-w2g-original");
    assert!(
        adapter
            .maybe_spawn_mob_comms_drain(&session_id, Arc::clone(&comms_runtime), mob_id.clone())
            .await,
        "first mob-owned attach should spawn"
    );

    let conflicting_comms: Arc<dyn CommsRuntime> = Arc::new(FakeDrainRuntime::idle());
    let conflicting_mob_id = crate::meerkat_machine::dsl::MobId::from("mob-w2g-conflict");
    let rebind = adapter
        .execute_meerkat_machine_command(
            Some(Arc::clone(&adapter)),
            MeerkatMachineCommand::SetPeerIngressContext {
                session_id: session_id.clone(),
                keep_alive: true,
                comms_runtime: Some(conflicting_comms),
                mob_id: Some(conflicting_mob_id),
            },
        )
        .await;
    assert!(
        matches!(
            rebind,
            Err(MeerkatMachineCommandError::Driver(
                RuntimeDriverError::ValidationFailed { .. }
            ))
        ),
        "conflicting mob-owned rebind must surface a DSL validation failure, got {rebind:?}"
    );

    let owner = adapter.peer_ingress_owner(&session_id).await;
    let expected_id = crate::meerkat_machine::dsl::CommsRuntimeId::from_runtime(&comms_runtime);
    assert!(
        matches!(
            owner,
            crate::meerkat_machine::PeerIngressOwner::MobOwned {
                ref comms_runtime_id,
                mob_id: ref actual_mob_id,
            } if *comms_runtime_id == expected_id && *actual_mob_id == mob_id
        ),
        "conflicting mob rebind must leave authoritative owner unchanged, got {owner:?}"
    );
}

#[tokio::test]
async fn detach_ingress_unattached_is_idempotent_noop() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    adapter.register_session(session_id.clone()).await;

    let detach = adapter
        .execute_meerkat_machine_command(
            Some(Arc::clone(&adapter)),
            MeerkatMachineCommand::SetPeerIngressContext {
                session_id: session_id.clone(),
                keep_alive: false,
                comms_runtime: None,
                mob_id: None,
            },
        )
        .await
        .expect("no-op detach from Unattached should be accepted");
    assert!(
        matches!(detach, MeerkatMachineCommandResult::Spawned(false)),
        "no-op detach should not spawn, got {detach:?}"
    );

    let owner = adapter.peer_ingress_owner(&session_id).await;
    assert!(
        matches!(owner, crate::meerkat_machine::PeerIngressOwner::Unattached),
        "no-op detach should leave owner Unattached, got {owner:?}"
    );
}

#[tokio::test]
async fn detach_ingress_clears_owner() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    adapter.register_session(session_id.clone()).await;

    // Attach a session-owned drain first.
    let comms_runtime: Arc<dyn CommsRuntime> = Arc::new(FakeDrainRuntime::idle());
    adapter
        .update_peer_ingress_context(&session_id, true, Some(Arc::clone(&comms_runtime)))
        .await;

    // Now request detach (keep_alive=false). The shell stages
    // `DetachIngress` into the DSL.
    adapter
        .update_peer_ingress_context(&session_id, false, None)
        .await;

    let owner = adapter.peer_ingress_owner(&session_id).await;
    assert!(
        matches!(owner, crate::meerkat_machine::PeerIngressOwner::Unattached),
        "keep_alive=false should detach peer-ingress owner, got {owner:?}"
    );
}

#[tokio::test]
async fn attach_mob_ingress_promotes_from_session_owned() {
    // Mob provisioning is allowed to take over a session-owned drain
    // (the spec's promotion case). Silent downgrade is blocked; promotion
    // is not.
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    adapter.register_session(session_id.clone()).await;

    // Step 1: session attach.
    let session_comms: Arc<dyn CommsRuntime> = Arc::new(FakeDrainRuntime::idle());
    adapter
        .update_peer_ingress_context(&session_id, true, Some(Arc::clone(&session_comms)))
        .await;

    // Step 2: mob provisioning promotes to MobOwned with (possibly) a
    // different comms runtime.
    let mob_comms: Arc<dyn CommsRuntime> = Arc::new(FakeDrainRuntime::idle());
    let mob_id = crate::meerkat_machine::dsl::MobId::from("mob-w2g-promotion");
    adapter
        .maybe_spawn_mob_comms_drain(&session_id, Arc::clone(&mob_comms), mob_id.clone())
        .await;

    let owner = adapter.peer_ingress_owner(&session_id).await;
    let expected_id = crate::meerkat_machine::dsl::CommsRuntimeId::from_runtime(&mob_comms);
    match owner {
        crate::meerkat_machine::PeerIngressOwner::MobOwned {
            comms_runtime_id,
            mob_id: actual_mob_id,
        } => {
            assert_eq!(comms_runtime_id, expected_id);
            assert_eq!(actual_mob_id, mob_id);
        }
        other => panic!("expected MobOwned after promotion, got {other:?}"),
    }
}
