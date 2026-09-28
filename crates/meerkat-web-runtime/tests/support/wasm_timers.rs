use std::cell::Cell;
use std::future::{Future, pending, poll_fn};
use std::pin::Pin;
use std::rc::Rc;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::task::{Context, Poll};

use futures::task::{ArcWake, noop_waker, waker};
use meerkat_core::time_compat::{Duration, Instant};
use meerkat_core::tokio::time::{interval, sleep, timeout, timeout_at};
use wasm_bindgen::prelude::*;
use wasm_bindgen_test::wasm_bindgen_test;

// Instrumentation observes real host timers and forwards their exact handles.
// Cleanup happens only after taking the snapshot used by the assertions, so it
// cannot make a missing production cancellation pass or retain baseline leaks.
#[wasm_bindgen(inline_js = r#"
export function observeTimers() {
    const originalSet = globalThis.setTimeout;
    const originalClear = globalThis.clearTimeout;
    const records = [];
    globalThis.setTimeout = function(callback, delay, ...args) {
        const record = { callback, delay, cleared: false, fired: false };
        record.handle = originalSet.call(globalThis, (...values) => {
            record.fired = true;
            callback(...values);
        }, delay, ...args);
        records.push(record);
        return record.handle;
    };
    globalThis.clearTimeout = function(handle) {
        const record = records.find(record => Object.is(record.handle, handle));
        if (record) record.cleared = true;
        return originalClear.call(globalThis, handle);
    };
    return {
        fire(index) { records[index].callback(); },
        count() { return records.length; },
        finish() {
            const snapshot = records.map(({ delay, cleared, fired }) => ({ delay, cleared, fired }));
            globalThis.setTimeout = originalSet;
            globalThis.clearTimeout = originalClear;
            for (const record of records) originalClear.call(globalThis, record.handle);
            return JSON.stringify(snapshot);
        }
    };
}
export function fireObservedTimer(observer, index) { observer.fire(index); }
export function finishObservingTimers(observer) { return observer.finish(); }
export function observedTimerCount(observer) { return observer.count(); }
"#)]
extern "C" {
    #[wasm_bindgen(js_name = observeTimers)]
    fn observe_timers() -> JsValue;
    #[wasm_bindgen(js_name = fireObservedTimer)]
    fn fire_observed_timer(observer: &JsValue, index: u32);
    #[wasm_bindgen(js_name = finishObservingTimers)]
    fn finish_observing_timers(observer: &JsValue) -> String;
    #[wasm_bindgen(js_name = observedTimerCount)]
    fn observed_timer_count(observer: &JsValue) -> u32;
}

#[derive(serde::Deserialize, Debug)]
struct TimerRecord {
    delay: f64,
    cleared: bool,
    fired: bool,
}

fn finish(observer: &JsValue) -> Vec<TimerRecord> {
    serde_json::from_str(&finish_observing_timers(observer)).expect("timer observation")
}

fn poll_once<F: Future>(future: Pin<&mut F>) -> Poll<F::Output> {
    future.poll(&mut Context::from_waker(&noop_waker()))
}

#[wasm_bindgen_test]
fn unpolled_sleep_and_timeout_do_not_allocate_timers() {
    // Link the complete runtime composition and its authority bridge symbols.
    assert!(!meerkat_web_runtime::runtime_version().is_empty());
    let observer = observe_timers();
    drop(sleep(Duration::from_secs(3600)));
    drop(timeout(Duration::from_secs(3600), pending::<()>()));
    let records = finish(&observer);
    assert!(records.is_empty(), "{records:?}");
}

#[wasm_bindgen_test]
fn dropped_sleep_clears_its_exact_host_timer() {
    let observer = observe_timers();
    let mut future = Box::pin(sleep(Duration::from_secs(3600)));
    let polled = poll_once(future.as_mut());
    drop(future);
    let records = finish(&observer);
    assert!(polled.is_pending());
    assert_eq!(records.len(), 1);
    assert!(records[0].cleared, "{records:?}");
}

#[wasm_bindgen_test]
fn dropped_timeout_clears_its_exact_host_timer() {
    let observer = observe_timers();
    let mut future = Box::pin(timeout(Duration::from_secs(3600), pending::<()>()));
    let polled = poll_once(future.as_mut());
    drop(future);
    let records = finish(&observer);
    assert!(polled.is_pending());
    assert_eq!(records.len(), 1);
    assert!(records[0].cleared, "{records:?}");
}

