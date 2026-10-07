#![allow(clippy::expect_used, clippy::panic)]
use meerkat_machine_schema::identity::{FieldId, NamedTypeId};
use meerkat_machine_schema::{MachineSchema, MachineSchemaError, TlcValue};

fn schema() -> MachineSchema {
    meerkat_machine_schema::catalog::dsl::dsl_grant_authority_machine()
}
fn id(name: &str) -> NamedTypeId {
    NamedTypeId::parse(name).expect("type")
}
fn field(name: &str) -> FieldId {
    FieldId::parse(name).expect("field")
}

#[test]
fn grant_explicit_model_is_valid_and_covers_every_configured_identity() {
    let schema = schema();
    schema.validate().expect("valid canonical model");
    let model = schema.tlc_model.expect("explicit grant samples");
    assert!(model.require_ci_transition_coverage);
    for profile in [&model.ci, &model.deep] {
        let records = &profile.named_values[&id("GrantRecord")];
        for root in &profile.named_values[&id("GrantPrincipal")] {
            for incarnation in &profile.named_values[&id("GrantAuthorityIncarnation")] {
                assert!(
                    records
                        .iter()
                        .any(|value| matches!(value, TlcValue::Record(fields)
                    if fields[&field("issuer")] == *root
                    && fields[&field("authority_incarnation")] == *incarnation
                    && fields[&field("parent")] == TlcValue::None
                    && fields[&field("issued_revision")] == TlcValue::U64(1)))
                );
            }
        }
    }
}

#[test]
fn model_rejects_empty_unknown_and_duplicate_domains() {
    for mutation in 0..3 {
        let mut schema = schema();
        let values = &mut schema.tlc_model.as_mut().expect("model").ci.named_values;
        match mutation {
            0 => {
                values.insert(id("GrantRecord"), vec![]);
            }
            1 => {
                values.insert(id("MissingType"), vec![TlcValue::U64(1)]);
            }
            _ => {
                let first = values[&id("GrantRecord")][0].clone();
                values
                    .get_mut(&id("GrantRecord"))
                    .expect("records")
                    .push(first);
            }
        }
        assert!(matches!(
            schema.validate(),
            Err(MachineSchemaError::InvalidTlcModel { .. })
        ));
    }
}

#[test]
fn model_rejects_unknown_missing_and_mistyped_record_fields() {
    for mutation in 0..3 {
        let mut schema = schema();
        let record = &mut schema
            .tlc_model
            .as_mut()
            .expect("model")
            .ci
            .named_values
            .get_mut(&id("GrantRecord"))
            .expect("records")[0];
        let TlcValue::Record(fields) = record else {
            panic!("record");
        };
        match mutation {
            0 => {
                fields.insert(field("unexpected"), TlcValue::U64(0));
            }
            1 => {
                fields.remove(&field("id"));
            }
            _ => {
                fields.insert(field("issued_revision"), TlcValue::String("1".into()));
            }
        }
        assert!(matches!(
            schema.validate(),
            Err(MachineSchemaError::InvalidTlcModel { .. })
        ));
    }
}

#[test]
fn model_rejects_malformed_enum_payload_and_set_members() {
    for (name, value) in [
        (
            "LifetimeBound",
            TlcValue::Record([(field("tag"), TlcValue::String("Window".into()))].into()),
        ),
        (
            "LifetimeBound",
            TlcValue::Record([(field("tag"), TlcValue::String("UnknownVariant".into()))].into()),
        ),
        (
            "GrantUnresolved",
            TlcValue::Set(vec![TlcValue::String("NotDeclared".into())]),
        ),
        (
            "GrantUnresolved",
            TlcValue::Set(vec![TlcValue::String("Unknown".into()); 2]),
        ),
    ] {
        let mut schema = schema();
        schema
            .tlc_model
            .as_mut()
            .expect("model")
            .ci
            .named_values
            .insert(id(name), vec![value]);
        assert!(matches!(
            schema.validate(),
            Err(MachineSchemaError::InvalidTlcModel { .. })
        ));
    }
}

#[test]
fn all_other_canonical_owners_keep_implicit_model_defaults() {
    for schema in meerkat_machine_schema::canonical_machine_schemas() {
        if schema.machine.as_str() != "GrantAuthorityMachine" {
            assert!(
                schema.tlc_model.is_none(),
                "{} unexpectedly opted in",
                schema.machine
            );
        }
    }
}

#[test]
fn explicit_model_refuses_unsupported_integer_and_limit_literals() {
    let mut schema = schema();
    let value = &mut schema
        .tlc_model
        .as_mut()
        .expect("model")
        .ci
        .named_values
        .get_mut(&id("GrantRecord"))
        .expect("records")[0];
    let TlcValue::Record(fields) = value else {
        panic!("record");
    };
    fields.insert(field("issued_revision"), TlcValue::U64(i32::MAX as u64 + 1));
    assert!(matches!(
        schema.validate(),
        Err(MachineSchemaError::InvalidTlcModel { .. })
    ));
    for selected in 0..4 {
        let mut schema = meerkat_machine_schema::catalog::dsl::dsl_grant_authority_machine();
        let limits = &mut schema.tlc_model.as_mut().expect("model").ci_limits;
        match selected {
            0 => limits.step_limit = u32::MAX,
            1 => limits.seq_limit = u32::MAX,
            2 => limits.set_limit = u32::MAX,
            _ => limits.map_limit = u32::MAX,
        }
        assert!(matches!(
            schema.validate(),
            Err(MachineSchemaError::InvalidTlcModel { .. })
        ));
    }
}
