//! Protected sections: where code must not wait.
//!
//! A caller that holds a lock other actors need (an identity's lifecycle
//! lock, a session's mutation gate and driver lock, a turn boundary, a
//! store transaction) and calls into a host marks that call as a protected
//! section. Code that runs inside one never blocks, sleeps, or waits for a
//! lock another actor may hold across I/O: whatever it cannot answer now it
//! answers as "not now", and the owner of the section waits, if anyone does,
//! after it has released what it holds.
//!
//! This module is the witness of that rule, not its enforcement. A section
//! is entered with [`enter`] around a synchronous call or [`within`] around
//! an asynchronous one; code about to take a lock or sleep in a way that can
//! wait asks [`assert_may_wait`], which in a debug build panics inside a
//! section, naming the site and the section. A release build checks
//! nothing.
//!
//! An asynchronous section also carries the attempt's effect mark: the
//! first durable effect (a source record, a custody attempt, a settled
//! reservation) that code inside it recorded with [`note_effect`]. A nested
//! section shares the outermost one's mark, and nothing clears it, so a
//! binder that runs after another's write sees that write. "Not now" is an
//! answer only before an attempt's first durable effect: a binder reads the
//! mark with [`effect_recorded`], and the section's owner gets it back from
//! [`within_marked`].

use std::cell::{Cell, RefCell};
use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;
use std::sync::{Arc, OnceLock};
use std::task::{Context, Poll};

/// Where the current code runs while its caller holds what other actors
/// need.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ProtectedSection {
    /// A host binder's decision for one governed delivery: under the
    /// identity's lifecycle lock (a governed send), or the decision a peer
    /// delivery's binder makes inside its bind.
    GovernedBind,
    /// A host callback made under the identity's lifecycle lock after a
    /// bind: the bound input's entry fence and its permit, and the
    /// settlement custody.
    GovernedEntry,
    /// A host's reads of an original's custody, disposition or enrollment
    /// for native admission and operation authorization, under the session's
    /// mutation gate and driver lock.
    NativeAdmissionRead,
    /// The historical admission callback, under the actor's turn boundary,
    /// the registration's mutation gate and the driver lock.
    HistoricalAdmission,
    /// A host's own store transaction.
    HostStoreTransaction,
}

/// The asynchronous section a future runs in, with its attempt's effect
/// mark (shared by every section nested inside the outermost one).
#[derive(Clone)]
struct TaskScope {
    section: ProtectedSection,
    effect: Arc<OnceLock<&'static str>>,
}

thread_local! {
    /// The synchronous section the current thread runs in.
    static SYNC_SECTION: Cell<Option<ProtectedSection>> = const { Cell::new(None) };
    /// The asynchronous section of the future this thread is polling.
    static TASK_SCOPE: RefCell<Option<TaskScope>> = const { RefCell::new(None) };
    /// A wait its owner lists as known, running on this thread.
    static KNOWN_WAIT: Cell<Option<&'static str>> = const { Cell::new(None) };
}

/// A synchronous section, entered by [`enter`] and left when dropped. It
/// stays on the thread that entered it.
#[must_use = "the section ends when the guard is dropped"]
pub struct SectionGuard {
    previous: Option<ProtectedSection>,
    _thread_bound: PhantomData<*const ()>,
}

impl Drop for SectionGuard {
    fn drop(&mut self) {
        SYNC_SECTION.with(|section| section.set(self.previous));
    }
}

/// Enter `section` for the synchronous code that runs until the returned
/// guard is dropped.
pub fn enter(section: ProtectedSection) -> SectionGuard {
    let previous = SYNC_SECTION.with(|current| current.replace(Some(section)));
    SectionGuard {
        previous,
        _thread_bound: PhantomData,
    }
}

/// Restores the polled future's enclosing scope, also when its poll panics.
struct RestoreScope(Option<TaskScope>);

impl Drop for RestoreScope {
    fn drop(&mut self) {
        let previous = self.0.take();
        TASK_SCOPE.with(|scope| *scope.borrow_mut() = previous);
    }
}

/// A future polled inside its section.
struct Scoped<F> {
    scope: TaskScope,
    future: Pin<Box<F>>,
}

impl<F: Future> Future for Scoped<F> {
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<F::Output> {
        let this = self.get_mut();
        let entered = this.scope.clone();
        let _restore = RestoreScope(TASK_SCOPE.with(|scope| scope.replace(Some(entered))));
        this.future.as_mut().poll(cx)
    }
}

fn enclosing_effect() -> Arc<OnceLock<&'static str>> {
    TASK_SCOPE
        .with(|scope| scope.borrow().as_ref().map(|scope| scope.effect.clone()))
        .unwrap_or_default()
}

/// Run `future` as `section`: every poll of it runs inside the section.
/// Tasks it spawns are outside it.
pub async fn within<F>(section: ProtectedSection, future: F) -> F::Output
where
    F: Future,
{
    within_marked(section, future).await.0
}

