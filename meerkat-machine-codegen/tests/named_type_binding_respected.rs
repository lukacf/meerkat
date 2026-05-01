#![allow(clippy::expect_used, clippy::panic)]

//! Track-B wave-b B-4b — named-type binding authority.
//!
//! The codegen must render every `TypeRef::Named` slug through its
//! authoritative [`NamedTypeBinding`] entry on the schema. Scalar
//! [`RustTypeAtom`] variants keep the DSL name as a tuple-struct identity
//! boundary whose inner field uses the authoritative Rust atom:
//!
//! | atom                 | rust                                      |
//! | -------------------- | ----------------------------------------- |
//! | `U8` / `U16` / `U32` / `U64` | `pub struct Name(pub u8/u16/u32/u64)` |
//! | `Bool`               | `pub struct Name(pub bool)`               |
//! | `String`             | `pub struct Name(pub String)`             |
//! | `TypePath(path)`     | `pub type Name = path`                    |
//! | `TypePathEnum { path, .. }` | `pub type Name = path`             |
//!
//! A `TypeRef::Named` referenced without a matching binding is rejected by
//! `MachineSchema::validate()` — there is no name-based fallback.

use meerkat_machine_codegen::render_machine_kernel_module;
use meerkat_machine_schema::identity::{
    EnumVariantId, FieldId, MachineId, NamedTypeBinding, NamedTypeId, PhaseId, RustTypeAtom,
};
use meerkat_machine_schema::{
    EnumSchema, Expr, FieldInit, FieldSchema, InitSchema, MachineSchema, MachineSchemaError,
    RustBinding, StateSchema, TypeRef, VariantSchema,
};

/// Build a minimal machine schema that references a single named type via
/// one state field. The schema has an empty input/signal/effect surface,
/// a single phase, no transitions — just enough to exercise the
/// named-type lowering path in `render_machine_kernel_module`.
fn schema_with_single_named_type(field_name: &str, named: &str) -> MachineSchema {
    MachineSchema {
        machine: MachineId::parse("AtomProbeMachine").expect("machine slug"),
        version: 1,
        rust: RustBinding {
            crate_name: "meerkat-machine-codegen-test".into(),
            module: "generated::atom_probe".into(),
        },
        state: StateSchema {
            phase: EnumSchema {
                name: "AtomProbePhase".into(),
                variants: vec![VariantSchema {
                    name: EnumVariantId::parse("Running").expect("phase variant slug"),
                    fields: vec![],
                }],
            },
            fields: vec![FieldSchema {
                name: FieldId::parse(field_name).expect("field slug"),
                ty: TypeRef::Named(NamedTypeId::parse(named).expect("named-type slug")),
            }],
            init: InitSchema {
                phase: PhaseId::parse("Running").expect("phase slug"),
                fields: vec![FieldInit {
                    field: FieldId::parse(field_name).expect("field slug"),
                    expr: Expr::String(String::new()),
                }],
            },
            terminal_phases: vec![],
        },
        inputs: EnumSchema {
            name: "AtomProbeInput".into(),
            variants: vec![],
        },
        surface_only_inputs: vec![],
        runtime_internal_inputs: vec![],
        signals: EnumSchema {
            name: "AtomProbeSignal".into(),
            variants: vec![],
        },
        effects: EnumSchema {
            name: "AtomProbeEffect".into(),
            variants: vec![],
        },
        helpers: vec![],
        derived: vec![],
        invariants: vec![],
        transitions: vec![],
        effect_dispositions: vec![],
        named_types: vec![NamedTypeBinding {
            name: NamedTypeId::parse(named).expect("named-type slug"),
            rust: RustTypeAtom::String, // overridden by the callers
        }],
        ci_step_limit: None,
    }
}

fn with_atom(mut schema: MachineSchema, atom: RustTypeAtom) -> MachineSchema {
    schema.named_types[0].rust = atom;
    schema
}

