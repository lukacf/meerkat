//! Process-local observer custody for one exact admitted operation.
//!
//! Generated runtime and domain authorities remain the only owners of
//! lifecycle and terminal truth. This module retains their typed receipt and
//! manages detachable observers. It never cancels an operation.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};

use meerkat_core::{
    OperationAdmissionReceipt, OperationAttributionError, OperationTerminal, OperationWaitError,
    TerminalReceipt,
};
use serde::Serialize;

use crate::{CompletionHandle, CompletionOutcome, CompletionWaitError};

type ObservedResult<D, T, E> = Result<TerminalReceipt<D, T>, OperationWaitError<D, E>>;

struct ExactOperationCustodyState<D, T, E> {
    admission: OperationAdmissionReceipt<D>,
    retained_terminal: Option<TerminalReceipt<D, T>>,
    last_observation_failure: Option<OperationWaitError<D, E>>,
    observers: HashMap<uuid::Uuid, crate::tokio::sync::oneshot::Sender<ObservedResult<D, T, E>>>,
    relay_abort: Option<crate::tokio::task::AbortHandle>,
}

struct ExactOperationCustodyInner<D, T, E> {
    state: Mutex<ExactOperationCustodyState<D, T, E>>,
}

impl<D, T, E> Drop for ExactOperationCustodyInner<D, T, E> {
    fn drop(&mut self) {
        let relay_abort = self
            .state
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .relay_abort
            .take();
        if let Some(relay_abort) = relay_abort {
            relay_abort.abort();
        }
    }
}

/// Owner-scoped custody for process observers of one exact operation.
///
/// A durable terminal receipt can seed a fresh custody after restart. Active
/// custody may instead be connected to an existing [`CompletionHandle`]. No
/// process-global registry or second lifecycle state machine is introduced.
pub struct ExactOperationCustody<D, T, E> {
    inner: Arc<ExactOperationCustodyInner<D, T, E>>,
}

impl<D, T, E> Clone for ExactOperationCustody<D, T, E> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl<D, T, E> ExactOperationCustody<D, T, E>
where
    D: Clone + PartialEq,
    T: Clone + Serialize,
    E: Clone,
{
    pub fn active(admission: OperationAdmissionReceipt<D>) -> Self {
        Self {
            inner: Arc::new(ExactOperationCustodyInner {
                state: Mutex::new(ExactOperationCustodyState {
                    admission,
                    retained_terminal: None,
                    last_observation_failure: None,
                    observers: HashMap::new(),
                    relay_abort: None,
                }),
            }),
        }
    }

    /// Rehydrate process observation from a caller-loaded durable receipt.
    pub fn from_terminal_receipt(receipt: TerminalReceipt<D, T>) -> Self {
        let admission = receipt.admission().clone();
        Self {
            inner: Arc::new(ExactOperationCustodyInner {
                state: Mutex::new(ExactOperationCustodyState {
                    admission,
                    retained_terminal: Some(receipt),
                    last_observation_failure: None,
                    observers: HashMap::new(),
                    relay_abort: None,
                }),
            }),
        }
    }

    pub fn admission(&self) -> OperationAdmissionReceipt<D> {
        self.inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .admission
            .clone()
    }

    pub fn observe(&self) -> ExactOperationObserver<D, T, E> {
        let observer_id = meerkat_core::time_compat::new_uuid_v7();
        let (tx, rx) = crate::tokio::sync::oneshot::channel();
        let admission;
        {
            let mut state = self
                .inner
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            admission = state.admission.clone();
            if let Some(retained) = state.retained_terminal.clone() {
                let _ = tx.send(Ok(retained));
            } else if let Some(failure) = state.last_observation_failure.clone() {
                let _ = tx.send(Err(failure));
            } else {
                state.observers.insert(observer_id, tx);
            }
        }
        ExactOperationObserver {
            admission,
            observer_id,
            state: Arc::downgrade(&self.inner),
            rx: Some(rx),
        }
    }

    /// Retain one exact terminal and wake current observers.
    pub fn resolve_terminal(
        &self,
        terminal: OperationTerminal<D, T>,
    ) -> Result<TerminalReceipt<D, T>, ExactOperationResolveError> {
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let receipt = TerminalReceipt::try_from_terminal(state.admission.clone(), terminal)?;
        match state.retained_terminal.as_ref() {
            Some(retained) if retained.terminal_digest() == receipt.terminal_digest() => {
                return Ok(retained.clone());
            }
            Some(_) => return Err(ExactOperationResolveError::TerminalConflict),
            None => {
                state.retained_terminal = Some(receipt.clone());
                state.last_observation_failure = None;
                for (_, observer) in std::mem::take(&mut state.observers) {
                    let _ = observer.send(Ok(receipt.clone()));
                }
            }
        }
        Ok(receipt)
    }

    /// Retain mechanical observer failure without losing admission identity.
    pub fn fail_observation(&self, error: E) -> OperationWaitError<D, E> {
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let failure = OperationWaitError::new(state.admission.clone(), error);
        if state.retained_terminal.is_none() {
            state.last_observation_failure = Some(failure.clone());
            for (_, observer) in std::mem::take(&mut state.observers) {
                let _ = observer.send(Err(failure.clone()));
            }
        }
        failure
    }

    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn observer_count_for_test(&self) -> usize {
        self.inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .observers
            .len()
    }
}

