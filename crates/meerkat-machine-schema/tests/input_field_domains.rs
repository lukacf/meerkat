#![allow(clippy::expect_used, clippy::panic)]

//! Declared TLC payload domains for unsigned input fields: validation refuses
//! every declaration that would not be explored as written.

use std::collections::BTreeSet;

use meerkat_machine_schema::catalog::dsl::dsl_workgraph_lifecycle_machine;
use meerkat_machine_schema::identity::{FieldId, InputVariantId};
use meerkat_machine_schema::{
    FieldDisclosure, FieldSchema, InputFieldDomain, InputFieldDomainError, InputFieldDomainKind,
    MachineSchema, MachineSchemaError, TLC_MAX_UNSIGNED_INPUT_SAMPLE, TriggerMatch, TypeRef,
};

fn input(name: &str) -> InputVariantId {
    InputVariantId::parse(name).expect("input slug")
}

fn field(name: &str) -> FieldId {
    FieldId::parse(name).expect("field slug")
}

fn with_declaration(
    input_name: &str,
    field_name: &str,
    domain: InputFieldDomainKind,
) -> MachineSchema {
    let mut schema = dsl_workgraph_lifecycle_machine();
    schema.input_field_domains.push(InputFieldDomain {
        input: input(input_name),
        field: field(field_name),
        domain,
    });
    schema
}

fn refusal(schema: &MachineSchema) -> InputFieldDomainError {
    match schema.validate() {
        Err(MachineSchemaError::InvalidInputFieldDomain { reason, .. }) => reason,
        other => panic!("expected an input field domain refusal, got {other:?}"),
    }
}

fn values(values: &[u64]) -> InputFieldDomainKind {
    InputFieldDomainKind::AdditionalValues(values.iter().copied().collect::<BTreeSet<_>>())
}

#[test]
fn workgraph_declares_every_bound_expected_revision_as_the_current_revision() {
    let schema = dsl_workgraph_lifecycle_machine();
    schema
        .validate()
        .expect("the shipped declarations are valid");
    let bound_variants = schema
        .transitions
        .iter()
        .filter_map(|transition| match &transition.on {
            TriggerMatch::Input { variant, bindings }
                if bindings
                    .iter()
                    .any(|binding| binding.as_str() == "expected_revision") =>
            {
                Some(variant.as_str().to_owned())
            }
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    assert!(!bound_variants.is_empty());
    for variant in &bound_variants {
        assert_eq!(
            schema.input_field_domain(variant, "expected_revision"),
            Some(&InputFieldDomainKind::StateField(field("revision"))),
            "{variant}.expected_revision"
        );
    }
}

#[test]
fn additional_values_extend_a_bound_unsigned_field() {
    let schema = with_declaration("CreateOpen", "unresolved_blocker_count", values(&[3, 8]));
    schema
        .validate()
        .expect("a bound u64 field takes additional values");
}

#[test]
fn every_invalid_declaration_is_refused_with_its_reason() {
    assert_eq!(
        dsl_workgraph_lifecycle_machine()
            .input_field_domains
            .iter()
            .filter(|domain| domain.field.as_str() == "unresolved_blocker_count")
            .count(),
        0,
        "the fixture field starts undeclared"
    );
    assert_eq!(
        refusal(&with_declaration(
            "CreateOpen",
            "no_such_field",
            values(&[3])
        )),
        InputFieldDomainError::UnknownField
    );
    assert_eq!(
        refusal(&with_declaration(
            "CreateOpen",
            "unresolved_blocker_count",
            values(&[])
        )),
        InputFieldDomainError::EmptyValues
    );
    let too_big = TLC_MAX_UNSIGNED_INPUT_SAMPLE + 1;
    assert_eq!(
        refusal(&with_declaration(
            "CreateOpen",
            "unresolved_blocker_count",
            values(&[3, too_big])
        )),
        InputFieldDomainError::ValueOutOfRange { value: too_big }
    );
    assert_eq!(
        refusal(&with_declaration(
            "CreateOpen",
            "unresolved_blocker_count",
            InputFieldDomainKind::StateField(field("no_such_state"))
        )),
        InputFieldDomainError::UnknownStateField {
            state_field: "no_such_state".into()
        }
    );

    let schema = dsl_workgraph_lifecycle_machine();
    // A non-unsigned input field, and a state field of a different type.
    let (string_input, string_field) = schema
        .inputs
        .variants
        .iter()
        .find_map(|variant| {
            variant
                .fields
                .iter()
                .find(|field| !matches!(field.ty, TypeRef::U32 | TypeRef::U64))
                .map(|field| {
                    (
                        variant.name.as_str().to_owned(),
                        field.name.as_str().to_owned(),
                    )
                })
        })
        .expect("a non-unsigned input field");
    assert_eq!(
        refusal(&with_declaration(
            &string_input,
            &string_field,
            values(&[3])
        )),
        InputFieldDomainError::NotUnsigned
    );
    let non_u64_state = schema
        .state
        .fields
        .iter()
        .find(|field| field.ty != TypeRef::U64)
        .map(|field| field.name.as_str().to_owned())
        .expect("a non-u64 state field");
    assert_eq!(
        refusal(&with_declaration(
            "CreateOpen",
            "unresolved_blocker_count",
            InputFieldDomainKind::StateField(field(&non_u64_state))
        )),
        InputFieldDomainError::StateFieldTypeMismatch {
            state_field: non_u64_state
        }
    );

    // Declared twice.
    let mut twice = with_declaration("CreateOpen", "unresolved_blocker_count", values(&[3]));
    twice.input_field_domains.push(InputFieldDomain {
        input: input("CreateOpen"),
        field: field("unresolved_blocker_count"),
        domain: values(&[4]),
    });
    assert_eq!(refusal(&twice), InputFieldDomainError::Duplicate);

    // Declared on an input field no transition binds.
    let mut unbound = with_declaration("CreateOpen", "probe_sample", values(&[3]));
    unbound
        .inputs
        .variants
        .iter_mut()
        .find(|variant| variant.name.as_str() == "CreateOpen")
        .expect("CreateOpen input")
        .fields
        .push(FieldSchema {
            name: field("probe_sample"),
            ty: TypeRef::U64,
            disclosure: FieldDisclosure::Visible,
        });
    assert_eq!(refusal(&unbound), InputFieldDomainError::Unused);
}

#[test]
fn an_unknown_input_variant_is_refused() {
    assert_eq!(
        with_declaration("NoSuchInput", "expected_revision", values(&[3])).validate(),
        Err(MachineSchemaError::UnknownInputVariant {
            variant: "NoSuchInput".into()
        })
    );
}
