#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    unused_imports
)]

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use meerkat_machine_schema::identity::{
    ActorId, CompositionDriverId, CompositionId, CompositionWitnessId, EffectVariantId,
    EntryInputId, EnumTypeId, EnumVariantId, FieldId, IdentityError, IdentityErrorKind,
    InputVariantId, MachineId, MachineInstanceId, NamedTypeBinding, NamedTypeId, PhaseId,
    ProtocolId, RouteId, RustTypeAtom, SignalVariantId, StorePrimitiveId, TransactionPlanId,
    TransactionTriggerId, TransitionId,
};

macro_rules! for_each_identity {
    ($body:ident) => {
        $body!(MachineId);
        $body!(MachineInstanceId);
        $body!(PhaseId);
        $body!(InputVariantId);
        $body!(SignalVariantId);
        $body!(EffectVariantId);
        $body!(FieldId);
        $body!(TransitionId);
        $body!(RouteId);
        $body!(ProtocolId);
        $body!(ActorId);
        $body!(NamedTypeId);
        $body!(EnumTypeId);
        $body!(EnumVariantId);
        $body!(CompositionId);
        $body!(CompositionDriverId);
        $body!(TransactionPlanId);
        $body!(TransactionTriggerId);
        $body!(CompositionWitnessId);
        $body!(EntryInputId);
    };
}

#[test]
fn every_identity_accepts_a_valid_slug() {
    macro_rules! check {
        ($ty:ident) => {{
            let id = $ty::parse("ok_slug-1").expect(concat!(stringify!($ty), " should accept"));
            assert_eq!(id.as_str(), "ok_slug-1");
            assert_eq!(id.to_string(), "ok_slug-1");
            assert_eq!(AsRef::<str>::as_ref(&id), "ok_slug-1");
        }};
    }
    for_each_identity!(check);
}

#[test]
fn every_identity_accepts_underscore_prefix() {
    macro_rules! check {
        ($ty:ident) => {{
            assert!(
                $ty::parse("_ok").is_ok(),
                "{} must allow _-prefix",
                stringify!($ty)
            );
        }};
    }
    for_each_identity!(check);
}

#[test]
fn every_identity_rejects_invalid_inputs() {
    macro_rules! check_rejects {
        ($ty:ident) => {{
            assert!(
                matches!(
                    $ty::parse("").map_err(|e| e.kind),
                    Err(IdentityErrorKind::Empty)
                ),
                "{} must reject empty",
                stringify!($ty)
            );
            assert!(
                matches!(
                    $ty::parse("1bad").map_err(|e| e.kind),
                    Err(IdentityErrorKind::InvalidStartChar('1'))
                ),
                "{} must reject digit start",
                stringify!($ty)
            );
            assert!(
                matches!(
                    $ty::parse("bad space").map_err(|e| e.kind),
                    Err(IdentityErrorKind::InvalidChar { ch: ' ', .. })
                ),
                "{} must reject spaces",
                stringify!($ty)
            );
            assert!(
                matches!(
                    $ty::parse("bad.dot").map_err(|e| e.kind),
                    Err(IdentityErrorKind::InvalidChar { ch: '.', .. })
                ),
                "{} must reject dots",
                stringify!($ty)
            );
            assert!(
                matches!(
                    $ty::parse("bad/slash").map_err(|e| e.kind),
                    Err(IdentityErrorKind::InvalidChar { ch: '/', .. })
                ),
                "{} must reject slashes",
                stringify!($ty)
            );
            assert!(
                matches!(
                    $ty::parse("bad\x00nul").map_err(|e| e.kind),
                    Err(IdentityErrorKind::InvalidChar { ch: '\x00', .. })
                ),
                "{} must reject control chars",
                stringify!($ty)
            );
        }};
    }
    for_each_identity!(check_rejects);
}

#[test]
fn invalid_char_error_reports_character_and_position() {
    let err = FieldId::parse("ab cd").unwrap_err();
    assert_eq!(err.raw, "ab cd");
    match err.kind {
        IdentityErrorKind::InvalidChar { ch, position } => {
            assert_eq!(ch, ' ');
            assert_eq!(position, 2);
        }
        other => panic!("unexpected kind: {other:?}"),
    }
    let rendered = err.to_string();
    assert!(
        rendered.contains("' '"),
        "display should show the char: {rendered}"
    );
    assert!(
        rendered.contains("position 2"),
        "display should show position: {rendered}"
    );
}

#[test]
fn identity_error_implements_std_error() {
    fn assert_error<E: std::error::Error>(_: &E) {}
    let err = MachineId::parse("1foo").unwrap_err();
    assert_error(&err);
}

