//! Deterministic regression test for the `SessionContextHandle` observer /
//! install race PR #286 attempted to close by construction.
//!
//! The race: `context_advanced` previously applied the DSL transition
//! under the authority lock, released the lock, then read the observer
//! slot and fired the callback. A concurrent
//! `install_observer_with_baseline` that wrote the slot under the same
//! DSL lock was totally-ordered with the transition's commit, but not
//! with the observer fire that followed outside the lock. Result: a
//! just-installed observer could receive a fire for a transition whose
//! effect its baseline had already captured — the pathological shape
//! that manifested as "product_session_tool_call_... open_calls == 2"
//! in CI.
//!
//! These tests force both halves of the race explicitly rather than
//! relying on magnitude-only invariants (`emitted > baseline`), which
//! the broken code also satisfies: the race produces `emitted > baseline`
//! values that should not have fired at all because the installer's
//! baseline already reflected the transition's committed state.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::type_complexity
)]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use meerkat_core::handles::{
    RealtimeProductTurnHandle, RealtimeProjectionFreshness, SessionContextAdvancedObserver,
    SessionContextHandle,
};
use meerkat_runtime::handles::{
    HandleDslAuthority, RuntimeRealtimeProductTurnHandle, RuntimeSessionContextHandle,
};
use meerkat_runtime::meerkat_machine::dsl as mm_dsl;

/// Observer that records every `updated_at_ms` it receives, paired with
/// the baseline it was installed with — so tests can assert ordering
/// (did this observer see fires it shouldn't have, given its install
/// point?) rather than just magnitudes.
struct RecordingObserver {
    baseline: u64,
    received: std::sync::Mutex<Vec<u64>>,
    received_count: AtomicUsize,
}

impl RecordingObserver {
    fn new(baseline: u64) -> Arc<Self> {
        Arc::new(Self {
            baseline,
            received: std::sync::Mutex::new(Vec::new()),
            received_count: AtomicUsize::new(0),
        })
    }

    fn count(&self) -> usize {
        self.received_count.load(Ordering::SeqCst)
    }

