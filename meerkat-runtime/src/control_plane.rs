//! Concrete helpers for the current runtime control-plane seam.
//!
//! The long-term 0.5 architecture calls for a fuller `RuntimeControlMachine`,
//! but today's runtime still coordinates out-of-band executor control through
//! `RuntimeLoop` plus the per-session driver. These helpers keep the concrete
//! stop/preemption behavior in one place while the larger control-plane landing
//! is still in flight.

use meerkat_core::lifecycle::run_control::RunControlCommand;

use crate::runtime_state::RuntimeState;
use crate::session_adapter::{SharedCompletionRegistry, SharedDriver};
use crate::tokio::sync::mpsc;
use crate::traits::{RuntimeControlCommand, RuntimeDriverError};

async fn mark_runtime_stopped(driver: &SharedDriver) -> Result<(), RuntimeDriverError> {
    let mut driver = driver.lock().await;
    if matches!(
        driver.as_driver().runtime_state(),
        RuntimeState::Stopped | RuntimeState::Destroyed
    ) {
        return Ok(());
    }

    driver
        .as_driver_mut()
        .on_runtime_control(RuntimeControlCommand::Stop)
        .await
}

/// Deliver one executor control command and report whether the runtime loop
/// should stop after applying it.
pub(crate) async fn apply_executor_control(
    driver: &SharedDriver,
    completions: Option<&SharedCompletionRegistry>,
    executor: &mut dyn meerkat_core::lifecycle::CoreExecutor,
    command: RunControlCommand,
) -> bool {
    let should_stop = matches!(command, RunControlCommand::StopRuntimeExecutor { .. });

    if let Err(err) = executor.control(command).await {
        tracing::warn!(error = %err, "failed to deliver out-of-band executor control");
    }

    if should_stop && let Err(err) = mark_runtime_stopped(driver).await {
        tracing::warn!(
            error = %err,
            "failed to mark runtime stopped after stop-runtime-executor command"
        );
    }

    if should_stop && let Some(completions) = completions {
        let mut reg = completions.lock().await;
        reg.resolve_all_terminated("runtime stopped");
    }

    should_stop
}

/// Drain any ready executor control commands before starting another unit of
/// ordinary queued work.
pub(crate) async fn drain_ready_executor_controls(
    driver: &SharedDriver,
    completions: Option<&SharedCompletionRegistry>,
    executor: &mut dyn meerkat_core::lifecycle::CoreExecutor,
    control_rx: &mut mpsc::Receiver<RunControlCommand>,
) -> bool {
    loop {
        match control_rx.try_recv() {
            Ok(command) => {
                if apply_executor_control(driver, completions, executor, command).await {
                    return true;
                }
            }
            Err(mpsc::error::TryRecvError::Empty) => return false,
            Err(mpsc::error::TryRecvError::Disconnected) => return true,
        }
    }
}
