//! The runtime's one-use bootstrap completion: a native fact a host consumes
//! once, inside its own close boundary, and that cannot be published after
//! the runtime began to close.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use meerkat_mob::MobHandle;

use super::UnifiedRuntime;

/// The runtime's bootstrap completed, minted once by this runtime on its
/// bootstrap success path and taken once with
/// [`UnifiedRuntime::take_bootstrap_completion`] (on the stdio gateway, from
/// its Ready: [`crate::rpc_gateway::GatewayBootstrapReady::take_runtime_completion`]).
///
/// It is a fact, not a payload: the host already has the bootstrap's own
/// result (each member's outcome) from the call that minted it. It holds
/// neither the runtime state nor a public owner of the mob, so an untaken
/// completion never keeps a dropped composition alive.
#[must_use = "publish it once, inside the host's close boundary"]
pub struct RuntimeBootstrapCompleted {
    gate: Arc<RuntimeCloseGate>,
    /// Non-owning: it takes an owner lease only while a publication runs.
    observer: MobHandle,
}

impl std::fmt::Debug for RuntimeBootstrapCompleted {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RuntimeBootstrapCompleted")
            .finish_non_exhaustive()
    }
}

impl RuntimeBootstrapCompleted {
    /// Run `f` once, only while the runtime has not begun to close and its
    /// mob still admits work, serialized against both closes.
    ///
    /// The runtime's close ([`UnifiedRuntime::shutdown`], a failed
    /// bootstrap's cleanup, or the retained teardown of a dropped
    /// composition) begins under the same gate. A publication that has begun
    /// finishes before that close can begin, and one that comes after it is
    /// refused with [`BootstrapCompletionRefused::RuntimeClosing`]; `f` does
    /// not run. While `f` runs, a public owner lease keeps the mob owned. It
    /// is taken under the same lock as every native close, so a mob whose
    /// admission already closed refuses with
    /// [`BootstrapCompletionRefused::AdmissionClosed`].
    ///
    /// The lease defeats only the mob's idle close (`MobHandle::close_if_idle`,
    /// and the close once its last public owner is gone). An explicit
    /// `MobHandle::close_admission` can still land while `f` runs, so work
    /// that `f` spawns must handle `MobError::AdmissionClosed` like any other
    /// caller of the mob.
    ///
    /// The host calls this inside its own close boundary, as it calls
    /// [`crate::rpc_gateway::GatewayBootstrapReady::publish`], and `f` cannot
    /// reenter host ownership either:
    /// - `f` is short and synchronous: a shutdown that begins meanwhile waits
    ///   for it, blocking its thread;
    /// - `f` does not block on async work (no `block_on`, no waiting for a
    ///   runtime-driven result): on a current-thread runtime, an `f` that runs
    ///   off-thread and waits on runtime work deadlocks against a shutdown
    ///   waiting at the gate;
    /// - `f` does not take the host's close boundary again;
    /// - `f` does not shut the runtime down, or wait on anything that waits
    ///   for its close: the gate is not reentrant.
    ///
    /// Typically `f` records the completion and spawns the host's first
    /// recovery pass, which then runs under the host's ordinary admission.
    /// If `f` panics, the panic propagates, and the runtime can still close.
    pub fn publish<R>(self, f: impl FnOnce() -> R) -> Result<R, BootstrapCompletionRefused> {
        let state = self.gate.lock();
        if *state == CloseState::Closing {
            return Err(BootstrapCompletionRefused::RuntimeClosing);
        }
        let lease = self
            .observer
            .acquire_owner()
            .map_err(|_| BootstrapCompletionRefused::AdmissionClosed)?;
        let value = f();
        drop(lease);
        drop(state);
        Ok(value)
    }
}

#[cfg(test)]
impl RuntimeBootstrapCompleted {
    /// A second completion over the same gate and mob. A runtime mints one;
    /// a test that spends it in a panicking publication needs another to
    /// observe the gate afterwards.
    pub(crate) fn sibling_for_test(&self) -> Self {
        Self {
            gate: Arc::clone(&self.gate),
            observer: self.observer.clone(),
        }
    }
}

/// Why [`RuntimeBootstrapCompleted::publish`] did not run its publication.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum BootstrapCompletionRefused {
    /// The runtime's close began (a shutdown, a failed bootstrap's cleanup,
    /// or the retained teardown of a dropped composition) before the
    /// publication.
    RuntimeClosing,
    /// The mob's admission closed: no public owner lease can be taken.
    AdmissionClosed,
}

impl std::fmt::Display for BootstrapCompletionRefused {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::RuntimeClosing => "the runtime began to close before the publication",
            Self::AdmissionClosed => "the mob's admission closed before the publication",
        })
    }
}

impl std::error::Error for BootstrapCompletionRefused {}

/// Why [`UnifiedRuntime::take_bootstrap_completion`] has no completion to
/// hand out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum BootstrapCompletionUnavailable {
    /// No bootstrap success path has returned: not yet, it failed, or an
    /// identity context installed since withdrew the completion.
    NotCompleted,
    /// Already taken. On the stdio gateway the driver takes it for its Ready.
    AlreadyTaken,
}

