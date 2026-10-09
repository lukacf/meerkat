//! Diagnostics for awaits that normally finish quickly but can stall
//! silently (a live projection behind a lock, a session save on a slow
//! store). Each watched await logs at DEBUG on entry and exit, and at WARN
//! once it has been pending past [`SLOW_AWAIT_WARN_AFTER`] (then every
//! [`SLOW_AWAIT_REPEAT`] while it stays pending), naming the step. The
//! future itself is polled exactly as before; nothing decides on these
//! lines.

use std::fmt::Display;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

/// How long a watched await may stay pending before it is reported.
pub const SLOW_AWAIT_WARN_AFTER: Duration = Duration::from_secs(2);
/// How often a still-pending watched await is reported again.
pub const SLOW_AWAIT_REPEAT: Duration = Duration::from_secs(5);

#[cfg(not(target_arch = "wasm32"))]
fn millis(elapsed: Duration) -> u64 {
    u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
}

/// Await `future`, the named `step` of `scope` (a channel or session id).
///
/// The future arrives boxed: the watched awaits wrap large futures (a live
/// projection, a session save), and an unboxed one would be carried inline
/// in this wrapper, growing every caller's stack in unoptimized builds.
#[cfg(not(target_arch = "wasm32"))]
pub async fn warn_if_slow<S, F>(scope: &S, step: &'static str, future: Pin<Box<F>>) -> F::Output
where
    S: Display + Sync + ?Sized,
    F: Future + ?Sized,
{
    let started = tokio::time::Instant::now();
    tracing::debug!(scope = %scope, step, "watched await entered");
    let mut future = future;
    let mut report_at = started + SLOW_AWAIT_WARN_AFTER;
    let mut reported = false;
    let output = loop {
        tokio::select! {
            biased;
            output = &mut future => break output,
            () = tokio::time::sleep_until(report_at) => {
                tracing::warn!(
                    scope = %scope,
                    step,
                    elapsed_ms = millis(started.elapsed()),
                    "watched await is still pending"
                );
                reported = true;
                report_at += SLOW_AWAIT_REPEAT;
            }
        }
    };
    let elapsed_ms = millis(started.elapsed());
    if reported {
        tracing::warn!(scope = %scope, step, elapsed_ms, "slow watched await finished");
    } else {
        tracing::debug!(scope = %scope, step, elapsed_ms, "watched await finished");
    }
    output
}

/// Await `future`; browser builds keep no timer for diagnostics.
#[cfg(target_arch = "wasm32")]
pub async fn warn_if_slow<S, F>(_scope: &S, _step: &'static str, future: Pin<Box<F>>) -> F::Output
where
    S: Display + Sync + ?Sized,
    F: Future + ?Sized,
{
    future.await
}
