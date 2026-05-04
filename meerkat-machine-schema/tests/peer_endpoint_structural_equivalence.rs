#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

//! Typed structural-equivalence tripwire for the `PeerEndpoint` DSL twin.
//!
//! The required governance check is intentionally not a Rust source scanner.
//! `MachineSchema` typed metadata owns the structural field set, and real
//! `TrustedPeerDescriptor` projections own conversion coverage. A legacy source
//! token can still appear in fixture text below; it is never sufficient for a
//! green check when the typed schema or conversion disagrees.

use meerkat_core::comms::TrustedPeerDescriptor;
use meerkat_machine_schema::catalog::dsl::{
    dsl_meerkat_machine,
    meerkat_machine::{PeerEndpoint, PeerSigningKey},
};
use meerkat_machine_schema::identity::NamedTypeId;
use meerkat_machine_schema::{
    MachineSchema, RustTypeAtom, TypePathStructField, TypePathStructFieldAtom,
};

const PEER_ENDPOINT_TYPE: &str = "PeerEndpoint";
const PEER_ENDPOINT_PATH: &str = "crate::catalog::dsl::meerkat_machine::PeerEndpoint";
const LEGACY_SOURCE_TOKEN_FIXTURE: &str = r"
pub struct PeerEndpoint {
    pub name: PeerName,
    pub peer_id: PeerId,
    pub address: PeerAddress,
    pub signing_key: PeerSigningKey,
}
";

const EXPECTED_PEER_ENDPOINT_FIELDS: &[(&str, &str)] = &[
    ("name", "PeerName"),
    ("peer_id", "PeerId"),
    ("address", "PeerAddress"),
    ("signing_key", "PeerSigningKey"),
];

const EXPECTED_NAMED_TYPE_PATHS: &[(&str, &str)] = &[
    ("PeerName", "crate::catalog::dsl::meerkat_machine::PeerName"),
    ("PeerId", "crate::catalog::dsl::meerkat_machine::PeerId"),
    (
        "PeerAddress",
        "crate::catalog::dsl::meerkat_machine::PeerAddress",
    ),
    (
        "PeerSigningKey",
        "crate::catalog::dsl::meerkat_machine::PeerSigningKey",
    ),
];

#[test]
fn peer_endpoint_schema_metadata_owns_structural_field_parity() {
    let schema = dsl_meerkat_machine();

    schema
        .validate()
        .expect("typed MeerkatMachine schema metadata must validate");
    assert_peer_endpoint_structural_contract(&schema).unwrap();
}

#[test]
fn peer_endpoint_schema_from_trusted_peer_descriptor_round_trip() {
    let descriptor = trusted_peer_descriptor();
    let endpoint = PeerEndpoint::from(&descriptor);

    assert_schema_endpoint_matches_descriptor(&endpoint, &descriptor).unwrap();
}

#[test]
fn source_token_match_does_not_mask_schema_metadata_field_omission() {
    let mut schema = dsl_meerkat_machine();
    mutate_peer_endpoint_structural_metadata(&mut schema, |fields| {
        fields.retain(|field| field.name.as_str() != "signing_key");
    });

    assert_legacy_source_fixture_still_contains_peer_endpoint_tokens();
    let err = assert_peer_endpoint_structural_contract(&schema).unwrap_err();
    assert!(
        err.contains("PeerEndpoint structural fields"),
        "unexpected error: {err}"
    );
}

#[test]
fn source_token_match_does_not_mask_schema_metadata_field_type_drift() {
    let mut schema = dsl_meerkat_machine();
    mutate_peer_endpoint_structural_metadata(&mut schema, |fields| {
        fields
            .iter_mut()
            .find(|field| field.name.as_str() == "signing_key")
            .expect("signing_key field exists")
            .atom = TypePathStructFieldAtom::Named(named_type_id("PeerId"));
    });

    assert_legacy_source_fixture_still_contains_peer_endpoint_tokens();
    let err = assert_peer_endpoint_structural_contract(&schema).unwrap_err();
    assert!(
        err.contains("PeerEndpoint structural fields"),
        "unexpected error: {err}"
    );
}

#[test]
fn source_token_match_does_not_mask_schema_conversion_field_omission() {
    let descriptor = trusted_peer_descriptor();
    let endpoint = PeerEndpoint::new(
        descriptor.name.as_str().to_owned(),
        descriptor.peer_id.to_string(),
        descriptor.address.to_string(),
        PeerSigningKey([0u8; 32]),
    );

    assert_legacy_source_fixture_still_contains_peer_endpoint_tokens();
    let err = assert_schema_endpoint_matches_descriptor(&endpoint, &descriptor).unwrap_err();
    assert!(err.contains("signing_key"), "unexpected error: {err}");
}

