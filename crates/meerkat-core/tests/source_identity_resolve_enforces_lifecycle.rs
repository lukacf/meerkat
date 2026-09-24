#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

//! B-7 V4: `SourceIdentityRegistry::resolve` refuses Disabled/Retired sources
//! with a typed `ResolveError::SourceDisabled`, regardless of whether the
//! disabled source is reached via direct key or through a remap chain.

use meerkat_core::skills::{
    ResolveError, SkillKey, SkillKeyRemap, SkillName, SourceIdentityLineage,
    SourceIdentityLineageEvent, SourceIdentityRecord, SourceIdentityRegistry, SourceIdentityStatus,
    SourceTransportKind, SourceUuid,
};

fn source_uuid(raw: &str) -> SourceUuid {
    SourceUuid::parse(raw).expect("valid source uuid")
}

fn key(source_raw: &str, slug: &str) -> SkillKey {
    SkillKey {
        source_uuid: source_uuid(source_raw),
        skill_name: SkillName::parse(slug).expect("valid slug"),
    }
}

fn record(
    source_raw: &str,
    fingerprint: &str,
    status: SourceIdentityStatus,
) -> SourceIdentityRecord {
    SourceIdentityRecord {
        source_uuid: source_uuid(source_raw),
        display_name: "test".into(),
        transport_kind: SourceTransportKind::Filesystem,
        fingerprint: fingerprint.into(),
        status,
    }
}

#[test]
fn resolve_rejects_disabled_source_with_typed_error() {
    let registry = SourceIdentityRegistry::build(
        vec![record(
            "dc256086-0d2f-4f61-a307-320d4148107f",
            "fp-a",
            SourceIdentityStatus::Disabled,
        )],
        vec![],
        vec![],
        vec![],
    )
    .expect("registry should build");

    let err = registry
        .resolve(&key(
            "dc256086-0d2f-4f61-a307-320d4148107f",
            "email-extractor",
        ))
        .expect_err("disabled source must be rejected");

    match err {
        ResolveError::SourceDisabled { status, .. } => {
            assert_eq!(status, SourceIdentityStatus::Disabled);
        }
        other => panic!("expected SourceDisabled, got {other:?}"),
    }
}

#[test]
fn resolve_rejects_retired_source_with_typed_error() {
    let registry = SourceIdentityRegistry::build(
        vec![record(
            "dc256086-0d2f-4f61-a307-320d4148107f",
            "fp-a",
            SourceIdentityStatus::Retired,
        )],
        vec![],
        vec![],
        vec![],
    )
    .expect("registry should build");

    let err = registry
        .resolve(&key(
            "dc256086-0d2f-4f61-a307-320d4148107f",
            "pdf-processing",
        ))
        .expect_err("retired source must be rejected");

    assert!(matches!(
        err,
        ResolveError::SourceDisabled {
            status: SourceIdentityStatus::Retired,
            ..
        }
    ));
}

#[test]
fn resolve_accepts_active_source() {
    let registry = SourceIdentityRegistry::build(
        vec![record(
            "dc256086-0d2f-4f61-a307-320d4148107f",
            "fp-a",
            SourceIdentityStatus::Active,
        )],
        vec![],
        vec![],
        vec![],
    )
    .expect("registry should build");

    let resolved = registry
        .resolve(&key(
            "dc256086-0d2f-4f61-a307-320d4148107f",
            "email-extractor",
        ))
        .expect("active source must resolve");
    assert_eq!(resolved.source.status, SourceIdentityStatus::Active);
}

#[test]
fn resolve_follows_remap_then_rejects_disabled_target() {
    let source_old = "dc256086-0d2f-4f61-a307-320d4148107f";
    let source_new = "a93d587d-8f44-438f-8189-6e8cf549f6e7";

    let registry = SourceIdentityRegistry::build(
        vec![
            record(source_old, "fp-a", SourceIdentityStatus::Active),
            record(source_new, "fp-a", SourceIdentityStatus::Disabled),
        ],
        vec![SourceIdentityLineage {
            event_id: "rotate-1".into(),
            recorded_at_unix_secs: 1,
            required_from_skills: vec![SkillName::parse("email-extractor").expect("slug")],
            event: SourceIdentityLineageEvent::Rotate {
                from: source_uuid(source_old),
                to: source_uuid(source_new),
            },
        }],
        vec![SkillKeyRemap {
            from: key(source_old, "email-extractor"),
            to: key(source_new, "mail-extractor"),
            reason: None,
        }],
        vec![],
    )
    .expect("registry should build");

    let err = registry
        .resolve(&key(source_old, "email-extractor"))
        .expect_err("remapped-then-disabled must be rejected");

    assert!(
        matches!(err, ResolveError::RemappedButTargetDisabled { .. }),
        "expected RemappedButTargetDisabled, got {err:?}",
    );
}

#[test]
fn resolve_rejects_unknown_source() {
    let registry = SourceIdentityRegistry::default();
    let err = registry
        .resolve(&key(
            "dc256086-0d2f-4f61-a307-320d4148107f",
            "email-extractor",
        ))
        .expect_err("unknown source must be rejected");
    assert!(matches!(err, ResolveError::SourceUnknown(_)));
}
