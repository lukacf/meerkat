//! Filesystem skill-source behavioural tests (C-T §6 #13).
//!
//! The pre-wave-a filesystem suite covered quarantine diagnostics retention,
//! collection-md fallback, and invalid-ratio health transitions. In the
//! wave-c retype the `FilesystemSkillSource` is narrower: it recursively
//! scans `<root>/<skill_dir>/SKILL.md`, constructs a typed `SkillKey` from
//! the configured `source_uuid` + directory slug, and fails `load()` for a
//! mismatched source_uuid. The old "slash path parsing" and "collection.md
//! fallback" paths are gone (source docstring at `meerkat-skills/src/source/
//! filesystem.rs:3`). These tests cover the surface that actually exists in
//! wave-c — recursive scan finds nested SKILL.md files, `list()` applies
//! the `SkillFilter`, and `load()` enforces source-uuid matching.
//!
//! See `docs/wave-c-prep/test-coverage-audit.md` §6 #13 for the full
//! audit context.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use meerkat_core::skills::{SkillFilter, SkillKey, SkillName, SkillScope, SkillSource, SourceUuid};
use meerkat_skills::FilesystemSkillSource;
use std::fs;
use std::path::Path;

fn write_skill(root: &Path, skill: &str, body: &str) {
    let dir = root.join(skill);
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("SKILL.md"),
        format!(
            "---\n\
             name: {skill}\n\
             description: {skill} description\n\
             ---\n\
             {body}\n"
        ),
    )
    .unwrap();
}

fn write_nested_skill(root: &Path, nested: &str, skill: &str) {
    let dir = root.join(nested).join(skill);
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("SKILL.md"),
        format!(
            "---\n\
             name: {skill}\n\
             description: nested {skill}\n\
             ---\n\
             Nested body for {skill}.\n"
        ),
    )
    .unwrap();
}

#[tokio::test]
async fn recursive_scan_finds_top_level_and_nested_skills() {
    let tmp = tempfile::tempdir().unwrap();
    let source_uuid = SourceUuid::parse("dc256086-0d2f-4f61-a307-320d4148107f").unwrap();

    // Two top-level skill dirs + one skill nested two levels deep.
    write_skill(tmp.path(), "alpha", "alpha body");
    write_skill(tmp.path(), "beta", "beta body");
    write_nested_skill(tmp.path(), "cat-a/sub", "gamma");

    let source = FilesystemSkillSource::new_with_identity(
        tmp.path().to_path_buf(),
        SkillScope::Project,
        source_uuid.clone(),
        Default::default(),
    );

    let listed = source.list(&SkillFilter::default()).await.unwrap();

    let names: Vec<String> = listed
        .iter()
        .map(|d| d.key.skill_name.as_str().to_owned())
        .collect();
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(
        sorted,
        vec!["alpha".to_string(), "beta".to_string(), "gamma".to_string()],
        "recursive scan must surface top-level and nested SKILL.md dirs; got {names:?}",
    );

    for descriptor in &listed {
        assert_eq!(
            descriptor.key.source_uuid, source_uuid,
            "every filesystem skill inherits the configured source_uuid",
        );
        assert_eq!(descriptor.scope, SkillScope::Project);
    }
}

#[tokio::test]
async fn list_applies_query_filter_case_insensitive() {
    let tmp = tempfile::tempdir().unwrap();
    let source_uuid = SourceUuid::parse("dc256086-0d2f-4f61-a307-320d4148107f").unwrap();

    write_skill(tmp.path(), "alpha", "body");
    write_skill(tmp.path(), "beta", "body");

    let source = FilesystemSkillSource::new_with_identity(
        tmp.path().to_path_buf(),
        SkillScope::Project,
        source_uuid,
        Default::default(),
    );

    let filter = SkillFilter {
        query: Some("ALPHA".to_string()),
        source_uuid: None,
    };
    let listed = source.list(&filter).await.unwrap();
    assert_eq!(listed.len(), 1, "query filter must narrow results");
    assert_eq!(listed[0].key.skill_name.as_str(), "alpha");
}

#[tokio::test]
async fn load_rejects_mismatched_source_uuid() {
    let tmp = tempfile::tempdir().unwrap();
    let configured_uuid = SourceUuid::parse("dc256086-0d2f-4f61-a307-320d4148107f").unwrap();
    let other_uuid = SourceUuid::parse("fe52aa61-1111-4a22-9999-bbbbbbbbbbbb").unwrap();

    write_skill(tmp.path(), "alpha", "body");

    let source = FilesystemSkillSource::new_with_identity(
        tmp.path().to_path_buf(),
        SkillScope::Project,
        configured_uuid.clone(),
        Default::default(),
    );

    // Pre-condition: the skill loads under its own source_uuid.
    let correct_key = SkillKey {
        source_uuid: configured_uuid.clone(),
        skill_name: SkillName::parse("alpha").unwrap(),
    };
    let doc = source.load(&correct_key).await.unwrap();
    assert_eq!(doc.descriptor.key, correct_key);

    // A key with a different source_uuid must NOT resolve — the filesystem
    // source owns exactly one source_uuid and rejects everything else.
    let wrong_key = SkillKey {
        source_uuid: other_uuid,
        skill_name: SkillName::parse("alpha").unwrap(),
    };
    let err = source.load(&wrong_key).await.unwrap_err();
    assert!(
        matches!(err, meerkat_core::skills::SkillError::NotFound { .. }),
        "mismatched source_uuid must surface SkillError::NotFound, got {err:?}",
    );
}

#[tokio::test]
async fn load_returns_notfound_for_missing_directory() {
    let tmp = tempfile::tempdir().unwrap();
    let source_uuid = SourceUuid::parse("dc256086-0d2f-4f61-a307-320d4148107f").unwrap();

    let source = FilesystemSkillSource::new_with_identity(
        tmp.path().to_path_buf(),
        SkillScope::Project,
        source_uuid.clone(),
        Default::default(),
    );

    let key = SkillKey {
        source_uuid,
        skill_name: SkillName::parse("missing").unwrap(),
    };
    let err = source.load(&key).await.unwrap_err();
    assert!(
        matches!(err, meerkat_core::skills::SkillError::NotFound { .. }),
        "load() for a directory with no SKILL.md must surface NotFound, got {err:?}",
    );
}

#[tokio::test]
async fn directories_without_skill_md_are_silently_skipped_in_list() {
    let tmp = tempfile::tempdir().unwrap();
    let source_uuid = SourceUuid::parse("dc256086-0d2f-4f61-a307-320d4148107f").unwrap();

    write_skill(tmp.path(), "alpha", "body");
    // Sibling directory with no SKILL.md must be ignored by the recursive
    // scan — it's treated as "not a skill container", not an error.
    fs::create_dir_all(tmp.path().join("not-a-skill")).unwrap();
    fs::write(
        tmp.path().join("not-a-skill/README.md"),
        "this is not a skill directory",
    )
    .unwrap();

    let source = FilesystemSkillSource::new_with_identity(
        tmp.path().to_path_buf(),
        SkillScope::Project,
        source_uuid,
        Default::default(),
    );
    let listed = source.list(&SkillFilter::default()).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].key.skill_name.as_str(), "alpha");
}
