//! Heap placement for large runtime-construction futures.
//!
//! The gateway and runtime bootstrap futures are hundreds of kilobytes. A
//! caller that writes `inner().await` (or `Box::pin(inner()).await`) holds
//! that whole future as a by-value temporary in its own poll frame before it
//! moves it, and unoptimized builds keep one such slot per awaited call. The
//! nested frames then exceed an ordinary 2 MiB thread or tokio worker stack.
//! [`boxed`] constructs the future inside its own short-lived frame and hands
//! back only the heap pointer, so the awaiting frame stays small. The future
//! itself, and so its behavior, is unchanged.

use std::future::Future;
use std::pin::Pin;

/// Build `make()`'s future in this frame and return it heap-pinned.
pub(crate) fn boxed<F, Fut>(make: F) -> Pin<Box<Fut>>
where
    F: FnOnce() -> Fut,
    Fut: Future,
{
    Box::pin(make())
}
