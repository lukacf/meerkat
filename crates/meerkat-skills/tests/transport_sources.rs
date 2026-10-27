#![allow(clippy::unwrap_used, clippy::expect_used, clippy::disallowed_methods)]

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
        Duration::from_mins(5),
        Duration::from_secs(5),
    );

    let listed = source.list(&SkillFilter::default()).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].key, key(source_uuid.clone(), "remote-ok"));

    let quarantined = source.quarantined_diagnostics().await.unwrap();
    assert_eq!(quarantined.len(), 1);
    // Row #36: quarantine identity is typed (source_uuid + raw_id), not a
    // loadable SkillKey.
    assert_eq!(quarantined[0].identity.source_uuid, source_uuid);
    assert_eq!(quarantined[0].identity.raw_id, "remote-bad");
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
#[cfg(feature = "skills-http")]
async fn unhealthy_http_source_refuses_stale_cache() {
    // Row #165: once a remote source is Unhealthy (refresh failing past the
    // unhealthy threshold), it must refuse to serve stale-but-interval-fresh
    // cache with a typed stale error instead of laundering cached content.
    use meerkat_core::skills::SourceHealthThresholds;

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

    // unhealthy_failure_streak == 1 → a single refresh failure flips the
    // source to Unhealthy.
    let thresholds = SourceHealthThresholds {
        unhealthy_failure_streak: 1,
        ..SourceHealthThresholds::default()
    };
    let source = HttpSkillSource::new_with_thresholds(
        source_uuid,
        format!("{}/skills", server.uri()),
        None,
        Duration::from_secs(0),
        Duration::from_secs(5),
        thresholds,
    );

    // First call populates the cache from the live server.
    assert_eq!(source.list(&SkillFilter::default()).await.unwrap().len(), 1);

    // Server goes away: the next refresh fails and pushes the source Unhealthy.
    server.reset().await;
    let err = source.list(&SkillFilter::default()).await.unwrap_err();
    let message = err.to_string();
    assert!(
        message.contains("stale source") && message.contains("unhealthy"),
        "unhealthy source must surface a typed stale refusal; got {message}"
    );

    // Even a subsequent call (cache still interval-fresh under refresh_interval
    // semantics) must keep refusing while the source stays Unhealthy.
    let err = source.list(&SkillFilter::default()).await.unwrap_err();
    assert!(
        err.to_string().contains("stale source"),
        "Unhealthy source must keep refusing stale cache; got {err}"
    );
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
        Duration::from_mins(5),
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
        refresh_interval: Duration::from_mins(5),
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

/// A Git skill source runs no command smuggled through its ssh-key path: the
/// path goes into `GIT_SSH_COMMAND`, which git runs through a shell, so a
/// path crafted to carry a shell command must not execute it. A marker file
/// the payload would create stays absent.
#[cfg(unix)]
#[tokio::test]
async fn a_git_skill_ssh_key_path_runs_no_smuggled_command() {
    let tmp = tempfile::tempdir().unwrap();
    let marker = tmp.path().join("ssh-key-payload-ran");
    // The path is read as one argument to `ssh -i`; a shell that parsed it
    // would run the trailing command.
    let crafted = format!("/nonexistent-key; touch {} #", marker.display());
    let source = GitSkillSource::new(GitSkillConfig {
        repo_url: "git@nonexistent.invalid:owner/repo.git".to_string(),
        git_ref: GitRef::Branch("main".to_string()),
        cache_dir: tmp.path().join("cache"),
        skills_root: None,
        refresh_interval: Duration::from_mins(5),
        auth: Some(meerkat_skills::source::git::GitSkillAuth::SshKey(crafted)),
        depth: Some(1),
        source_uuid: source_uuid("c0ffee00-0000-4000-8000-000000000001"),
        health_thresholds: Default::default(),
    });

    // The refresh fails (the key is rejected, or ssh cannot reach the host);
    // either way the smuggled command never runs.
    let _ = source.list(&SkillFilter::default()).await;
    assert!(
        !marker.exists(),
        "the ssh-key path must not run a smuggled command"
    );
}

/// A Git skill `git_ref` that reads as an option must not reach git as one:
/// a ref crafted to carry `--upload-pack=<cmd>` runs no command. A marker
/// file the payload would create stays absent.
#[cfg(unix)]
#[tokio::test]
async fn a_git_skill_ref_that_looks_like_an_option_runs_no_command() {
    let tmp = tempfile::tempdir().unwrap();
    let remote = tmp.path().join("remote.git");
    let work = tmp.path().join("work");
    let skill_dir = work.join("git-skill");
    fs::create_dir_all(&skill_dir).unwrap();
    fs::write(
        skill_dir.join("SKILL.md"),
        skill_body("git-skill", "from git", "git body"),
    )
    .unwrap();
    init_test_repo(&remote, &work);

    let marker = tmp.path().join("ref-payload-ran");
    let crafted = format!("--upload-pack=touch {}", marker.display());
    let source = GitSkillSource::new(GitSkillConfig {
        repo_url: remote.to_string_lossy().to_string(),
        git_ref: GitRef::Branch(crafted),
        cache_dir: tmp.path().join("cache"),
        skills_root: None,
        refresh_interval: Duration::from_mins(5),
        auth: None,
        depth: None,
        source_uuid: source_uuid("c0ffee00-0000-4000-8000-000000000002"),
        health_thresholds: Default::default(),
    });

    let _ = source.list(&SkillFilter::default()).await;
    assert!(
        !marker.exists(),
        "a ref that looks like an option must not run a command"
    );
}

/// A Git skill repository URL that selects a git remote helper (`ext::`,
/// `fd::`, any `<word>::<rest>`) is refused: it is not a supported
/// transport, and git never runs it.
#[tokio::test]
async fn a_git_skill_remote_helper_url_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let marker = tmp.path().join("remote-helper-ran");
    let source = GitSkillSource::new(GitSkillConfig {
        repo_url: format!("ext::sh -c \"touch {}\"", marker.display()),
        git_ref: GitRef::Branch("main".to_string()),
        cache_dir: tmp.path().join("cache"),
        skills_root: None,
        refresh_interval: Duration::from_mins(5),
        auth: None,
        depth: None,
        source_uuid: source_uuid("c0ffee00-0000-4000-8000-000000000003"),
        health_thresholds: Default::default(),
    });
    let result = source.list(&SkillFilter::default()).await;
    assert!(
        result.is_err(),
        "a remote-helper url is refused: {result:?}"
    );
    assert!(!marker.exists(), "the remote helper must not run");
}

