//! Lifetime custody of one actual native reviewed entry, never authority.
//!
//! Clones retain the same native effect. They neither admit another operation
//! nor replace its current owner checks. Keep them in the owners of actual I/O
//! and remove recovery holds only when that existing recovery owner settles.
use std::sync::Arc;

#[derive(Clone, Default)]
pub(crate) struct ReviewCustody {
    _entry: Option<Arc<dyn Send + Sync>>,
}

impl ReviewCustody {
    // Only the native host's controls hold a native reviewed entry.
    #[cfg(all(test, feature = "native-host", unix))]
    pub(crate) fn entered(entry: meerkat_core::approval::review::ReviewedEntryCustody) -> Self {
        Self {
            _entry: Some(Arc::new(entry)),
        }
    }

    /// A lifetime witness in place of a native reviewed entry, for controls
    /// that count the holds their custody keeps of it.
    #[cfg(all(test, feature = "native-host", unix))]
    pub(crate) fn witness_for_test(entry: Arc<dyn Send + Sync>) -> Self {
        Self {
            _entry: Some(entry),
        }
    }

    /// Retain a clone for lifetime only. The typed capability is passed
    /// separately to the exact operation's native steps and is never exposed here.
    pub(crate) fn entered_operation(entry: &meerkat_core::EnteredDispatchOperation) -> Self {
        Self {
            _entry: Some(Arc::new(entry.clone())),
        }
    }

    // Box before constructing the async wrapper. Boxing inside an async fn
    // would still embed the large operation in that function's initial state
    // and multiply its debug stack cost through the store and timeout scopes.
    #[inline(never)]
    pub(crate) fn retain_io<'a, F: std::future::Future + 'a>(
        &'a self,
        future: F,
    ) -> impl std::future::Future<Output = F::Output> + 'a {
        let future = Box::pin(future);
        async move {
            // Establish custody at first poll, as before. No task is spawned:
            // dropping this wrapper still synchronously drops the operation.
            match &self._entry {
                Some(entry) => toolkit_store::retain_io_custody(entry.clone(), future).await,
                None => future.await,
            }
        }
    }

    #[cfg(all(feature = "native-host", unix))]
    pub(crate) fn bind(
        &self,
        inner: Arc<dyn meerkat_mobkit::identity_first::GovernedInputBinder>,
    ) -> Arc<dyn meerkat_mobkit::identity_first::GovernedInputBinder> {
        Arc::new(HeldBinder {
            inner,
            _review: self.clone(),
        })
    }
}

// Meta owns this binder through actual target materialization and the native
// admission/settlement callbacks. Retained source evidence owns a separately
// removable recovery hold; a settlement wrapper would miss its inner transfers.
#[cfg(all(feature = "native-host", unix))]
struct HeldBinder {
    // Drop the actual binder (including its foreground-finished signal) before
    // our final native review hold. Do not retain the binder from a settlement.
    inner: Arc<dyn meerkat_mobkit::identity_first::GovernedInputBinder>,
    _review: ReviewCustody,
}

#[cfg(all(feature = "native-host", unix))]
#[async_trait::async_trait]
impl meerkat_mobkit::identity_first::GovernedInputBinder for HeldBinder {
    async fn bind(
        &self,
        context: &meerkat_mobkit::identity_first::GovernedBindContext,
        input: meerkat_runtime::Input,
    ) -> Result<meerkat_mobkit::identity_first::BoundGovernedInput, String> {
        self._review
            .retain_io(self.inner.bind(context, input))
            .await
    }

    fn retain_unresolved(
        &self,
        settlement: Box<dyn meerkat_mobkit::identity_first::GovernedSettlement>,
        detail: &str,
    ) {
        self.inner.retain_unresolved(settlement, detail);
    }
}

#[cfg(test)]
mod tests {
    use super::ReviewCustody;
    use std::{
        cell::Cell,
        future::Future,
        marker::PhantomPinned,
        pin::Pin,
        sync::Arc,
        task::{Context, Poll, Waker},
    };

