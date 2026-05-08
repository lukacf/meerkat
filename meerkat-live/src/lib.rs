//! meerkat-live — Live audio/text transport for Meerkat.
//!
//! Composable WebSocket bridge between browser/test clients and
//! `LiveAdapterHost`. Any Meerkat surface (CLI, RPC, REST, MCP) can
//! mount the axum router or start a standalone listener.
//!
//! Frame protocol: client sends `LiveInputChunk` JSON, receives
//! `LiveAdapterObservation` JSON. Token-based channel auth.

pub mod transport;

pub use transport::{LIVE_WS_PATH, LiveWsState, live_ws_router, serve_live_ws_listener};