impl<D, T> ExactOperationCustody<D, T, CompletionWaitError>
where
    D: Clone + PartialEq + Send + 'static,
    T: Clone + Serialize + Send + 'static,
{
    /// Connect existing generated-authority completion plumbing to this exact
    /// custody. The adapter maps the domain-specific typed terminal.
    pub fn from_completion_handle<F>(
        admission: OperationAdmissionReceipt<D>,
        handle: CompletionHandle,
        map_terminal: F,
    ) -> Self
    where
        F: FnOnce(CompletionOutcome) -> OperationTerminal<D, T> + Send + 'static,
    {
        let custody = Self::active(admission);
        let relay = Arc::downgrade(&custody.inner);
        let relay_task = crate::tokio::spawn(async move {
            match handle.try_wait().await {
                Ok(outcome) => {
                    let Some(inner) = relay.upgrade() else {
                        return;
                    };
                    let relay = ExactOperationCustody { inner };
                    if let Err(error) = relay.resolve_terminal(map_terminal(outcome)) {
                        relay.fail_observation(CompletionWaitError::AuthorityUnavailable(
                            error.to_string(),
                        ));
                    }
                }
                Err(error) => {
                    let Some(inner) = relay.upgrade() else {
                        return;
                    };
                    let relay = ExactOperationCustody { inner };
                    relay.fail_observation(error);
                }
            }
        });
        custody
            .inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .relay_abort = Some(relay_task.abort_handle());
        custody
    }
}

/// RAII observer for one exact operation.
///
/// Drop removes only this process sender. It cannot cancel or terminalize the
/// generated operation authority.
pub struct ExactOperationObserver<D, T, E> {
    admission: OperationAdmissionReceipt<D>,
    observer_id: uuid::Uuid,
    state: Weak<ExactOperationCustodyInner<D, T, E>>,
    rx: Option<crate::tokio::sync::oneshot::Receiver<ObservedResult<D, T, E>>>,
}

impl<D, T, E> ExactOperationObserver<D, T, E> {
    pub fn admission(&self) -> &OperationAdmissionReceipt<D> {
        &self.admission
    }

    pub async fn wait(mut self) -> ObservedResult<D, T, E>
    where
        D: Clone,
        E: From<ExactOperationObserverChannelClosed>,
    {
        let Some(rx) = self.rx.take() else {
            return Err(OperationWaitError::new(
                self.admission.clone(),
                E::from(ExactOperationObserverChannelClosed),
            ));
        };
        match rx.await {
            Ok(result) => result,
            Err(_) => Err(OperationWaitError::new(
                self.admission.clone(),
                E::from(ExactOperationObserverChannelClosed),
            )),
        }
    }
}

