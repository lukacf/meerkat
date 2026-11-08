//! What a cold settle of one session's owed compaction projections did
//! (`PersistentSessionService::settle_owed_compaction_projections`).
//!
//! A compaction's transcript rewrite and its semantic-memory projection are a
//! pair. The runtime boundary that commits the rewrite inserts a pending row
//! in the runtime compaction outbox, and the memory batch waits staged and
//! invisible until a finalization publishes it. A live agent finalizes at its
//! next boundary, or at its startup reconciliation when it is revived. A
//! session that is never revived (a dormant member that is rotated or
//! deleted) is finalized cold instead: the outbox row is the commit
//! authority, and the finalized batch is answered to the caller, so a host
//! can harvest its discards before the session ends.

use meerkat_core::memory::{CompactionProjectionId, MemoryRecord};

/// What a cold settle of one session's owed compaction projections did.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum OwedCompactionSettlement {
    /// Nothing owed: the runtime's compaction outbox holds no pending row
    /// for the session. Nothing changed.
    NothingOwed,
    /// Every owed projection is finalized and acknowledged, in outbox order.
    Settled {
        projections: Vec<SettledCompactionProjection>,
    },
    /// The session is hosted in this process (a live agent or a runtime
    /// registration): its live agent owns the finalization, at its next
    /// boundary or its startup reconciliation. Nothing changed.
    Live,
}

/// One owed compaction projection a cold settle finalized and acknowledged.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct SettledCompactionProjection {
    /// The projection, as the outbox row names it.
    pub projection: CompactionProjectionId,
    /// The intent's summary facts: the fields a live finalization's
    /// `CompactionCompleted` carries.
    pub summary_tokens: u64,
    pub messages_before: usize,
    pub messages_after: usize,
    /// The records the finalization published (content and source range),
    /// in stage order: what a scoped enumeration of the session's memory
    /// then reads for this batch. Empty when the batch was published before
    /// this settle (an earlier settle or finalization that did not reach
    /// its acknowledgement) or the session's memory scope was dropped.
    pub entries: Vec<MemoryRecord>,
}

impl SettledCompactionProjection {
    /// One settled projection with its summary facts and published records.
    #[must_use]
    pub fn new(
        projection: CompactionProjectionId,
        summary_tokens: u64,
        messages_before: usize,
        messages_after: usize,
        entries: Vec<MemoryRecord>,
    ) -> Self {
        Self {
            projection,
            summary_tokens,
            messages_before,
            messages_after,
            entries,
        }
    }
}
