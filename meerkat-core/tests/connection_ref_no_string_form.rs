#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

//! Wave B (V6): `ConnectionRef` has no string form.
//!
//! `ConnectionRef::parse(...)` is gone, and the core type no longer
//! implements `Display`. Callers must construct a `ConnectionRef` from
//! typed `RealmId` / `BindingId` / `ProfileId` newtypes built at the
//! (single) CLI input boundary. This test is both a behavioral check and
//! an anti-regression gate: attempts to reintroduce the parser or the
//! display will fail this test's serde and construction contracts.

use meerkat_core::ConnectionRef;
use meerkat_core::connection::{BindingId, ProfileId, RealmId};

#[test]
fn connection_ref_constructs_from_typed_ids_only() {
    let c = ConnectionRef {
        realm: RealmId::parse("dev").expect("valid"),
        binding: BindingId::parse("gpt5_default").expect("valid"),
        profile: None,
    };
    assert_eq!(c.realm.as_str(), "dev");
    assert_eq!(c.binding.as_str(), "gpt5_default");
    assert!(c.profile.is_none());
}

#[test]
fn connection_ref_serde_uses_struct_shape_not_string() {
    let c = ConnectionRef {
        realm: RealmId::parse("prod").expect("valid"),
        binding: BindingId::parse("openai_main").expect("valid"),
        profile: Some(ProfileId::parse("ci").expect("valid")),
    };
    let s = serde_json::to_string(&c).expect("serialize");
    // Typed fields — realm/binding/profile — not a colon-joined scalar.
    assert!(s.contains("\"realm\":\"prod\""), "serialized: {s}");
    assert!(s.contains("\"binding\":\"openai_main\""), "serialized: {s}");
    assert!(s.contains("\"profile\":\"ci\""), "serialized: {s}");
    assert!(!s.contains("prod:openai_main"), "serialized: {s}");

    let back: ConnectionRef = serde_json::from_str(&s).expect("deserialize");
    assert_eq!(back, c);
}

#[test]
fn connection_ref_rejects_empty_or_invalid_slugs() {
    assert!(RealmId::parse("").is_err());
    assert!(RealmId::parse("has space").is_err());
    assert!(BindingId::parse("colon:inside").is_err());
    // Valid slugs
    assert!(RealmId::parse("dev").is_ok());
    assert!(BindingId::parse("openai_default.v1").is_ok());
    assert!(ProfileId::parse("override-ci").is_ok());
}
