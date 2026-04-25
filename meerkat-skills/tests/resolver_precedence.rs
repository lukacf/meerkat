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
//!   back to `"."`.
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
#[cfg(feature = "skills-http")]
use wiremock::matchers::{header, method, path};
#[cfg(feature = "skills-http")]
use wiremock::{Mock, MockServer, ResponseTemplate};

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

    let composite = resolve_repositories_with_roots(&cfg, Some(tmp.path()), None, None)
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

    let composite =
        resolve_repositories_with_roots(&cfg, Some(ctx_tmp.path()), None, Some(cache_tmp.path()))
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

    let composite = resolve_repositories_with_roots(&cfg, None, None, Some(cache_tmp.path()))
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
#[cfg(feature = "skills-http")]
async fn http_repository_is_wired_and_preserves_filesystem_precedence() {
    let tmp = tempfile::tempdir().unwrap();
    write_skill(tmp.path(), "fs-skill");
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/skills"))
        .and(header("authorization", "Bearer secret"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "source_uuid": "fe52aa61-1111-4a22-9999-bbbbbbbbbbbb",
            "skills": [{
                "name": "http-skill",
                "body": "---\nname: http-skill\ndescription: remote\n---\nremote body"
            }]
        })))
        .mount(&server)
        .await;

    let cfg = SkillsConfig {
        repositories: vec![
            fs_repo("fs", "dc256086-0d2f-4f61-a307-320d4148107f", "."),
            SkillRepositoryConfig {
                name: "net".to_string(),
                source_uuid: source_uuid("fe52aa61-1111-4a22-9999-bbbbbbbbbbbb"),
                transport: SkillRepoTransport::Http {
                    url: format!("{}/skills", server.uri()),
                    auth_header: None,
                    auth_token: Some("secret".to_string()),
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
        vec!["fs-skill".to_string(), "http-skill".to_string()],
        "filesystem and HTTP repos both contribute in configured precedence; got {names:?}",
    );
}

#[cfg(target_arch = "wasm32")]
#[tokio::test]
async fn git_repository_reports_unavailable_on_wasm() {
    let cfg = SkillsConfig {
        repositories: vec![SkillRepositoryConfig {
            name: "git".to_string(),
            source_uuid: source_uuid("a93d587d-8f44-438f-8189-6e8cf549f6e7"),
            transport: SkillRepoTransport::Git {
                url: "https://example.invalid/skills.git".to_string(),
                git_ref: "main".to_string(),
                ref_type: Default::default(),
                skills_root: None,
                auth_token: None,
                ssh_key: None,
                refresh_seconds: 300,
                depth: Some(1),
            },
        }],
        ..Default::default()
    };

    let err = match resolve_repositories_with_roots(&cfg, None, None, None).await {
        Ok(_) => panic!("wasm Git repos must not resolve successfully"),
        Err(err) => err,
    };
    assert!(
        err.to_string().contains("Git") && err.to_string().contains("unavailable on wasm"),
        "wasm Git repos must fail with a typed unavailable transport error, got {err:?}",
    );
}
