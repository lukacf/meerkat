//! Skill-resolver precedence tests (C-T §6 #14).
//!
//! The pre-wave-a resolver suite covered "defaults, mixed repos,
//! explicit-roots precedence, no-roots-skips-filesystem-defaults". In
//! wave-c the resolver is narrower: `resolve_repositories_with_roots`
//! resolves **only the repositories declared in `SkillsConfig.repositories`**.
//! There are no built-in "default" filesystem repos injected by the
//! resolver — `SkillsConfig.default()` leaves `repositories` empty, so
//! callers who want any source at all configure one explicitly. The
//! user-vs-project config merge (which runs ahead of resolution via
//! `SkillsConfig::load_from_paths`) is covered by in-file tests at
//! `meerkat-core/src/skills_config.rs:620,679`.
//!
//! This file covers the resolver's narrow contract:
//! - `enabled = false` → `Ok(None)` (skip resolution entirely).
//! - filesystem repo with **relative path** resolves against
//!   `context_root` when provided, falling back to `cache_root`, falling
//!   back to `"."`. Non-filesystem transports are wave-c stubs and do
//!   not produce a source yet (logged warning only; the resolver still
//!   returns an empty composite).
//! - filesystem repo with **absolute path** ignores the roots entirely.
//! - the resolved source lists what's on disk under the fully-resolved
//!   path, not just what the config declares.
//!
//! See `docs/wave-c-prep/test-coverage-audit.md` §6 #14.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use meerkat_core::skills::{SkillFilter, SkillSource, SourceUuid};
use meerkat_core::skills_config::{SkillRepoTransport, SkillRepositoryConfig, SkillsConfig};
use meerkat_skills::resolve_repositories_with_roots;
use std::fs;
use std::path::Path;

fn write_skill(root: &Path, skill: &str) {
    let dir = root.join(skill);
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("SKILL.md"),
        format!(
            "---\n\
             name: {skill}\n\
             description: {skill}\n\
             ---\n\
             Body for {skill}.\n"
        ),
    )
    .unwrap();
}

fn source_uuid(raw: &str) -> SourceUuid {
    SourceUuid::parse(raw).unwrap()
}

fn fs_repo(name: &str, uuid: &str, path: &str) -> SkillRepositoryConfig {
    SkillRepositoryConfig {
        name: name.to_string(),
        source_uuid: source_uuid(uuid),
        transport: SkillRepoTransport::Filesystem {
            path: path.to_string(),
        },
    }
}

#[tokio::test]
async fn disabled_config_skips_resolution_entirely() {
    let cfg = SkillsConfig {
        enabled: false,
        repositories: vec![fs_repo(
            "project",
            "dc256086-0d2f-4f61-a307-320d4148107f",
            "skills",
        )],
        ..Default::default()
    };

    let result = resolve_repositories_with_roots(&cfg, None, None, None)
        .await
        .unwrap();
    assert!(
        result.is_none(),
        "enabled=false must short-circuit to None; got Some(composite)",
    );
}

#[tokio::test]
async fn empty_repository_list_yields_empty_composite() {
    // Enabled but with zero configured repos — wave-c does NOT inject
    // built-in filesystem defaults. The resolver returns `Some(composite)`
    // with no sources, listing nothing.
    let cfg = SkillsConfig::default();
    assert!(cfg.enabled);
    assert!(cfg.repositories.is_empty());

    let composite = resolve_repositories_with_roots(&cfg, None, None, None)
        .await
        .unwrap()
        .expect("enabled config must yield Some(composite), even if empty");

    let listed = composite.list(&SkillFilter::default()).await.unwrap();
    assert!(
        listed.is_empty(),
        "no configured repos = no skills; got {listed:?}",
    );
}

#[tokio::test]
async fn relative_filesystem_path_resolves_against_context_root() {
    let tmp = tempfile::tempdir().unwrap();
    let repo_dir = tmp.path().join("rel-skills");
    fs::create_dir_all(&repo_dir).unwrap();
    write_skill(&repo_dir, "alpha");

    let cfg = SkillsConfig {
        repositories: vec![fs_repo(
            "project",
            "dc256086-0d2f-4f61-a307-320d4148107f",
            "rel-skills", // relative — must be joined to context_root
        )],
        ..Default::default()
    };

    let composite =
        resolve_repositories_with_roots(&cfg, Some(tmp.path()), None, None)
            .await
            .unwrap()
            .expect("resolver returns Some(composite) when enabled");

    let listed = composite.list(&SkillFilter::default()).await.unwrap();
    let names: Vec<String> = listed
        .iter()
        .map(|d| d.key.skill_name.as_str().to_owned())
        .collect();
    assert_eq!(
        names,
        vec!["alpha".to_string()],
        "relative path must resolve against context_root; got {names:?}",
    );
}

