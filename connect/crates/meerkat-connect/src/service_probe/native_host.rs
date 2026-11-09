//! Mechanical custody for the actual native service invocation.
//!
//! This is not a parent authentication implementation. In particular it has no
//! constructor accepting forwarded login labels, no ConsoleHuman constructor,
//! and no public authentication capability mint. The protected parent contract must
//! supply that separate authority before an installed host can use this seam.
//! Existing trusted Rust selection still owns the original ServiceHost.
//! This bookkeeping does not stand in for the future original-parent
//! reservation and cannot repair a dropped runtime future or its cleanup.
//!
//! A native fence and a joined invocation are different observations. Only the
//! real stdio owner completes the latter, after its workers, close and writer.

use super::{ConnectError, ProbeLifetime, ServiceHost, ServiceProbeControl, unavailable};
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, Default)]
enum InvocationState {
    #[default]
    Unstarted,
    NeverEntered,
    Entered,
    Joined,
    Unconfirmed,
}

/// Local ownership bookkeeping, never a native input or effect outcome.
#[derive(Default)]
pub(super) struct InvocationCustody {
    state: Mutex<InvocationState>,
    changed: tokio::sync::Notify,
}

/// Exact-owner acknowledgement that the existing native admission fence ran.
/// This object is not serialized and does not attest cleanup or authentication.
/// It carries no original-parent reservation or cross-process acknowledgement.
/// Only an already-owned [`ServiceProbeControl`] can produce this receipt.
pub struct NativeHostFence {
    lifetime: Arc<ProbeLifetime>,
}

impl NativeHostFence {
    pub(super) fn establish(lifetime: Arc<ProbeLifetime>) -> Result<Self, ConnectError> {
        // Registering the actual invocation and fencing it have one ordering.
        // Neither the native caller policy nor publication checks take this
        // mutex; no callback runs under it. A never-entered host cannot later
        // register after its fence has been acknowledged.
        let mut state = match lifetime.invocation.state.lock() {
            Ok(state) => state,
            Err(_) => {
                // Custody failure cannot leave actual native admission open.
                // Retire best effort, but never acknowledge the poisoned owner.
                lifetime.retire();
                return Err(unavailable());
            }
        };
        let retired = lifetime.retire_checked();
        if matches!(*state, InvocationState::Unstarted) {
            *state = InvocationState::NeverEntered;
        }
        drop(state);
        lifetime.invocation.changed.notify_waiters();
        retired?;
        Ok(Self { lifetime })
    }

    /// Match the exact retained native host allocation, not equal text fields.
    pub fn belongs_to(&self, control: &ServiceProbeControl) -> bool {
        Arc::ptr_eq(&self.lifetime, &control.0)
    }

    /// Observe successful joins of the actual stdio invocation's work, close
    /// and output writer, after its reader is disposed. Continue driving that
    /// invocation; this method does not run or repair its cleanup.
    ///
    /// A never-entered invocation returns an error immediately. Failed, lost
    /// or poisoned custody also returns no successful observation; the actual
    /// invocation result retains its detailed error. This receipt never proves
    /// authentication, an original-parent reservation or a native effect result.
    pub async fn joined(&self) -> Result<NativeHostClosed, ConnectError> {
        loop {
            let changed = self.lifetime.invocation.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            let state = *self
                .lifetime
                .invocation
                .state
                .lock()
                .map_err(|_| unavailable())?;
            match state {
                InvocationState::Joined => {
                    return Ok(NativeHostClosed {
                        lifetime: self.lifetime.clone(),
                    });
                }
                InvocationState::Unconfirmed => return Err(unavailable()),
                InvocationState::Unstarted | InvocationState::NeverEntered => {
                    return Err(ConnectError::InvalidInput(
                        "The native service invocation never entered; no joined close was observed"
                            .into(),
                    ));
                }
                InvocationState::Entered => changed.await,
            }
        }
    }
}

/// Only the actual invocation guard can establish this local close observation.
/// It says nothing about a particular native effect or reusable service access.
/// It does not authenticate a parent, release an original-parent reservation,
/// or acknowledge a cross-process protocol. No public constructor is provided.
pub struct NativeHostClosed {
    lifetime: Arc<ProbeLifetime>,
}
impl NativeHostClosed {
    /// Match the exact retained native host allocation, not equal text fields.
    pub fn belongs_to(&self, control: &ServiceProbeControl) -> bool {
        Arc::ptr_eq(&self.lifetime, &control.0)
    }
}

/// Retain the same native host through its actual invocation owner.
/// A lost/panicking observer cannot manufacture a successful close observation.
pub(crate) struct NativeHostInvocation {
    host: Arc<ServiceHost>,
    finished: bool,
}

impl NativeHostInvocation {
    pub(crate) fn enter(host: Arc<ServiceHost>) -> Result<Self, ConnectError> {
        {
            let mut state = host
                .lifetime
                .invocation
                .state
                .lock()
                .map_err(|_| unavailable())?;
            if !matches!(*state, InvocationState::Unstarted)
                || host
                    .lifetime
                    .retired
                    .load(std::sync::atomic::Ordering::Acquire)
            {
                return Err(unavailable());
            }
            *state = InvocationState::Entered;
        }
        Ok(Self {
            host,
            finished: false,
        })
    }

    // Called only by run_io_composed after observing its actual owner joins.
    // Error ownership stays in the unchanged returned ConnectError; it is not
    // replaced by this local status or promoted merely because work drained.
    pub(crate) fn finish(
        mut self,
        work_drained: bool,
        writer_joined: bool,
        outcome: &Result<(), ConnectError>,
    ) {
        let confirmed = work_drained && writer_joined && outcome.is_ok();
        self.record(if confirmed {
            InvocationState::Joined
        } else {
            InvocationState::Unconfirmed
        });
        self.finished = true;
    }

    fn record(&self, outcome: InvocationState) {
        if let Ok(mut state) = self.host.lifetime.invocation.state.lock() {
            // No re-entry or repair can turn an unconfirmed invocation into a
            // joined one. The same owner is retained for its original attempt.
            if matches!(*state, InvocationState::Entered) {
                *state = outcome;
            }
        }
        self.host.lifetime.invocation.changed.notify_waiters();
    }
}

impl Drop for NativeHostInvocation {
    fn drop(&mut self) {
        if !self.finished {
            self.record(InvocationState::Unconfirmed);
            // This guard is dropped outside publication/login writer custody.
            // Retirement is best effort here; no fence acknowledgement escapes.
            self.host.lifetime.retire();
        }
    }
}

#[cfg(test)]
mod tests;
