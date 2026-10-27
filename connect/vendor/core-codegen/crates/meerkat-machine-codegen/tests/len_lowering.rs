#![allow(clippy::expect_used, clippy::panic)]

//! `.len()` lowers to the TLA+ operator that matches the collection's model
//! domain. The kernel counts the entries of a sequence, set or map alike, but
//! TLA+ `Len` is defined only on sequences: a set (including a field-presence
//! set reached through a structural record field) needs `Cardinality`, and a
//! map needs `Cardinality(DOMAIN ..)`.

use meerkat_machine_codegen::render_machine_semantic_model;
use meerkat_machine_schema::catalog::canonical_machine_schemas;
use meerkat_machine_schema::catalog::dsl::dsl_session_document_machine;
use meerkat_machine_schema::identity::{FieldId, MachineId, NamedTypeBinding, NamedTypeId};
use meerkat_machine_schema::{
    Expr, FieldSchema, HelperSchema, RustTypeAtom, TypePathStructField, TypeRef,
};

fn named(slug: &str) -> NamedTypeId {
    NamedTypeId::parse(slug).expect("named-type slug")
}

fn param(name: &str, ty: TypeRef) -> FieldSchema {
    FieldSchema {
        name: FieldId::parse(name).expect("field slug"),
        ty,
        disclosure: meerkat_machine_schema::FieldDisclosure::Visible,
    }
}

fn field(base: Expr, name: &str) -> Expr {
    Expr::FieldAccess {
        base: Box::new(base),
        field: FieldId::parse(name).expect("field slug"),
    }
}

fn len_is_zero(collection: Expr) -> Expr {
    Expr::Eq(
        Box::new(Expr::Len(Box::new(collection))),
        Box::new(Expr::U64(0)),
    )
}

#[test]
fn len_lowers_to_cardinality_for_sets_and_maps_and_len_for_sequences() {
    // Grafted onto an existing machine so the probe exercises the real
    // renderer; the machine id is renamed so no canonical binding applies.
    let mut schema = dsl_session_document_machine();
    schema.machine = MachineId::parse("LenLoweringProbeMachine").expect("machine slug");
    schema.named_types.push(NamedTypeBinding {
        name: named("ProbeUnresolved"),
        rust: RustTypeAtom::TypePathFieldPresenceSet {
            path: "std::collections::BTreeSet<probe::Unresolved>".into(),
            fields: vec![
                FieldId::parse("Absent").expect("field slug"),
                FieldId::parse("Unknown").expect("field slug"),
            ],
        },
    });
    schema.named_types.push(NamedTypeBinding {
        name: named("ProbeLifetime"),
        rust: RustTypeAtom::TypePathStruct {
            path: "probe::Lifetime".into(),
            fields: vec![TypePathStructField::named("unresolved", "ProbeUnresolved")],
        },
    });
    schema.named_types.push(NamedTypeBinding {
        name: named("ProbeRestrictions"),
        rust: RustTypeAtom::TypePathStruct {
            path: "probe::Restrictions".into(),
            fields: vec![TypePathStructField::named("lifetime", "ProbeLifetime")],
        },
    });
    schema.helpers.push(HelperSchema {
        name: "probe_len_lowering".into(),
        params: vec![
            param("restrictions", TypeRef::Named(named("ProbeRestrictions"))),
            param(
                "by_key",
                TypeRef::Map(Box::new(TypeRef::String), Box::new(TypeRef::String)),
            ),
            param("ordered", TypeRef::Seq(Box::new(TypeRef::String))),
            param("members", TypeRef::Set(Box::new(TypeRef::String))),
        ],
        returns: TypeRef::Bool,
        body: Expr::And(vec![
            len_is_zero(field(
                field(Expr::Binding("restrictions".into()), "lifetime"),
                "unresolved",
            )),
            len_is_zero(Expr::Binding("by_key".into())),
            len_is_zero(Expr::Binding("ordered".into())),
            len_is_zero(Expr::Binding("members".into())),
        ]),
    });

    let model = render_machine_semantic_model(&schema).expect("render probe model");
    let helper = model
        .lines()
        .find(|line| line.starts_with("probe_len_lowering("))
        .unwrap_or_else(|| panic!("probe helper missing from model:\n{model}"));
    for expected in [
        "Cardinality(restrictions.lifetime.unresolved) = 0",
        "Cardinality(DOMAIN by_key) = 0",
        "Len(ordered) = 0",
        "Cardinality(members) = 0",
    ] {
        assert!(
            helper.contains(expected),
            "expected `{expected}` in the rendered helper:\n{helper}"
        );
    }
}

/// Every `Len(<state field>)` in a canonical machine model applies to a
/// sequence-typed field: no set or map is measured with `Len`.
#[test]
fn canonical_models_apply_len_only_to_sequence_state_fields() {
    for schema in canonical_machine_schemas() {
        let model = render_machine_semantic_model(&schema)
            .unwrap_or_else(|error| panic!("render {}: {error:?}", schema.machine));
        for field in &schema.state.fields {
            let call = format!("Len({})", field.name.as_str());
            if model.contains(&call) {
                assert!(
                    matches!(field.ty, TypeRef::Seq(_)),
                    "{}: `{call}` measures a {:?} field with the sequence operator",
                    schema.machine,
                    field.ty
                );
            }
        }
    }
}
