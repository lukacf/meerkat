//! Body-free observation of a session's durable transcript source.
//!
//! Live readiness asks one question of the store: does this canonical session
//! have a current committed document that a live executor may later read? The
//! answer comes from the session authority row, the RuntimeStore catalog
//! entry, and the machine lifecycle row. It never reads, decodes, or hashes
//! the session body, and it takes neither the recovery gate nor the
//! turn-finalization boundary, so a member mid-turn or a slow body cannot stall
//! it. The actual live open still validates the body through its own path.

/// Body-free classification of a session's durable transcript source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum LiveDurableSourceObservation {
    /// A current, non-archived session document is committed.
    ///
    /// `revision` is the store-issued revision of the committed session
    /// authority. It is `None` only for backends whose sessions live in
    /// process memory and therefore issue no store revision.
    Committed { revision: Option<u64> },
    /// The store holds an absorbing archive terminal for the session: the
    /// catalog entry's lifecycle terminal, or a retired/destroyed lifecycle
    /// row.
    Archived,
    /// No current durable session authority exists for the session.
    Absent,
}

impl LiveDurableSourceObservation {
    /// Whether the observation admits the session as a live durable source.
    #[must_use]
    pub const fn is_committed(self) -> bool {
        matches!(self, Self::Committed { .. })
    }
}