fn assert_peer_endpoint_structural_contract(schema: &MachineSchema) -> Result<(), String> {
    let actual = peer_endpoint_structural_fields(schema)?;
    let expected = EXPECTED_PEER_ENDPOINT_FIELDS
        .iter()
        .map(|(field, ty)| ((*field).to_owned(), (*ty).to_owned()))
        .collect::<Vec<_>>();
    if actual != expected {
        return Err(format!(
            "PeerEndpoint structural fields drifted.\nexpected: {expected:?}\nactual: {actual:?}"
        ));
    }

    for (name, expected_path) in EXPECTED_NAMED_TYPE_PATHS {
        let binding = schema
            .named_type_binding(&named_type_id(name))
            .ok_or_else(|| format!("PeerEndpoint field type `{name}` has no named binding"))?;
        match &binding.rust {
            RustTypeAtom::TypePath(path) if path == expected_path => {}
            other => {
                return Err(format!(
                    "PeerEndpoint field type `{name}` must bind to `{expected_path}`, got {other:?}"
                ));
            }
        }
    }

    Ok(())
}

fn peer_endpoint_structural_fields(
    schema: &MachineSchema,
) -> Result<Vec<(String, String)>, String> {
    let binding = schema
        .named_type_binding(&named_type_id(PEER_ENDPOINT_TYPE))
        .ok_or_else(|| "PeerEndpoint named-type binding is missing".to_owned())?;

    match &binding.rust {
        RustTypeAtom::TypePathStruct { path, fields } => {
            if path != PEER_ENDPOINT_PATH {
                return Err(format!(
                    "PeerEndpoint must bind to `{PEER_ENDPOINT_PATH}`, got `{path}`"
                ));
            }
            fields
                .iter()
                .map(|field| match &field.atom {
                    TypePathStructFieldAtom::Named(name) => {
                        Ok((field.name.as_str().to_owned(), name.as_str().to_owned()))
                    }
                    TypePathStructFieldAtom::String => Err(format!(
                        "PeerEndpoint.{} must be a typed named field, got String",
                        field.name
                    )),
                })
                .collect()
        }
        other => Err(format!(
            "PeerEndpoint must use TypePathStruct metadata, got {other:?}"
        )),
    }
}

fn mutate_peer_endpoint_structural_metadata(
    schema: &mut MachineSchema,
    mutate: impl FnOnce(&mut Vec<TypePathStructField>),
) {
    let binding = schema
        .named_types
        .iter_mut()
        .find(|binding| binding.name.as_str() == PEER_ENDPOINT_TYPE)
        .expect("PeerEndpoint binding exists");
    let RustTypeAtom::TypePathStruct { fields, .. } = &mut binding.rust else {
        panic!("PeerEndpoint binding must be TypePathStruct");
    };
    mutate(fields);
}

fn assert_schema_endpoint_matches_descriptor(
    endpoint: &PeerEndpoint,
    descriptor: &TrustedPeerDescriptor,
) -> Result<(), String> {
    if endpoint.name.as_str() != descriptor.name.as_str() {
        return Err(format!(
            "name mismatch: endpoint `{}`, descriptor `{}`",
            endpoint.name.as_str(),
            descriptor.name.as_str()
        ));
    }
    let expected_peer_id = descriptor.peer_id.to_string();
    if endpoint.peer_id.as_str() != expected_peer_id {
        return Err(format!(
            "peer_id mismatch: endpoint `{}`, descriptor `{expected_peer_id}`",
            endpoint.peer_id.as_str()
        ));
    }
    let expected_address = descriptor.address.to_string();
    if endpoint.address.as_str() != expected_address {
        return Err(format!(
            "address mismatch: endpoint `{}`, descriptor `{expected_address}`",
            endpoint.address.as_str()
        ));
    }
    if endpoint.signing_key.0 != descriptor.pubkey {
        return Err("signing_key mismatch between endpoint and descriptor".to_owned());
    }
    Ok(())
}

fn assert_legacy_source_fixture_still_contains_peer_endpoint_tokens() {
    for (field, ty) in EXPECTED_PEER_ENDPOINT_FIELDS {
        let token = format!("pub {field}: {ty}");
        assert!(
            LEGACY_SOURCE_TOKEN_FIXTURE.contains(&token),
            "legacy source fixture should contain `{token}`"
        );
    }
}

fn trusted_peer_descriptor() -> TrustedPeerDescriptor {
    TrustedPeerDescriptor::test_only_unsigned(
        "alice",
        "11111111-2222-5333-8444-555555555555",
        "inproc://alice",
    )
    .expect("synthesize a valid trusted peer descriptor")
    .with_pubkey([42u8; 32])
}

fn named_type_id(name: &str) -> NamedTypeId {
    NamedTypeId::parse(name).expect("valid named type")
}