/// [`within`], also answering the attempt's effect mark as it stands when
/// `future` completes: the first durable effect recorded inside the
/// outermost section (this one, or the one this runs nested in).
pub async fn within_marked<F>(
    section: ProtectedSection,
    future: F,
) -> (F::Output, Option<&'static str>)
where
    F: Future,
{
    let effect = enclosing_effect();
    let output = Scoped {
        scope: TaskScope {
            section,
            effect: effect.clone(),
        },
        future: Box::pin(future),
    }
    .await;
    (output, effect.get().copied())
}

/// The section the current code runs in, if any: the synchronous one of
/// this thread, else the asynchronous one of the future being polled.
#[must_use]
pub fn current() -> Option<ProtectedSection> {
    SYNC_SECTION
        .with(Cell::get)
        .or_else(|| TASK_SCOPE.with(|scope| scope.borrow().as_ref().map(|scope| scope.section)))
}

/// Record that the current attempt made the durable effect `effect`, in the
/// outermost asynchronous section it runs in. The first effect stays; nothing
/// clears it. Outside an asynchronous section this records nothing: a host
/// that writes before its delegate binds runs both inside one section.
pub fn note_effect(effect: &'static str) {
    TASK_SCOPE.with(|scope| {
        if let Some(scope) = scope.borrow().as_ref() {
            let _ = scope.effect.set(effect);
        }
    });
}

/// The first durable effect the current attempt recorded in its outermost
/// asynchronous section, if any. Code that answers "not now" asks this
/// first: once an effect is recorded, it answers what its host shipped
/// instead.
#[must_use]
pub fn effect_recorded() -> Option<&'static str> {
    TASK_SCOPE.with(|scope| {
        scope
            .borrow()
            .as_ref()
            .and_then(|scope| scope.effect.get().copied())
    })
}

/// About to wait at `site` (take a lock another actor may hold across I/O,
/// sleep, spin): in a debug build this panics when the current code runs in
/// a protected section, unless it runs inside [`known_wait`]. A release
/// build checks nothing.
#[track_caller]
pub fn assert_may_wait(site: &'static str) {
    debug_assert!(
        current().is_none() || KNOWN_WAIT.with(Cell::get).is_some(),
        "protected-section wait: {site} may wait inside {:?}",
        current()
    );
}

/// Run the synchronous `wait` as a known wait at `site`: one its owner has
/// listed as still waiting inside a protected section, to be removed by a
/// named later change. [`assert_may_wait`] does not report what runs inside.
/// Only a host's checked-in list of known sites may name one.
pub fn known_wait<T>(site: &'static str, wait: impl FnOnce() -> T) -> T {
    let previous = KNOWN_WAIT.with(|known| known.replace(Some(site)));
    let result = wait();
    KNOWN_WAIT.with(|known| known.set(previous));
    result
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    /// Outside any section nothing is witnessed and a wait is allowed.
    #[test]
    fn outside_a_section_a_wait_is_allowed() {
        assert_eq!(current(), None);
        assert_may_wait("test.outside");
    }

    /// A known wait inside a section is not reported, and the exemption ends
    /// with it.
    #[test]
    fn a_known_wait_inside_a_section_is_not_reported() {
        let _section = enter(ProtectedSection::GovernedBind);
        known_wait("test.known", || assert_may_wait("test.known.lock"));
        let caught = std::panic::catch_unwind(|| assert_may_wait("test.after"));
        assert_eq!(
            caught.is_err(),
            cfg!(debug_assertions),
            "the exemption ends with the known wait"
        );
    }

    /// A synchronous section is witnessed until its guard drops, nested
    /// sections restore the outer one, and a wait inside one panics.
    #[test]
    fn a_synchronous_section_is_witnessed_and_restored() {
        {
            let _outer = enter(ProtectedSection::NativeAdmissionRead);
            assert_eq!(current(), Some(ProtectedSection::NativeAdmissionRead));
            {
                let _inner = enter(ProtectedSection::HistoricalAdmission);
                assert_eq!(current(), Some(ProtectedSection::HistoricalAdmission));
            }
            assert_eq!(current(), Some(ProtectedSection::NativeAdmissionRead));
            let caught = std::panic::catch_unwind(|| assert_may_wait("test.inside"));
            assert_eq!(
                caught.is_err(),
                cfg!(debug_assertions),
                "a wait inside a section panics in debug"
            );
        }
        assert_eq!(current(), None);
    }

    /// An asynchronous section covers every poll of its future, not a task
    /// that future spawns, and ends with the future.
    #[tokio::test]
    async fn an_asynchronous_section_covers_its_future_only() {
        let inside = within(ProtectedSection::GovernedBind, async {
            tokio::task::yield_now().await;
            let spawned = tokio::spawn(async { current() }).await.ok().flatten();
            (current(), spawned)
        })
        .await;
        assert_eq!(inside, (Some(ProtectedSection::GovernedBind), None));
        assert_eq!(current(), None);
    }

    /// The effect mark lives in the outermost section: a nested section
    /// records into it and reads it, the first effect stays, and the owner
    /// gets it back. Outside a section nothing is recorded.
    #[tokio::test]
    async fn the_effect_mark_is_the_outermost_sections() {
        note_effect("test.outside");
        assert_eq!(effect_recorded(), None);
        let ((before, nested, after), mark) =
            within_marked(ProtectedSection::GovernedBind, async {
                let before = effect_recorded();
                let nested = within(ProtectedSection::GovernedBind, async {
                    note_effect("test.first");
                    tokio::task::yield_now().await;
                    note_effect("test.second");
                    effect_recorded()
                })
                .await;
                (before, nested, effect_recorded())
            })
            .await;
        assert_eq!(before, None);
        assert_eq!(nested, Some("test.first"));
        assert_eq!(
            after,
            Some("test.first"),
            "a nested section never resets it"
        );
        assert_eq!(mark, Some("test.first"));
        assert_eq!(effect_recorded(), None, "the mark ends with its attempt");
        let (_, fresh) = within_marked(ProtectedSection::GovernedBind, async {}).await;
        assert_eq!(fresh, None, "every outermost section starts unmarked");
    }

    /// A panicking poll leaves no section behind.
    #[test]
    fn a_panicking_section_restores_the_enclosing_scope() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("test runtime");
        let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            runtime.block_on(within(ProtectedSection::GovernedBind, async {
                std::panic::resume_unwind(Box::new("test.section.panic"))
            }))
        }));
        assert!(caught.is_err());
        assert_eq!(current(), None);
    }
}
