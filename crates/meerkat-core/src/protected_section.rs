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
//!
//! The attempt also records its decision point with [`note_decided`]: where
//! its binder passed its last "not now" site. Durable write funnels call the
//! write witness [`assert_effect_marked`], which in a debug build panics on a
//! write inside a [`ProtectedSection::GovernedBind`] attempt that has neither
//! passed its decision point nor recorded an effect: a "not now" after that
//! write would leave it behind for the next bind. Work an attempt hands to a
//! blocking thread reaches the attempt through its [`EffectMark`].

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

/// One attempt of an outermost asynchronous section: the section it began
/// as, its effect mark and its decision point, each recorded once and never
/// cleared.
struct Attempt {
    outermost: ProtectedSection,
    effect: OnceLock<&'static str>,
    decided: OnceLock<&'static str>,
}

impl Attempt {
    fn new(outermost: ProtectedSection) -> Self {
        Self {
            outermost,
            effect: OnceLock::new(),
            decided: OnceLock::new(),
        }
    }

    /// A durable write now would precede a possible "not now" unmarked.
    fn write_is_unmarked(&self) -> bool {
        self.outermost == ProtectedSection::GovernedBind
            && self.effect.get().is_none()
            && self.decided.get().is_none()
    }
}

/// The asynchronous section a future runs in, with its attempt (shared by
/// every section nested inside the outermost one).
#[derive(Clone)]
struct TaskScope {
    section: ProtectedSection,
    attempt: Arc<Attempt>,
}

thread_local! {
    /// The synchronous section the current thread runs in.
    static SYNC_SECTION: Cell<Option<ProtectedSection>> = const { Cell::new(None) };
    /// The asynchronous section of the future this thread is polling.
    static TASK_SCOPE: RefCell<Option<TaskScope>> = const { RefCell::new(None) };
    /// A wait its owner lists as known, running on this thread.
    static KNOWN_WAIT: Cell<Option<&'static str>> = const { Cell::new(None) };
    /// The attempt a blocking thread entered through its [`EffectMark`].
    static ENTERED_ATTEMPT: RefCell<Option<Arc<Attempt>>> = const { RefCell::new(None) };
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

fn enclosing_attempt() -> Option<Arc<Attempt>> {
    TASK_SCOPE.with(|scope| scope.borrow().as_ref().map(|scope| scope.attempt.clone()))
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
    let attempt = enclosing_attempt().unwrap_or_else(|| Arc::new(Attempt::new(section)));
    let output = Scoped {
        scope: TaskScope {
            section,
            attempt: attempt.clone(),
        },
        future: Box::pin(future),
    }
    .await;
    (output, attempt.effect.get().copied())
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
            let _ = scope.attempt.effect.set(effect);
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
            .and_then(|scope| scope.attempt.effect.get().copied())
    })
}

/// Record that the current attempt passed its decision point at `site`: its
/// binder's last "not now" site, after which it never answers "not now", so
/// a durable write after it needs no effect mark. The first decision stays;
/// nothing clears it. Outside an asynchronous section this records nothing.
/// Only the write witness ([`assert_effect_marked`]) and tests read it.
pub fn note_decided(site: &'static str) {
    TASK_SCOPE.with(|scope| {
        if let Some(scope) = scope.borrow().as_ref() {
            let _ = scope.attempt.decided.set(site);
        }
    });
}

/// The decision point the current attempt recorded with [`note_decided`],
/// if any. For tests that pin where a binder records it.
#[must_use]
pub fn decision_recorded() -> Option<&'static str> {
    TASK_SCOPE.with(|scope| {
        scope
            .borrow()
            .as_ref()
            .and_then(|scope| scope.attempt.decided.get().copied())
    })
}

/// The current attempt's effect mark, for work the attempt hands to another
/// thread (a blocking write), where no section is in scope: it records into
/// the outermost section's mark, as [`note_effect`] does, and the thread
/// enters the attempt with [`EffectMark::enter`] so that the write witness
/// sees it there.
#[derive(Clone)]
pub struct EffectMark(Arc<Attempt>);

impl EffectMark {
    /// Record `effect`; the first effect recorded stays.
    pub fn note(&self, effect: &'static str) {
        let _ = self.0.effect.set(effect);
    }

    /// Enter this attempt on the current thread until the guard drops, for
    /// the write witness ([`assert_effect_marked`]) only: it enters no
    /// section, so a wait there is not witnessed.
    pub fn enter(&self) -> EnteredAttempt {
        let previous = ENTERED_ATTEMPT.with(|entered| entered.replace(Some(self.0.clone())));
        EnteredAttempt {
            previous,
            _thread_bound: PhantomData,
        }
    }
}

impl std::fmt::Debug for EffectMark {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EffectMark")
            .field("effect", &self.0.effect.get())
            .field("decided", &self.0.decided.get())
            .finish()
    }
}

/// An attempt entered on a thread by [`EffectMark::enter`], left when
/// dropped. It stays on the thread that entered it.
#[must_use = "the attempt is left when the guard is dropped"]
pub struct EnteredAttempt {
    previous: Option<Arc<Attempt>>,
    _thread_bound: PhantomData<*const ()>,
}

impl Drop for EnteredAttempt {
    fn drop(&mut self) {
        let previous = self.previous.take();
        ENTERED_ATTEMPT.with(|entered| *entered.borrow_mut() = previous);
    }
}

/// The current attempt's [`EffectMark`], when the current code runs in an
/// asynchronous section.
#[must_use]
pub fn effect_mark() -> Option<EffectMark> {
    enclosing_attempt().map(EffectMark)
}

