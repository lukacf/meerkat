//! # meerkat-store-conformance
//!
//! Storage conformance harness for Meerkat store backends. Any
//! implementation of the `meerkat-core` storage traits — the in-repo SQLite/
//! JSONL/memory stores or a downstream remote backend (BigQuery, Postgres,
//! object stores) — runs the identical suite by supplying store factories.
//!
//! The suite is organized as **per-trait capability profiles**, not one
//! universal matrix (a realm legitimately composes a JSONL session store
//! with SQLite runtime state, disabled scheduling, and filesystem blobs).
//! Instantiate exactly the chapters a backend's declared capabilities
//! support:
//!
//! - [`chapters::baseline`] — every `SessionStore`: save/load round-trips,
//!   list/delete, `delete_if_current_revision` guard semantics,
//!   checkpoint-stamp preservation, append-only save-guard enforcement,
//!   concurrent-writer contention, multi-MB payloads.
//! - [`chapters::incremental`] — stores whose `as_incremental` returns
//!   `Some`: O(delta) `append_messages`, `commit_rewrite` CAS vs the head
//!   token, `save_head` `Create`/`IfToken` semantics,
//!   `TranscriptRevisionConflict` on mismatch, `load_messages`/
//!   `load_rewrites` round-trips.
//! - [`chapters::transcript_rewrite`] — stores implementing
//!   `save_transcript_rewrite` (the whole-blob compaction path): the proven
//!   rewrite round-trips and survives reopen, an unproven rewrite (plain
//!   save of the rewritten document) is refused, a stale proof conflicts.
//! - [`chapters::guarded_projection`] — stores implementing
//!   `save_authoritative_projection_if_current_revision`: CAS semantics,
//!   revision-conflict behavior, and the non-atomic projection-vs-authority
//!   recovery protocol (rollback via guarded save; quarantine via
//!   `delete_if_current_revision`).
//! - [`chapters::append_only`] — every `SessionStore`: pins what a revision
//!   guard MEANS for emulated-CAS backends (superseded-sibling-row
//!   deduplication ownership; checkpoint monotonicity across generation
//!   rebinds). The in-crate [`EmulatedCasSessionStore`] passes this chapter
//!   and documents the contract.
//! - [`chapters::assert_forwards_incremental`] — `SessionStore` →
//!   `SessionStore` delegating wrappers: fails loudly when a wrapper
//!   swallows a `Some(as_incremental)` from its inner store, and proves the
//!   forwarded capability shares storage identity with the inner store
//!   (writes through either side are served by the other). Reference
//!   wrappers: [`ForwardingSessionStore`] (correct) and
//!   [`SwallowingSessionStore`] (the bug class).
//! - [`chapters::legacy_data`] — every `SessionStore`: byte-literal 0.7.x
//!   fixture documents (current-envelope, unstamped, legacy string-content
//!   form) installed via
//!   [`SessionStoreFactory::install_session_document`] load and report
//!   `LegacyUnverified`, documents round-trip preserved against the fixture
//!   bytes, adopted (stamped) sessions stay `Verified`.
//! - [`chapters::blobs`] / [`chapters::dangling_blob_reference`] /
//!   [`chapters::artifacts`] — `BlobStore` and `ArtifactStore`: content
//!   round-trips, restart survival when `is_persistent()`, delete/exists
//!   honesty, and the dangling session→blob reference shape (`NotFound`,
//!   never silent success).
//!
//! Chapters contain **no provider-specific code**: everything is expressed
//! through the `meerkat-core` trait contracts, so a downstream backend runs
//! the same assertions CI runs against the in-repo stores.
//!
//! # wasm32 support
//!
//! The crate compiles for `wasm32-unknown-unknown`: the factory traits and
//! reference stores follow the same `async_trait(?Send)` relaxation as the
//! `meerkat-core` store traits, tokio is a native-only dependency, and the
//! in-crate reference store uses a `std` mutex. One honest gap remains: the
//! baseline chapter's multi-task writer-contention step is native-only
//! (wasm32 has no multi-threaded task spawn), so real write-race coverage
//! for a wasm-hosted backend must come from a native run against the same
//! backend.

pub mod chapters;
mod emulated_cas;
mod factory;
mod failure;
pub mod fixtures;
mod wrappers;

