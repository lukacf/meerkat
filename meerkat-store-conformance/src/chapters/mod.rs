//! Conformance chapters.
//!
//! Each chapter is an async entry point taking store factories and returning
//! `Result<(), ConformanceFailure>` with chapter/step context. Chapters are
//! organized as per-trait capability profiles, not one universal matrix —
//! instantiate exactly the chapters a backend's declared capabilities
//! support:
//!
//! | chapter                                    | applies to |
//! |--------------------------------------------|------------|
//! | [`baseline`]                               | every `SessionStore` |
//! | [`incremental`]                            | stores whose `as_incremental` is `Some` |
//! | [`guarded_projection`]                     | stores implementing `save_authoritative_projection_if_current_revision` |
//! | [`append_only`]                            | every `SessionStore` (pins emulated-CAS semantics) |
//! | [`legacy_data`]                            | every `SessionStore` |
//! | [`assert_forwards_incremental`]            | `SessionStore`→`SessionStore` delegating wrappers |
//! | [`blobs`] / [`dangling_blob_reference`]    | `BlobStore` (+ a `SessionStore` for the dangling shape) |
//! | [`artifacts`]                              | `ArtifactStore` |

mod append_only;
mod artifacts;
mod baseline;
mod blobs;
mod capability;
mod guarded_projection;
mod incremental;
mod legacy_data;

pub use append_only::append_only;
pub use artifacts::artifacts;
pub use baseline::baseline;
pub use blobs::{blobs, dangling_blob_reference};
pub use capability::assert_forwards_incremental;
pub use guarded_projection::guarded_projection;
pub use incremental::incremental;
pub use legacy_data::legacy_data;
