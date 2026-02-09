//! meerkat-memory — Semantic memory store for Meerkat.
//!
//! Provides memory store implementations for indexing discarded conversation
//! history during compaction, enabling search across past sessions.

pub mod simple;

pub use simple::SimpleMemoryStore;
