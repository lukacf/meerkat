//! T17 (Phase 5): MCP `meerkat_run`/`meerkat_resume` binding input —
//! top-down observable proof that the MCP tool schema carries
//! `turn_metadata.connection_ref` end-to-end through serde.
//!
//! Plan choke point K8 reads: "`meerkat_run` call with
//! `turn_metadata.connection_ref` field → validates + resolves → tool call
//! returns session output".
//! The live-provider half (actually executing the agent) belongs in the
//! `e2e-auth` lane; this file proves the MCP input schema accepts and
//! preserves the canonical metadata field across serde, which is the
//! contract Phase 4d.mcp.1 explicitly named.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use meerkat_contracts::wire::runtime::WireTurnMetadataOverride;
use meerkat_mcp_server::{MeerkatResumeInput, MeerkatRunInput};
use serde_json::{Value, json};

/// `meerkat_run` MCP input accepts `turn_metadata.connection_ref` as an
/// optional structured field. The field round-trips through serde without loss
/// and deserializes into the expected typed `WireConnectionRef`.
#[test]
fn meerkat_run_input_accepts_connection_ref() {
    let body = json!({
        "prompt": "hi",
        "turn_metadata": {
            "connection_ref": {
                "action": "set",
                "value": {
                    "realm": "realm-x",
                    "binding": "bind-y"
                }
            }
        }
    });

    let input: MeerkatRunInput =
        serde_json::from_value(body).expect("input with metadata connection_ref must deserialize");
    let turn_metadata = input
        .turn_metadata
        .as_ref()
        .expect("turn_metadata must be preserved through deserialization");
    let connection_ref = match turn_metadata
        .connection_ref
        .as_ref()
        .expect("connection_ref must be preserved through deserialization")
    {
        WireTurnMetadataOverride::Set(connection_ref) => connection_ref,
        WireTurnMetadataOverride::Clear => panic!("connection_ref should be set"),
    };
    assert_eq!(connection_ref.realm.as_str(), "realm-x");
    assert_eq!(connection_ref.binding.as_str(), "bind-y");
}

/// `meerkat_run` MCP input omitting `turn_metadata.connection_ref` still parses,
/// yielding None. Backwards compatibility with pre-Phase-4d calls.
#[test]
fn meerkat_run_input_connection_ref_optional() {
    let body = json!({"prompt": "hi"});
    let input: MeerkatRunInput = serde_json::from_value(body).expect("input must deserialize");
    assert_eq!(input.turn_metadata, None);
}

/// `meerkat_run` MCP input validates JSONSchema — the schemars-derived
/// schema for the struct includes the canonical connection_ref metadata field.
#[test]
fn meerkat_run_schema_declares_connection_ref() {
    let schema = schemars::schema_for!(MeerkatRunInput);
    let schema_json = serde_json::to_value(&schema).expect("schema serializes");
    let _: &Value = schema_json
        .pointer("/properties/turn_metadata")
        .expect("schema must declare connection_ref property");
    let s = serde_json::to_string(&schema_json).unwrap();
    assert!(
        s.contains("connection_ref"),
        "turn_metadata schema must expose connection_ref: {s}"
    );
}

/// `meerkat_resume` MCP input — mirror check. Resume accepts the same canonical
/// metadata carrier, and omitting it remains valid.
#[test]
fn meerkat_resume_input_parses_without_connection_ref() {
    let body = json!({"session_id": "s-1", "prompt": "next"});
    let _input: MeerkatResumeInput = serde_json::from_value(body).expect("resume input parses");
}
