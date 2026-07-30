//! Typed session-local transcript revision-history graph.
//!
//! Extracted verbatim from `session.rs`; the extraction commit changes no
//! behaviour, only where the code lives.

mod audit_receipt;
pub(crate) mod graph;
pub(crate) mod heal;
pub(crate) mod sealed;
pub(crate) mod validate;

pub use audit_receipt::TranscriptRewriteAuditReceiptBatch;
pub use graph::{
    TRANSCRIPT_HISTORY_FORMAT_CURRENT, TranscriptEndpointWitness, TranscriptGraphPrefixAccumulator,
    TranscriptHistoryState, TranscriptParentAdvance, TranscriptRevisionBody,
    TranscriptRevisionEdge, TranscriptRewriteCommit, TranscriptRewriteParentTransition,
    TranscriptRewritePatch, TranscriptRewritePrefixAccumulator, TranscriptRewriteRecord,
    extend_transcript_rewrite_prefix_accumulator, transcript_history_full_body_materializations,
    transcript_rewrite_prefix_digest,
};
pub use sealed::{ValidatedTranscriptHistory, ValidatedTranscriptRewriteSuffix};
