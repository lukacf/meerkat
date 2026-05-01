//! Runtime-owned surface request lifecycle authority.
//!
//! The request executor in surface crates owns transport mechanics (task
//! handles and cleanup/cancel closures). This handle owns the semantic state
//! that classifies terminal responses and arbitrates publish/cancel/complete
//! transitions.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use meerkat_core::handles::{
    CancelActionInstallDecision, CancelOutcome, CancelTransition, CompleteOutcome,
    CompleteTransition, RequestAlreadyExists, RequestTransitionError,
    SurfaceRequestLifecycleHandle, SurfaceRequestPhase, SurfaceRequestTerminalDisposition,
};

fn lock_or_recover<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SurfaceRequestTerminalPolicy {
    InlineObservation,
    CancellableObservation,
    PublishOnSuccess,
}

impl SurfaceRequestTerminalPolicy {
    fn cancellation_enabled(self) -> bool {
        matches!(self, Self::CancellableObservation | Self::PublishOnSuccess)
    }
}

#[derive(Debug, Clone, Copy)]
struct RequestLifecycleEntry {
    phase: SurfaceRequestPhase,
    terminal_policy: SurfaceRequestTerminalPolicy,
}

impl RequestLifecycleEntry {
    fn new() -> Self {
        Self {
            phase: SurfaceRequestPhase::Pending,
            terminal_policy: SurfaceRequestTerminalPolicy::InlineObservation,
        }
    }
}

/// Runtime-backed [`SurfaceRequestLifecycleHandle`] implementation.
#[derive(Debug, Clone, Default)]
pub(crate) struct RuntimeSurfaceRequestLifecycleHandle {
    entries: Arc<Mutex<HashMap<String, RequestLifecycleEntry>>>,
}

impl RuntimeSurfaceRequestLifecycleHandle {
    pub(crate) fn new() -> Self {
        Self::default()
    }
}

impl SurfaceRequestLifecycleHandle for RuntimeSurfaceRequestLifecycleHandle {
    fn try_begin_request(&self, key: String) -> Result<(), RequestAlreadyExists> {
        let mut entries = lock_or_recover(&self.entries);
        if entries.contains_key(&key) {
            return Err(RequestAlreadyExists);
        }
        entries.insert(key, RequestLifecycleEntry::new());
        Ok(())
    }

    fn authorize_publish_on_success(&self, key: &str) -> Result<(), RequestTransitionError> {
        let mut entries = lock_or_recover(&self.entries);
        let entry = entries
            .get_mut(key)
            .ok_or(RequestTransitionError::NotFound)?;
        match entry.phase {
            SurfaceRequestPhase::Pending | SurfaceRequestPhase::Cancelled => {
                entry.terminal_policy = SurfaceRequestTerminalPolicy::PublishOnSuccess;
                Ok(())
            }
            current => Err(RequestTransitionError::AlreadyTerminal { current }),
        }
    }

    fn authorize_cancellable_observation(&self, key: &str) -> Result<(), RequestTransitionError> {
        let mut entries = lock_or_recover(&self.entries);
        let entry = entries
            .get_mut(key)
            .ok_or(RequestTransitionError::NotFound)?;
        match entry.phase {
            SurfaceRequestPhase::Pending | SurfaceRequestPhase::Cancelled => {
                if !matches!(
                    entry.terminal_policy,
                    SurfaceRequestTerminalPolicy::PublishOnSuccess
                ) {
                    entry.terminal_policy = SurfaceRequestTerminalPolicy::CancellableObservation;
                }
                Ok(())
            }
            current => Err(RequestTransitionError::AlreadyTerminal { current }),
        }
    }

