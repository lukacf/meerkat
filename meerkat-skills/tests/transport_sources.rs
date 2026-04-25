#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use meerkat_core::skills::{SkillFilter, SkillKey, SkillName, SkillSource, SourceUuid};
use meerkat_skills::source::git::{GitRef, GitSkillConfig, GitSkillSource};
#[cfg(feature = "skills-http")]
use meerkat_skills::source::http::{HttpSkillAuth, HttpSkillSource};
use meerkat_skills::source::protocol::{ExternalSkillSource, StdioExternalClient};
#[cfg(feature = "skills-http")]
use wiremock::matchers::{header, method, path};
#[cfg(feature = "skills-http")]
use wiremock::{Mock, MockServer, ResponseTemplate};

fn source_uuid(raw: &str) -> SourceUuid {
    SourceUuid::parse(raw).unwrap()
}

fn key(source_uuid: SourceUuid, name: &str) -> SkillKey {
    SkillKey::new(source_uuid, SkillName::parse(name).unwrap())
}

fn skill_body(name: &str, description: &str, body: &str) -> String {
    format!("---\nname: {name}\ndescription: {description}\n---\n{body}")
}

#[tokio::test]
#[cfg(feature = "skills-http")]
async fn http_source_uses_configured_uuid_and_quarantines_entry_mismatch() {
    let source_uuid = source_uuid("fe52aa61-1111-4a22-9999-bbbbbbbbbbbb");
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/skills"))
        .and(header("x-api-key", "secret"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "source_uuid": source_uuid.to_string(),
            "skills": [
                {
                    "name": "remote-ok",
                    "body": skill_body("remote-ok", "ok", "body")
                },
                {
                    "source_uuid": "dc256086-0d2f-4f61-a307-320d4148107f",
                    "name": "remote-bad",
                    "body": skill_body("remote-bad", "bad", "body")
                }
            ]
        })))
        .mount(&server)
        .await;

    let source = HttpSkillSource::new_with_source_uuid(
        source_uuid.clone(),
        format!("{}/skills", server.uri()),
        Some(HttpSkillAuth::Header {
            name: "x-api-key".to_string(),
            value: "secret".to_string(),
        }),
        Duration::from_secs(300),
        Duration::from_secs(5),
    );

    let listed = source.list(&SkillFilter::default()).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].key, key(source_uuid.clone(), "remote-ok"));

    let quarantined = source.quarantined_diagnostics().await.unwrap();
    assert_eq!(quarantined.len(), 1);
    assert_eq!(quarantined[0].key, key(source_uuid, "remote-bad"));
    assert!(quarantined[0].message.contains("source_uuid mismatch"));
}

#[tokio::test]
#[cfg(feature = "skills-http")]
async fn http_source_serves_stale_cache_after_refresh_error() {
    let source_uuid = source_uuid("fe52aa61-1111-4a22-9999-bbbbbbbbbbbb");
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/skills"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "skills": [{
                "name": "cached",
                "body": skill_body("cached", "cached", "cached body")
            }]
        })))
        .expect(1)
        .mount(&server)
        .await;

    let source = HttpSkillSource::new_with_source_uuid(
        source_uuid,
        format!("{}/skills", server.uri()),
        None,
        Duration::from_secs(0),
        Duration::from_secs(5),
    );

    assert_eq!(source.list(&SkillFilter::default()).await.unwrap().len(), 1);
    server.reset().await;
    let stale = source.list(&SkillFilter::default()).await.unwrap();
    assert_eq!(stale.len(), 1);
    assert_eq!(stale[0].key.skill_name.as_str(), "cached");
}