    // The inline payload must survive a pending poll. Borrowed Cells also
    // ensure the wrapper does not impose Send or 'static on its operation.
    struct LargeBorrowedOperation<'a> {
        payload: [u8; 64 * 1024],
        ready: &'a Cell<bool>,
        polls: &'a Cell<usize>,
        drops: &'a Cell<usize>,
        _pinned: PhantomPinned,
    }

    impl Future for LargeBorrowedOperation<'_> {
        type Output = u8;

        fn poll(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Self::Output> {
            self.polls.set(self.polls.get() + 1);
            if self.ready.get() {
                Poll::Ready(self.payload[0] + self.payload[self.payload.len() - 1])
            } else {
                Poll::Pending
            }
        }
    }

    impl Drop for LargeBorrowedOperation<'_> {
        fn drop(&mut self) {
            self.drops.set(self.drops.get() + 1);
        }
    }

    #[test]
    fn large_borrowed_operation_keeps_custody_future_small_and_preserves_completion() {
        for entered in [false, true] {
            let entry = Arc::new(());
            // This probe supplies lifetime custody only, never native authority.
            let review = ReviewCustody {
                _entry: entered.then(|| entry.clone() as Arc<dyn Send + Sync>),
            };
            let (ready, polls, drops) = (Cell::new(false), Cell::new(0), Cell::new(0));
            let operation = LargeBorrowedOperation {
                payload: [7; 64 * 1024],
                ready: &ready,
                polls: &polls,
                drops: &drops,
                _pinned: PhantomPinned,
            };
            let wrapped = review.retain_io(operation);
            assert!(
                std::mem::size_of_val(&wrapped) <= 256,
                "custody wrappers must not embed the operation's large inline state"
            );
            assert_eq!(polls.get(), 0, "construction must not poll the operation");
            let baseline = if entered { 2 } else { 1 };
            assert_eq!(
                Arc::strong_count(&entry),
                baseline,
                "construction must not acquire an additional custody hold"
            );
            let mut wrapped = Box::pin(wrapped);
            let mut context = Context::from_waker(Waker::noop());
            assert_eq!(wrapped.as_mut().poll(&mut context), Poll::Pending);
            assert_eq!(polls.get(), 1);
            assert_eq!(drops.get(), 0);
            assert_eq!(Arc::strong_count(&entry), baseline + usize::from(entered));
            ready.set(true);
            assert_eq!(wrapped.as_mut().poll(&mut context), Poll::Ready(14));
            drop(wrapped);
            assert_eq!(polls.get(), 2);
            assert_eq!(drops.get(), 1);
            assert_eq!(Arc::strong_count(&entry), baseline);
        }
    }

    #[test]
    fn custody_cancellation_drops_the_operation_and_releases_only_its_scope() {
        for entered in [false, true] {
            for poll_first in [false, true] {
                let entry = Arc::new(());
                let review = ReviewCustody {
                    _entry: entered.then(|| entry.clone() as Arc<dyn Send + Sync>),
                };
                let (ready, polls, drops) = (Cell::new(false), Cell::new(0), Cell::new(0));
                let operation = LargeBorrowedOperation {
                    payload: [3; 64 * 1024],
                    ready: &ready,
                    polls: &polls,
                    drops: &drops,
                    _pinned: PhantomPinned,
                };
                let mut wrapped = Box::pin(review.retain_io(operation));
                let baseline = if entered { 2 } else { 1 };
                assert_eq!(Arc::strong_count(&entry), baseline);
                if poll_first {
                    let mut context = Context::from_waker(Waker::noop());
                    assert_eq!(wrapped.as_mut().poll(&mut context), Poll::Pending);
                    assert_eq!(Arc::strong_count(&entry), baseline + usize::from(entered));
                }
                drop(wrapped);
                assert_eq!(polls.get(), usize::from(poll_first));
                assert_eq!(
                    drops.get(),
                    1,
                    "cancellation must drop the exact operation synchronously"
                );
                assert_eq!(
                    Arc::strong_count(&entry),
                    baseline,
                    "the caller's existing custody must survive cancellation"
                );
                drop(review);
                assert_eq!(Arc::strong_count(&entry), 1);
            }
        }
    }
}
