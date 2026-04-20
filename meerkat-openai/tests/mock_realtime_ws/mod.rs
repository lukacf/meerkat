//! Mock Realtime WS harness — deterministic replacement for live
//! OpenAI Realtime sessions in `e2e-fast`.
//!
//! See `README.md` in this directory for usage and design notes.

#![allow(dead_code, unused_imports)]

pub mod server;

pub use server::{
    RealtimeMockServer, RealtimeMockSession, RealtimeMockSessionFactory, ScriptedEvent,
    ScriptedScenario,
};
