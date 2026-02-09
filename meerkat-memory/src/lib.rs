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
