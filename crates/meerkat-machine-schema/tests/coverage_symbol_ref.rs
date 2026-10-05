#![allow(clippy::expect_used, clippy::panic)]

//! `SymbolRef::parse` is the one public constructor for a coverage anchor
//! path. It is lexical: it never touches the filesystem, so a path that does
//! not exist yet is accepted, and the owning coverage validator decides
//! existence and ownership.

use meerkat_machine_schema::{
    NonPortableComponentKind, SymbolRef, SymbolRefError, canonical_composition_coverage_manifests,
    canonical_machine_coverage_manifests,
};

#[test]
fn portable_repository_relative_paths_parse_without_filesystem_access() {
    for accepted in [
        "src/owner.rs",
        "crates/example-catalog/src/dsl/acquisition.rs",
        "src/does-not-exist.rs",
        "src/caf\u{e9}.rs",
        "src/console.rs",
        "src/COM10.rs",
        "src/.hidden/owner.rs",
    ] {
        assert_eq!(
            SymbolRef::parse(accepted).expect("portable path").as_str(),
            accepted
        );
    }
}

#[test]
fn every_rejection_is_typed() {
    let cases: &[(&str, SymbolRefError)] = &[
        ("", SymbolRefError::Empty),
        ("/tmp/owner.rs", SymbolRefError::Absolute),
        ("src\\owner.rs", SymbolRefError::Backslash),
        ("\\\\host\\share\\owner.rs", SymbolRefError::Backslash),
        ("C:/owner.rs", SymbolRefError::Colon),
        ("C:owner.rs", SymbolRefError::Colon),
        ("src/owner.rs:stream", SymbolRefError::Colon),
        ("src/owner\n.rs", SymbolRefError::ControlCharacter),
        ("src/\0owner.rs", SymbolRefError::ControlCharacter),
        ("../owner.rs", SymbolRefError::InvalidComponent),
        ("src/../owner.rs", SymbolRefError::InvalidComponent),
        ("./owner.rs", SymbolRefError::InvalidComponent),
        ("src//owner.rs", SymbolRefError::InvalidComponent),
        ("src/", SymbolRefError::InvalidComponent),
        (
            "src/owner. ",
            SymbolRefError::NonPortableComponent(NonPortableComponentKind::TrailingDotOrSpace),
        ),
        (
            "src/owner.",
            SymbolRefError::NonPortableComponent(NonPortableComponentKind::TrailingDotOrSpace),
        ),
        (
            "src/owner?rs",
            SymbolRefError::NonPortableComponent(NonPortableComponentKind::ReservedCharacter),
        ),
        (
            "src/a<b.rs",
            SymbolRefError::NonPortableComponent(NonPortableComponentKind::ReservedCharacter),
        ),
        (
            "src/CON.rs",
            SymbolRefError::NonPortableComponent(NonPortableComponentKind::ReservedDeviceName),
        ),
        (
            "src/nul",
            SymbolRefError::NonPortableComponent(NonPortableComponentKind::ReservedDeviceName),
        ),
        (
            "src/LPT1",
            SymbolRefError::NonPortableComponent(NonPortableComponentKind::ReservedDeviceName),
        ),
        (
            "src/com\u{b9}.rs",
            SymbolRefError::NonPortableComponent(NonPortableComponentKind::ReservedDeviceName),
        ),
    ];
    for (path, expected) in cases {
        assert_eq!(
            SymbolRef::parse(*path),
            Err(*expected),
            "{path:?} must be refused as {expected:?}"
        );
    }
}

/// The built-in catalogs construct their anchors through the same parser, so
/// an in-repo anchor path is held to the same portability rules as an
/// external one.
#[test]
fn builtin_coverage_anchor_paths_are_portable() {
    let mut anchors = 0usize;
    for manifest in canonical_machine_coverage_manifests() {
        for anchor in &manifest.code_anchors {
            assert!(SymbolRef::parse(anchor.symbol.as_str()).is_ok());
            anchors += 1;
        }
    }
    for manifest in canonical_composition_coverage_manifests() {
        for anchor in &manifest.code_anchors {
            assert!(SymbolRef::parse(anchor.symbol.as_str()).is_ok());
            anchors += 1;
        }
    }
    assert!(
        anchors > 0,
        "the built-in catalogs declare coverage anchors"
    );
}
