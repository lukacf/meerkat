//! Registers an owned file descriptor with tokio's reactor.
use std::{
    io,
    os::fd::{AsRawFd, OwnedFd},
};
use tokio::io::unix::AsyncFd;

/// `AsyncFd::new` for a value that owns its descriptor (`File`, `OwnedFd`).
/// tokio 1.53 deprecated the safe constructor in favour of `unsafe fn
/// register`, whose whole contract is that the descriptor stays open and the
/// same for the `AsyncFd`'s life; an owned descriptor meets it by
/// construction, and Connect forbids unsafe code.
#[allow(
    deprecated,
    reason = "the replacement is unsafe; an owned descriptor meets its contract"
)]
pub(crate) fn register<T: AsRawFd + Into<OwnedFd>>(inner: T) -> io::Result<AsyncFd<T>> {
    AsyncFd::new(inner)
}
