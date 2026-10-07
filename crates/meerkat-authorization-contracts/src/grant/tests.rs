#![allow(clippy::expect_used)]

use super::{GrantAuthorityIncarnation, GrantLineageRef, GrantReferenceError};
use meerkat_core::auth::{PrincipalKind, PrincipalRef, TrustDomainId};
use serde_json::json;

const WIRE: &str = r#"{"root_authority":{"kind":"service_account","id":"root","qualification":{"kind":"qualified","trust_domain_id":"grant-fixture"}},"authority_namespace":"namespace","authority_generation":1,"authority_incarnation":"8ce78497-72c4-4f5e-8e28-4570e54d4b5b","grant_id":"grant","issued_revision":7}"#;

fn reference() -> GrantLineageRef {
    GrantLineageRef {
        root_authority: PrincipalRef::in_domain(
            PrincipalKind::ServiceAccount,
            "root",
            TrustDomainId::new("grant-fixture").expect("domain"),
        )
        .expect("principal"),
        authority_namespace: crate::evidence::EvidenceId::new("namespace").expect("namespace"),
        authority_generation: 1,
        authority_incarnation: GrantAuthorityIncarnation::from_uuid(
            uuid::Uuid::parse_str("8ce78497-72c4-4f5e-8e28-4570e54d4b5b").expect("UUID"),
        )
        .expect("v4 incarnation"),
        grant_id: crate::evidence::EvidenceId::new("grant").expect("grant"),
        issued_revision: 7,
    }
}

#[test]
fn moved_reference_keeps_literal_wire_equality_and_protected_debug() {
    let reference = reference();
    assert_eq!(serde_json::to_string(&reference).expect("encode"), WIRE);
    assert_eq!(
        serde_json::from_str::<GrantLineageRef>(WIRE).expect("decode"),
        reference
    );
    assert_eq!(reference.validate(), Ok(()));
    assert_eq!(format!("{reference:?}"), "GrantLineageRef([protected])");
}

#[test]
fn moved_reference_rejects_unknown_missing_duplicate_and_malformed_fields() {
    let wire = serde_json::to_value(reference()).expect("wire");
    for key in [
        "root_authority",
        "authority_namespace",
        "authority_generation",
        "authority_incarnation",
        "grant_id",
        "issued_revision",
    ] {
        let mut missing = wire.clone();
        missing.as_object_mut().expect("object").remove(key);
        assert!(
            serde_json::from_value::<GrantLineageRef>(missing).is_err(),
            "{key}"
        );
    }
    let mut unknown = wire.clone();
    unknown["approved"] = json!(true);
    assert!(serde_json::from_value::<GrantLineageRef>(unknown).is_err());
    let duplicate = WIRE.replace(
        "\"issued_revision\":7",
        "\"issued_revision\":7,\"issued_revision\":8",
    );
    assert!(serde_json::from_str::<GrantLineageRef>(&duplicate).is_err());
    let mut malformed = wire;
    malformed["authority_namespace"] = json!("");
    assert!(serde_json::from_value::<GrantLineageRef>(malformed).is_err());
}

#[test]
fn validation_remains_data_only_and_preserves_error_precedence() {
    let mut value = reference();
    value.issued_revision = 0;
    let decoded: GrantLineageRef =
        serde_json::from_str(&serde_json::to_string(&value).expect("encode"))
            .expect("candidate decode");
    assert_eq!(decoded.validate(), Ok(()));

    value.authority_generation = 0;
    assert_eq!(value.validate(), Err(GrantReferenceError::Shape));
    value.root_authority =
        PrincipalRef::new(PrincipalKind::ServiceAccount, "legacy").expect("legacy principal");
    assert_eq!(value.validate(), Err(GrantReferenceError::Unqualified));
    value.authority_generation = 1;
    assert_eq!(value.validate(), Err(GrantReferenceError::Unqualified));
}

#[test]
fn incarnation_wire_rejects_missing_malformed_and_non_v4_values() {
    let good = reference();
    let encoded = serde_json::to_value(good.authority_incarnation).expect("incarnation wire");
    assert_eq!(
        serde_json::from_value::<GrantAuthorityIncarnation>(encoded).expect("roundtrip"),
        good.authority_incarnation
    );
    assert_eq!(
        format!("{:?}", good.authority_incarnation),
        "GrantAuthorityIncarnation([protected])"
    );
    for value in [
        json!(null),
        json!(""),
        json!("not-a-uuid"),
        json!("00000000-0000-0000-0000-000000000000"),
        json!("8ce78497-72c4-1f5e-8e28-4570e54d4b5b"),
        json!("8ce78497-72c4-4f5e-0e28-4570e54d4b5b"),
        json!({"uuid":"8ce78497-72c4-4f5e-8e28-4570e54d4b5b"}),
    ] {
        let mut wire = serde_json::to_value(&good).expect("reference wire");
        wire["authority_incarnation"] = value;
        assert!(serde_json::from_value::<GrantLineageRef>(wire).is_err());
    }
}
