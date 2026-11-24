//! # meta-store-conformance
//!
//! Storage conformance harness for Meta store backends. Any implementation
//! of Meta's storage traits — the bundled SQLite/in-memory stores or a
//! downstream remote backend (BigQuery, Postgres, object stores) — runs the
//! identical suite by supplying store factories, exactly like the upstream
//! `meerkat-store-conformance` harness this crate extends.
//!
//! The suite is organized as **per-trait capability profiles**:
//!
//! - [`chapters::event_log`] — the store-facing `EventLogStore` contract:
//!   `append_batch` idempotency under redelivery, including the compound
//!   post-failure batch shape and the post-cap suffix shape the Meta
//!   flusher produces, plus `after_seq` cursor semantics.
//! - [`chapters::console_log`] — `ConsoleLogStore`: `append_if_absent`
//!   idempotency on one handle, the per-handle watermark cache contract
//!   (reopen sees the last durably written watermark; concurrent cross-handle
//!   visibility is NOT contractual), windowed-query pagination under
//!   concurrent append.
//! - [`chapters::console_log_mobs`] - `ConsoleLogStore` on a host that
//!   creates a mob beside its default mob: `remove_frames_of_mob` (one mob's
//!   frames go, every other frame keeps its cursor, the default mob's are
//!   never removed) and the read of one mob's frames as `reads_one_mob`
//!   states it; and the window of `unsettled_send_frames`, which is the
//!   default mob's own frames (another mob's frames are never counted in
//!   it and never move the default mob's send out of it); and the six
//!   operations on a send that name its mob (`update_frame_status_of_mob`,
//!   `transition_send_frame_of_mob`, `take_send_frame_hold_of_mob`,
//!   `frame_by_dedupe_key_of_mob`, `send_frame_by_interaction_of_mob`,
//!   `unsettled_send_frames_of_mob`), each reaching the send of the mob it
//!   names and no other. A store that keeps the trait's default
//!   `remove_frames_of_mob` fails it, as such a host refuses every create
//!   of a second mob over that store; so does a store that reads one mob's
//!   frames and keeps the defaults of the six, which refuse another mob.
//! - [`chapters::binary_blobs`] / [`chapters::legacy_blob_layout`] —
//!   `BinaryBlobStore`: content-address round-trips, `is_persistent()`
//!   honesty, typed dangling-reference `NotFound`, and the legacy
//!   `<root>/<2hex>/<sha>.json` filesystem read fallback.
//! - [`chapters::agent_memory`] — `AgentMemoryProvider`: every `supports_*`
//!   capability flag backed by a behavioral test; a provider advertising a
//!   capability must pass its operation, one refusing must return the typed
//!   `Unsupported` error.
//! - [`chapters::mob_references`] / [`chapters::mob_id_holders`] - the
//!   host-facing mob id contract of an `ExtensionDocumentStore`: whether live
//!   documents name a mob id, and which mob holds it. A store that keeps the
//!   trait's default holder methods fails `mob_id_holders`, as a host treats
//!   their error as a quarantine of every restored mob whose id documents
//!   name.
//! - [`chapters::legacy_memory_database`] - the legacy-data axis: a
//!   pre-ledger memory database (below the mobkit 0.8.8 floor) must be
//!   refused typed at open, with its fabricated rows left untouched; the
//!   0.8.11 reset retired silent pre-floor convergence.
//!
//! Failures are reported through the upstream
//! [`ConformanceFailure`] type (re-exported here) so downstream CI treats
//! Meerkat and Meta conformance output identically.

pub mod chapters;
mod factory;
pub mod fixtures;
mod reference;
mod steps;

pub use factory::{
    AgentMemoryProviderFactory, BinaryBlobStoreFactory, ConsoleLogStoreFactory,
    EventLogStoreFactory, ExtensionDocumentStoreFactory,
};
pub use meerkat_store_conformance::ConformanceFailure;
pub use reference::{
    ReferenceEventLogStore, ReferenceInMemoryAgentMemoryStore, ReferenceMemoryBundleProvider,
    ReferenceMemoryRealmProvider,
};
