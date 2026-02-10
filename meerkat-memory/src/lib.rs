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
    meerkat_contracts::CapabilityRegistration {
        id: meerkat_contracts::CapabilityId::MemoryStore,
        description: "HNSW semantic search + redb persistence (indexes compaction discards)",
        scope: meerkat_contracts::CapabilityScope::Universal,
        requires_feature: Some("memory-store"),
        prerequisites: &[],
        status_resolver: None,
    }
}
