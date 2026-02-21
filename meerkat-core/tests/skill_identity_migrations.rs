#![allow(clippy::expect_used)]

use meerkat_core::skills::{
    SkillKey, SkillKeyRemap, SkillName, SkillRef, SourceIdentityLineage,
    SourceIdentityLineageEvent, SourceIdentityRecord, SourceIdentityRegistry, SourceIdentityStatus,
    SourceTransportKind, SourceUuid,
};

fn source_uuid(raw: &str) -> SourceUuid {
    SourceUuid::parse(raw).expect("valid uuid")
}

fn skill_name(raw: &str) -> SkillName {
    SkillName::parse(raw).expect("valid skill name")
}

fn key(source: &str, name: &str) -> SkillKey {
    SkillKey {
        source_uuid: source_uuid(source),
        skill_name: skill_name(name),
    }
}

fn record(source: &str, fp: &str) -> SourceIdentityRecord {
    SourceIdentityRecord {
        source_uuid: source_uuid(source),
        display_name: source.to_string(),
        transport_kind: SourceTransportKind::Filesystem,
        fingerprint: fp.to_string(),
        status: SourceIdentityStatus::Active,
    }
}

#[test]
fn rotate_migration_resolves_old_ref_through_remap() {
    let registry = SourceIdentityRegistry::build(
        vec![
            record("dc256086-0d2f-4f61-a307-320d4148107f", "fp-1"),
            record("a93d587d-8f44-438f-8189-6e8cf549f6e7", "fp-1"),
        ],
        vec![SourceIdentityLineage {
            event_id: "rotate-1".to_string(),
            recorded_at_unix_secs: 1,
            required_from_skills: vec![skill_name("email-extractor")],
            event: SourceIdentityLineageEvent::Rotate {
                from: source_uuid("dc256086-0d2f-4f61-a307-320d4148107f"),
                to: source_uuid("a93d587d-8f44-438f-8189-6e8cf549f6e7"),
            },
        }],
        vec![SkillKeyRemap {
            from: key("dc256086-0d2f-4f61-a307-320d4148107f", "email-extractor"),
            to: key("a93d587d-8f44-438f-8189-6e8cf549f6e7", "mail-extractor"),
            reason: Some("rotation".to_string()),
        }],
        vec![],
    )
    .expect("registry");

    let resolved = registry
        .resolve_skill_ref(&SkillRef::Legacy(
            "dc256086-0d2f-4f61-a307-320d4148107f/email-extractor".to_string(),
        ))
        .expect("old ref should resolve");
    assert_eq!(
        resolved,
        key("a93d587d-8f44-438f-8189-6e8cf549f6e7", "mail-extractor")
    );
}

#[test]
fn split_migration_requires_remap_table() {
    let result = SourceIdentityRegistry::build(
        vec![
            record("dc256086-0d2f-4f61-a307-320d4148107f", "fp-1"),
            record("a93d587d-8f44-438f-8189-6e8cf549f6e7", "fp-1"),
            record("e8df561d-d38f-4242-af55-3a6efb34c950", "fp-1"),
        ],
        vec![SourceIdentityLineage {
            event_id: "split-1".to_string(),
            recorded_at_unix_secs: 1,
            required_from_skills: vec![skill_name("email-extractor")],
            event: SourceIdentityLineageEvent::Split {
                from: source_uuid("dc256086-0d2f-4f61-a307-320d4148107f"),
                into: vec![
                    source_uuid("a93d587d-8f44-438f-8189-6e8cf549f6e7"),
                    source_uuid("e8df561d-d38f-4242-af55-3a6efb34c950"),
                ],
            },
        }],
        vec![],
        vec![],
    );

    assert!(result.is_err(), "split without remaps must be rejected");
}

#[test]
fn split_migration_resolves_old_refs_when_remaps_present() {
    let registry = SourceIdentityRegistry::build(
        vec![
            record("dc256086-0d2f-4f61-a307-320d4148107f", "fp-1"),
            record("a93d587d-8f44-438f-8189-6e8cf549f6e7", "fp-1"),
            record("e8df561d-d38f-4242-af55-3a6efb34c950", "fp-1"),
        ],
        vec![SourceIdentityLineage {
            event_id: "split-1".to_string(),
            recorded_at_unix_secs: 1,
            required_from_skills: vec![skill_name("email-extractor")],
            event: SourceIdentityLineageEvent::Split {
                from: source_uuid("dc256086-0d2f-4f61-a307-320d4148107f"),
                into: vec![
                    source_uuid("a93d587d-8f44-438f-8189-6e8cf549f6e7"),
                    source_uuid("e8df561d-d38f-4242-af55-3a6efb34c950"),
                ],
            },
        }],
        vec![
            SkillKeyRemap {
                from: key("dc256086-0d2f-4f61-a307-320d4148107f", "email-extractor"),
                to: key("a93d587d-8f44-438f-8189-6e8cf549f6e7", "mail-extractor"),
                reason: None,
            },
            SkillKeyRemap {
                from: key("dc256086-0d2f-4f61-a307-320d4148107f", "pdf-processing"),
                to: key("e8df561d-d38f-4242-af55-3a6efb34c950", "pdf-processing"),
                reason: None,
            },
        ],
        vec![],
    )
    .expect("registry");

    let resolved = registry
        .resolve_skill_ref(&SkillRef::Legacy(
            "dc256086-0d2f-4f61-a307-320d4148107f/pdf-processing".to_string(),
        ))
        .expect("split remap");
    assert_eq!(
        resolved,
        key("e8df561d-d38f-4242-af55-3a6efb34c950", "pdf-processing")
    );
}