fn assert_scalar_newtype(rendered: &str, name: &str, rust_type: &str) {
    let expected = format!("pub struct {name}(pub {rust_type});");
    assert!(
        rendered.contains(&expected),
        "rendered kernel module must keep `{name}` as a named scalar over `{rust_type}`:\n\
         expected line: `{expected}`\n\
         rendered:\n{rendered}"
    );
    assert!(
        !rendered.contains(&format!("pub type {name} = {rust_type};")),
        "named scalar `{name}` must not erase to a primitive alias:\n{rendered}"
    );
}

fn assert_type_alias(rendered: &str, alias: &str, rust_type: &str) {
    let expected = format!("pub type {alias} = {rust_type};");
    assert!(
        rendered.contains(&expected),
        "rendered kernel module must bind `{alias}` to `{rust_type}`:\n\
         expected line: `{expected}`\n\
         rendered:\n{rendered}"
    );
}

#[test]
fn rust_type_atom_u64_lowers_to_u64() {
    let schema = with_atom(
        schema_with_single_named_type("counter", "AtomU64"),
        RustTypeAtom::U64,
    );
    schema.validate().expect("schema validates");
    let rendered = render_machine_kernel_module(&schema);
    assert_scalar_newtype(&rendered, "AtomU64", "u64");
}

#[test]
fn rust_type_atom_u32_lowers_to_u32() {
    let schema = with_atom(
        schema_with_single_named_type("counter", "AtomU32"),
        RustTypeAtom::U32,
    );
    schema.validate().expect("schema validates");
    let rendered = render_machine_kernel_module(&schema);
    assert_scalar_newtype(&rendered, "AtomU32", "u32");
}

#[test]
fn rust_type_atom_u16_lowers_to_u16() {
    let schema = with_atom(
        schema_with_single_named_type("counter", "AtomU16"),
        RustTypeAtom::U16,
    );
    schema.validate().expect("schema validates");
    let rendered = render_machine_kernel_module(&schema);
    assert_scalar_newtype(&rendered, "AtomU16", "u16");
}

#[test]
fn rust_type_atom_u8_lowers_to_u8() {
    let schema = with_atom(
        schema_with_single_named_type("counter", "AtomU8"),
        RustTypeAtom::U8,
    );
    schema.validate().expect("schema validates");
    let rendered = render_machine_kernel_module(&schema);
    assert_scalar_newtype(&rendered, "AtomU8", "u8");
}

#[test]
fn rust_type_atom_bool_lowers_to_bool() {
    let schema = with_atom(
        schema_with_single_named_type("flag", "AtomBool"),
        RustTypeAtom::Bool,
    );
    schema.validate().expect("schema validates");
    let rendered = render_machine_kernel_module(&schema);
    assert_scalar_newtype(&rendered, "AtomBool", "bool");
}

#[test]
fn rust_type_atom_string_lowers_to_string() {
    let schema = with_atom(
        schema_with_single_named_type("name", "AtomString"),
        RustTypeAtom::String,
    );
    schema.validate().expect("schema validates");
    let rendered = render_machine_kernel_module(&schema);
    assert_scalar_newtype(&rendered, "AtomString", "String");
}

#[test]
fn rust_type_atom_string_enum_lowers_to_closed_enum() {
    let schema = with_atom(
        schema_with_single_named_type("status", "AtomStatus"),
        RustTypeAtom::StringEnum {
            variants: vec![
                EnumVariantId::parse("Pending").expect("variant slug"),
                EnumVariantId::parse("Running").expect("variant slug"),
                EnumVariantId::parse("Completed").expect("variant slug"),
            ],
        },
    );
    schema.validate().expect("schema validates");
    let rendered = render_machine_kernel_module(&schema);

    assert!(
        rendered.contains("pub enum AtomStatus"),
        "string enum named type must render as a closed enum:\n{rendered}"
    );
    for variant in ["Pending", "Running", "Completed"] {
        assert!(
            rendered.contains(&format!("    {variant},")),
            "string enum named type must include `{variant}`:\n{rendered}"
        );
    }
    assert!(
        rendered.contains("impl std::convert::TryFrom<&str> for AtomStatus"),
        "string enum named type must expose checked string parsing:\n{rendered}"
    );
    assert!(
        !rendered.contains("pub struct AtomStatus(pub String);"),
        "string enum named type must not render as an arbitrary string newtype:\n{rendered}"
    );
    assert!(
        !rendered.contains("impl From<&str> for AtomStatus"),
        "string enum named type must not expose unchecked string conversion:\n{rendered}"
    );
}

