//! Nonblocking local time for process-local grant and policy observations.
//! The default clock is host-trusted, not authenticated or rollback-resistant.

use meerkat_core::time_compat::{Instant, SystemTime, UNIX_EPOCH};

/// The host cannot provide a representable local time observation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum LocalClockError {
    #[error("local authorization clock unavailable")]
    Unavailable,
}

/// Trusted host time. The default profile does not authenticate wall time.
#[derive(Clone, Copy)]
pub struct LocalAuthorizationTime {
    pub unix_ms: u64,
    pub monotonic: Instant,
}

/// Local clock seam shared by preparation and its final checks. Implementations
/// must be nonblocking and must not perform I/O, refreshes or authentication.
pub trait LocalAuthorizationClock: Send + Sync {
    fn now(&self) -> Result<LocalAuthorizationTime, LocalClockError>;
}

pub struct HostAuthorizationClock;

impl LocalAuthorizationClock for HostAuthorizationClock {
    fn now(&self) -> Result<LocalAuthorizationTime, LocalClockError> {
        let monotonic = Instant::now();
        let unix_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .and_then(|duration| u64::try_from(duration.as_millis()).ok())
            .ok_or(LocalClockError::Unavailable)?;
        Ok(LocalAuthorizationTime { unix_ms, monotonic })
    }
}