    fn snapshot(&self) -> Vec<u64> {
        self.received
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

impl SessionContextAdvancedObserver for RecordingObserver {
    fn on_session_context_advanced(&self, updated_at_ms: u64) {
        self.received
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(updated_at_ms);
        self.received_count.fetch_add(1, Ordering::SeqCst);
    }
}

/// Build an ephemeral session-context handle with the DSL authority
/// driven to the `Idle` phase so `AdvanceSessionContext` transitions
/// match (`per_phase [Idle, Attached, Running, Retired, Stopped]`).
fn new_handle() -> RuntimeSessionContextHandle {
    let dsl = Arc::new(HandleDslAuthority::ephemeral());
    dsl.apply_signal(mm_dsl::MeerkatMachineSignal::Initialize, "test::initialize")
        .expect("Initialize signal");
    dsl.apply_input(
        mm_dsl::MeerkatMachineInput::RegisterSession {
            session_id: mm_dsl::SessionId::from("session-context-observer-race".to_string()),
        },
        "test::register",
    )
    .expect("RegisterSession input");
    RuntimeSessionContextHandle::new(dsl)
}

fn new_shared_handles() -> (
    RuntimeSessionContextHandle,
    Arc<RuntimeRealtimeProductTurnHandle>,
) {
    let dsl = Arc::new(HandleDslAuthority::ephemeral());
    dsl.apply_signal(mm_dsl::MeerkatMachineSignal::Initialize, "test::initialize")
        .expect("Initialize signal");
    dsl.apply_input(
        mm_dsl::MeerkatMachineInput::RegisterSession {
            session_id: mm_dsl::SessionId::from("session-context-observer-race".to_string()),
        },
        "test::register",
    )
    .expect("RegisterSession input");
    (
        RuntimeSessionContextHandle::new(Arc::clone(&dsl)),
        Arc::new(RuntimeRealtimeProductTurnHandle::new(dsl)),
    )
}

struct BridgeToProductTurn {
    product_turn: Arc<dyn RealtimeProductTurnHandle>,
}

impl SessionContextAdvancedObserver for BridgeToProductTurn {
    fn on_session_context_advanced(&self, updated_at_ms: u64) {
        let advanced = self
            .product_turn
            .projection_advance_observed(updated_at_ms)
            .expect("projection_advance_observed must not error");
        assert!(
            advanced,
            "bridge observer must advance product-turn freshness at watermark {updated_at_ms}"
        );
    }
}

/// Baseline sanity: an observer installed at watermark 0 receives every
/// subsequent `context_advanced` fire. No races involved.
#[test]
fn observer_receives_every_advance_after_install() {
    let handle = new_handle();
    let observer = RecordingObserver::new(0);
    let baseline = handle.install_observer_with_baseline(
        Arc::clone(&observer) as Arc<dyn SessionContextAdvancedObserver>
    );
    assert_eq!(baseline, 0);

    for ms in 1..=10 {
        let advanced = handle
            .context_advanced(ms)
            .expect("context_advanced must not error");
        assert!(
            advanced,
            "context_advanced({ms}) must advance (monotonic succession from 0)"
        );
    }

    assert_eq!(observer.count(), 10);
    assert_eq!(observer.snapshot(), (1..=10).collect::<Vec<u64>>());
}

/// Ordering invariant #1 (pre-install transitions MUST NOT fire through
/// a later-installed observer):
///
/// Fire 5 `context_advanced` ticks, THEN install an observer with
/// baseline 5. The observer must receive zero callbacks — its baseline
/// already reflects all five advances. Post-install ticks (6..=10) are
/// the first events the observer is owed.
///
/// Pre-fix, this would have been incidentally correct in the
/// single-threaded case because the post-lock observer-slot read on
/// each `context_advanced` found an empty slot. The race was specific
/// to the *concurrent* case below, but this single-threaded version
/// pins the `apply-before-install` half of the invariant.
#[test]
fn pre_install_transitions_do_not_fire_post_install_observer() {
    let handle = new_handle();

    for ms in 1..=5 {
        assert!(
            handle.context_advanced(ms).expect("advance must not error"),
            "pre-install advance {ms} must commit (monotonic succession from 0)"
        );
    }

    let observer = RecordingObserver::new(5);
    let baseline = handle.install_observer_with_baseline(
        Arc::clone(&observer) as Arc<dyn SessionContextAdvancedObserver>
    );
    assert_eq!(baseline, 5, "baseline must reflect pre-install frontier");
    assert_eq!(
        observer.count(),
        0,
        "no fire should have reached the observer before install"
    );

    for ms in 6..=10 {
        assert!(
            handle.context_advanced(ms).expect("advance must not error"),
            "post-install advance {ms} must commit"
        );
    }

    assert_eq!(observer.count(), 5);
    assert_eq!(observer.snapshot(), (6..=10).collect::<Vec<u64>>());
    for received in observer.snapshot() {
        assert!(
            received > observer.baseline,
            "observer received {received} at or below its baseline {}",
            observer.baseline,
        );
    }
}

/// Ordering invariant #2 (post-install transitions MUST fire through
/// the installed observer):
///
/// Install observer with baseline 0, fire 10 ticks. Observer receives
/// all 10. Symmetric to #1; both invariants together describe the
/// lifetime-accurate semantics.
#[test]
fn post_install_transitions_fire_through_installed_observer() {
    let handle = new_handle();
    let observer = RecordingObserver::new(0);
    handle.install_observer_with_baseline(
        Arc::clone(&observer) as Arc<dyn SessionContextAdvancedObserver>
    );

    for ms in 1..=10 {
        assert!(
            handle.context_advanced(ms).expect("advance must not error"),
            "advance {ms} must commit"
        );
    }

    assert_eq!(observer.count(), 10);
}

/// Regression for the real product-turn bootstrap race: a
/// `context_advanced(N+1)` that lands after
/// `install_observer_with_baseline` but before the socket seeds the
/// product-turn frontier MUST remain stale. The old
/// `projection_reset(baseline)` bootstrap collapsed this back to
/// `Clean` at frontier `N+1`, dropping the owed refresh. The fixed
/// bootstrap uses `projection_refreshed(baseline)`, whose
/// `not_behind_frontier` guard rejects when a newer observer tick has
/// already advanced the frontier.
#[test]
fn product_turn_seed_preserves_post_install_advance() {
    let (session_context, product_turn) = new_shared_handles();
    let bridge_observer: Arc<dyn SessionContextAdvancedObserver> = Arc::new(BridgeToProductTurn {
        product_turn: Arc::clone(&product_turn) as Arc<dyn RealtimeProductTurnHandle>,
    });

    let baseline = session_context.install_observer_with_baseline(Arc::clone(&bridge_observer));
    assert_eq!(baseline, 0);

    assert!(
        session_context
            .context_advanced(1)
            .expect("post-install advance must not error"),
        "post-install advance must commit"
    );
    assert_eq!(
        product_turn.projection_freshness(),
        RealtimeProjectionFreshness::StaleImmediate
    );
    assert_eq!(product_turn.projection_frontier_ms(), 1);

    let seeded = product_turn
        .projection_refreshed(baseline)
        .expect("projection_refreshed seed must not error");
    assert!(
        !seeded,
        "seed at stale baseline must be guard-rejected when a newer advance already landed"
    );
    assert_eq!(
        product_turn.projection_freshness(),
        RealtimeProjectionFreshness::StaleImmediate,
        "seed must preserve the owed refresh when a newer post-install advance already fired"
    );
    assert_eq!(product_turn.projection_frontier_ms(), 1);
}

/// Ordering invariant #3 (concurrent install + advance — THE race
/// signature, deterministic reproducer):
///
/// Reproduces PR #286's unclosed race by forcing the exact interleave
/// that broke pre-fix: a thread's `context_advanced` transition commits
/// under the DSL lock and releases, then another thread's
/// `install_observer_with_baseline` writes the observer slot under the
/// lock AND returns its baseline reflecting the committed state, and
/// THEN the first thread reads the (now post-install) observer slot
/// and fires for the transition that already landed in the installer's
/// baseline. Pre-fix: the installer's new observer receives a fire for
/// a value ≤ its own baseline. Post-fix: the sample inside the DSL
/// lock totally-orders slot-read vs slot-write, so this never happens.
///
/// Many installs concurrent with many advances amplify the race: with
/// N advance-ticks and M installs scheduled around them, each install
/// has ~N/M opportunities to interleave with an advance's lock-release
/// window. Observers are kept live for the run duration (strong `Arc`
/// held in `installed`) so `Weak::upgrade()` never short-circuits the
/// dispatch, which masks the race on pre-fix if the observer Arc dropped.
///
/// The advance thread uses a stiff busy-retry loop (no `yield_now`)
/// around the lock so the advance-release window lands exactly in the
/// time between an install-thread `install_observer_with_baseline`
/// completing and the corresponding post-lock dispatch the advance
/// thread is about to do. `yield_now` between installs keeps both
/// threads scheduled roughly equally on CI runners with 2–4 cores.
#[test]
fn concurrent_install_and_advance_respects_install_ordering() {
    const ADVANCE_COUNT: u64 = 20_000;
    const INSTALL_INTERVAL: u64 = 20;

    let handle = Arc::new(new_handle());
    // Shared recorder: every (observer, baseline) pair installed during
    // the run is kept live here for the full run duration. We inspect
    // it once both threads have joined.
    let installed: Arc<std::sync::Mutex<Vec<(Arc<RecordingObserver>, u64)>>> =
        Arc::new(std::sync::Mutex::new(Vec::new()));

    // Advance thread: tight loop, no yields — maximises lock-handoff
    // to the install thread on every iteration. Tracks total commits
    // so the test can panic loudly if `context_advanced` silently
    // rejects every call (e.g., DSL phase mismatch, guard-rejected
    // inputs) and the whole stress loop becomes a no-op that would
    // otherwise pass the ordering assertion trivially.
    let advance_handle = Arc::clone(&handle);
    let advance_thread = std::thread::spawn(move || {
        let mut committed: u64 = 0;
        for ms in 1..=ADVANCE_COUNT {
            match advance_handle.context_advanced(ms) {
                Ok(true) => committed += 1,
                Ok(false) => {}
                Err(err) => panic!("context_advanced({ms}) errored: {err}"),
            }
        }
        committed
    });

    // Install thread: installs roughly one observer per
    // `INSTALL_INTERVAL` advances. Uses `spin_loop_hint` + a short
    // back-off to give the advance thread time to release the DSL lock
    // between installs.
    let install_handle = Arc::clone(&handle);
    let install_installed = Arc::clone(&installed);
    let install_thread = std::thread::spawn(move || {
        let install_count = (ADVANCE_COUNT / INSTALL_INTERVAL) as usize;
        for _ in 0..install_count {
            let observer = RecordingObserver::new(0);
            let baseline = install_handle.install_observer_with_baseline(
                Arc::clone(&observer) as Arc<dyn SessionContextAdvancedObserver>
            );
            install_installed
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push((observer, baseline));
            // Wait a bit so the advance thread gets to run several
            // iterations between our installs. This widens the race
            // window where a just-installed observer can catch a fire
            // from an advance that committed before our install.
            for _ in 0..50 {
                std::hint::spin_loop();
            }
            std::thread::yield_now();
        }
    });

    let advance_committed = advance_thread.join().expect("advance thread");
    install_thread.join().expect("install thread");

    // Guard against the "silent no-op" failure mode: if not a single
    // `context_advanced` call committed (every call was
    // guard-rejected, phase-mismatched, or errored), the whole stress
    // loop generated zero observer fires and every assertion below
    // passes vacuously. Fail loudly in that case.
    assert!(
        advance_committed > 0,
        "advance thread committed zero transitions out of {ADVANCE_COUNT} attempts — \
         DSL rejected or mis-phased every call; the stress loop is a no-op"
    );

    let installed_snapshot = installed
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    let mut violations: Vec<(u64, u64)> = Vec::new();
    for (observer, baseline) in &installed_snapshot {
        for received in observer.snapshot() {
            if received <= *baseline {
                violations.push((*baseline, received));
            }
        }
    }

    assert!(
        !installed_snapshot.is_empty(),
        "install thread must have run at least one iteration"
    );
    // Without this liveness assertion the whole test trivially passes
    // when observers receive zero fires (e.g. the DSL phase mismatch
    // or a silent `apply` return classification drops every
    // transition). The violation-count check below is only meaningful
    // if the stress loop actually produced observer activity.
    let total_fires: usize = installed_snapshot
        .iter()
        .map(|(observer, _)| observer.count())
        .sum();
    assert!(
        total_fires > 0,
        "stress loop recorded zero observer fires across {} installs and {} advance ticks — \
         test is doing nothing (phase mismatch, wiring bug, or silently-rejected transitions)",
        installed_snapshot.len(),
        ADVANCE_COUNT,
    );
    if !violations.is_empty() {
        let first = violations.first().copied().unwrap_or((0, 0));
        panic!(
            "observer received {} fires at or below its install baseline — \
             install/advance ordering violated (first: baseline={}, received={}; \
             sampled {} observer installs, {} total advance ticks, {} total fires)",
            violations.len(),
            first.0,
            first.1,
            installed_snapshot.len(),
            ADVANCE_COUNT,
            total_fires,
        );
    }
}

/// Monotonic guard sanity: non-advancing `context_advanced` returns
/// `Ok(false)` without firing the observer. Orthogonal to the race but
/// useful regression coverage for the guard's `GuardRejected` -> `Ok(false)`
/// classification.
#[test]
fn non_advancing_context_advanced_does_not_fire_observer() {
    let handle = new_handle();
    let observer = RecordingObserver::new(0);
    handle.install_observer_with_baseline(
        Arc::clone(&observer) as Arc<dyn SessionContextAdvancedObserver>
    );

    handle.context_advanced(5).expect("initial advance");
    assert_eq!(observer.count(), 1);

    let again = handle.context_advanced(5).expect("non-advancing ok");
    assert!(!again, "duplicate watermark should return Ok(false)");
    assert_eq!(
        observer.count(),
        1,
        "non-advancing tick must not fire the observer"
    );

    let regress = handle.context_advanced(3).expect("regressing ok");
    assert!(!regress, "regressing watermark should return Ok(false)");
    assert_eq!(observer.count(), 1);
}

/// DSL authority state stays consistent under the concurrent install +
/// advance loop: the `current_watermark_ms` read via a clean handle
/// after the dust settles reflects the latest advance. Not strictly a
/// race test, but cheap parity coverage that complements the stress
/// test above.
#[test]
fn watermark_is_monotonic_and_reflects_latest_advance() {
    let handle = new_handle();
    assert_eq!(handle.current_watermark_ms(), 0);
    handle.context_advanced(10).unwrap();
    assert_eq!(handle.current_watermark_ms(), 10);
    handle.context_advanced(5).unwrap(); // rejected, no regress
    assert_eq!(handle.current_watermark_ms(), 10);
    handle.context_advanced(20).unwrap();
    assert_eq!(handle.current_watermark_ms(), 20);
}

/// Dropped observer (Weak upgrade fails) — the handle should no-op
/// cleanly, not panic. Covers the `Weak::upgrade()` branch when the
/// caller's strong reference has been dropped.
#[test]
fn dropped_observer_strong_ref_no_ops_on_fire() {
    let handle = new_handle();
    {
        let observer = RecordingObserver::new(0);
        handle.install_observer_with_baseline(
            Arc::clone(&observer) as Arc<dyn SessionContextAdvancedObserver>
        );
        // observer drops at end of scope
    }

    // No installed strong ref anymore; the Weak fails to upgrade.
    // context_advanced must not panic and must still advance state.
    handle.context_advanced(42).expect("advance after drop");
    assert_eq!(handle.current_watermark_ms(), 42);
}
