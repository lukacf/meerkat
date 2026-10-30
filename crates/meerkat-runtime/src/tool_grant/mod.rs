//! Scoped tool grants.
//!
//! One grant opens tool names a session build marked grant-gated, for a
//! bounded window. The canonical `ToolGrantLifecycleMachine` owns each
//! grant's lifecycle; see `dsl.rs` for its production instantiation and
//! `crates/meerkat-machine-schema/src/catalog/dsl/tool_grant_lifecycle.rs`
//! for the catalog body (the two must stay structurally identical; the
//! schema parity test enforces this).
//!
//! - [`service`]: [`ToolGrantService`], stateless over a
//!   [`RuntimeStore`](crate::store::RuntimeStore). It grants, revokes, lists
//!   and reads the audit, and adjudicates a served session's model-request
//!   boundary and each gated dispatch. Every answer rebuilds the generated
//!   machine from the stored row, applies one input at the clock reading of
//!   that answer and decides from the input's effects alone.
//! - [`gate`]: [`RuntimeSessionToolGrants`], the core
//!   [`SessionToolGrants`](meerkat_core::tool_grant::SessionToolGrants) seam
//!   of one served session, carried on its runtime bindings.
//! - [`deadlines`]: [`ToolGrantDeadlines`], the deadline driver of one
//!   runtime owner's served sessions.
//! - [`audit`]: the audit rows each subject's grants append.
//! - [`wire`]: the projection of grants, listings and audit rows onto their
//!   wire contracts.
//!
//! Nothing caches whether a grant is live, nothing scans at boot, and
//! nothing here wakes a dormant member or registers a session: a grant,
//! revoke, list or audit read touches only grant rows.

use std::time::Duration;

use meerkat_core::time_compat::{SystemTime, UNIX_EPOCH};

pub mod audit;
pub mod deadlines;
#[allow(
    dead_code,
    clippy::assign_op_pattern,
    clippy::cmp_owned,
    clippy::unused_self,
    clippy::unnecessary_map_on_constructor
)]
pub mod dsl;
pub mod gate;
pub mod service;
pub mod wire;

pub use audit::{
    ToolGrantAuditEvent, ToolGrantAuditPage, ToolGrantAuditRecord, ToolGrantAuditRow,
    ToolGrantExpiryDriver, ToolGrantRefusalKind, UnreadableToolGrantAuditRow,
};
pub use deadlines::ToolGrantDeadlines;
pub use gate::RuntimeSessionToolGrants;
pub use service::{
    RevokedToolGrant, StoredToolGrant, ToolGrantListing, ToolGrantRequest, ToolGrantRevocation,
    ToolGrantService, ToolGrantServiceError, ToolGrantStatus, ToolGrantWindow, UnreadableToolGrant,
};

/// The host clock the grant service reads: wall-clock UNIX milliseconds.
///
/// Read once per adjudication, after every await of that adjudication, and
/// passed to the generated machine as `now_ms`. Implementations must not
/// block or perform I/O. The default host clock is trusted, not
/// authenticated, and not protected against rollback: an expired or revoked
/// grant stays ended whatever the clock later reads, and a live grant keeps
/// its fixed deadline.
pub trait ToolGrantClock: Send + Sync {
    /// The current time in UNIX milliseconds.
    ///
    /// # Errors
    /// [`ToolGrantClockError`] when the host cannot give a representable
    /// time.
    fn now_ms(&self) -> Result<u64, ToolGrantClockError>;
}

/// The host cannot give a representable time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("the tool grant clock is unavailable")]
pub struct ToolGrantClockError;

/// The host's wall clock.
#[derive(Debug, Clone, Copy, Default)]
pub struct HostToolGrantClock;

impl ToolGrantClock for HostToolGrantClock {
    fn now_ms(&self) -> Result<u64, ToolGrantClockError> {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .and_then(|elapsed| u64::try_from(elapsed.as_millis()).ok())
            .ok_or(ToolGrantClockError)
    }
}

/// The host's policy for the grants it issues.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolGrantPolicy {
    /// The longest window a grant may have. A grant whose window is longer
    /// is refused at issuance.
    pub max_lifetime: Duration,
}

impl ToolGrantPolicy {
    /// The default maximum lifetime: 24 hours.
    pub const DEFAULT_MAX_LIFETIME: Duration = Duration::from_secs(24 * 60 * 60);

    /// The maximum lifetime in milliseconds, saturating.
    #[must_use]
    pub fn max_lifetime_ms(&self) -> u64 {
        u64::try_from(self.max_lifetime.as_millis()).unwrap_or(u64::MAX)
    }
}

impl Default for ToolGrantPolicy {
    fn default() -> Self {
        Self {
            max_lifetime: Self::DEFAULT_MAX_LIFETIME,
        }
    }
}