/// The write witness: the current code is about to make the durable write
/// `site` (a grant, a record). In a debug build this panics when it runs in
/// a [`ProtectedSection::GovernedBind`] attempt that has neither passed its
/// decision point ([`note_decided`]) nor recorded an effect
/// ([`note_effect`]). The attempt is the current task's, or the one this
/// thread entered through an [`EffectMark`]. A release build checks nothing.
#[track_caller]
pub fn assert_effect_marked(site: &'static str) {
    debug_assert!(
        !enclosing_attempt()
            .or_else(|| ENTERED_ATTEMPT.with(|entered| entered.borrow().clone()))
            .is_some_and(|attempt| attempt.write_is_unmarked()),
        "protected-section write: {site} writes inside a GovernedBind attempt before its \
         decision point, with no effect marked"
    );
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
/// Only a host's checked-in list of known sites may name one. The exemption
/// ends with the wait, also when the wait unwinds.
pub fn known_wait<T>(site: &'static str, wait: impl FnOnce() -> T) -> T {
    /// Restores the enclosing known wait when dropped, on return or unwind.
    struct Restore(Option<&'static str>);
    impl Drop for Restore {
        fn drop(&mut self) {
            // A thread being torn down has no exemption left to restore.
            let _ = KNOWN_WAIT.try_with(|known| known.set(self.0));
        }
    }
    let _restore = Restore(KNOWN_WAIT.with(|known| known.replace(Some(site))));
    wait()
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

    /// A known wait that unwinds ends its exemption too: a wait witnessed on
    /// the same thread afterwards is reported again.
    #[test]
    fn a_known_wait_that_unwinds_ends_its_exemption() {
        let _section = enter(ProtectedSection::GovernedBind);
        let unwound = std::panic::catch_unwind(|| {
            known_wait("test.known.unwinds", || {
                std::panic::resume_unwind(Box::new("test.known.unwind"))
            })
        });
        assert!(unwound.is_err(), "the known wait unwound");
        let caught = std::panic::catch_unwind(|| assert_may_wait("test.after_unwind"));
        assert_eq!(
            caught.is_err(),
            cfg!(debug_assertions),
            "the exemption ends with the known wait, also when it unwinds"
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
        let ((), fresh) = within_marked(ProtectedSection::GovernedBind, async {}).await;
        assert_eq!(fresh, None, "every outermost section starts unmarked");
    }

    /// Work an attempt hands to another thread records into the attempt's
    /// mark through its handle; outside a section there is no handle.
    #[tokio::test]
    async fn an_effect_noted_on_another_thread_marks_the_attempt() {
        assert!(effect_mark().is_none());
        let (noted, mark) = within_marked(ProtectedSection::GovernedBind, async {
            let handle = effect_mark().expect("inside a section");
            tokio::task::spawn_blocking(move || handle.note("test.blocking_write"))
                .await
                .expect("the blocking write");
            effect_recorded()
        })
        .await;
        assert_eq!(noted, Some("test.blocking_write"));
        assert_eq!(mark, Some("test.blocking_write"));
    }

    /// The write witness: inside a bind attempt a write needs the attempt's
    /// effect mark or its decision point first; elsewhere nothing is
    /// witnessed. The first decision stays.
    #[tokio::test]
    async fn a_bind_write_needs_a_mark_or_the_decision_point() {
        assert_effect_marked("test.outside");
        let unmarked = within(ProtectedSection::GovernedBind, async {
            within(ProtectedSection::GovernedEntry, async {
                std::panic::catch_unwind(|| assert_effect_marked("test.unmarked")).is_err()
            })
            .await
        })
        .await;
        assert_eq!(
            unmarked,
            cfg!(debug_assertions),
            "a nested section's write is the bind attempt's"
        );
        within(ProtectedSection::GovernedBind, async {
            note_effect("test.mark");
            assert_effect_marked("test.marked");
        })
        .await;
        let decided = within(ProtectedSection::GovernedBind, async {
            let before = decision_recorded();
            note_decided("test.decided");
            note_decided("test.later");
            assert_effect_marked("test.after_decision");
            (before, decision_recorded())
        })
        .await;
        assert_eq!(decided, (None, Some("test.decided")));
        assert_eq!(
            decision_recorded(),
            None,
            "the decision ends with its attempt"
        );
        within(ProtectedSection::HostStoreTransaction, async {
            assert_effect_marked("test.other_section");
        })
        .await;
    }

    /// A blocking thread sees the attempt for the write witness only once it
    /// enters the attempt's handle, and leaves it with the guard.
    #[tokio::test]
    async fn a_blocking_write_is_witnessed_through_the_attempt_handle() {
        let (entered, outside, marked) = within(ProtectedSection::GovernedBind, async {
            let handle = effect_mark().expect("inside a section");
            let blocking = handle.clone();
            let entered = tokio::task::spawn_blocking(move || {
                let caught = {
                    let _entered = blocking.enter();
                    assert_eq!(current(), None, "entering is not a section");
                    std::panic::catch_unwind(|| assert_effect_marked("test.blocking_unmarked"))
                        .is_err()
                };
                assert_effect_marked("test.left");
                caught
            })
            .await
            .expect("the blocking write");
            let outside = tokio::task::spawn_blocking(|| {
                std::panic::catch_unwind(|| assert_effect_marked("test.not_entered")).is_err()
            })
            .await
            .expect("the blocking write");
            let blocking = handle.clone();
            let marked = tokio::task::spawn_blocking(move || {
                let _entered = blocking.enter();
                blocking.note("test.blocking_mark");
                std::panic::catch_unwind(|| assert_effect_marked("test.blocking_marked")).is_err()
            })
            .await
            .expect("the blocking write");
            (entered, outside, marked)
        })
        .await;
        assert_eq!(entered, cfg!(debug_assertions));
        assert!(!outside, "a thread that did not enter sees no attempt");
        assert!(!marked);
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
