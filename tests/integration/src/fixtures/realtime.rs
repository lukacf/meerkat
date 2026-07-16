//! Thin re-export shim over the facade-owned deterministic realtime
//! fixtures (ADJ-P6B-4): the shared home is
//! `meerkat::test_fixtures::realtime` (feature `test-realtime-fixtures`);
//! this crate re-exports it so integration rows import one local path.

pub use meerkat::test_fixtures::realtime::{
    ScriptedLiveAdapter, ScriptedRealtimeOpen, ScriptedRealtimeSessionFactory,
};