impl std::fmt::Display for BootstrapCompletionUnavailable {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::NotCompleted => "the runtime's bootstrap has not completed",
            Self::AlreadyTaken => "the runtime's bootstrap completion was already taken",
        })
    }
}

impl std::error::Error for BootstrapCompletionUnavailable {}

/// Serializes the start of a runtime's close against a running publication
/// of its [`RuntimeBootstrapCompleted`]. A std mutex: the publication is
/// synchronous, and a close only flips the state under it.
pub(crate) struct RuntimeCloseGate {
    state: Mutex<CloseState>,
    /// Told once, as the next close reaches the gate.
    #[cfg(test)]
    close_reached: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CloseState {
    Open,
    Closing,
}

impl RuntimeCloseGate {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(CloseState::Open),
            #[cfg(test)]
            close_reached: Mutex::new(None),
        })
    }

    fn lock(&self) -> MutexGuard<'_, CloseState> {
        // A publication whose `f` panicked poisons the lock; the state it
        // guards is still exact, so a later close or refusal reads through.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Begin the runtime's close: wait for a running publication to end, and
    /// refuse every later one.
    pub(crate) fn close(&self) {
        #[cfg(test)]
        if let Some(reached) = self
            .close_reached
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
        {
            let _ = reached.send(());
        }
        *self.lock() = CloseState::Closing;
    }

    /// Be told when the next close reaches the gate, before it waits there.
    #[cfg(test)]
    pub(crate) fn tell_close_reached(&self, reached: tokio::sync::oneshot::Sender<()>) {
        *self
            .close_reached
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(reached);
    }
}

/// A runtime's completion between its mint and its take.
pub(crate) enum CompletionSlot {
    /// No bootstrap success path has returned, or an identity context
    /// installed since withdrew the completion.
    Pending,
    Minted(Box<RuntimeBootstrapCompleted>),
    Taken,
}

impl UnifiedRuntime {
    /// Take this runtime's bootstrap completion: once per runtime, after a
    /// bootstrap success path returned.
    ///
    /// The success paths that mint it:
    /// - [`Self::install_and_bootstrap_identity_first_context`] and
    ///   [`Self::bootstrap_installed_identity_first_context`], on `Ok`;
    /// - [`Self::bootstrap_installed_identity_first_context_held_until`] on
    ///   `Activated` (so [`Self::bootstrap_installed_identity_first_context_held`]
    ///   on `Ok`);
    /// - [`Self::activate_without_identity_context`] on `Ok`, also when
    ///   nothing was pending. [`crate::UnifiedRuntimeBuilder::build`] passes
    ///   through one of these.
    ///
    /// On a runtime without an identity context the completion therefore
    /// exists only after `activate_without_identity_context` returned `Ok`.
    /// The legacy constructors ([`Self::bootstrap`] and the
    /// `bootstrap_with_*` family) return an active runtime without passing a
    /// mint site, so a host that built one calls it before taking.
    ///
    /// Installing an identity context ([`Self::install_identity_first_context`],
    /// which [`Self::install_and_bootstrap_identity_first_context`] calls)
    /// withdraws a completion not yet taken: the runtime's bootstrap is then
    /// its identity bootstrap, which mints again when it succeeds. Attaching
    /// one ([`Self::attach_identity_first_context`]) leaves the completion as
    /// it is and mints nothing, because this runtime does not bootstrap an
    /// attached roster. A failed bootstrap mints nothing; it closes the
    /// runtime, and a stray completion could not be published.
    ///
    /// On the stdio gateway the driver takes the completion for its Ready,
    /// and this answers [`BootstrapCompletionUnavailable::AlreadyTaken`].
    pub fn take_bootstrap_completion(
        &self,
    ) -> Result<RuntimeBootstrapCompleted, BootstrapCompletionUnavailable> {
        let mut slot = self
            .bootstrap_completion
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        match std::mem::replace(&mut *slot, CompletionSlot::Taken) {
            CompletionSlot::Minted(completion) => Ok(*completion),
            CompletionSlot::Taken => Err(BootstrapCompletionUnavailable::AlreadyTaken),
            CompletionSlot::Pending => {
                *slot = CompletionSlot::Pending;
                Err(BootstrapCompletionUnavailable::NotCompleted)
            }
        }
    }

    /// Mint the completion on a bootstrap success path. A second mint, or a
    /// mint after the take, changes nothing.
    pub(super) fn mint_bootstrap_completion(&self) {
        let mut slot = self
            .bootstrap_completion
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if matches!(*slot, CompletionSlot::Pending) {
            *slot = CompletionSlot::Minted(Box::new(RuntimeBootstrapCompleted {
                gate: Arc::clone(&self.close_gate),
                observer: self.mob_runtime.handle().observer(),
            }));
        }
    }

    /// Withdraw a completion not yet taken: an identity context was installed,
    /// so the bootstrap it attested is no longer the whole one.
    pub(super) fn withdraw_bootstrap_completion(&self) {
        let mut slot = self
            .bootstrap_completion
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if matches!(*slot, CompletionSlot::Minted(_)) {
            *slot = CompletionSlot::Pending;
        }
    }
}
