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
    let runtime_terminated_completion = if completions.is_some() {
        Some(
            crate::meerkat_machine::driver::machine_resolve_runtime_terminated_completion_result(
                driver,
            )
            .await?,
        )
    } else {
        None
    };

    {
        let mut driver = driver.lock().await;
        if !matches!(driver.runtime_state(), crate::RuntimeState::Destroyed) {
            machine_stop_runtime(&mut driver).await?;
        }
    }

    if let (Some(completions), Some(result_class)) = (completions, runtime_terminated_completion) {
        let mut reg = completions.lock().await;
        reg.resolve_all_runtime_terminated("runtime stopped", result_class);
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
) -> Result<bool, RuntimeDriverError> {
    match effect.into_inner() {
        RuntimeEffectInner::CancelAfterBoundary { reason } => {
            executor
                .cancel_after_boundary(reason)
                .await
                .map_err(|err| {
                    RuntimeDriverError::Internal(format!(
                        "failed to apply cancel-after-boundary executor effect: {err}"
                    ))
                })?;
            Ok(false)
        }
        RuntimeEffectInner::StopRuntimeExecutor { reason } => {
            executor
                .stop_runtime_executor(reason)
                .await
                .map_err(|err| {
                    RuntimeDriverError::Internal(format!(
                        "failed to apply stop-runtime-executor effect: {err}"
                    ))
                })?;
            terminalize_async_stop(driver, completions).await?;
            if let Err(err) = executor.cleanup_after_runtime_stop_terminalized().await {
                tracing::warn!(
                    error = %err,
                    "failed to clean up executor after durable runtime stop"
                );
            }
            Ok(true)
        }
    }
}

/// Typed outcome of draining ready executor effects.
///
/// Distinguishes the three terminal shapes a drain can produce so the loop
/// driver never conflates a closed effect channel with an applied stop:
/// channel closure must still route through the canonical stop/terminalize
/// path (StopRuntimeExecutor + [`terminalize_async_stop`] + waiter resolution)
/// instead of silently exiting as if a stop effect had already been applied.
#[derive(Debug)]
pub(crate) enum EffectDrainOutcome {
    /// A stop effect was received but NOT applied: stop realization awaits
    /// executor cleanup that may re-enter the machine (e.g. a mob executor
    /// unregistering its session), so it must never run under the session
    /// mutation gate the drain callers hold. The caller drops its authority
    /// guard first, then applies the returned effect.
    StopEffectPending(RuntimeEffect),
    /// No effects were pending; the channel remains open. The loop may
    /// continue processing queued work.
    Empty,
    /// The effect sender was dropped (control plane gone). The loop must
    /// terminalize through the stop path rather than exit bare.
    ChannelClosed,
}