#[test]
fn rust_type_atom_string_enum_preserves_raw_serde_values() {
    let schema = with_atom(
        schema_with_single_named_type("status", "AtomStatus"),
        RustTypeAtom::StringEnum {
            variants: vec![
                EnumVariantId::parse("waiting-for-peer").expect("variant slug"),
                EnumVariantId::parse("Running").expect("variant slug"),
            ],
        },
    );
    schema.validate().expect("schema validates");
    let rendered = render_machine_kernel_module(&schema);

    assert!(
        rendered.contains("    #[serde(rename = \"waiting-for-peer\")]\n    waiting_for_peer,"),
        "string enum serde must use the raw domain value, not the sanitized Rust identifier:\n{rendered}"
    );
}

#[test]
fn rust_type_atom_string_enum_rejects_sanitized_variant_collisions() {
    let schema = with_atom(
        schema_with_single_named_type("status", "AtomStatus"),
        RustTypeAtom::StringEnum {
            variants: vec![
                EnumVariantId::parse("waiting-for-peer").expect("variant slug"),
                EnumVariantId::parse("waiting_for_peer").expect("variant slug"),
            ],
        },
    );

    let error = schema
        .validate()
        .expect_err("schema validation rejects colliding string enum variants");
    assert!(
        matches!(
            error,
            MachineSchemaError::InvalidStringEnumBinding { ref name, ref reason }
                if name == "AtomStatus"
                    && reason.contains("waiting-for-peer")
                    && reason.contains("waiting_for_peer")
                    && reason.contains("duplicate Rust identifier")
        ),
        "string enum variants that sanitize to the same Rust identifier must be rejected at schema validation, got: {error:?}"
    );
}

#[test]
fn rust_type_atom_type_path_lowers_verbatim_not_to_string() {
    let schema = with_atom(
        schema_with_single_named_type("custom", "AtomTypePath"),
        RustTypeAtom::TypePath("my::special::MyType".into()),
    );
    schema.validate().expect("schema validates");
    let rendered = render_machine_kernel_module(&schema);
    assert_type_alias(&rendered, "AtomTypePath", "my::special::MyType");
    assert!(
        !rendered.contains("pub type AtomTypePath = String;"),
        "named-type aliases bound to TypePath must not fall back to String:\n{rendered}"
    );
}

#[test]
fn rust_type_atom_type_path_enum_lowers_verbatim_not_to_string() {
    let schema = with_atom(
        schema_with_single_named_type("custom", "AtomTypePathEnum"),
        RustTypeAtom::TypePathEnum {
            path: "my::special::MyEnum".into(),
            unit_variants: vec![EnumVariantId::parse("All").expect("variant slug")],
            structural_variants: Vec::new(),
        },
    );
    schema.validate().expect("schema validates");
    let rendered = render_machine_kernel_module(&schema);
    assert_type_alias(&rendered, "AtomTypePathEnum", "my::special::MyEnum");
    assert!(
        !rendered.contains("pub type AtomTypePathEnum = String;"),
        "named-type aliases bound to TypePathEnum must not fall back to String:\n{rendered}"
    );
}

#[test]
fn missing_named_type_binding_is_rejected_by_validate() {
    // Build a schema that references `UnboundAtom` in a field but supplies
    // no `NamedTypeBinding` entry for it.
    let mut schema = schema_with_single_named_type("field", "UnboundAtom");
    schema.named_types.clear();

    let err = schema
        .validate()
        .expect_err("validate must reject unbound named type");
    match err {
        MachineSchemaError::MissingNamedTypeBinding { name } => {
            assert_eq!(name, "UnboundAtom", "error must name the offending slug");
        }
        other => panic!("expected MissingNamedTypeBinding, got {other:?}"),
    }
}