#[test]
fn split_migration_rejects_partial_target_coverage() {
    let result = SourceIdentityRegistry::build(
        vec![
            record("dc256086-0d2f-4f61-a307-320d4148107f", "fp-1"),
            record("a93d587d-8f44-438f-8189-6e8cf549f6e7", "fp-1"),
            record("e8df561d-d38f-4242-af55-3a6efb34c950", "fp-1"),
        ],
        vec![SourceIdentityLineage {
            event_id: "split-1".to_string(),
            recorded_at_unix_secs: 1,
            required_from_skills: vec![skill_name("email-extractor")],
            event: SourceIdentityLineageEvent::Split {
                from: source_uuid("dc256086-0d2f-4f61-a307-320d4148107f"),
                into: vec![
                    source_uuid("a93d587d-8f44-438f-8189-6e8cf549f6e7"),
                    source_uuid("e8df561d-d38f-4242-af55-3a6efb34c950"),
                ],
            },
        }],
        vec![SkillKeyRemap {
            from: key("dc256086-0d2f-4f61-a307-320d4148107f", "email-extractor"),
            to: key("a93d587d-8f44-438f-8189-6e8cf549f6e7", "mail-extractor"),
            reason: None,
        }],
        vec![],
    );

    assert!(
        result.is_err(),
        "split with partial target coverage must fail"
    );
}

#[test]
fn merge_migration_requires_remap_table() {
    let result = SourceIdentityRegistry::build(
        vec![
            record("dc256086-0d2f-4f61-a307-320d4148107f", "fp-1"),
            record("a93d587d-8f44-438f-8189-6e8cf549f6e7", "fp-1"),
            record("e8df561d-d38f-4242-af55-3a6efb34c950", "fp-1"),
        ],
        vec![SourceIdentityLineage {
            event_id: "merge-1".to_string(),
            recorded_at_unix_secs: 1,
            required_from_skills: vec![skill_name("email-extractor")],
            event: SourceIdentityLineageEvent::Merge {
                from: vec![
                    source_uuid("dc256086-0d2f-4f61-a307-320d4148107f"),
                    source_uuid("a93d587d-8f44-438f-8189-6e8cf549f6e7"),
                ],
                to: source_uuid("e8df561d-d38f-4242-af55-3a6efb34c950"),
            },
        }],
        vec![],
        vec![],
    );

    assert!(result.is_err(), "merge without remaps must be rejected");
}

#[test]
fn merge_migration_resolves_old_refs_when_remaps_present() {
    let registry = SourceIdentityRegistry::build(
        vec![
            record("dc256086-0d2f-4f61-a307-320d4148107f", "fp-1"),
            record("a93d587d-8f44-438f-8189-6e8cf549f6e7", "fp-1"),
            record("e8df561d-d38f-4242-af55-3a6efb34c950", "fp-1"),
        ],
        vec![SourceIdentityLineage {
            event_id: "merge-1".to_string(),
            recorded_at_unix_secs: 1,
            required_from_skills: vec![skill_name("email-extractor")],
            event: SourceIdentityLineageEvent::Merge {
                from: vec![
                    source_uuid("dc256086-0d2f-4f61-a307-320d4148107f"),
                    source_uuid("a93d587d-8f44-438f-8189-6e8cf549f6e7"),
                ],
                to: source_uuid("e8df561d-d38f-4242-af55-3a6efb34c950"),
            },
        }],
        vec![
            SkillKeyRemap {
                from: key("dc256086-0d2f-4f61-a307-320d4148107f", "email-extractor"),
                to: key("e8df561d-d38f-4242-af55-3a6efb34c950", "email-extractor"),
                reason: None,
            },
            SkillKeyRemap {
                from: key("a93d587d-8f44-438f-8189-6e8cf549f6e7", "email-extractor"),
                to: key("e8df561d-d38f-4242-af55-3a6efb34c950", "email-extractor"),
                reason: None,
            },
        ],
        vec![],
    )
    .expect("registry");

    let resolved = registry
        .resolve_skill_ref(&SkillRef::Legacy(
            "a93d587d-8f44-438f-8189-6e8cf549f6e7/email-extractor".to_string(),
        ))
        .expect("merge remap");
    assert_eq!(
        resolved,
        key("e8df561d-d38f-4242-af55-3a6efb34c950", "email-extractor")
    );
}

#[test]
fn merge_migration_rejects_partial_origin_coverage() {
    let result = SourceIdentityRegistry::build(
        vec![
            record("dc256086-0d2f-4f61-a307-320d4148107f", "fp-1"),
            record("a93d587d-8f44-438f-8189-6e8cf549f6e7", "fp-1"),
            record("e8df561d-d38f-4242-af55-3a6efb34c950", "fp-1"),
        ],
        vec![SourceIdentityLineage {
            event_id: "merge-1".to_string(),
            recorded_at_unix_secs: 1,
            required_from_skills: vec![skill_name("email-extractor"), skill_name("pdf-processing")],
            event: SourceIdentityLineageEvent::Merge {
                from: vec![
                    source_uuid("dc256086-0d2f-4f61-a307-320d4148107f"),
                    source_uuid("a93d587d-8f44-438f-8189-6e8cf549f6e7"),
                ],
                to: source_uuid("e8df561d-d38f-4242-af55-3a6efb34c950"),
            },
        }],
        vec![SkillKeyRemap {
            from: key("dc256086-0d2f-4f61-a307-320d4148107f", "email-extractor"),
            to: key("e8df561d-d38f-4242-af55-3a6efb34c950", "email-extractor"),
            reason: None,
        }],
        vec![],
    );

    assert!(
        result.is_err(),
        "merge with partial origin coverage must fail"
    );
}
