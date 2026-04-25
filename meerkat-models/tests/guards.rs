//! Architectural guard tests for `meerkat-models`.
//!
//! These tests protect invariants that would otherwise only be caught in
//! review: the blast-radius "abort condition" for the per-model capability
//! refactor, and the public field shape of [`meerkat_models::ModelProfile`]
//! that is mirrored by `meerkat-contracts::WireModelProfile`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;
use std::path::PathBuf;

/// State-machine / runtime crates that MUST NOT depend on `meerkat-models`.
///
/// The per-model capability refactor was scoped to keep state-machine and
/// runtime crates untouched. If any of these gains a dependency on
/// `meerkat-models`, the refactor's abort condition is violated — either the
/// dep is wrong, or a larger-scoped PR is required.
const FORBIDDEN_CONSUMERS: &[&str] = &[
    "meerkat-runtime",
    "meerkat-machine-schema",
    "meerkat-machine-kernels",
    "meerkat-machine-codegen",
];

fn workspace_root() -> PathBuf {
    // `CARGO_MANIFEST_DIR` points at `meerkat-models/`; the workspace root
    // is one level up.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("meerkat-models must have a parent workspace dir")
        .to_path_buf()
}

#[test]
fn blast_radius_state_machines_and_runtime() {
    let root = workspace_root();
    for crate_name in FORBIDDEN_CONSUMERS {
        let toml_path = root.join(crate_name).join("Cargo.toml");
        let toml = std::fs::read_to_string(&toml_path)
            .unwrap_or_else(|e| panic!("failed to read {}: {e}", toml_path.display()));
        assert!(
            !toml.contains("meerkat-models"),
            "'{crate_name}' must not depend on 'meerkat-models' — the capability \
             refactor's abort condition forbids state-machine / runtime crates \
             from importing model-catalog types. See PR #258 / the refactor plan. \
             Either remove the dep, or split the change into a separate PR that \
             explicitly expands the abort-condition scope."
        );
    }
}

/// Fields that `ModelProfile` must expose, mirrored by `WireModelProfile` in
/// `meerkat-contracts::wire::models`. Adding a field here is a wire-contract
/// change that needs parallel updates in `meerkat-contracts` and a schema
/// regeneration.
const EXPECTED_MODEL_PROFILE_FIELDS: &[&str] = &[
    "provider",
    "model_family",
    "supports_temperature",
    "supports_thinking",
    "supports_reasoning",
    "supports_web_search",
    "inline_video",
    "vision",
    "image_tool_results",
    "realtime",
    "params_schema",
    "call_timeout_secs",
    "beta_headers",
];

#[test]
fn model_profile_wire_field_parity() {
    let schema = schemars::schema_for!(meerkat_models::ModelProfile);
    let value = serde_json::to_value(&schema).expect("schema serializes to JSON");
    let props = value
        .get("properties")
        .and_then(|p| p.as_object())
        .expect("ModelProfile schema has a properties map")
        .keys()
        .cloned()
        .collect::<BTreeSet<_>>();
    let expected: BTreeSet<String> = EXPECTED_MODEL_PROFILE_FIELDS
        .iter()
        .map(|s| (*s).to_string())
        .collect();
    assert_eq!(
        props, expected,
        "ModelProfile field set drift — update meerkat-contracts::WireModelProfile \
         in lockstep, regenerate artifact schemas (make regen-schemas), and update \
         EXPECTED_MODEL_PROFILE_FIELDS if this change is intentional."
    );
}
