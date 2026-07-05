//! meerkat-memory — Semantic memory store for Meerkat.
//!
//! Provides `HnswMemoryStore` for indexing discarded conversation history
//! during compaction, enabling session-scoped semantic search over compacted
//! history.
//!
//! Uses `hnsw_rs` for approximate nearest-neighbor search and SQLite for
//! metadata persistence.

pub mod hnsw;
pub mod simple;
pub mod tool;

pub use hnsw::{BagOfWordsEmbeddingModel, HnswMemoryStore, default_ranking_policy};
pub use simple::SimpleMemoryStore;
pub use tool::MemorySearchDispatcher;

// Skill registration
inventory::submit! {
    meerkat_skills::SkillRegistration {
        id: "memory-retrieval",
        name: "Memory Retrieval",
        description: "How semantic memory works with compaction and the MemoryStore trait",
        scope: meerkat_core::skills::SkillScope::Builtin,
        requires_capabilities: &["memory_store"],
        body: include_str!("../skills/memory-retrieval/SKILL.md"),
        extensions: &[],
    }
}

// Capability registration
inventory::submit! {
    meerkat_capabilities::CapabilityRegistration {
        id: meerkat_capabilities::CapabilityId::MemoryStore,
        description: "HNSW semantic search + SQLite persistence (indexes compaction discards)",
        scope: meerkat_capabilities::CapabilityScope::Universal,
        requires_feature: Some("memory-store"),
        prerequisites: &[],
        status_resolver: None,
    }
}
