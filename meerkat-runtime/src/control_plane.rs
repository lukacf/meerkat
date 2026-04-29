//! Concrete helpers for the runtime control-plane seam.
//!
//! Control-plane semantics are now part of `MeerkatMachine`, but the runtime
//! still coordinates out-of-band executor control through `RuntimeLoop` plus
//! the per-session driver. These helpers keep the concrete stop/preemption
//! behavior in one place.

use std::sync::Weak;

use meerkat_core::lifecycle::run_control::RunControlCommand;
use meerkat_core::types::SessionId;

use crate::meerkat_machine::{MeerkatMachine, SharedCompletionRegistry, SharedDriver};
use crate::tokio::sync::mpsc;
use crate::traits::RuntimeDriverError;

async fn finalize_runtime_stopped(driver: &SharedDriver) -> Result<(), RuntimeDriverError> {
    let mut driver = driver.lock().await;
    if matches!(
        driver.as_driver().runtime_state(),
        crate::RuntimeState::Destroyed
    ) {
        return Ok(());
    }

    driver.finalize_stop_runtime().await?;
    driver.sync_control_projection_from_dsl_authority();
    Ok(())
}

/// Deliver one executor control command and report whether the runtime loop
/// should stop after applying it.
///
/// When a stop completes, fires the DSL `RuntimeExecutorExited` transition on
/// the session's machine so the machine phase reflects the async realisation
/// of the stop (driver → Stopped). No shell→DSL read-through bridge.
pub(crate) async fn apply_executor_control(
    driver: &SharedDriver,
    completions: Option<&SharedCompletionRegistry>,
    executor: &mut dyn meerkat_core::lifecycle::CoreExecutor,
    command: RunControlCommand,
    machine: &Weak<MeerkatMachine>,
    session_id: &SessionId,
) -> bool {
    let should_stop = matches!(command, RunControlCommand::StopRuntimeExecutor { .. });

    if let Err(err) = executor.control(command).await {
        tracing::warn!(error = %err, "failed to deliver out-of-band executor control");
    }

    if should_stop && let Some(machine) = machine.upgrade() {
        machine.notify_runtime_executor_exited(session_id).await;
    }

    if should_stop && let Err(err) = finalize_runtime_stopped(driver).await {
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
    machine: &Weak<MeerkatMachine>,
    session_id: &SessionId,
) -> bool {
    loop {
        match control_rx.try_recv() {
            Ok(command) => {
                if apply_executor_control(
                    driver,
                    completions,
                    executor,
                    command,
                    machine,
                    session_id,
                )
                .await
                {
                    return true;
                }
            }
            Err(mpsc::error::TryRecvError::Empty) => return false,
            Err(mpsc::error::TryRecvError::Disconnected) => return true,
        }
    }
}