impl<D, T, E> Drop for ExactOperationObserver<D, T, E> {
    fn drop(&mut self) {
        let Some(state) = self.state.upgrade() else {
            return;
        };
        state
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .observers
            .remove(&self.observer_id);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("exact operation observer channel closed without a retained receipt")]
pub struct ExactOperationObserverChannelClosed;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ExactOperationResolveError {
    #[error(transparent)]
    Attribution(#[from] OperationAttributionError),
    #[error("exact operation already retained a different terminal or wait result")]
    TerminalConflict,
}

impl From<ExactOperationObserverChannelClosed> for CompletionWaitError {
    fn from(_: ExactOperationObserverChannelClosed) -> Self {
        Self::ChannelClosed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use meerkat_core::{
        ExactOperationIdentity, InputId, OperationAcceptClass, OperationId,
        OperationTerminalIdentity, RuntimeEpochId, SessionId,
    };

    fn admission(
        submitted: InputId,
        canonical: InputId,
        accept_class: OperationAcceptClass,
    ) -> OperationAdmissionReceipt<u64> {
        OperationAdmissionReceipt::new(
            ExactOperationIdentity::for_runtime_input(
                OperationId::new(),
                SessionId::new(),
                RuntimeEpochId::new(),
                submitted,
                canonical,
                17,
            ),
            accept_class,
            None,
        )
    }

    fn terminal(
        admission: &OperationAdmissionReceipt<u64>,
        value: &str,
    ) -> OperationTerminal<u64, String> {
        OperationTerminal::new(
            OperationTerminalIdentity::from(admission.identity()),
            value.to_string(),
        )
    }

    #[tokio::test]
    async fn completion_before_wait_is_retained_and_restart_rehydrates_same_receipt() {
        let canonical = InputId::new();
        let admitted = admission(canonical.clone(), canonical, OperationAcceptClass::Fresh);
        let custody = ExactOperationCustody::<_, _, CompletionWaitError>::active(admitted.clone());
        let receipt = custody
            .resolve_terminal(terminal(&admitted, "done"))
            .expect("exact completion");
        assert_eq!(custody.observe().wait().await.expect("late wait"), receipt);
        let recovered = ExactOperationCustody::<_, _, CompletionWaitError>::from_terminal_receipt(
            receipt.clone(),
        );
        assert_eq!(
            recovered.observe().wait().await.expect("rehydrated wait"),
            receipt
        );
    }

    #[tokio::test]
    async fn dropped_waiter_prunes_only_observer_and_operation_still_completes() {
        let canonical = InputId::new();
        let admitted = admission(canonical.clone(), canonical, OperationAcceptClass::Fresh);
        let custody = ExactOperationCustody::<_, _, CompletionWaitError>::active(admitted.clone());
        let dropped = custody.observe();
        assert_eq!(custody.observer_count_for_test(), 1);
        drop(dropped);
        assert_eq!(custody.observer_count_for_test(), 0);
        custody
            .resolve_terminal(terminal(&admitted, "continued"))
            .expect("dropping observer never cancels operation");
        assert_eq!(
            custody
                .observe()
                .wait()
                .await
                .expect("late observer")
                .terminal(),
            "continued"
        );
    }

    #[tokio::test]
    async fn channel_failure_preserves_deduplicated_admission() {
        let submitted = InputId::new();
        let canonical = InputId::new();
        let admission = admission(
            submitted.clone(),
            canonical.clone(),
            OperationAcceptClass::InFlightDuplicate,
        );
        let custody =
            ExactOperationCustody::<u64, String, CompletionWaitError>::active(admission.clone());
        let observer = custody.observe();
        custody.fail_observation(CompletionWaitError::ChannelClosed);
        let failure = observer.wait().await.expect_err("mechanical failure");
        assert_eq!(failure.admission(), &admission);
        match failure.admission().identity().execution_scope() {
            meerkat_core::OperationExecutionScope::RuntimeInput {
                submitted_input_id,
                canonical_input_id,
                ..
            } => {
                assert_eq!(submitted_input_id, &submitted);
                assert_eq!(canonical_input_id, &canonical);
            }
            meerkat_core::OperationExecutionScope::Domain => {
                panic!("runtime admission lost its runtime input coordinates")
            }
        }
        let subsequent = custody
            .observe()
            .wait()
            .await
            .expect_err("latest process failure is prompt for subsequent observers");
        assert_eq!(subsequent.admission(), &admission);
        custody
            .resolve_terminal(terminal(&admission, "recovered"))
            .expect("mechanical failure cannot poison later terminal truth");
        let receipt = custody
            .observe()
            .wait()
            .await
            .expect("late observer reads recovered terminal");
        assert_eq!(receipt.terminal(), "recovered");
        let rehydrated = ExactOperationCustody::<_, _, CompletionWaitError>::from_terminal_receipt(
            receipt.clone(),
        );
        assert_eq!(
            rehydrated.observe().wait().await.expect("restart receipt"),
            receipt
        );
    }

    #[tokio::test]
    async fn another_operation_cannot_resolve_observers() {
        let canonical = InputId::new();
        let admitted = admission(canonical.clone(), canonical, OperationAcceptClass::Fresh);
        let custody = ExactOperationCustody::<_, _, CompletionWaitError>::active(admitted.clone());
        let observer = custody.observe();
        let other_canonical = InputId::new();
        let other = admission(
            other_canonical.clone(),
            other_canonical,
            OperationAcceptClass::Fresh,
        );
        assert_eq!(
            custody.resolve_terminal(terminal(&other, "wrong")),
            Err(ExactOperationResolveError::Attribution(
                OperationAttributionError::OperationMismatch
            ))
        );
        assert_eq!(custody.observer_count_for_test(), 1);
        custody
            .resolve_terminal(terminal(&admitted, "right"))
            .expect("exact operation resolves");
        assert_eq!(
            observer.wait().await.expect("observer result").terminal(),
            "right"
        );
    }

    #[tokio::test]
    async fn identical_terminal_replay_is_idempotent_but_conflict_is_rejected() {
        let canonical = InputId::new();
        let admission = admission(canonical.clone(), canonical, OperationAcceptClass::Fresh);
        let custody = ExactOperationCustody::<_, _, CompletionWaitError>::active(admission.clone());
        let first = custody
            .resolve_terminal(terminal(&admission, "first"))
            .expect("first terminal");
        let replay = custody
            .resolve_terminal(terminal(&admission, "first"))
            .expect("identical replay");
        assert_eq!(replay, first);
        assert_eq!(
            custody.resolve_terminal(terminal(&admission, "conflict")),
            Err(ExactOperationResolveError::TerminalConflict)
        );
        assert_eq!(
            custody.observe().wait().await.expect("retained terminal"),
            first
        );
    }

    #[tokio::test]
    async fn last_custody_owner_aborts_only_relay_plumbing() {
        let canonical = InputId::new();
        let admission = admission(canonical.clone(), canonical, OperationAcceptClass::Fresh);
        let mut pending_completion = CompletionHandle::pending_for_test();
        let completion_handle = pending_completion.take_handle();
        let custody =
            ExactOperationCustody::<u64, String, CompletionWaitError>::from_completion_handle(
                admission.clone(),
                completion_handle,
                move |_| terminal(&admission, "done"),
            );
        let clone = custody.clone();
        let relay_abort = custody
            .inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .relay_abort
            .as_ref()
            .expect("relay task owned")
            .clone();
        drop(custody);
        assert!(
            !relay_abort.is_finished(),
            "non-final clone keeps relay owned"
        );
        drop(clone);
        crate::tokio::task::yield_now().await;
        assert!(
            relay_abort.is_finished(),
            "last owner aborts detached observer relay"
        );
        assert!(pending_completion.sender_is_closed());
    }

    #[tokio::test]
    async fn concurrent_final_clone_drop_aborts_relay_exactly_once() {
        let canonical = InputId::new();
        let admission = admission(canonical.clone(), canonical, OperationAcceptClass::Fresh);
        let mut pending_completion = CompletionHandle::pending_for_test();
        let completion_handle = pending_completion.take_handle();
        let custody =
            ExactOperationCustody::<u64, String, CompletionWaitError>::from_completion_handle(
                admission.clone(),
                completion_handle,
                move |_| terminal(&admission, "done"),
            );
        let clone = custody.clone();
        let relay_abort = custody
            .inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .relay_abort
            .as_ref()
            .expect("relay task owned")
            .clone();
        let first = std::thread::spawn(move || drop(custody));
        let second = std::thread::spawn(move || drop(clone));
        first.join().expect("first drop thread");
        second.join().expect("second drop thread");
        crate::tokio::task::yield_now().await;
        assert!(relay_abort.is_finished());
        assert!(pending_completion.sender_is_closed());
    }

    #[tokio::test]
    async fn generated_completion_handle_maps_into_exact_retained_terminal() {
        let canonical = InputId::new();
        let admission = admission(canonical.clone(), canonical, OperationAcceptClass::Fresh);
        let handle = CompletionHandle::already_completed_without_result()
            .expect("generated completed-without-result handle");
        let terminal_admission = admission.clone();
        let custody =
            ExactOperationCustody::<u64, String, CompletionWaitError>::from_completion_handle(
                admission,
                handle,
                move |outcome| {
                    assert!(matches!(outcome, CompletionOutcome::CompletedWithoutResult));
                    terminal(&terminal_admission, "mapped")
                },
            );
        let receipt = custody
            .observe()
            .wait()
            .await
            .expect("generated handle relay");
        assert_eq!(receipt.terminal(), "mapped");
        assert_eq!(
            custody.observe().wait().await.expect("completion retained"),
            receipt
        );
    }

    #[tokio::test]
    async fn final_custody_drop_unregisters_exact_completion_registry_waiter() {
        let canonical = InputId::new();
        let admitted = admission(
            canonical.clone(),
            canonical.clone(),
            OperationAcceptClass::Fresh,
        );
        let mut registry = crate::completion::CompletionRegistry::new();
        let handle = registry.register(canonical);
        assert_eq!(registry.debug_waiter_count(), 1);
        let terminal_admission = admitted.clone();
        let custody =
            ExactOperationCustody::<u64, String, CompletionWaitError>::from_completion_handle(
                admitted,
                handle,
                move |_| terminal(&terminal_admission, "done"),
            );
        drop(custody);
        crate::tokio::task::yield_now().await;
        assert_eq!(registry.debug_waiter_count(), 0);
    }
}