#[tokio::test]
async fn stdio_source_lists_and_loads_with_configured_uuid() {
    let tmp = tempfile::tempdir().unwrap();
    let script = tmp.path().join("skills-server.sh");
    fs::write(
        &script,
        r#"#!/bin/sh
cat >/dev/null
cat <<'JSON'
{
  "skills": [{
    "name": "stdio-skill",
    "body": "---\nname: stdio-skill\ndescription: stdio\n---\nstdio body"
  }]
}
JSON
"#,
    )
    .unwrap();
    let mut perms = fs::metadata(&script).unwrap().permissions();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        perms.set_mode(0o755);
        fs::set_permissions(&script, perms).unwrap();
    }

    let source_uuid = source_uuid("dc256086-0d2f-4f61-a307-320d4148107f");
    let client = StdioExternalClient::new(
        script.to_string_lossy().to_string(),
        Vec::new(),
        Default::default(),
        None,
    );
    let source = ExternalSkillSource::new_with_source_uuid(
        client,
        source_uuid.clone(),
        Duration::from_secs(300),
        Default::default(),
    );

    let listed = source.list(&SkillFilter::default()).await.unwrap();
    assert_eq!(listed[0].key, key(source_uuid.clone(), "stdio-skill"));
    let loaded = source.load(&key(source_uuid, "stdio-skill")).await.unwrap();
    assert_eq!(loaded.body.trim(), "stdio body");
}

#[tokio::test]
async fn git_source_uses_configured_uuid_and_loads_skill_documents() {
    let tmp = tempfile::tempdir().unwrap();
    let remote = tmp.path().join("remote.git");
    let work = tmp.path().join("work");
    let skill_dir = work.join("skills/git-skill");
    fs::create_dir_all(&skill_dir).unwrap();
    fs::write(
        skill_dir.join("SKILL.md"),
        skill_body("git-skill", "from git", "git body"),
    )
    .unwrap();
    init_test_repo(&remote, &work);

    let source_uuid = source_uuid("a93d587d-8f44-438f-8189-6e8cf549f6e7");
    let source = GitSkillSource::new(GitSkillConfig {
        repo_url: remote.to_string_lossy().to_string(),
        git_ref: GitRef::Branch("main".to_string()),
        cache_dir: tmp.path().join("cache"),
        skills_root: Some("skills".to_string()),
        refresh_interval: Duration::from_secs(300),
        auth: None,
        depth: Some(1),
        source_uuid: source_uuid.clone(),
        health_thresholds: Default::default(),
    });

    let listed = source.list(&SkillFilter::default()).await.unwrap();
    assert_eq!(listed[0].key, key(source_uuid.clone(), "git-skill"));
    assert_eq!(listed[0].key.source_uuid, source_uuid);
    let loaded = source.load(&key(source_uuid, "git-skill")).await.unwrap();
    assert_eq!(loaded.body.trim(), "git body");
}

#[test]
fn transport_contracts_are_truthful_about_git_and_stub_deletion() {
    let cargo_toml = include_str!("../Cargo.toml");
    assert!(
        !cargo_toml.contains("dep:gix")
            && !cargo_toml.contains("gix =")
            && !cargo_toml.contains("skills-git"),
        "Git transport must not advertise gix or a fake git feature while implemented with system git",
    );

    let resolve = include_str!("../src/resolve.rs");
    assert!(
        resolve.contains("unavailable_on_wasm(\"Git\")"),
        "resolver must keep the wasm Git unavailable path as a typed SkillError::Load contract",
    );

    for source in [
        include_str!("../src/resolve.rs"),
        include_str!("../src/source/git.rs"),
        include_str!("../src/source/http.rs"),
        include_str!("../src/source/protocol.rs"),
    ] {
        let lower = source.to_ascii_lowercase();
        for forbidden in [
            format!("{} {}", "wave-b", "stub"),
            format!("{} {}", "wave-c", "stub"),
            format!("{}-{}", "wave-c", "gated"),
            format!("{}{}", "notyet", "implemented"),
        ] {
            assert!(!lower.contains(&forbidden));
        }
    }
}

fn init_test_repo(repo: &Path, work: &Path) {
    fs::create_dir_all(repo).unwrap();
    fs::create_dir_all(work).unwrap();
    run_git(repo, &["init", "--bare"]);
    run_git(work, &["init"]);
    run_git(work, &["config", "user.email", "skills@example.com"]);
    run_git(work, &["config", "user.name", "Skills Test"]);
    run_git(work, &["add", "."]);
    run_git(work, &["commit", "-m", "initial"]);
    run_git(work, &["branch", "-M", "main"]);
    run_git(
        work,
        &["remote", "add", "origin", repo.to_string_lossy().as_ref()],
    );
    run_git(work, &["push", "-u", "origin", "main"]);
}

fn run_git(cwd: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?} failed");
}
