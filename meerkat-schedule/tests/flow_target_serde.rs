//! Regression: `MobTargetBinding::Flow` params must survive a serde
//! round-trip through the internally-tagged `TargetBinding` (the shape
//! persisted in schedule/occurrence store rows). Prior to the canonicalizing
//! `FlowParams` carrier, the raw `Box<RawValue>` field failed to deserialize
//! through serde's internally-tagged content buffering, bricking every
//! persisted Flow-target row on read.

use meerkat_schedule::{FlowParams, MobTargetBinding, TargetBinding};

fn flow_binding(params: &str) -> TargetBinding {
    TargetBinding::Mob(Box::new(MobTargetBinding::Flow {
        mob_id: "mob-1".to_string(),
        flow_id: "flow-1".to_string(),
        params: FlowParams::parse(params).expect("valid flow params"),
    }))
}

#[test]
fn flow_target_binding_serde_round_trips() {
    let binding = flow_binding(r#"{"a":1}"#);
    let json = serde_json::to_string(&binding).expect("serialize flow target");
    let parsed: TargetBinding = serde_json::from_str(&json).expect("deserialize flow target");
    assert_eq!(binding, parsed);
}

#[test]
fn flow_target_binding_reads_old_binary_rows_with_noncanonical_params() {
    // Rows written by older binaries carry the host's original params
    // formatting; ingress must canonicalize rather than reject.
    let row = r#"{"target_kind":"mob","type":"flow","mob_id":"mob-1","flow_id":"flow-1","params":{ "a" :  1 }}"#;
    let parsed: TargetBinding = serde_json::from_str(row).expect("deserialize old-format row");
    assert_eq!(parsed, flow_binding(r#"{"a":1}"#));
}

#[test]
fn flow_target_binding_preserves_null_params() {
    // The Flow contract has always allowed JSON null as an explicit
    // "no parameters" payload; canonicalization must preserve it.
    let binding = flow_binding("null");
    let json = serde_json::to_string(&binding).expect("serialize null-params flow target");
    let parsed: TargetBinding = serde_json::from_str(&json).expect("deserialize null-params");
    assert_eq!(binding, parsed);
}

#[test]
fn flow_target_binding_stable_key_is_formatting_independent() {
    let compact = flow_binding(r#"{"a":1,"b":[2,3]}"#);
    let spaced = flow_binding(r#"{ "a" : 1 , "b" : [ 2 , 3 ] }"#);
    assert_eq!(compact.stable_key(), spaced.stable_key());
}