#[wasm_bindgen_test]
fn timeout_inner_winner_releases_timer_before_wrapper_drop() {
    let observer = observe_timers();
    let ready = Rc::new(Cell::new(false));
    let inner_ready = Rc::clone(&ready);
    let mut future = Box::pin(timeout(
        Duration::from_secs(3600),
        poll_fn(move |_| {
            if inner_ready.get() {
                Poll::Ready(42)
            } else {
                Poll::Pending
            }
        }),
    ));
    let first = poll_once(future.as_mut());
    ready.set(true);
    let second = poll_once(future.as_mut());
    let records = finish(&observer);
    drop(future);
    assert!(first.is_pending());
    assert_eq!(second, Poll::Ready(Ok(42)));
    assert_eq!(records.len(), 1);
    assert!(records[0].cleared, "{records:?}");
}

#[wasm_bindgen_test]
fn timeout_polls_inner_first_even_at_zero_duration() {
    let observer = observe_timers();
    let mut future = Box::pin(timeout(Duration::ZERO, std::future::ready(42)));
    let result = poll_once(future.as_mut());
    let records = finish(&observer);
    assert_eq!(result, Poll::Ready(Ok(42)));
    assert!(records.is_empty(), "{records:?}");
}

#[wasm_bindgen_test(async)]
async fn completed_sleep_releases_timer_before_wrapper_drop() {
    let observer = observe_timers();
    let mut future = Box::pin(sleep(Duration::from_millis(2)));
    future.as_mut().await;
    let records = finish(&observer);
    drop(future);
    assert!(!records.is_empty());
    assert!(records.iter().any(|record| record.fired));
    assert!(records.iter().all(|record| record.cleared), "{records:?}");
}

#[wasm_bindgen_test(async)]
async fn expired_timeout_releases_timer_and_reports_elapsed() {
    let observer = observe_timers();
    let mut future = Box::pin(timeout(Duration::from_millis(2), pending::<()>()));
    let result = future.as_mut().await;
    let records = finish(&observer);
    drop(future);
    assert_eq!(
        result.expect_err("elapsed").to_string(),
        "deadline has elapsed"
    );
    assert!(!records.is_empty());
    assert!(records.iter().all(|record| record.cleared), "{records:?}");
}

#[wasm_bindgen_test(async)]
async fn zero_sleep_yields_and_positive_submillisecond_sleep_does_not_expire_early() {
    let observer = observe_timers();
    let mut zero = Box::pin(sleep(Duration::ZERO));
    let initial = poll_once(zero.as_mut());
    zero.as_mut().await;
    let start = Instant::now();
    sleep(Duration::from_micros(500)).await;
    let elapsed = start.elapsed();
    let records = finish(&observer);
    assert!(initial.is_pending(), "zero sleep must yield to the host");
    assert!(elapsed >= Duration::from_micros(500), "{elapsed:?}");
    assert!(records.iter().all(|record| record.cleared), "{records:?}");
}

#[wasm_bindgen_test(async)]
async fn very_large_duration_uses_bounded_chunks_and_rechecks_after_callback() {
    let observer = observe_timers();
    let mut future = Box::pin(sleep(Duration::MAX));
    let first = poll_once(future.as_mut());
    fire_observed_timer(&observer, 0);
    // Let the dependency's Promise wake too, so the regression sees its early
    // completion instead of accidentally passing because of a queued microtask.
    wasm_bindgen_futures::JsFuture::from(js_sys::Promise::resolve(&JsValue::UNDEFINED))
        .await
        .expect("microtask");
    let second = poll_once(future.as_mut());
    drop(future);
    let records = finish(&observer);
    assert!(first.is_pending());
    assert!(
        second.is_pending(),
        "an early host callback cannot expire a long sleep"
    );
    assert_eq!(records.len(), 2, "must arm the remaining chunk");
    assert!(
        records
            .iter()
            .all(|record| record.delay == f64::from(i32::MAX) && record.cleared),
        "{records:?}"
    );
}

#[wasm_bindgen_test]
fn interval_drop_and_reset_release_pending_timer_without_immediate_tick() {
    let observer = observe_timers();
    let mut ticker = interval(Duration::from_secs(3600));
    let first = {
        let mut tick = Box::pin(ticker.tick());
        poll_once(tick.as_mut())
    };
    ticker.reset();
    let second = {
        let mut tick = Box::pin(ticker.tick());
        poll_once(tick.as_mut())
    };
    drop(ticker);
    let records = finish(&observer);
    assert!(first.is_pending());
    assert!(second.is_pending());
    assert_eq!(records.len(), 2);
    assert!(records.iter().all(|record| record.cleared), "{records:?}");
}

#[derive(Default)]
struct WakeCount(AtomicUsize);