/// A Git skill repository that is a local repository a confined process
/// could change is refused: clone runs its upload-pack against that
/// repository's own config, which such a process controls.
#[cfg(unix)]
#[tokio::test]
async fn a_git_skill_local_repository_in_a_write_grant_is_refused() {
    use std::sync::Arc;

    let tmp = tempfile::tempdir().unwrap();
    let grant = tmp.path().join("grant");
    let remote = grant.join("remote.git");
    let work = tmp.path().join("work");
    let skill_dir = work.join("git-skill");
    fs::create_dir_all(&skill_dir).unwrap();
    fs::write(
        skill_dir.join("SKILL.md"),
        skill_body("git-skill", "from git", "git body"),
    )
    .unwrap();
    init_test_repo(&remote, &work);

    let grant_for_predicate = grant.clone();
    let source = GitSkillSource::new(GitSkillConfig {
        repo_url: format!("file://{}", remote.display()),
        git_ref: GitRef::Branch("main".to_string()),
        cache_dir: tmp.path().join("cache"),
        skills_root: None,
        refresh_interval: Duration::from_mins(5),
        auth: None,
        depth: None,
        source_uuid: source_uuid("c0ffee00-0000-4000-8000-000000000004"),
        health_thresholds: Default::default(),
    })
    .with_untrusted_paths(Arc::new(move |path: &std::path::Path, _| {
        path.starts_with(&grant_for_predicate)
    }));

    let result = source.list(&SkillFilter::default()).await;
    assert!(
        result.is_err(),
        "a local repository under a write grant is refused: {result:?}"
    );
}

/// A real ssh-key file whose name contains shell metacharacters runs no
/// command: the path is single-quoted into `GIT_SSH_COMMAND`, so git's shell
/// takes it as one literal argument. A marker the name spells out is absent.
#[cfg(unix)]
#[tokio::test]
async fn a_real_ssh_key_with_metacharacters_runs_no_command() {
    let tmp = tempfile::tempdir().unwrap();
    let key = tmp.path().join("id; touch pwned");
    std::fs::write(&key, "key").unwrap();
    let source = GitSkillSource::new(GitSkillConfig {
        repo_url: "git@nonexistent.invalid:owner/repo.git".to_string(),
        git_ref: GitRef::Branch("main".to_string()),
        cache_dir: tmp.path().join("cache"),
        skills_root: None,
        refresh_interval: Duration::from_mins(5),
        auth: Some(meerkat_skills::source::git::GitSkillAuth::SshKey(
            key.to_string_lossy().to_string(),
        )),
        depth: Some(1),
        source_uuid: source_uuid("c0ffee00-0000-4000-8000-000000000005"),
        health_thresholds: Default::default(),
    });
    let _ = source.list(&SkillFilter::default()).await;
    // The payload would create `pwned` in git's working directory; it runs
    // nowhere, so no such file appears under the test's temp tree.
    fn contains_marker(dir: &Path) -> bool {
        std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .any(|entry| {
                let path = entry.path();
                path.file_name().and_then(|name| name.to_str()) == Some("pwned")
                    || (path.is_dir() && contains_marker(&path))
            })
    }
    assert!(
        !contains_marker(tmp.path()),
        "the metacharacter key path must run no command"
    );
}