#[test]
fn serde_json_roundtrip_preserves_value() {
    macro_rules! check {
        ($ty:ident) => {{
            let original = $ty::parse("round_trip-1").unwrap();
            let json = serde_json::to_string(&original).unwrap();
            assert_eq!(json, "\"round_trip-1\"");
            let restored: $ty = serde_json::from_str(&json).unwrap();
            assert_eq!(original, restored);
        }};
    }
    for_each_identity!(check);
}

#[test]
fn serde_rejects_invalid_slug_on_deserialize() {
    let err = serde_json::from_str::<MachineId>("\"1bad\"").unwrap_err();
    assert!(
        err.to_string().contains("1bad"),
        "deserialize error should surface the offending input: {err}"
    );
}

#[test]
fn store_primitive_identity_accepts_qualified_rust_operation_paths() {
    let primitive = StorePrimitiveId::parse("ScheduleStore::claim_due_occurrences").unwrap();
    assert_eq!(primitive.as_str(), "ScheduleStore::claim_due_occurrences");
    assert!(matches!(
        StorePrimitiveId::parse("").map_err(|err| err.kind),
        Err(IdentityErrorKind::Empty)
    ));
    assert!(matches!(
        StorePrimitiveId::parse("ScheduleStore::claim due").map_err(|err| err.kind),
        Err(IdentityErrorKind::InvalidChar { ch: ' ', .. })
    ));
}

#[test]
fn hash_is_stable_for_equal_values() {
    fn hash_of<T: Hash>(value: &T) -> u64 {
        let mut hasher = DefaultHasher::new();
        value.hash(&mut hasher);
        hasher.finish()
    }

    macro_rules! check {
        ($ty:ident) => {{
            let a = $ty::parse("stable_name-42").unwrap();
            let b = $ty::parse("stable_name-42").unwrap();
            assert_eq!(a, b);
            assert_eq!(hash_of(&a), hash_of(&b));
        }};
    }
    for_each_identity!(check);
}

#[test]
fn ordering_is_lexicographic_on_inner_string() {
    let a = PhaseId::parse("aaa").unwrap();
    let b = PhaseId::parse("bbb").unwrap();
    assert!(a < b);
    assert!(b > a);
}

#[test]
fn rust_type_atom_type_path_roundtrips() {
    let atom = RustTypeAtom::TypePath("crate::domain::MySpecialType".into());
    let json = serde_json::to_string(&atom).unwrap();
    let restored: RustTypeAtom = serde_json::from_str(&json).unwrap();
    assert_eq!(atom, restored);
    match restored {
        RustTypeAtom::TypePath(inner) => {
            assert_eq!(inner, "crate::domain::MySpecialType");
        }
        other => panic!("expected TypePath, got {other:?}"),
    }
}

#[test]
fn rust_type_atom_string_enum_roundtrips() {
    let atom = RustTypeAtom::StringEnum {
        variants: vec![
            EnumVariantId::parse("Pending").unwrap(),
            EnumVariantId::parse("Running").unwrap(),
        ],
    };
    let json = serde_json::to_string(&atom).unwrap();
    let restored: RustTypeAtom = serde_json::from_str(&json).unwrap();
    assert_eq!(atom, restored);
}

#[test]
fn rust_type_atom_scalar_variants_roundtrip() {
    for atom in [
        RustTypeAtom::U64,
        RustTypeAtom::U32,
        RustTypeAtom::U16,
        RustTypeAtom::U8,
        RustTypeAtom::Bool,
        RustTypeAtom::String,
    ] {
        let json = serde_json::to_string(&atom).unwrap();
        let restored: RustTypeAtom = serde_json::from_str(&json).unwrap();
        assert_eq!(atom, restored);
    }
}

#[test]
fn named_type_binding_roundtrips_through_json() {
    let binding = NamedTypeBinding {
        name: NamedTypeId::parse("turn_number").unwrap(),
        rust: RustTypeAtom::U64,
    };
    let json = serde_json::to_string(&binding).unwrap();
    let restored: NamedTypeBinding = serde_json::from_str(&json).unwrap();
    assert_eq!(binding, restored);
    assert_eq!(restored.name.as_str(), "turn_number");
    assert_eq!(restored.rust, RustTypeAtom::U64);
}

#[test]
fn identity_error_raw_is_preserved_for_non_ascii() {
    let err = MachineId::parse("naïve").unwrap_err();
    assert_eq!(err.raw, "naïve");
    assert!(matches!(
        err.kind,
        IdentityErrorKind::InvalidChar { ch: 'ï', .. }
    ));
}