/// Drain any ready executor effects before starting another unit of
/// ordinary queued work.
pub(crate) async fn drain_ready_executor_effects(
    driver: &SharedDriver,
    completions: Option<&SharedCompletionRegistry>,
    executor: &mut dyn meerkat_core::lifecycle::CoreExecutor,
    effect_rx: &mut mpsc::Receiver<RuntimeEffect>,
) -> Result<EffectDrainOutcome, RuntimeDriverError> {
    loop {
        match effect_rx.try_recv() {
            Ok(effect) => {
                if effect.is_stop() {
                    // Never applied here: the caller holds the session
                    // mutation gate during drains, and the stop path's
                    // cleanup can re-enter the machine (unregister), which
                    // acquires the same gate — a self-deadlock that parked
                    // the loop task forever and wedged whole mobs.
                    return Ok(EffectDrainOutcome::StopEffectPending(effect));
                }
                apply_executor_effect(driver, completions, executor, effect).await?;
            }
            Err(mpsc::error::TryRecvError::Empty) => return Ok(EffectDrainOutcome::Empty),
            Err(mpsc::error::TryRecvError::Disconnected) => {
                return Ok(EffectDrainOutcome::ChannelClosed);
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use meerkat_core::lifecycle::core_executor::{
        CoreApplyOutput, CoreExecutor, CoreExecutorError,
    };
    use meerkat_core::lifecycle::{RunId, run_primitive::RunPrimitive};

    /// Deterministically fails every `apply` while keeping the stop path
    /// healthy — models a poison payload whose run terminally fails on each
    /// stage attempt (the field shape behind the defer-wedge class tests).
    pub(crate) struct ApplyFailingExecutor {
        apply_calls: Arc<AtomicUsize>,
        stop_calls: Arc<AtomicUsize>,
    }

    impl ApplyFailingExecutor {
        pub(crate) fn new(apply_calls: Arc<AtomicUsize>, stop_calls: Arc<AtomicUsize>) -> Self {
            Self {
                apply_calls,
                stop_calls,
            }
        }
    }

    #[async_trait::async_trait]
    impl CoreExecutor for ApplyFailingExecutor {
        async fn apply(
            &mut self,
            _run_id: RunId,
            _primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            self.apply_calls.fetch_add(1, Ordering::SeqCst);
            Err(CoreExecutorError::apply_failed_runtime_turn(
                "synthetic deterministic apply failure",
            ))
        }

        async fn cancel_after_boundary(
            &mut self,
            _reason: String,
        ) -> Result<(), CoreExecutorError> {
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

    pub(crate) struct StopFailingExecutor {
        stop_calls: Arc<AtomicUsize>,
        apply_calls: Arc<AtomicUsize>,
    }

    impl StopFailingExecutor {
        pub(crate) fn new(stop_calls: Arc<AtomicUsize>, apply_calls: Arc<AtomicUsize>) -> Self {
            Self {
                stop_calls,
                apply_calls,
            }
        }
    }

    #[async_trait::async_trait]
    impl CoreExecutor for StopFailingExecutor {
        async fn apply(
            &mut self,
            _run_id: RunId,
            _primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            self.apply_calls.fetch_add(1, Ordering::SeqCst);
            Err(CoreExecutorError::apply_failed_runtime_turn(
                "stop failure regression must not apply queued work",
            ))
        }

        async fn cancel_after_boundary(
            &mut self,
            _reason: String,
        ) -> Result<(), CoreExecutorError> {
            Ok(())
        }

        async fn stop_runtime_executor(
            &mut self,
            _reason: String,
        ) -> Result<(), CoreExecutorError> {
            self.stop_calls.fetch_add(1, Ordering::SeqCst);
            Err(CoreExecutorError::control_failed_runtime(
                "synthetic stop effect failure",
            ))
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
        cleanup_calls: Arc<AtomicUsize>,
        fail_boundary: bool,
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
            if self.fail_boundary {
                return Err(CoreExecutorError::control_failed_runtime(
                    "synthetic boundary cancel failure",
                ));
            }
            Ok(())
        }

        async fn stop_runtime_executor(
            &mut self,
            _reason: String,
        ) -> Result<(), CoreExecutorError> {
            self.stop_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        async fn cleanup_after_runtime_stop_terminalized(
            &mut self,
        ) -> Result<(), CoreExecutorError> {
            self.cleanup_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    #[tokio::test]
    async fn apply_executor_effect_dispatches_only_boundary_and_stop_effects() {
        let boundary_calls = Arc::new(AtomicUsize::new(0));
        let stop_calls = Arc::new(AtomicUsize::new(0));
        let cleanup_calls = Arc::new(AtomicUsize::new(0));
        let mut executor = RecordingExecutor {
            boundary_calls: Arc::clone(&boundary_calls),
            stop_calls: Arc::clone(&stop_calls),
            cleanup_calls: Arc::clone(&cleanup_calls),
            fail_boundary: false,
        };
        let driver = shared_driver();

        let should_stop = apply_executor_effect(
            &driver,
            None,
            &mut executor,
            runtime_effect(RuntimeEffectKind::CancelAfterBoundary, "boundary"),
        )
        .await
        .expect("boundary effect should apply");

        assert!(!should_stop);
        assert_eq!(boundary_calls.load(Ordering::SeqCst), 1);
        assert_eq!(stop_calls.load(Ordering::SeqCst), 0);
        assert_eq!(cleanup_calls.load(Ordering::SeqCst), 0);

        let should_stop = apply_executor_effect(
            &driver,
            None,
            &mut executor,
            runtime_effect(RuntimeEffectKind::StopRuntimeExecutor, "stop"),
        )
        .await
        .expect("stop effect should apply");

        assert!(should_stop);
        assert_eq!(boundary_calls.load(Ordering::SeqCst), 1);
        assert_eq!(stop_calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            cleanup_calls.load(Ordering::SeqCst),
            1,
            "post-stop cleanup must run only after machine-owned terminalization succeeds"
        );
    }

    #[tokio::test]
    async fn apply_executor_effect_boundary_cancel_failure_is_fail_closed() {
        let boundary_calls = Arc::new(AtomicUsize::new(0));
        let stop_calls = Arc::new(AtomicUsize::new(0));
        let cleanup_calls = Arc::new(AtomicUsize::new(0));
        let mut executor = RecordingExecutor {
            boundary_calls: Arc::clone(&boundary_calls),
            stop_calls,
            cleanup_calls,
            fail_boundary: true,
        };
        let driver = shared_driver();

        let err = apply_executor_effect(
            &driver,
            None,
            &mut executor,
            runtime_effect(RuntimeEffectKind::CancelAfterBoundary, "boundary"),
        )
        .await
        .expect_err("boundary cancel effect failure must not be warn-only");

        assert!(
            err.to_string()
                .contains("failed to apply cancel-after-boundary executor effect"),
            "unexpected error: {err}"
        );
        assert_eq!(boundary_calls.load(Ordering::SeqCst), 1);
    }

    /// Regression for #287: a closed effect channel must surface as a distinct
    /// `ChannelClosed` outcome, NOT collapse into the same `AppliedStop` signal a
    /// real stop effect produces. Under the old `Result<bool, _>` shape both
    /// returned `Ok(true)` and the loop exited bare without terminalizing.
    #[tokio::test]
    async fn drain_ready_executor_effects_channel_closed_is_distinct_from_applied_stop() {
        let driver = shared_driver();

        // Stop-pending case: a stop effect is delivered, then the sender is
        // dropped. The drain must NOT apply the stop (its cleanup can
        // re-enter the machine and deadlock on the session mutation gate
        // the drain callers hold); it surfaces the effect for the caller to
        // apply after releasing its authority guard.
        {
            let stop_calls = Arc::new(AtomicUsize::new(0));
            let mut executor = RecordingExecutor {
                boundary_calls: Arc::new(AtomicUsize::new(0)),
                stop_calls: Arc::clone(&stop_calls),
                cleanup_calls: Arc::new(AtomicUsize::new(0)),
                fail_boundary: false,
            };
            let (tx, mut rx) = mpsc::channel::<RuntimeEffect>(4);
            tx.send(runtime_effect(
                RuntimeEffectKind::StopRuntimeExecutor,
                "stop",
            ))
            .await
            .expect("send stop effect");
            drop(tx);

            let outcome = drain_ready_executor_effects(&driver, None, &mut executor, &mut rx)
                .await
                .expect("drain with pending stop should succeed");
            let EffectDrainOutcome::StopEffectPending(effect) = outcome else {
                panic!("a stop effect must surface as StopEffectPending, got {outcome:?}");
            };
            assert_eq!(
                stop_calls.load(Ordering::SeqCst),
                0,
                "the drain must never apply the stop itself"
            );
            let should_stop = apply_executor_effect(&driver, None, &mut executor, effect)
                .await
                .expect("caller applies the pending stop guard-free");
            assert!(should_stop);
            assert_eq!(stop_calls.load(Ordering::SeqCst), 1);
        }

        // Closed-channel-only case: the sender is dropped without ever
        // delivering a stop effect. The drain must report `ChannelClosed`,
        // distinguishable from `AppliedStop`, and must NOT invoke the
        // executor stop.
        {
            let stop_calls = Arc::new(AtomicUsize::new(0));
            let mut executor = RecordingExecutor {
                boundary_calls: Arc::new(AtomicUsize::new(0)),
                stop_calls: Arc::clone(&stop_calls),
                cleanup_calls: Arc::new(AtomicUsize::new(0)),
                fail_boundary: false,
            };
            let (tx, mut rx) = mpsc::channel::<RuntimeEffect>(4);
            drop(tx);

            let outcome = drain_ready_executor_effects(&driver, None, &mut executor, &mut rx)
                .await
                .expect("drain on closed channel should succeed");
            assert!(
                matches!(outcome, EffectDrainOutcome::ChannelClosed),
                "a closed effect channel must be distinguishable from a pending stop: {outcome:?}"
            );
            assert_eq!(
                stop_calls.load(Ordering::SeqCst),
                0,
                "bare channel closure does not itself apply a stop effect"
            );
        }
    }
}
