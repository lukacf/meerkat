//! meerkat-memory — Semantic memory store for Meerkat.
//!
//! Provides `HnswMemoryStore` for indexing discarded conversation history
//! during compaction, enabling semantic search across past sessions.
//!
//! Uses `hnsw_rs` for approximate nearest-neighbor search and `redb` for
//! metadata persistence.

pub mod hnsw;
pub mod simple;

pub use hnsw::HnswMemoryStore;
pub use simple::SimpleMemoryStore;

// Capability registration
inventory::submit! {
    meerkat_contracts::CapabilityRegistration {
        id: meerkat_contracts::CapabilityId::MemoryStore,
        description: "HNSW semantic search + redb persistence (indexes compaction discards)",
        scope: meerkat_contracts::CapabilityScope::Universal,
        requires_feature: Some("memory-store"),
        prerequisites: &[],
    }
}
