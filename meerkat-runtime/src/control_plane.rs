//! Concrete helpers for the runtime control-plane seam.
//!
//! Control-plane semantics are now part of `MeerkatMachine`, but the runtime
//! still coordinates out-of-band executor control through `RuntimeLoop` plus
//! the per-session driver. These helpers keep the concrete stop/preemption
//! behavior in one place.

use crate::effect::{RuntimeEffect, RuntimeEffectInner};
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

/// Deliver one executor effect and report whether the runtime loop
/// should stop after applying it.
///
/// When a stop completes, routes all semantic stop terminalization through
/// [`terminalize_async_stop`] so DSL executor-exit, driver finalization, durable
/// stopped truth, and waiter resolution cannot split.
pub(crate) async fn apply_executor_effect(
    driver: &SharedDriver,
    completions: Option<&SharedCompletionRegistry>,
    executor: &mut dyn meerkat_core::lifecycle::CoreExecutor,
    effect: RuntimeEffect,
) -> bool {
    let (result, should_stop) = match effect.into_inner() {
        RuntimeEffectInner::CancelAfterBoundary { reason } => {
            (executor.cancel_after_boundary(reason).await, false)
        }
        RuntimeEffectInner::StopRuntimeExecutor { reason } => {
            (executor.stop_runtime_executor(reason).await, true)
        }
    };

    if let Err(err) = result {
        tracing::warn!(error = %err, "failed to apply runtime executor effect");
    }

    if should_stop && let Err(err) = terminalize_async_stop(driver, completions).await {
        tracing::warn!(
            error = %err,
            "failed to terminalize runtime stop after stop-runtime-executor effect"
        );
    }

    should_stop
}

/// Stop the concrete executor task after a runtime-loop boundary commit has
/// already rolled back. This is not a semantic `StopRuntimeExecutor` command:
/// the driver state remains the rollback state accepted by the machine.
pub(crate) async fn stop_executor_after_boundary_commit_rollback(
    executor: &mut dyn meerkat_core::lifecycle::CoreExecutor,
    reason: String,
) -> bool {
    if let Err(error) = executor.stop_runtime_executor(reason).await {
        tracing::warn!(
            error = %error,
            "failed to stop runtime loop executor after boundary commit rollback"
        );
    }
    true
}

/// Drain any ready executor effects before starting another unit of
/// ordinary queued work.
pub(crate) async fn drain_ready_executor_effects(
    driver: &SharedDriver,
    completions: Option<&SharedCompletionRegistry>,
    executor: &mut dyn meerkat_core::lifecycle::CoreExecutor,
    effect_rx: &mut mpsc::Receiver<RuntimeEffect>,
) -> bool {
    loop {
        match effect_rx.try_recv() {
            Ok(effect) => {
                if apply_executor_effect(driver, completions, executor, effect).await {
                    return true;
                }
            }
            Err(mpsc::error::TryRecvError::Empty) => return false,
            Err(mpsc::error::TryRecvError::Disconnected) => return true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::driver::ephemeral::EphemeralRuntimeDriver;
    use crate::effect::{RuntimeEffect, runtime_effect_for_test};
    use crate::identifiers::LogicalRuntimeId;
    use crate::meerkat_machine::driver::DriverEntry;
    use crate::meerkat_machine::dsl::RuntimeEffectKind;
    use meerkat_core::lifecycle::core_executor::{
        CoreApplyOutput, CoreExecutor, CoreExecutorError,
    };
    use meerkat_core::lifecycle::{RunId, run_primitive::RunPrimitive};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn shared_driver() -> SharedDriver {
        Arc::new(crate::tokio::sync::Mutex::new(DriverEntry::Ephemeral(
            EphemeralRuntimeDriver::new(LogicalRuntimeId::new("effect-test")),
        )))
    }

    fn runtime_effect(kind: RuntimeEffectKind, reason: &str) -> RuntimeEffect {
        runtime_effect_for_test(kind, reason)
    }

    struct RecordingExecutor {
        boundary_calls: Arc<AtomicUsize>,
        stop_calls: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl CoreExecutor for RecordingExecutor {
        async fn apply(
            &mut self,
            _run_id: RunId,
            _primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            unreachable!("effect tests do not apply primitives")
        }

        async fn cancel_after_boundary(
            &mut self,
            _reason: String,
        ) -> Result<(), CoreExecutorError> {
            self.boundary_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        async fn stop_runtime_executor(
            &mut self,
            _reason: String,
        ) -> Result<(), CoreExecutorError> {
            self.stop_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    #[tokio::test]
    async fn apply_executor_effect_dispatches_only_boundary_and_stop_effects() {
        let boundary_calls = Arc::new(AtomicUsize::new(0));
        let stop_calls = Arc::new(AtomicUsize::new(0));
        let mut executor = RecordingExecutor {
            boundary_calls: Arc::clone(&boundary_calls),
            stop_calls: Arc::clone(&stop_calls),
        };
        let driver = shared_driver();

        let should_stop = apply_executor_effect(
            &driver,
            None,
            &mut executor,
            runtime_effect(RuntimeEffectKind::CancelAfterBoundary, "boundary"),
        )
        .await;

        assert!(!should_stop);
        assert_eq!(boundary_calls.load(Ordering::SeqCst), 1);
        assert_eq!(stop_calls.load(Ordering::SeqCst), 0);

        let should_stop = apply_executor_effect(
            &driver,
            None,
            &mut executor,
            runtime_effect(RuntimeEffectKind::StopRuntimeExecutor, "stop"),
        )
        .await;

        assert!(should_stop);
        assert_eq!(boundary_calls.load(Ordering::SeqCst), 1);
        assert_eq!(stop_calls.load(Ordering::SeqCst), 1);
    }
}
