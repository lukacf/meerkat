//! Generated kernel value structs honor `FieldDisclosure::Redacted`.
//!
//! A struct with a redacted field must not derive `Debug`; the generated
//! hand-written impl prints presence for an option, the entry count for a
//! collection and `"<redacted>"` otherwise, so the value never reaches a
//! formatter. Structs without redacted fields keep the derived `Debug`.

#![allow(clippy::expect_used, clippy::panic)]

use meerkat_machine_codegen::render_machine_kernel_module;
use meerkat_machine_schema::identity::{EnumVariantId, FieldId, MachineId, PhaseId};
use meerkat_machine_schema::{
    EnumSchema, Expr, FieldDisclosure, FieldInit, FieldSchema, InitSchema, MachineSchema,
    RustBinding, StateSchema, TypeRef, VariantSchema,
};

fn field(name: &str, ty: TypeRef, disclosure: FieldDisclosure) -> FieldSchema {
    FieldSchema {
        name: FieldId::parse(name).expect("field slug"),
        ty,
        disclosure,
    }
}

fn variant(name: &str, fields: Vec<FieldSchema>) -> VariantSchema {
    VariantSchema {
        name: EnumVariantId::parse(name).expect("variant slug"),
        fields,
    }
}

fn init(name: &str, expr: Expr) -> FieldInit {
    FieldInit {
        field: FieldId::parse(name).expect("field slug"),
        expr,
    }
}

fn schema() -> MachineSchema {
    MachineSchema {
        machine: MachineId::parse("RedactionProbeMachine").expect("machine slug"),
        version: 1,
        rust: RustBinding {
            crate_name: "meerkat-machine-codegen-test".into(),
            module: "generated::redaction_probe".into(),
        },
        state: StateSchema {
            phase: EnumSchema {
                name: "RedactionProbePhase".into(),
                variants: vec![variant("Running", vec![])],
            },
            fields: vec![
                field("issued_count", TypeRef::U64, FieldDisclosure::Visible),
                field(
                    "channel_by_token",
                    TypeRef::Map(Box::new(TypeRef::String), Box::new(TypeRef::String)),
                    FieldDisclosure::Redacted,
                ),
                field(
                    "pending_token",
                    TypeRef::Option(Box::new(TypeRef::String)),
                    FieldDisclosure::Redacted,
                ),
            ],
            init: InitSchema {
                phase: PhaseId::parse("Running").expect("phase slug"),
                fields: vec![
                    init("issued_count", Expr::U64(0)),
                    init("channel_by_token", Expr::EmptyMap),
                    init("pending_token", Expr::None),
                ],
            },
            terminal_phases: vec![],
        },
        inputs: EnumSchema {
            name: "RedactionProbeInput".into(),
            variants: vec![
                variant(
                    "RecordToken",
                    vec![
                        field("channel_id", TypeRef::String, FieldDisclosure::Visible),
                        field("token", TypeRef::String, FieldDisclosure::Redacted),
                    ],
                ),
                variant(
                    "Close",
                    vec![field(
                        "channel_id",
                        TypeRef::String,
                        FieldDisclosure::Visible,
                    )],
                ),
            ],
        },
        surface_only_inputs: vec![],
        runtime_internal_inputs: vec![],
        tlc_representative_inputs: vec![],
        signals: EnumSchema {
            name: "RedactionProbeSignal".into(),
            variants: vec![],
        },
        effects: EnumSchema {
            name: "RedactionProbeEffect".into(),
            variants: vec![variant(
                "TokenRecorded",
                vec![field("token", TypeRef::String, FieldDisclosure::Redacted)],
            )],
        },
        helpers: vec![],
        derived: vec![],
        command_plans: vec![],
        invariants: vec![],
        transitions: vec![],
        effect_dispositions: vec![],
        named_types: vec![],
        ci_step_limit: None,
        deep_domain_overrides: Default::default(),
        input_field_domains: Default::default(),
    }
}

/// The derive line emitted directly above `struct_line`.
fn derive_above<'a>(rendered: &'a str, struct_line: &str) -> &'a str {
    let lines: Vec<&str> = rendered.lines().collect();
    let index = lines
        .iter()
        .position(|line| line.trim() == struct_line)
        .unwrap_or_else(|| panic!("missing `{struct_line}` in:\n{rendered}"));
    lines[index - 1].trim()
}

#[test]
fn redacted_fields_get_a_hand_written_debug_that_hides_values() {
    let rendered = render_machine_kernel_module(&schema()).expect("kernel renders");

    for struct_line in [
        "pub struct State {",
        "pub struct RecordToken {",
        "pub struct TokenRecorded {",
    ] {
        let derive = derive_above(&rendered, struct_line);
        assert!(
            derive.starts_with("#[derive(") && !derive.contains("Debug"),
            "`{struct_line}` has a redacted field and must not derive Debug: {derive}"
        );
    }
    for expected in [
        "impl std::fmt::Debug for State {",
        ".field(\"phase\", &self.phase)",
        ".field(\"issued_count\", &self.issued_count)",
        ".field(\"channel_by_token\", &format_args!(\"<redacted; {} entries>\", self.channel_by_token.len()))",
        ".field(\"pending_token\", &self.pending_token.as_ref().map(|_| \"<redacted>\"))",
        "impl std::fmt::Debug for RecordToken {",
        ".field(\"channel_id\", &self.channel_id)",
        ".field(\"token\", &\"<redacted>\")",
        "impl std::fmt::Debug for TokenRecorded {",
    ] {
        assert!(
            rendered.contains(expected),
            "missing `{expected}` in:\n{rendered}"
        );
    }
}

#[test]
fn structs_without_redacted_fields_keep_the_derived_debug() {
    let rendered = render_machine_kernel_module(&schema()).expect("kernel renders");
    let derive = derive_above(&rendered, "pub struct Close {");
    assert!(
        derive.contains("Debug"),
        "`Close` keeps derived Debug: {derive}"
    );
    assert!(
        !rendered.contains("impl std::fmt::Debug for Close {"),
        "`Close` must not get a hand-written Debug:\n{rendered}"
    );
}