pub use emulated_cas::EmulatedCasSessionStore;
pub use factory::{
    ArtifactStoreFactory, BlobStoreFactory, FnArtifactStoreFactory, FnBlobStoreFactory,
    FnSessionStoreFactory, SessionStoreFactory,
};
pub use failure::ConformanceFailure;
pub use wrappers::{ForwardingSessionStore, SwallowingSessionStore};

#[cfg(all(test, not(target_arch = "wasm32")))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use std::sync::Arc;

    use meerkat_core::SessionStore;

    use super::*;

    struct SharedEmulatedCas {
        store: Arc<EmulatedCasSessionStore>,
    }

    impl SharedEmulatedCas {
        fn correct() -> Self {
            Self {
                store: Arc::new(EmulatedCasSessionStore::new()),
            }
        }

        fn naive() -> Self {
            Self {
                store: Arc::new(EmulatedCasSessionStore::new_naive()),
            }
        }
    }

    #[async_trait::async_trait]
    impl SessionStoreFactory for SharedEmulatedCas {
        async fn open(&self) -> Result<Arc<dyn SessionStore>, ConformanceFailure> {
            // In-memory reference store: reopen shares state.
            Ok(Arc::clone(&self.store) as Arc<dyn SessionStore>)
        }
    }

    #[tokio::test]
    async fn emulated_cas_reference_store_passes_baseline() {
        chapters::baseline(&SharedEmulatedCas::correct())
            .await
            .expect("reference emulated-CAS store must satisfy the baseline chapter");
    }

    #[tokio::test]
    async fn emulated_cas_reference_store_passes_guarded_projection() {
        chapters::guarded_projection(&SharedEmulatedCas::correct())
            .await
            .expect("reference emulated-CAS store must satisfy the guarded-projection chapter");
    }

    #[tokio::test]
    async fn emulated_cas_reference_store_passes_append_only() {
        chapters::append_only(&SharedEmulatedCas::correct())
            .await
            .expect("reference emulated-CAS store must satisfy the append-only chapter");
    }

    #[tokio::test]
    async fn emulated_cas_reference_store_passes_legacy_data() {
        chapters::legacy_data(&SharedEmulatedCas::correct())
            .await
            .expect("reference emulated-CAS store must satisfy the legacy-data chapter");
    }

    #[tokio::test]
    async fn emulated_cas_reference_store_passes_transcript_rewrite() {
        chapters::transcript_rewrite(&SharedEmulatedCas::correct())
            .await
            .expect("reference emulated-CAS store must satisfy the transcript-rewrite chapter");
    }

    /// The harness canary: a store whose windowed read adopts orphan
    /// (uncommitted) sibling rows as current — the ob3 zombie incident —
    /// must FAIL the append-only chapter, at the orphan step.
    #[tokio::test]
    async fn naive_orphan_visible_store_fails_append_only() {
        let failure = chapters::append_only(&SharedEmulatedCas::naive())
            .await
            .expect_err("the append-only chapter must detect orphan-sibling adoption");
        assert_eq!(failure.chapter(), "append_only");
        assert_eq!(failure.step(), "orphan_siblings_never_wedge_guarded_saves");
    }

    /// The incremental chapter fails loudly (not vacuously) when invoked for
    /// a store without the capability.
    #[tokio::test]
    async fn incremental_chapter_refuses_non_incremental_store() {
        let failure = chapters::incremental(&SharedEmulatedCas::correct())
            .await
            .expect_err("incremental profile must refuse a whole-blob store");
        assert_eq!(failure.chapter(), "incremental");
        assert_eq!(failure.step(), "capability_probe");
    }

    /// The swallow test fails loudly when the inner store has no capability
    /// to forward (vacuous invocation).
    #[tokio::test]
    async fn swallow_test_refuses_vacuous_inner_store() {
        let inner: Arc<dyn SessionStore> = Arc::new(EmulatedCasSessionStore::new());
        let failure = chapters::assert_forwards_incremental(inner, ForwardingSessionStore::wrap)
            .await
            .expect_err("a non-incremental inner store must be refused, not passed vacuously");
        assert_eq!(failure.step(), "inner_capability_present");
    }
}
