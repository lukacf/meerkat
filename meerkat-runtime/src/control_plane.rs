//! Concrete helpers for the runtime control-plane seam.
//!
//! Control-plane semantics are now part of `MeerkatMachine`, but the runtime
//! still coordinates out-of-band executor control through `RuntimeLoop` plus
//! the per-session driver. These helpers keep the concrete stop/preemption
//! behavior in one place.

use meerkat_core::lifecycle::run_control::RunControlCommand;

use crate::meerkat_machine::{SharedCompletionRegistry, SharedDriver, machine_stop_runtime};
use crate::tokio::sync::mpsc;
use crate::traits::RuntimeDriverError;

/// Canonical terminalization for an async stop after the executor has accepted
/// the stop control command.
pub(crate) async fn terminalize_async_stop(
    driver: &SharedDriver,
    completions: Option<&SharedCompletionRegistry>,
) -> Result<(), RuntimeDriverError> {
    {
        let mut driver = driver.lock().await;
        if !matches!(driver.runtime_state(), crate::RuntimeState::Destroyed) {
            machine_stop_runtime(&mut driver).await?;
        }
    }

    if let Some(completions) = completions {
        let mut reg = completions.lock().await;
        reg.resolve_all_terminated("runtime stopped");
    }

    Ok(())
}

/// Deliver one executor control command and report whether the runtime loop
/// should stop after applying it.
///
/// When a stop completes, routes all semantic stop terminalization through
/// [`terminalize_async_stop`] so DSL executor-exit, driver finalization, durable
/// stopped truth, and waiter resolution cannot split.
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

    if should_stop && let Err(err) = terminalize_async_stop(driver, completions).await {
        tracing::warn!(
            error = %err,
            "failed to terminalize runtime stop after stop-runtime-executor command"
        );
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