#[tokio::test]
async fn context_root_takes_precedence_over_cache_root_for_relative_paths() {
    let ctx_tmp = tempfile::tempdir().unwrap();
    let cache_tmp = tempfile::tempdir().unwrap();

    // Put a skill under each root with the same relative subpath — if
    // the resolver picks context_root, it must surface only "ctx-skill".
    write_skill(&ctx_tmp.path().join("shared"), "ctx-skill");
    write_skill(&cache_tmp.path().join("shared"), "cache-skill");

    let cfg = SkillsConfig {
        repositories: vec![fs_repo(
            "project",
            "dc256086-0d2f-4f61-a307-320d4148107f",
            "shared",
        )],
        ..Default::default()
    };

    let composite = resolve_repositories_with_roots(
        &cfg,
        Some(ctx_tmp.path()),
        None,
        Some(cache_tmp.path()),
    )
    .await
    .unwrap()
    .expect("resolver returns Some(composite)");

    let listed = composite.list(&SkillFilter::default()).await.unwrap();
    let names: Vec<String> = listed
        .iter()
        .map(|d| d.key.skill_name.as_str().to_owned())
        .collect();
    assert_eq!(
        names,
        vec!["ctx-skill".to_string()],
        "context_root must take precedence over cache_root for relative paths; \
         got {names:?}",
    );
}

#[tokio::test]
async fn cache_root_is_fallback_when_no_context_root() {
    let cache_tmp = tempfile::tempdir().unwrap();
    write_skill(&cache_tmp.path().join("rel"), "cached");

    let cfg = SkillsConfig {
        repositories: vec![fs_repo(
            "project",
            "dc256086-0d2f-4f61-a307-320d4148107f",
            "rel",
        )],
        ..Default::default()
    };

    let composite =
        resolve_repositories_with_roots(&cfg, None, None, Some(cache_tmp.path()))
            .await
            .unwrap()
            .expect("resolver returns Some");

    let listed = composite.list(&SkillFilter::default()).await.unwrap();
    let names: Vec<String> = listed
        .iter()
        .map(|d| d.key.skill_name.as_str().to_owned())
        .collect();
    assert_eq!(names, vec!["cached".to_string()]);
}

#[tokio::test]
async fn absolute_path_ignores_roots() {
    let abs_tmp = tempfile::tempdir().unwrap();
    write_skill(abs_tmp.path(), "abs-skill");

    // Pass a decoy context_root that would resolve something else for a
    // relative path — absolute should bypass it.
    let decoy = tempfile::tempdir().unwrap();
    write_skill(decoy.path(), "decoy");

    let abs_path = abs_tmp.path().to_string_lossy().to_string();
    let cfg = SkillsConfig {
        repositories: vec![fs_repo(
            "abs",
            "dc256086-0d2f-4f61-a307-320d4148107f",
            &abs_path,
        )],
        ..Default::default()
    };

    let composite = resolve_repositories_with_roots(&cfg, Some(decoy.path()), None, None)
        .await
        .unwrap()
        .expect("resolver returns Some");

    let listed = composite.list(&SkillFilter::default()).await.unwrap();
    let names: Vec<String> = listed
        .iter()
        .map(|d| d.key.skill_name.as_str().to_owned())
        .collect();
    assert_eq!(
        names,
        vec!["abs-skill".to_string()],
        "absolute path must ignore context_root; got {names:?} (decoy at \
         {:?})",
        decoy.path(),
    );
}

#[tokio::test]
async fn non_filesystem_repos_are_wavec_stubs_and_contribute_no_sources() {
    // The resolver logs a warning for http/stdio/git transports but
    // still returns a composite; those transports are wave-b stubs per
    // the docstring at `meerkat-skills/src/resolve.rs:3`. Mixing a
    // filesystem repo with an http one must not let the http one steal
    // the filesystem's skills.
    let tmp = tempfile::tempdir().unwrap();
    write_skill(tmp.path(), "fs-skill");

    let cfg = SkillsConfig {
        repositories: vec![
            fs_repo("fs", "dc256086-0d2f-4f61-a307-320d4148107f", "."),
            SkillRepositoryConfig {
                name: "net".to_string(),
                source_uuid: source_uuid("fe52aa61-1111-4a22-9999-bbbbbbbbbbbb"),
                transport: SkillRepoTransport::Http {
                    url: "http://127.0.0.1:1/skills".to_string(),
                    auth_header: None,
                    auth_token: None,
                    refresh_seconds: 300,
                    timeout_seconds: 15,
                },
            },
        ],
        ..Default::default()
    };

    let composite = resolve_repositories_with_roots(&cfg, Some(tmp.path()), None, None)
        .await
        .unwrap()
        .expect("resolver returns Some");

    let listed = composite.list(&SkillFilter::default()).await.unwrap();
    let names: Vec<String> = listed
        .iter()
        .map(|d| d.key.skill_name.as_str().to_owned())
        .collect();
    assert_eq!(
        names,
        vec!["fs-skill".to_string()],
        "only the filesystem repo contributes; http is a wave-c stub. \
         got {names:?}",
    );
}
