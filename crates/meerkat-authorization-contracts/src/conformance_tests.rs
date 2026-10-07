//! Mechanical fixture decoding for the shared language-independent JSON corpus.
//! Expected results are read literally, never computed by the implementation.

#![allow(clippy::expect_used)]

use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::constraints::{
    DelegationDepth, ExactRestriction, ExecutionRestrictions, LifetimeRestriction,
};

pub(crate) fn corpus() -> Value {
    let corpus: Value = serde_json::from_str(include_str!("../conformance/v1/restrictions.json"))
        .expect("versioned shared conformance JSON");
    assert_eq!(corpus["schema_version"], 1);
    assert_eq!(corpus["contract_version"], json!({"major": 1, "minor": 0}));
    corpus
}

pub(crate) fn decode<T: DeserializeOwned>(value: &Value) -> T {
    serde_json::from_value(value.clone()).expect("typed conformance fixture")
}

pub(crate) fn entries(value: &Value) -> &[Value] {
    value.as_array().expect("fixture array")
}

pub(crate) fn name(value: &Value) -> &str {
    value.as_str().expect("fixture exact string")
}

pub(crate) fn restriction<T: Ord + DeserializeOwned>(
    spec: &Value,
    atoms: &Value,
) -> ExactRestriction<T> {
    let kind = name(&spec["bound"]);
    let values: Vec<_> = entries(&spec["values"])
        .iter()
        .map(|id| atoms[name(id)].clone())
        .collect();
    let bound = if kind == "exact" {
        json!({"bound": "exact", "values": values})
    } else {
        assert!(values.is_empty(), "unrestricted has no listed exact values");
        json!({"bound": kind})
    };
    decode(&json!({"bound": bound, "unresolved": spec["unresolved"]}))
}

pub(crate) fn lifetime(spec: &Value) -> LifetimeRestriction {
    let kind = name(&spec["bound"]);
    let bound = if kind == "window" {
        json!({"bound": kind, "window": {"not_before_ms": spec["not_before_ms"], "expires_at_ms": spec["expires_at_ms"]}})
    } else {
        json!({"bound": kind})
    };
    decode(&json!({"bound": bound, "unresolved": spec["unresolved"]}))
}

pub(crate) fn depth(spec: &Value) -> DelegationDepth {
    let kind = name(&spec["bound"]);
    let bound = if kind == "remaining" {
        json!({"bound": kind, "edges": spec["remaining"]})
    } else {
        json!({"bound": kind})
    };
    decode(&json!({"bound": bound, "unresolved": spec["unresolved"]}))
}

pub(crate) fn execution(spec: &Value, corpus: &Value) -> ExecutionRestrictions {
    let atoms = &corpus["atoms"];
    ExecutionRestrictions {
        actions: restriction(&spec["actions"], &atoms["actions"]),
        resource_domains: restriction(&spec["resource_domains"], &atoms["resource_domains"]),
        processors: restriction(&spec["processors"], &atoms["processors"]),
        audiences: restriction(&spec["audiences"], &atoms["audiences"]),
        lifetime: lifetime(&spec["lifetime"]),
        delegation_depth: depth(&spec["delegation_depth"]),
    }
}
