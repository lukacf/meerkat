//! WASM host timers with Rust-owned cancellation.
//!
//! Each pending timer owns its exact JavaScript handle and callback. Dropping a
//! future clears that handle before releasing the callback, in browsers and Node.
//! This module only adapts host scheduling; callers retain deadline policy.

use std::cell::RefCell;
use std::error::Error;
use std::fmt::{Display, Formatter};
use std::future::Future;
use std::io;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll, Waker};

use wasm_bindgen::prelude::*;

pub use super::{Duration, Instant};

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = globalThis, js_name = setTimeout)]
    fn set_timeout(callback: &js_sys::Function, milliseconds: f64) -> JsValue;
    #[wasm_bindgen(js_namespace = globalThis, js_name = clearTimeout)]
    fn clear_timeout(handle: &JsValue);
}

#[derive(Default)]
struct TimerState {
    fired: bool,
    waker: Option<Waker>,
}

struct Timer {
    handle: JsValue,
    // Owned, never forgotten or handed to the JavaScript garbage collector.
    _callback: Closure<dyn FnMut()>,
    state: Rc<RefCell<TimerState>>,
}

impl Timer {
    fn new(duration: Duration, waker: &Waker) -> Self {
        let state = Rc::new(RefCell::new(TimerState {
            fired: false,
            waker: Some(waker.clone()),
        }));
        let weak = Rc::downgrade(&state);
        // A panic from an executor's waker cannot strand a RefCell borrow or a
        // partly updated timer state: the state is committed before waking.
        let callback = Closure::own_assert_unwind_safe(move || {
            let Some(state) = weak.upgrade() else {
                return;
            };
            let waker = {
                let mut state = state.borrow_mut();
                if state.fired {
                    return;
                }
                state.fired = true;
                state.waker.take()
            };
            if let Some(waker) = waker {
                waker.wake();
            }
        });
        // Hosts convert delays to signed 32-bit milliseconds. Round positive
        // fractions up and split larger durations, never overflow or fire early.
        let milliseconds =
            duration.as_millis() + u128::from(!duration.subsec_nanos().is_multiple_of(1_000_000));
        let milliseconds = milliseconds.min(i32::MAX as u128) as u32;
        let handle = set_timeout(callback.as_ref().unchecked_ref(), f64::from(milliseconds));
        Self {
            handle,
            _callback: callback,
            state,
        }
    }

    fn has_fired(&self, waker: &Waker) -> bool {
        let mut state = self.state.borrow_mut();
        if state.fired {
            true
        } else {
            if !state.waker.as_ref().is_some_and(|old| old.will_wake(waker)) {
                state.waker = Some(waker.clone());
            }
            false
        }
    }
}

impl Drop for Timer {
    fn drop(&mut self) {
        self.state.borrow_mut().waker = None;
        clear_timeout(&self.handle);
        // Rust drops _callback only after clearTimeout has released host custody.
    }
}

/// Waits for a duration, starting when first polled. Even zero yields to the host.
pub fn sleep(duration: Duration) -> Sleep {
    Sleep {
        duration,
        started: None,
        timer: None,
        complete: false,
    }
}

/// A cancellable host timer, allocated lazily on the first pending poll.
pub struct Sleep {
    duration: Duration,
    started: Option<Instant>,
    timer: Option<Timer>,
    complete: bool,
}

impl Sleep {
    fn from_start(started: Instant, duration: Duration) -> Self {
        Self {
            started: Some(started),
            ..sleep(duration)
        }
    }

    fn cancel(&mut self) {
        self.timer = None;
        self.complete = true;
    }
}

impl Future for Sleep {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        if self.complete {
            return Poll::Ready(());
        }
        let elapsed = if let Some(timer) = &self.timer {
            if !timer.has_fired(cx.waker()) {
                return Poll::Pending;
            }
            self.timer = None;
            self.started.map_or(Duration::ZERO, |start| start.elapsed())
        } else if let Some(started) = self.started {
            // Intervals and absolute deadlines start before the first poll.
            started.elapsed()
        } else {
            self.started = Some(Instant::now());
            self.timer = Some(Timer::new(self.duration, cx.waker()));
            return Poll::Pending;
        };
        if elapsed >= self.duration {
            self.complete = true;
            return Poll::Ready(());
        }
        self.timer = Some(Timer::new(
            self.duration.saturating_sub(elapsed),
            cx.waker(),
        ));
        Poll::Pending
    }
}

/// Polls the inner future before its deadline, including a zero deadline.
pub fn timeout<F: Future>(duration: Duration, future: F) -> Timeout<F> {
    Timeout {
        future: Box::pin(future),
        sleep: sleep(duration),
    }
}

/// Polls the inner future with an absolute monotonic deadline.
pub fn timeout_at<F: Future>(deadline: Instant, future: F) -> Timeout<F> {
    let started = Instant::now();
    Timeout {
        future: Box::pin(future),
        sleep: Sleep::from_start(started, deadline.saturating_duration_since(started)),
    }
}

/// Future returned by [`timeout`] and [`timeout_at`].
pub struct Timeout<F: Future> {
    future: Pin<Box<F>>,
    sleep: Sleep,
}

impl<F: Future> Future for Timeout<F> {
    type Output = Result<F::Output, Elapsed>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if let Poll::Ready(output) = self.future.as_mut().poll(cx) {
            // Completion releases the losing host timer even if the caller
            // retains this completed Timeout wrapper.
            self.sleep.cancel();
            return Poll::Ready(Ok(output));
        }
        Pin::new(&mut self.sleep)
            .poll(cx)
            .map(|()| Err(Elapsed(())))
    }
}

/// A timeout expired before its inner future completed.
#[derive(Debug, PartialEq, Eq)]
pub struct Elapsed(());

impl Display for Elapsed {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        "deadline has elapsed".fmt(formatter)
    }
}

impl Error for Elapsed {}

impl From<Elapsed> for io::Error {
    fn from(_: Elapsed) -> Self {
        io::ErrorKind::TimedOut.into()
    }
}

/// Creates an interval whose first tick occurs after one full period.
///
/// Missed ticks remain available in order. Scheduling is lazy, so an idle
/// interval retains its monotonic position without allocating queued callbacks.
pub fn interval(period: Duration) -> Interval {
    let started = Instant::now();
    Interval {
        period,
        started,
        next_tick: period,
        sleep: interval_sleep(started, period),
    }
}

/// A delayed-first-tick WASM interval backed by the same cancellable timer.
pub struct Interval {
    period: Duration,
    started: Instant,
    next_tick: Duration,
    sleep: Sleep,
}

impl Interval {
    /// Waits for the next tick. Dropping this future does not lose the tick.
    pub async fn tick(&mut self) {
        (&mut self.sleep).await;
        self.next_tick = self.next_tick.saturating_add(self.period);
        self.sleep = if self.period.is_zero() {
            // Zero-period intervals still yield on every tick.
            sleep(Duration::ZERO)
        } else {
            Sleep::from_start(self.started, self.next_tick)
        };
    }

    /// Cancels any pending tick and starts the original period again now.
    pub fn reset(&mut self) {
        self.started = Instant::now();
        self.next_tick = self.period;
        self.sleep = interval_sleep(self.started, self.period);
    }
}

fn interval_sleep(started: Instant, period: Duration) -> Sleep {
    if period.is_zero() {
        sleep(Duration::ZERO)
    } else {
        Sleep::from_start(started, period)
    }
}