    fn classify_terminal(&self, key: &str, committed: bool) -> SurfaceRequestTerminalDisposition {
        let policy = lock_or_recover(&self.entries)
            .get(key)
            .map(|entry| entry.terminal_policy)
            .unwrap_or(SurfaceRequestTerminalPolicy::InlineObservation);
        match policy {
            SurfaceRequestTerminalPolicy::PublishOnSuccess if committed => {
                SurfaceRequestTerminalDisposition::Publish
            }
            SurfaceRequestTerminalPolicy::PublishOnSuccess
            | SurfaceRequestTerminalPolicy::CancellableObservation => {
                SurfaceRequestTerminalDisposition::RespondWithoutPublish
            }
            SurfaceRequestTerminalPolicy::InlineObservation => {
                SurfaceRequestTerminalDisposition::Inline
            }
        }
    }

    fn phase(&self, key: &str) -> Option<SurfaceRequestPhase> {
        lock_or_recover(&self.entries)
            .get(key)
            .map(|entry| entry.phase)
    }

    fn cancel_action_install_decision(&self, key: &str) -> CancelActionInstallDecision {
        let entries = lock_or_recover(&self.entries);
        let Some(entry) = entries.get(key) else {
            return CancelActionInstallDecision {
                phase: None,
                fire_cancel_action: false,
            };
        };
        CancelActionInstallDecision {
            phase: Some(entry.phase),
            fire_cancel_action: matches!(entry.phase, SurfaceRequestPhase::Cancelled)
                && entry.terminal_policy.cancellation_enabled(),
        }
    }

    fn cancel_request(&self, key: &str) -> CancelTransition {
        let mut entries = lock_or_recover(&self.entries);
        let Some(entry) = entries.get_mut(key) else {
            return CancelTransition {
                outcome: CancelOutcome::NotFound,
                fire_cancel_action: false,
            };
        };
        match entry.phase {
            SurfaceRequestPhase::Pending => {
                entry.phase = SurfaceRequestPhase::Cancelled;
                CancelTransition {
                    outcome: CancelOutcome::Cancelled,
                    fire_cancel_action: entry.terminal_policy.cancellation_enabled(),
                }
            }
            SurfaceRequestPhase::Published => CancelTransition {
                outcome: CancelOutcome::AlreadyPublished,
                fire_cancel_action: false,
            },
            SurfaceRequestPhase::Cancelled => CancelTransition {
                outcome: CancelOutcome::AlreadyCancelled,
                fire_cancel_action: false,
            },
            SurfaceRequestPhase::Completed => CancelTransition {
                outcome: CancelOutcome::AlreadyCompleted,
                fire_cancel_action: false,
            },
        }
    }

    fn publish_and_complete(&self, key: &str) -> Result<(), RequestTransitionError> {
        let mut entries = lock_or_recover(&self.entries);
        let Some(entry) = entries.get_mut(key) else {
            return Err(RequestTransitionError::NotFound);
        };
        match entry.phase {
            SurfaceRequestPhase::Pending => {
                entry.phase = SurfaceRequestPhase::Published;
                entries.remove(key);
                Ok(())
            }
            current => Err(RequestTransitionError::AlreadyTerminal { current }),
        }
    }

    fn finish_unpublished(&self, key: &str) -> CompleteTransition {
        let mut entries = lock_or_recover(&self.entries);
        let Some(entry) = entries.get_mut(key) else {
            return CompleteTransition {
                outcome: CompleteOutcome::Completed,
                run_unpublished_cleanup: false,
            };
        };
        match entry.phase {
            SurfaceRequestPhase::Pending => {
                entry.phase = SurfaceRequestPhase::Completed;
                entries.remove(key);
                CompleteTransition {
                    outcome: CompleteOutcome::Completed,
                    run_unpublished_cleanup: true,
                }
            }
            SurfaceRequestPhase::Cancelled => {
                entries.remove(key);
                CompleteTransition {
                    outcome: CompleteOutcome::SupersededByCancel,
                    run_unpublished_cleanup: true,
                }
            }
            SurfaceRequestPhase::Published | SurfaceRequestPhase::Completed => {
                entries.remove(key);
                CompleteTransition {
                    outcome: CompleteOutcome::Completed,
                    run_unpublished_cleanup: false,
                }
            }
        }
    }

    fn remove(&self, key: &str) {
        lock_or_recover(&self.entries).remove(key);
    }
}
