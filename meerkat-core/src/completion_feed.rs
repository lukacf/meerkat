//! Canonical completion delivery seam for async operation terminals.
//!
//! The [`CompletionFeed`] trait provides a monotonic, append-only read handle
//! over operation terminal events. The runtime registry is the sole writer;
//! consumers (agent boundary, idle wake, wait tool) are read-only.
//!
//! This replaces the prior parallel-truth paths (shell job projection,
//! detached wake booleans, poll_external_updates background_completions).

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use crate::ops::OperationId;
use crate::ops_lifecycle::{OperationKind, OperationTerminalOutcome};

/// Monotonic sequence number assigned to each completion event.
pub type CompletionSeq = u64;

/// A single completion event in the feed.
#[derive(Debug, Clone)]
pub struct CompletionEntry {
    pub seq: CompletionSeq,
    pub operation_id: OperationId,
    pub kind: OperationKind,
    pub display_name: String,
    pub terminal_outcome: OperationTerminalOutcome,
    pub completed_at_ms: Option<u64>,
}

/// A batch of completion entries with an atomically-captured watermark.
///
/// The `watermark` is captured at the same instant as the entry snapshot,
/// ensuring no completions land between the read and the watermark check.
#[derive(Debug, Clone)]
pub struct CompletionBatch {
    pub entries: Vec<CompletionEntry>,
    /// Feed watermark at the time entries were read.
    pub watermark: CompletionSeq,
}

/// Read-only handle to the canonical completion feed.
///
/// Consumers call [`list_since`](CompletionFeed::list_since) to drain entries
/// past a local cursor, and [`wait_for_advance`](CompletionFeed::wait_for_advance)
/// to block until the watermark advances past a given sequence.
pub trait CompletionFeed: Send + Sync + std::fmt::Debug {
    /// Current feed watermark (highest seq written).
    fn watermark(&self) -> CompletionSeq;

    /// Return all entries with `seq > after_seq`, plus the current watermark.
    ///
    /// The watermark in the returned batch is captured atomically with the
    /// entry snapshot — no entry can land between the snapshot and the
    /// watermark read.
    fn list_since(&self, after_seq: CompletionSeq) -> CompletionBatch;

    /// Block until the feed watermark advances past `after_seq`.
    ///
    /// Returns the new watermark. If the watermark is already past `after_seq`,
    /// returns immediately.
    fn wait_for_advance(
        &self,
        after_seq: CompletionSeq,
    ) -> Pin<Box<dyn Future<Output = CompletionSeq> + Send + '_>>;
}

// ---------------------------------------------------------------------------
// Enrichment provider for agent-boundary completion projection
// ---------------------------------------------------------------------------

/// Shell-level enrichment for a completed operation.
///
/// Carries display details (job ID, detail string) that the agent boundary
/// needs for `BackgroundJobCompleted` events. These come from the shell
/// `JobManager`, not from the feed itself.
#[derive(Debug, Clone)]
pub struct CompletionEnrichmentData {
    pub job_id: String,
    pub detail: String,
}

/// Provider of shell-level enrichment for completed operations.
///
/// The shell `JobManager` implements this trait. The agent boundary
/// calls [`enrich`](CompletionEnrichmentProvider::enrich) to look up
/// display details by operation ID.
pub trait CompletionEnrichmentProvider: Send + Sync {
    fn enrich(&self, operation_id: &OperationId) -> Option<CompletionEnrichmentData>;
}

// ---------------------------------------------------------------------------
// Arc delegation
// ---------------------------------------------------------------------------

impl<T: CompletionFeed + ?Sized> CompletionFeed for Arc<T> {
    fn watermark(&self) -> CompletionSeq {
        (**self).watermark()
    }

    fn list_since(&self, after_seq: CompletionSeq) -> CompletionBatch {
        (**self).list_since(after_seq)
    }

    fn wait_for_advance(
        &self,
        after_seq: CompletionSeq,
    ) -> Pin<Box<dyn Future<Output = CompletionSeq> + Send + '_>> {
        (**self).wait_for_advance(after_seq)
    }
}
