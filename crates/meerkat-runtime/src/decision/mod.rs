//! Durable decisions: the owner of `DurableDecisionMachine` records.
//!
//! One durable decision holds one operation until an attested party decides
//! it, its requester or the host withdraws it, or its deadline passes; the
//! operation's owner then executes the held request at most once, under an
//! execution permit. The canonical `DurableDecisionMachine` owns each
//! decision's lifecycle; see `dsl.rs` for its production instantiation and
//! `crates/meerkat-machine-schema/src/catalog/dsl/durable_decision.rs` for the
//! catalog body (the two must stay structurally identical; the schema parity
//! test enforces this).
//!
//! - [`service`]: [`DurableDecisionService`], stateless over a
//!   [`DecisionStore`](meerkat_core::approval::decision::DecisionStore). Every
//!   call reads the one row it needs, reads the clock once (after its reads),
//!   rebuilds the generated machine from the row, applies one input and
//!   decides from the input's effects alone.
//! - [`permit`]: [`DecisionExecutionPermit`], the machine-authorized right to
//!   execute one approved decision's held request.
//!
//! Nothing caches a decision, nothing is read at open, and nothing scans
//! every row: a call touches one row or one index page.

use meerkat_core::time_compat::{SystemTime, UNIX_EPOCH};

#[allow(
    dead_code,
    unused_variables,
    unreachable_code,
    clippy::assign_op_pattern,
    clippy::cmp_owned,
    clippy::unused_self,
    clippy::unnecessary_map_on_constructor
)]
pub mod dsl;
pub mod permit;
pub mod service;
#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod service_tests;

pub use permit::DecisionExecutionPermit;
pub use service::{
    DecisionError, DecisionListing, DecisionRefusal, DecisionWithdrawer, DurableDecisionService,
};

/// Whether a decision owner may write.
///
/// Fixed when the owner is opened: an owner composed for a candidate launch
/// is narrowed to [`Self::CandidateLaunch`] before it is shared, and nothing
/// widens it again. More modes may be added.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum DecisionWriteMode {
    /// An admitted launch: the owner reads and writes decisions.
    Admitted,
    /// A candidate launch: the owner reads (and answers an expiry it
    /// observes without recording it) and refuses every write.
    CandidateLaunch,
}

/// The host clock a decision owner reads: wall-clock UNIX milliseconds.
///
/// Read once per call, after the call's reads, and passed to the generated
/// machine as `now_ms`. The default clock is trusted, not authenticated,
/// and not protected against rollback: an ended decision stays ended
/// whatever the clock later reads, and a deadline never moves.
pub trait DecisionClock: Send + Sync {
    /// The current time in UNIX milliseconds.
    ///
    /// # Errors
    /// [`DecisionClockError`] when the host cannot give a representable
    /// time.
    fn now_ms(&self) -> Result<u64, DecisionClockError>;
}

/// The host cannot give a representable time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("the decision clock is unavailable")]
pub struct DecisionClockError;

/// The host's wall clock.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemDecisionClock;

impl DecisionClock for SystemDecisionClock {
    fn now_ms(&self) -> Result<u64, DecisionClockError> {
        let elapsed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| DecisionClockError)?;
        u64::try_from(elapsed.as_millis()).map_err(|_| DecisionClockError)
    }
}
