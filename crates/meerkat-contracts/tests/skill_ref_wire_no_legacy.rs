#![allow(clippy::expect_used)]

//! B-7 V4: the wire `SkillRef` type is single-variant (`Structured`) and
//! refuses every legacy string encoding. Round-trip asserts the shape is
//! a tagged object with the `kind: "structured"` discriminator.

use meerkat_contracts::SkillsParams;
use meerkat_core::skills::{SkillKey, SkillName, SkillRef, SourceUuid};

fn test_key(slug: &str) -> SkillKey {
    SkillKey {
        source_uuid: SourceUuid::parse("dc256086-0d2f-4f61-a307-320d4148107f").expect("valid uuid"),
        skill_name: SkillName::parse(slug).expect("valid slug"),
    }
}

#[test]
fn structured_skill_ref_round_trips_as_tagged_object() {
    let key = test_key("email-extractor");
    let r = SkillRef::Structured(key.clone());
    let json = serde_json::to_value(&r).expect("serialize");
    assert_eq!(json["kind"], "structured");
    assert_eq!(json["source_uuid"], "dc256086-0d2f-4f61-a307-320d4148107f");
    assert_eq!(json["skill_name"], "email-extractor");

    let decoded: SkillRef = serde_json::from_value(json).expect("deserialize");
    assert_eq!(decoded, r);
    assert_eq!(decoded.into_key(), key);
}

#[test]
fn legacy_string_form_is_rejected_at_skill_ref_boundary() {
    let res: Result<SkillRef, _> =
        serde_json::from_str("\"dc256086-0d2f-4f61-a307-320d4148107f/email-extractor\"");
    assert!(
        res.is_err(),
        "legacy slash-delimited string must not deserialize as a SkillRef",
    );
}

#[test]
fn skills_params_rejects_legacy_string_skill_refs() {
    let legacy = r#"{"skill_refs":["dc256086-0d2f-4f61-a307-320d4148107f/email-extractor"]}"#;
    let res: Result<SkillsParams, _> = serde_json::from_str(legacy);
    assert!(
        res.is_err(),
        "SkillsParams must refuse legacy string refs on the wire",
    );
}

#[test]
fn skills_params_round_trips_typed_key() {
    let key = test_key("email-extractor");
    let params = SkillsParams {
        preload_skills: Some(vec![key.clone()]),
        skill_refs: Some(vec![SkillRef::Structured(key.clone())]),
    };
    let json = serde_json::to_string(&params).expect("serialize");
    let parsed: SkillsParams = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(parsed, params);

    let canonical = parsed.canonical_skill_keys().expect("keys");
    assert_eq!(canonical, vec![key]);
}
