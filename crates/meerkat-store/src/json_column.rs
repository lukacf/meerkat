//! Read boundary for UTF-8 JSON payload columns.
//!
//! The codec moved to `meerkat-sqlite` (the shared SQLite mechanics crate)
//! so stores below `meerkat-store` in the dependency order stop re-solving
//! the TEXT-vs-BLOB question; this module re-exports it so existing
//! `meerkat_store::json_column::JsonColumnBytes` imports keep working.

pub use meerkat_sqlite::json_column::JsonColumnBytes;