impl ArcWake for WakeCount {
    fn wake_by_ref(arc: &Arc<Self>) {
        arc.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[wasm_bindgen_test]
fn timer_wakes_latest_waiter_once_and_releases_waker_on_drop() {
    let observer = observe_timers();
    let first = Arc::new(WakeCount::default());
    let second = Arc::new(WakeCount::default());
    let mut future = Box::pin(sleep(Duration::from_secs(3600)));
    let first_poll = future
        .as_mut()
        .poll(&mut Context::from_waker(&waker(Arc::clone(&first))));
    let second_poll = future
        .as_mut()
        .poll(&mut Context::from_waker(&waker(Arc::clone(&second))));
    fire_observed_timer(&observer, 0);
    fire_observed_timer(&observer, 0);
    drop(future);
    let records = finish(&observer);
    assert!(first_poll.is_pending() && second_poll.is_pending());
    assert_eq!(first.0.load(Ordering::SeqCst), 0);
    assert_eq!(second.0.load(Ordering::SeqCst), 1);
    assert_eq!(Arc::strong_count(&first), 1);
    assert_eq!(Arc::strong_count(&second), 1);
    assert_eq!(records.len(), 1);
    assert!(records[0].cleared);
}

#[wasm_bindgen_test(async)]
async fn zero_interval_yields_on_initial_tick_reset_and_later_ticks() {
    let observer = observe_timers();
    let mut ticker = interval(Duration::ZERO);
    let initial = {
        let mut tick = Box::pin(ticker.tick());
        poll_once(tick.as_mut())
    };
    ticker.tick().await;
    let subsequent = {
        let mut tick = Box::pin(ticker.tick());
        poll_once(tick.as_mut())
    };
    ticker.reset();
    let reset = {
        let mut tick = Box::pin(ticker.tick());
        poll_once(tick.as_mut())
    };
    drop(ticker);
    let records = finish(&observer);
    assert!(initial.is_pending());
    assert!(subsequent.is_pending());
    assert!(reset.is_pending());
    assert_eq!(records.len(), 3);
    assert!(records.iter().all(|record| record.cleared), "{records:?}");
}

#[wasm_bindgen_test(async)]
async fn interval_preserves_overdue_ticks_and_reset_discards_backlog() {
    let observer = observe_timers();
    let period = Duration::from_millis(20);
    let mut ticker = interval(period);
    // No timer is needed while the interval is idle, but its cadence continues.
    sleep(Duration::from_millis(65)).await;
    let mut overdue = 0;
    // Timer throttling may delay the host, so drain every observed overdue tick
    // rather than assuming that precisely three periods elapsed.
    let next = loop {
        let mut tick = Box::pin(ticker.tick());
        let result = poll_once(tick.as_mut());
        if result.is_pending() {
            break result;
        }
        overdue += 1;
        assert!(overdue < 1000, "interval never reaches a future tick");
    };
    // The pending tick future was dropped. Awaiting another observes that same
    // tick and does not restart the interval or allocate a replacement timer.
    let before = observed_timer_count(&observer);
    let still_pending = {
        let mut tick = Box::pin(ticker.tick());
        poll_once(tick.as_mut())
    };
    let after = observed_timer_count(&observer);
    ticker.tick().await;
    sleep(Duration::from_millis(65)).await;
    ticker.reset();
    let reset = {
        let mut tick = Box::pin(ticker.tick());
        poll_once(tick.as_mut())
    };
    drop(ticker);
    let records = finish(&observer);
    assert!(overdue >= 3);
    assert!(next.is_pending());
    assert!(still_pending.is_pending());
    assert_eq!(
        before, after,
        "reattaching to a pending tick must reuse its timer"
    );
    assert!(reset.is_pending(), "reset must discard overdue ticks");
    assert!(records.iter().all(|record| record.cleared), "{records:?}");
}

#[wasm_bindgen_test(async)]
async fn absolute_timeout_keeps_deadline_when_first_poll_is_delayed() {
    let observer = observe_timers();
    let deadline = Instant::now() + Duration::from_millis(2);
    let mut future = Box::pin(timeout_at(deadline, pending::<()>()));
    sleep(Duration::from_millis(5)).await;
    let before = observed_timer_count(&observer);
    let result = poll_once(future.as_mut());
    let after = observed_timer_count(&observer);
    let records = finish(&observer);
    drop(future);
    assert!(matches!(result, Poll::Ready(Err(_))));
    assert_eq!(
        before, after,
        "elapsed deadline must not restart a host timer"
    );
    assert!(records.iter().all(|record| record.cleared), "{records:?}");
}

#[wasm_bindgen_test]
fn dropped_yield_releases_its_exact_host_timer() {
    let observer = observe_timers();
    let mut future = Box::pin(meerkat_core::tokio::task::yield_now());
    let result = poll_once(future.as_mut());
    drop(future);
    let records = finish(&observer);
    assert!(result.is_pending());
    assert_eq!(records.len(), 1);
    assert!(records[0].cleared, "{records:?}");
}
