//! Typed receipt of a host's compaction request
//! ([`crate::MeerkatMachine::request_compaction`]).
//!
//! A host asks a session's next compaction check to compact: the check that
//! precedes the next model request of the session's running turn, or of its
//! next run when it is idle, runs a compaction whatever the compactor's
//! trigger (its thresholds and cadence) would decide. The session is not
//! rebuilt and no threshold changes. MeerkatMachine records the request and
//! decides the receipt; the agent's compaction check consumes it.

/// What [`crate::MeerkatMachine::request_compaction`] recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum CompactionRequestReceipt {
    /// The session's registration now holds the request: its next compaction
    /// check compacts. A running turn keeps running, and its next check (once
    /// its tool results are in) consumes the request; a turn that makes no
    /// further model request leaves it to the first check of the session's
    /// next run. A check whose compaction finds nothing to fold (a run in
    /// flight keeps its own request and newest round) consumes the request
    /// and compacts nothing, and so does the session's first model request.
    Requested,
    /// The registration already held a request that no check consumed yet:
    /// nothing changed, and the next check compacts once.
    AlreadyRequested,
}

impl CompactionRequestReceipt {
    /// The wire outcome of this receipt (`session/compact`,
    /// `mob/compact_member`), shared by every surface.
    #[must_use]
    pub fn wire_outcome(self) -> meerkat_contracts::WireCompactionRequestOutcome {
        match self {
            Self::Requested => meerkat_contracts::WireCompactionRequestOutcome::Requested,
            Self::AlreadyRequested => {
                meerkat_contracts::WireCompactionRequestOutcome::AlreadyRequested
            }
        }
    }
}
