//! Typed session-local transcript revision-history graph.
//!
//! Extracted verbatim from `session.rs`; the extraction commit changes no
//! behaviour, only where the code lives.

pub(crate) mod decode_memo;
pub(crate) mod graph;
pub(crate) mod heal;
pub(crate) mod sealed;
pub(crate) mod validate;

pub use graph::{
    TranscriptHistoryState, TranscriptRevisionBody, TranscriptRewriteCommit,
    TranscriptRewriteRecord,
};
pub use sealed::ValidatedTranscriptHistory;
