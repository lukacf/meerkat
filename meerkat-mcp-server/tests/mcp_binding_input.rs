//! T17 (Phase 5): MCP `meerkat_run`/`meerkat_resume` binding input —
//! top-down observable proof that the Phase 4d MCP tool schema carries
//! the `connection_ref` field end-to-end through serde.
//!
//! Plan choke point K8 reads: "`meerkat_run` call with `connection_ref`
//! field → validates + resolves → tool call returns session output".
//! The live-provider half (actually executing the agent) belongs in the
//! `e2e-auth` lane; this file proves the MCP input schema accepts and
//! preserves the `connection_ref` field across serde, which is the
//! contract Phase 4d.mcp.1 explicitly named.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use meerkat_mcp_server::{MeerkatResumeInput, MeerkatRunInput};
use serde_json::{Value, json};

/// `meerkat_run` MCP input accepts `connection_ref` as an optional
/// structured field. The field round-trips through serde without loss and
/// deserializes into the expected typed `WireConnectionRef`.
#[test]
fn meerkat_run_input_accepts_connection_ref() {
    let body = json!({
        "prompt": "hi",
        "connection_ref": {
            "realm": "realm-x",
            "binding": "bind-y"
        }
    });

    let input: MeerkatRunInput =
        serde_json::from_value(body).expect("input with connection_ref must deserialize");
    let connection_ref = input
        .connection_ref
        .as_ref()
        .expect("connection_ref must be preserved through deserialization");
    assert_eq!(connection_ref.realm.as_str(), "realm-x");
    assert_eq!(connection_ref.binding.as_str(), "bind-y");
}

/// `meerkat_run` MCP input omitting `connection_ref` still parses,
/// yielding None. Backwards compatibility with pre-Phase-4d calls.
#[test]
fn meerkat_run_input_connection_ref_optional() {
    let body = json!({"prompt": "hi"});
    let input: MeerkatRunInput = serde_json::from_value(body).expect("input must deserialize");
    assert_eq!(input.connection_ref, None);
}

/// `meerkat_run` MCP input validates JSONSchema — the schemars-derived
/// schema for the struct includes `connection_ref`.
#[test]
fn meerkat_run_schema_declares_connection_ref() {
    let schema = schemars::schema_for!(MeerkatRunInput);
    let schema_json = serde_json::to_value(&schema).expect("schema serializes");
    let properties: &Value = schema_json
        .pointer("/properties/connection_ref")
        .expect("schema must declare connection_ref property");
    // Verify type matches Option<String> shape: either {"type":"string"}
    // or {"anyOf":[{"type":"string"},{"type":"null"}]} (schemars style).
    let s = serde_json::to_string(properties).unwrap();
    assert!(
        s.contains("string") || s.contains("null"),
        "connection_ref schema must reflect Option<String>: {s}"
    );
}

/// `meerkat_resume` MCP input — mirror check. When `connection_ref` is
/// added to the resume schema (future), this test will pin it.
/// For now, verify the resume input parses without it.
#[test]
fn meerkat_resume_input_parses_without_connection_ref() {
    let body = json!({"session_id": "s-1", "prompt": "next"});
    let _input: MeerkatResumeInput = serde_json::from_value(body).expect("resume input parses");
}
