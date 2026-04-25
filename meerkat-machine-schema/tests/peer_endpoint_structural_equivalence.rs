#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    unused_imports
)]

//! Structural-equivalence tripwire for the two `PeerEndpoint` twins.
//!
//! Invariant: the schema catalog twin at
//! `meerkat-machine-schema/src/catalog/dsl/meerkat_machine.rs` and the
//! runtime-side twin at `meerkat-runtime/src/meerkat_machine/dsl.rs`
//! must stay in lockstep — identical field set with identical typed
//! shapes (`PeerName`, `PeerId`, `PeerAddress`, `PeerSigningKey`) — and both must expose
//! `From<&meerkat_core::comms::TrustedPeerDescriptor>` so the
//! runtime/comms seam can project trusted-peer descriptors into either
//! DSL without per-site coercion. Renames / additions / deletions on
//! one side fail this tripwire loudly.
//!
//! The structural check is done at source-text level because the two
//! crates sit on opposite sides of the dep DAG — pulling runtime into
//! the schema crate's test just for symbol introspection would invert
//! the dependency direction and is not justified for a structural
//! assertion. Text extraction is equivalent in intent: we read each
//! `pub struct PeerEndpoint` body, parse its field list, and compare.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

const RUNTIME_DSL: &str = "meerkat-runtime/src/meerkat_machine/dsl.rs";
const SCHEMA_CATALOG: &str = "meerkat-machine-schema/src/catalog/dsl/meerkat_machine.rs";

fn workspace_root() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    loop {
        let toml = p.join("Cargo.toml");
        if toml.exists() {
            let text = fs::read_to_string(&toml).unwrap_or_default();
            if text.contains("[workspace]") {
                return p;
            }
        }
        assert!(p.pop(), "could not locate workspace root");
    }
}

fn read_source(root: &Path, relative: &str) -> String {
    let path = root.join(relative);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("could not read {}: {e}", path.display()))
}

/// Extract the `{ ... }` body of `pub struct PeerEndpoint` and parse
/// its fields into a `name -> type` map. Panics with a diagnostic if
/// the struct is not present.
fn peer_endpoint_fields(label: &str, body: &str) -> BTreeMap<String, String> {
    let Some(head) = body.find("pub struct PeerEndpoint") else {
        panic!("{label}: `pub struct PeerEndpoint` not found — expected twin to exist");
    };
    let open = body[head..]
        .find('{')
        .unwrap_or_else(|| panic!("{label}: PeerEndpoint has no opening brace"))
        + head;
    let close_rel = body[open..]
        .find("\n}")
        .unwrap_or_else(|| panic!("{label}: PeerEndpoint has no terminating `\\n}}`"));
    let inner = &body[open + 1..open + close_rel];

    let mut fields = BTreeMap::new();
    for raw in inner.split(',') {
        let line = raw
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with("//"))
            .collect::<Vec<_>>()
            .join(" ");
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Some(rest) = line.strip_prefix("pub ") else {
            panic!("{label}: PeerEndpoint field is not `pub`: `{line}`");
        };
        let Some((name, ty)) = rest.split_once(':') else {
            panic!("{label}: malformed PeerEndpoint field: `{line}`");
        };
        let name = name.trim().to_owned();
        let ty = ty.trim().to_owned();
        assert!(
            fields.insert(name.clone(), ty.clone()).is_none(),
            "{label}: duplicate field `{name}` in PeerEndpoint"
        );
    }
    fields
}

#[test]
fn peer_endpoint_schema_and_runtime_fields_are_identical() {
    let root = workspace_root();
    let runtime_body = read_source(&root, RUNTIME_DSL);
    let schema_body = read_source(&root, SCHEMA_CATALOG);
    let runtime_fields = peer_endpoint_fields("runtime DSL", &runtime_body);
    let schema_fields = peer_endpoint_fields("schema catalog", &schema_body);

    // Identical field names.
    let runtime_names: Vec<&String> = runtime_fields.keys().collect();
    let schema_names: Vec<&String> = schema_fields.keys().collect();
    assert_eq!(
        runtime_names, schema_names,
        "PeerEndpoint field sets drifted between schema and runtime.\n\
         runtime: {runtime_names:?}\nschema: {schema_names:?}"
    );

    // Identical typed shapes — name=>type must match across both sides.
    assert_eq!(
        runtime_fields, schema_fields,
        "PeerEndpoint field types drifted between schema and runtime.\n\
         runtime: {runtime_fields:#?}\nschema: {schema_fields:#?}"
    );

    // Guard against silent destringification regressions — both sides
    // must carry the typed newtypes (not `String`) at the known
    // canonical field names.
    for (field, expected_ty) in [
        ("name", "PeerName"),
        ("peer_id", "PeerId"),
        ("address", "PeerAddress"),
        ("signing_key", "PeerSigningKey"),
    ] {
        let actual = schema_fields
            .get(field)
            .unwrap_or_else(|| panic!("schema PeerEndpoint missing field `{field}`"));
        assert_eq!(
            actual, expected_ty,
            "schema PeerEndpoint.{field}: expected `{expected_ty}`, got `{actual}`"
        );
    }
}

#[test]
fn peer_endpoint_both_sides_have_trusted_peer_descriptor_from_impl() {
    let root = workspace_root();
    let runtime_body = read_source(&root, RUNTIME_DSL);
    let schema_body = read_source(&root, SCHEMA_CATALOG);

    let runtime_needle = "impl From<&meerkat_core::comms::TrustedPeerDescriptor> for PeerEndpoint";
    assert!(
        runtime_body.contains(runtime_needle),
        "runtime DSL missing `{runtime_needle}`"
    );
    assert!(
        schema_body.contains(runtime_needle),
        "schema catalog missing `{runtime_needle}` \
         — wave-d D-e requires the schema-side twin to expose the \
         same `From<&TrustedPeerDescriptor>` conversion as the \
         runtime side"
    );
}

/// Runtime check that the schema-side `From<&TrustedPeerDescriptor>`
/// impl actually wires every identity atom through to the correct
/// `PeerEndpoint` field. Text scans catch signatures; this catches
/// body bugs (e.g. swapping `name` and `address` inside the impl).
#[test]
fn peer_endpoint_schema_from_trusted_peer_descriptor_round_trip() {
    use meerkat_core::comms::TrustedPeerDescriptor;
    use meerkat_machine_schema::catalog::dsl::meerkat_machine::PeerEndpoint;

    let descriptor = TrustedPeerDescriptor::test_only_unsigned(
        "alice",
        "11111111-2222-5333-8444-555555555555",
        "inproc://alice",
    )
    .expect("synthesize a valid trusted peer descriptor")
    .with_pubkey([42u8; 32]);

    let endpoint = PeerEndpoint::from(&descriptor);

    assert_eq!(
        endpoint.name.as_str(),
        descriptor.name.as_str(),
        "schema PeerEndpoint.name must mirror descriptor.name"
    );
    assert_eq!(
        endpoint.peer_id.as_str(),
        descriptor.peer_id.to_string().as_str(),
        "schema PeerEndpoint.peer_id must mirror descriptor.peer_id"
    );
    assert_eq!(
        endpoint.address.as_str(),
        descriptor.address.to_string().as_str(),
        "schema PeerEndpoint.address must mirror descriptor.address"
    );
    assert_eq!(
        endpoint.signing_key.0, descriptor.pubkey,
        "schema PeerEndpoint.signing_key must mirror descriptor.pubkey"
    );
}
