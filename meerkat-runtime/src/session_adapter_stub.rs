use std::future::Future;
use std::sync::Arc;

use meerkat_core::lifecycle::core_executor::CoreApplyOutput;
use meerkat_core::lifecycle::run_control::RunControlCommand;
use meerkat_core::lifecycle::{InputId, RunId};
use meerkat_core::types::SessionId;

use crate::accept::AcceptOutcome;
use crate::identifiers::LogicalRuntimeId;
use crate::input::Input;
use crate::input_state::InputState;
use crate::runtime_state::RuntimeState;
use crate::service_ext::RuntimeMode;
use crate::store::RuntimeStore;
use crate::traits::{ResetReport, RetireReport, RuntimeDriverError};

#[derive(Clone, Default)]
pub struct RuntimeSessionAdapter;

impl RuntimeSessionAdapter {
    pub fn ephemeral() -> Self {
        Self
    }

    pub fn persistent(_store: Arc<dyn RuntimeStore>) -> Self {
        Self
    }

    pub fn runtime_mode(&self) -> RuntimeMode {
        RuntimeMode::V9Compliant
    }

    pub async fn register_session(&self, _session_id: SessionId) {}

    pub async fn register_session_with_executor(
        &self,
        _session_id: SessionId,
        _executor: Box<dyn meerkat_core::lifecycle::CoreExecutor>,
    ) {
    }

    pub async fn ensure_session_with_executor(
        &self,
        _session_id: SessionId,
        _executor: Box<dyn meerkat_core::lifecycle::CoreExecutor>,
    ) {
    }

    pub async fn unregister_session(&self, _session_id: &SessionId) {}

    pub async fn contains_session(&self, _session_id: &SessionId) -> bool {
        false
    }

    pub async fn interrupt_current_run(
        &self,
        _session_id: &SessionId,
    ) -> Result<(), RuntimeDriverError> {
        Err(RuntimeDriverError::NotReady {
            state: RuntimeState::Destroyed,
        })
    }

    pub async fn accept_input_and_run<T, F, Fut>(
        &self,
        _session_id: &SessionId,
        _input: Input,
        _op: F,
    ) -> Result<T, RuntimeDriverError>
    where
        F: FnOnce(RunId, meerkat_core::lifecycle::run_primitive::RunPrimitive) -> Fut,
        Fut: Future<Output = Result<(T, CoreApplyOutput), RuntimeDriverError>>,
    {
        Err(RuntimeDriverError::NotReady {
            state: RuntimeState::Destroyed,
        })
    }

    pub async fn accept_input(
        &self,
        _session_id: &SessionId,
        _input: Input,
    ) -> Result<AcceptOutcome, RuntimeDriverError> {
        Err(RuntimeDriverError::NotReady {
            state: RuntimeState::Destroyed,
        })
    }

    pub async fn runtime_state(
        &self,
        _session_id: &SessionId,
    ) -> Result<RuntimeState, RuntimeDriverError> {
        Err(RuntimeDriverError::NotReady {
            state: RuntimeState::Destroyed,
        })
    }

    pub async fn retire_runtime(
        &self,
        _session_id: &SessionId,
    ) -> Result<RetireReport, RuntimeDriverError> {
        Err(RuntimeDriverError::NotReady {
            state: RuntimeState::Destroyed,
        })
    }

    pub async fn reset_runtime(
        &self,
        _session_id: &SessionId,
    ) -> Result<ResetReport, RuntimeDriverError> {
        Err(RuntimeDriverError::NotReady {
            state: RuntimeState::Destroyed,
        })
    }

    pub async fn input_state(
        &self,
        _session_id: &SessionId,
        _input_id: &InputId,
    ) -> Result<Option<InputState>, RuntimeDriverError> {
        Err(RuntimeDriverError::NotReady {
            state: RuntimeState::Destroyed,
        })
    }

    pub async fn list_active_inputs(
        &self,
        _session_id: &SessionId,
    ) -> Result<Vec<InputId>, RuntimeDriverError> {
        Err(RuntimeDriverError::NotReady {
            state: RuntimeState::Destroyed,
        })
    }

    pub fn logical_runtime_id(session_id: &SessionId) -> LogicalRuntimeId {
        LogicalRuntimeId::new(session_id.to_string())
    }

    pub async fn on_run_completed(
        &self,
        _session_id: &SessionId,
        _run_id: &RunId,
        _contributing_input_ids: &[InputId],
    ) -> Result<(), RuntimeDriverError> {
        Err(RuntimeDriverError::NotReady {
            state: RuntimeState::Destroyed,
        })
    }

    pub async fn on_run_failed(
        &self,
        _session_id: &SessionId,
        _run_id: &RunId,
        _contributing_input_ids: &[InputId],
        _reason: String,
    ) -> Result<(), RuntimeDriverError> {
        Err(RuntimeDriverError::NotReady {
            state: RuntimeState::Destroyed,
        })
    }

    pub async fn stop_runtime_executor(
        &self,
        _session_id: &SessionId,
        _command: RunControlCommand,
    ) -> Result<(), RuntimeDriverError> {
        Err(RuntimeDriverError::NotReady {
            state: RuntimeState::Destroyed,
        })
    }
}
