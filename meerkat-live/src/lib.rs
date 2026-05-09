//! meerkat-live — Live audio/text transport for Meerkat.
//!
//! Composable WebSocket bridge between browser/test clients and
//! `LiveAdapterHost`. Any Meerkat surface (CLI, RPC, REST, MCP) can
//! mount the axum router or start a standalone listener.
//!
//! Frame protocol: client sends `LiveInputChunk` JSON, receives
//! `LiveAdapterObservation` JSON. Token-based channel auth.

pub mod host;
pub mod transport;

pub use host::{
    LiveAdapterHost, LiveAdapterHostError, LiveChannelId, LiveProjectionError, LiveProjectionSink,
    LiveTranscriptIdentity, ObservationOutcome, ObservationRouting, ToolDispatchSkipReason,
};
pub use transport::{LIVE_WS_PATH, LiveWsState, live_ws_router, serve_live_ws_listener};

// E26 regression: `meerkat-live` must not depend on `meerkat-runtime`. The
// dependency direction is: `meerkat-live` owns the live-adapter host
// (`crate::host`) and the transport (`crate::transport`); surfaces compose
// the two with their own session-runtime backing. A previous design had
// `LiveAdapterHost` living in `meerkat-runtime` and `meerkat-live`
// depending on it — wrong direction (transport pulling runtime). This
// inline test parses `Cargo.toml` at compile time and asserts the
// `meerkat-runtime` line is absent. Anyone who re-introduces the dep will
// fail this test before E26 silently regresses.
#[cfg(test)]
mod e26_dependency_direction {
    /// E26: `meerkat-live` must not list `meerkat-runtime` as a dependency.
    /// We read `Cargo.toml` via `include_str!` (compile-time embed, no I/O
    /// at test runtime) and assert the line is absent. This catches both
    /// `meerkat-runtime = { workspace = true }` and the older
    /// `meerkat-runtime = "..."` shapes; we simply look for the bare
    /// crate name as a TOML key.
    #[test]
    fn cargo_toml_does_not_depend_on_meerkat_runtime() {
        let cargo_toml = include_str!("../Cargo.toml");
        // Per-line scan so we don't trip on the word in a comment (none
        // present today, but futureproof against documentation drift).
        for line in cargo_toml.lines() {
            let trimmed = line.trim_start();
            assert!(
                !trimmed.starts_with("meerkat-runtime"),
                "E26 regression: meerkat-live must not depend on meerkat-runtime; \
                 found Cargo.toml line: {line}"
            );
        }
    }
}
