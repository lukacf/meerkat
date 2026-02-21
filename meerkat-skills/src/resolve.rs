//! Skill repository resolution.
//!
//! Turns `SkillsConfig` into a composed concrete `CompositeSkillSource`.
//! This is the single canonical path for all surfaces.

use meerkat_core::skills::{SkillError, SkillScope, SourceUuid};
use meerkat_core::skills_config::{SkillRepoTransport, SkillsConfig};
use std::path::Path;

use crate::source::composite::NamedSource;
use crate::source::{
    CompositeSkillSource, EmbeddedSkillSource, ExternalSkillSource, FilesystemSkillSource,
    SourceNode, StdioExternalClient,
};

/// Resolve skill repository config into a composed `CompositeSkillSource`.
///
/// Returns `None` when `config.enabled == false`.
/// When `config.repositories` is empty, falls back to the default chain:
/// project FS → user FS → embedded.
pub async fn resolve_repositories(
    config: &SkillsConfig,
    project_root: Option<&Path>,
) -> Result<Option<CompositeSkillSource>, SkillError> {
    let user_root = std::env::var_os("HOME").map(std::path::PathBuf::from);
    resolve_repositories_with_roots(config, project_root, user_root.as_deref(), project_root).await
}

/// Resolve repositories with explicit convention roots.
///
/// Precedence for default filesystem sources:
/// context root (project) > user root > embedded.
pub async fn resolve_repositories_with_roots(
    config: &SkillsConfig,
    context_root: Option<&Path>,
    user_root: Option<&Path>,
    cache_root: Option<&Path>,
) -> Result<Option<CompositeSkillSource>, SkillError> {
    if !config.enabled {
        return Ok(None);
    }

    let mut sources: Vec<NamedSource> = Vec::new();

    if config.repositories.is_empty() {
        // Explicit default chain: context FS -> user FS.
        if let Some(root) = context_root {
            sources.push(NamedSource {
                name: "project".into(),
                source: SourceNode::Filesystem(FilesystemSkillSource::new_with_identity(
                    root.join(".rkat/skills"),
                    SkillScope::Project,
                    source_uuid("00000000-0000-4000-8000-000000000101"),
                    config.health_thresholds,
                )),
            });
        }
        if let Some(user) = user_root {
            sources.push(NamedSource {
                name: "user".into(),
                source: SourceNode::Filesystem(FilesystemSkillSource::new_with_identity(
                    user.join(".rkat/skills"),
                    SkillScope::User,
                    source_uuid("00000000-0000-4000-8000-000000000102"),
                    config.health_thresholds,
                )),
            });
        }
    } else {
        // Config-driven sources
        for repo in &config.repositories {
            match &repo.transport {
                SkillRepoTransport::Filesystem { path } => {
                    let resolution_root = context_root
                        .or(cache_root)
                        .unwrap_or_else(|| Path::new("."));
                    let full_path = if Path::new(path).is_relative() {
                        resolution_root.join(path)
                    } else {
                        path.into()
                    };
                    sources.push(NamedSource {
                        name: repo.name.clone(),
                        source: SourceNode::Filesystem(FilesystemSkillSource::new_with_identity(
                            full_path,
                            SkillScope::Project,
                            repo.source_uuid.clone(),
                            config.health_thresholds,
                        )),
                    });
                }
                SkillRepoTransport::Http {
                    url,
                    auth_header,
                    auth_token,
                    refresh_seconds,
                    timeout_seconds,
                } => {
                    #[cfg(feature = "skills-http")]
                    {
                        use crate::source::http::{HttpSkillAuth, HttpSkillSource};

                        let auth = match (auth_header.as_deref(), auth_token.as_deref()) {
                            (Some(header), Some(token)) => Some(HttpSkillAuth::Header {
                                name: header.to_string(),
                                value: token.to_string(),
                            }),
                            (None, Some(token)) => Some(HttpSkillAuth::Bearer(token.to_string())),
                            _ => None,
                        };

                        sources.push(NamedSource {
                            name: repo.name.clone(),
                            source: SourceNode::Http(Box::new(
                                HttpSkillSource::new_with_source_uuid(
                                    repo.source_uuid.to_string(),
                                    url.clone(),
                                    auth,
                                    std::time::Duration::from_secs(*refresh_seconds),
                                    std::time::Duration::from_secs(*timeout_seconds),
                                ),
                            )),
                        });
                    }
                    #[cfg(not(feature = "skills-http"))]
                    {
                        let _ = (
                            url,
                            auth_header,
                            auth_token,
                            refresh_seconds,
                            timeout_seconds,
                        );
                        tracing::warn!(
                            "Skill repository '{}' uses HTTP transport. \
                             Compile with `skills-http` feature to enable. Skipping.",
                            repo.name
                        );
                    }
                }
                SkillRepoTransport::Stdio {
                    command,
                    args,
                    cwd,
                    env,
                    timeout_seconds,
                } => {
                    let resolution_root = context_root
                        .or(cache_root)
                        .unwrap_or_else(|| Path::new("."));
                    let cwd = cwd.as_ref().map(|dir| {
                        let path = Path::new(dir);
                        if path.is_relative() {
                            resolution_root.join(path)
                        } else {
                            path.to_path_buf()
                        }
                    });
                    let client = StdioExternalClient::new_with_timeout(
                        command.clone(),
                        args.clone(),
                        env.clone(),
                        cwd,
                        std::time::Duration::from_secs(*timeout_seconds),
                    );
                    sources.push(NamedSource {
                        name: repo.name.clone(),
                        source: SourceNode::External(ExternalSkillSource::new(
                            repo.source_uuid.to_string(),
                            client,
                        )),
                    });
                }
                SkillRepoTransport::Git {
                    url,
                    git_ref,
                    ref_type,
                    skills_root,
                    auth_token,
                    ssh_key,
                    refresh_seconds,
                    depth,
                } => {
                    use crate::source::git::{
                        GitRef, GitSkillAuth, GitSkillConfig, GitSkillSource,
                    };

                    let git_ref_enum = match ref_type {
                        meerkat_core::skills_config::GitRefType::Branch => {
                            GitRef::Branch(git_ref.clone())
                        }
                        meerkat_core::skills_config::GitRefType::Tag => {
                            GitRef::Tag(git_ref.clone())
                        }
                        meerkat_core::skills_config::GitRefType::Commit => {
                            GitRef::Commit(git_ref.clone())
                        }
                    };

                    // Derive cache dir from repo URL hash
                    let cache_dir = cache_root
                        .or(context_root)
                        .unwrap_or_else(|| Path::new("."))
                        .join(".rkat/skill-repos")
                        .join(sanitize_repo_name(url));

                    if ssh_key.is_some() {
                        tracing::warn!(
                            repo = %repo.name,
                            "ssh_key is not yet supported for git skill repos; use auth_token instead"
                        );
                    }

                    let auth = auth_token
                        .as_ref()
                        .map(|t| GitSkillAuth::HttpsToken(t.clone()));

                    let config = GitSkillConfig {
                        repo_url: url.clone(),
                        git_ref: git_ref_enum,
                        cache_dir,
                        skills_root: skills_root.clone(),
                        refresh_interval: std::time::Duration::from_secs(*refresh_seconds),
                        auth,
                        depth: *depth,
                        source_uuid: repo.source_uuid.clone(),
                        health_thresholds: config.health_thresholds,
                    };

                    sources.push(NamedSource {
                        name: repo.name.clone(),
                        source: SourceNode::Git(Box::new(GitSkillSource::new(config))),
                    });
                }
            }
        }
    }

    // Embedded skills always appended as lowest precedence.
    sources.push(NamedSource {
        name: "embedded".into(),
        source: SourceNode::Embedded(EmbeddedSkillSource::new()),
    });

    Ok(Some(CompositeSkillSource::from_named(sources)))
}

/// Sanitize a repo URL into a valid directory name.
fn sanitize_repo_name(url: &str) -> String {
    url.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn source_uuid(raw: &str) -> SourceUuid {
    match SourceUuid::parse(raw) {
        Ok(source_uuid) => source_uuid,
        Err(_) => unreachable!("hardcoded source UUID must remain valid"),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use meerkat_core::skills::{
        SkillFilter, SkillSource, SourceHealthState, SourceHealthThresholds,
    };
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_resolve_empty_config_uses_defaults() {
        let tmp = TempDir::new().unwrap();
        let config = SkillsConfig::default();

        let source = resolve_repositories(&config, Some(tmp.path()))
            .await
            .unwrap();
        assert!(source.is_some());

        // Should work (no skills in empty dirs, but no error)
        let skills = source.unwrap().list(&SkillFilter::default()).await.unwrap();
        // Only embedded skills (if any registered)
        // The important thing is it doesn't error
        let _ = skills;
    }

    #[tokio::test]
    async fn test_resolve_filesystem_repo() {
        let tmp = TempDir::new().unwrap();
        let skills_dir = tmp.path().join("my-skills");
        tokio::fs::create_dir_all(skills_dir.join("test-skill"))
            .await
            .unwrap();
        tokio::fs::write(
            skills_dir.join("test-skill/SKILL.md"),
            "---\nname: test-skill\ndescription: A test skill\n---\n\nBody.",
        )
        .await
        .unwrap();

        let config = SkillsConfig {
            repositories: vec![meerkat_core::skills_config::SkillRepositoryConfig {
                name: "test".into(),
                source_uuid: source_uuid("00000000-0000-4000-8000-000000000001"),
                transport: SkillRepoTransport::Filesystem {
                    path: skills_dir.to_str().unwrap().into(),
                },
            }],
            ..Default::default()
        };

        let source = resolve_repositories(&config, Some(tmp.path()))
            .await
            .unwrap()
            .unwrap();

        let skills = source.list(&SkillFilter::default()).await.unwrap();
        // Should find the test skill (plus any embedded)
        assert!(skills.iter().any(|s| s.id.0 == "test-skill"));
    }

    #[tokio::test]
    async fn test_resolve_embedded_always_appended() {
        let tmp = TempDir::new().unwrap();
        let config = SkillsConfig {
            repositories: vec![meerkat_core::skills_config::SkillRepositoryConfig {
                name: "test".into(),
                source_uuid: source_uuid("00000000-0000-4000-8000-000000000002"),
                transport: SkillRepoTransport::Filesystem {
                    path: "/nonexistent/path".into(),
                },
            }],
            ..Default::default()
        };

        let source = resolve_repositories(&config, Some(tmp.path()))
            .await
            .unwrap();
        // Should not error — embedded is always appended
        assert!(source.is_some());
    }

    #[tokio::test]
    async fn test_resolve_disabled_returns_none() {
        let config = SkillsConfig {
            enabled: false,
            ..Default::default()
        };

        let source = resolve_repositories(&config, None).await.unwrap();
        assert!(source.is_none());
    }

    #[tokio::test]
    #[ignore] // integration-real: spawns git CLI for test fixture setup
    async fn test_resolve_git_repo() {
        let tmp = TempDir::new().unwrap();

        // Create a local bare repo with a skill
        let repo_dir = tmp.path().join("test-repo");
        let work_dir = tmp.path().join("work");

        tokio::fs::create_dir_all(&repo_dir).await.unwrap();
        crate::source::git::tests_support::init_test_repo(&repo_dir, &work_dir)
            .await
            .unwrap();

        let config = SkillsConfig {
            repositories: vec![meerkat_core::skills_config::SkillRepositoryConfig {
                name: "git-test".into(),
                source_uuid: source_uuid("00000000-0000-4000-8000-000000000003"),
                transport: SkillRepoTransport::Git {
                    url: format!("file://{}", repo_dir.display()),
                    git_ref: "main".into(),
                    ref_type: meerkat_core::skills_config::GitRefType::Branch,
                    skills_root: None,
                    auth_token: None,
                    ssh_key: None,
                    refresh_seconds: 300,
                    depth: None,
                },
            }],
            ..Default::default()
        };

        let source = resolve_repositories(&config, Some(tmp.path()))
            .await
            .unwrap()
            .unwrap();

        let skills = source.list(&SkillFilter::default()).await.unwrap();
        assert!(skills.iter().any(|s| s.id.0 == "extraction/email"));
    }

    #[tokio::test]
    async fn test_resolve_mixed_repos() {
        let tmp = TempDir::new().unwrap();

        // Create filesystem skills
        let fs_dir = tmp.path().join("fs-skills/test-skill");
        tokio::fs::create_dir_all(&fs_dir).await.unwrap();
        tokio::fs::write(
            fs_dir.join("SKILL.md"),
            "---\nname: test-skill\ndescription: FS skill\n---\n\nBody.",
        )
        .await
        .unwrap();

        let config = SkillsConfig {
            repositories: vec![
                meerkat_core::skills_config::SkillRepositoryConfig {
                    name: "filesystem".into(),
                    source_uuid: source_uuid("00000000-0000-4000-8000-000000000004"),
                    transport: SkillRepoTransport::Filesystem {
                        path: tmp.path().join("fs-skills").to_str().unwrap().into(),
                    },
                },
                // HTTP repo would require a server — skip in this test
            ],
            ..Default::default()
        };

        let source = resolve_repositories(&config, Some(tmp.path()))
            .await
            .unwrap()
            .unwrap();

        let skills = source.list(&SkillFilter::default()).await.unwrap();
        assert!(skills.iter().any(|s| s.id.0 == "test-skill"));
    }

    #[tokio::test]
    async fn test_resolve_stdio_repo_wires_external_source_node() {
        let tmp = TempDir::new().unwrap();
        let script = r##"
read line
if echo "$line" | grep -q '"method":"capabilities/get"'; then
  echo '{"jsonrpc":"2.0","id":"1","payload":{"method":"capabilities/get","result":{"protocol_version":1,"methods":["skills/list_summaries","skills/load_package"]}}}'
elif echo "$line" | grep -q '"method":"skills/list_summaries"'; then
  echo '{"jsonrpc":"2.0","id":"1","payload":{"method":"skills/list_summaries","result":{"summaries":[{"source_uuid":"00000000-0000-4000-8000-000000000008","skill_name":"remote-skill","description":"remote"}]}}}'
elif echo "$line" | grep -q '"method":"skills/load_package"'; then
  echo '{"jsonrpc":"2.0","id":"1","payload":{"method":"skills/load_package","result":{"package":{"summary":{"source_uuid":"00000000-0000-4000-8000-000000000008","skill_name":"remote-skill","description":"remote"},"body":"body"}}}}'
fi
"##;

        let config = SkillsConfig {
            repositories: vec![meerkat_core::skills_config::SkillRepositoryConfig {
                name: "external-stdio".into(),
                source_uuid: source_uuid("00000000-0000-4000-8000-000000000008"),
                transport: SkillRepoTransport::Stdio {
                    command: "sh".into(),
                    args: vec!["-c".into(), script.into()],
                    cwd: None,
                    env: std::collections::BTreeMap::new(),
                    timeout_seconds: 5,
                },
            }],
            ..Default::default()
        };

        let source = resolve_repositories(&config, Some(tmp.path()))
            .await
            .unwrap()
            .unwrap();
        let skills = source.list(&SkillFilter::default()).await.unwrap();
        assert!(skills.iter().any(|s| s.id.0 == "remote-skill"));
    }

    #[tokio::test]
    async fn test_resolve_wires_non_default_health_thresholds_into_filesystem_source() {
        let tmp = TempDir::new().unwrap();
        let skills_root = tmp.path().join("threshold-skills");
        tokio::fs::create_dir_all(skills_root.join("valid-skill"))
            .await
            .unwrap();
        tokio::fs::create_dir_all(skills_root.join("broken-skill"))
            .await
            .unwrap();
        tokio::fs::write(
            skills_root.join("valid-skill/SKILL.md"),
            "---\nname: valid-skill\ndescription: valid\n---\n\nbody",
        )
        .await
        .unwrap();
        tokio::fs::write(
            skills_root.join("broken-skill/SKILL.md"),
            "---\nname: wrong-name\ndescription: invalid\n---\n\nbody",
        )
        .await
        .unwrap();

        let config = SkillsConfig {
            repositories: vec![meerkat_core::skills_config::SkillRepositoryConfig {
                name: "threshold-test".into(),
                source_uuid: source_uuid("00000000-0000-4000-8000-000000000007"),
                transport: SkillRepoTransport::Filesystem {
                    path: skills_root.to_str().unwrap().into(),
                },
            }],
            health_thresholds: SourceHealthThresholds {
                degraded_invalid_ratio: 0.75,
                unhealthy_invalid_ratio: 0.95,
                degraded_failure_streak: 10,
                unhealthy_failure_streak: 20,
            },
            ..Default::default()
        };

        let source = resolve_repositories(&config, Some(tmp.path()))
            .await
            .unwrap()
            .unwrap();
        let listed = source.list(&SkillFilter::default()).await.unwrap();
        assert!(listed.iter().any(|s| s.id.0 == "valid-skill"));

        let snapshot = source.health_snapshot().await.unwrap();
        assert_eq!(snapshot.invalid_count, 1);
        assert_eq!(snapshot.total_count, 2);
        assert_eq!(snapshot.state, SourceHealthState::Healthy);
    }

    #[tokio::test]
    async fn test_resolve_precedence_matches_config() {
        let tmp = TempDir::new().unwrap();

        // Create two filesystem sources with the same skill ID
        let fs1_dir = tmp.path().join("fs1/shared-skill");
        tokio::fs::create_dir_all(&fs1_dir).await.unwrap();
        tokio::fs::write(
            fs1_dir.join("SKILL.md"),
            "---\nname: shared-skill\ndescription: From first source\n---\n\nFirst.",
        )
        .await
        .unwrap();

        let fs2_dir = tmp.path().join("fs2/shared-skill");
        tokio::fs::create_dir_all(&fs2_dir).await.unwrap();
        tokio::fs::write(
            fs2_dir.join("SKILL.md"),
            "---\nname: shared-skill\ndescription: From second source\n---\n\nSecond.",
        )
        .await
        .unwrap();

        let config = SkillsConfig {
            repositories: vec![
                meerkat_core::skills_config::SkillRepositoryConfig {
                    name: "first".into(),
                    source_uuid: source_uuid("00000000-0000-4000-8000-000000000005"),
                    transport: SkillRepoTransport::Filesystem {
                        path: tmp.path().join("fs1").to_str().unwrap().into(),
                    },
                },
                meerkat_core::skills_config::SkillRepositoryConfig {
                    name: "second".into(),
                    source_uuid: source_uuid("00000000-0000-4000-8000-000000000006"),
                    transport: SkillRepoTransport::Filesystem {
                        path: tmp.path().join("fs2").to_str().unwrap().into(),
                    },
                },
            ],
            ..Default::default()
        };

        let source = resolve_repositories(&config, Some(tmp.path()))
            .await
            .unwrap()
            .unwrap();

        let skills = source.list(&SkillFilter::default()).await.unwrap();
        // First source wins
        let shared = skills.iter().find(|s| s.id.0 == "shared-skill").unwrap();
        assert_eq!(shared.description, "From first source");
        assert_eq!(shared.source_name, "first");
    }

    #[tokio::test]
    async fn test_resolve_with_explicit_roots_context_over_user() {
        let tmp = TempDir::new().unwrap();
        let context_root = tmp.path().join("context");
        let user_root = tmp.path().join("user");
        tokio::fs::create_dir_all(context_root.join(".rkat/skills/shared-skill"))
            .await
            .unwrap();
        tokio::fs::create_dir_all(user_root.join(".rkat/skills/shared-skill"))
            .await
            .unwrap();

        tokio::fs::write(
            context_root.join(".rkat/skills/shared-skill/SKILL.md"),
            "---\nname: shared-skill\ndescription: context description\n---\n\nContext.",
        )
        .await
        .unwrap();
        tokio::fs::write(
            user_root.join(".rkat/skills/shared-skill/SKILL.md"),
            "---\nname: shared-skill\ndescription: user description\n---\n\nUser.",
        )
        .await
        .unwrap();

        let source = resolve_repositories_with_roots(
            &SkillsConfig::default(),
            Some(&context_root),
            Some(&user_root),
            Some(tmp.path()),
        )
        .await
        .unwrap()
        .unwrap();

        let skills = source.list(&SkillFilter::default()).await.unwrap();
        let shared = skills.iter().find(|s| s.id.0 == "shared-skill").unwrap();
        assert_eq!(shared.description, "context description");
        assert_eq!(shared.source_name, "project");
    }

    #[tokio::test]
    async fn test_resolve_with_no_roots_skips_filesystem_defaults() {
        let tmp = TempDir::new().unwrap();
        let context_root = tmp.path().join("context");
        tokio::fs::create_dir_all(context_root.join(".rkat/skills/ambient-skill"))
            .await
            .unwrap();
        tokio::fs::write(
            context_root.join(".rkat/skills/ambient-skill/SKILL.md"),
            "---\nname: ambient-skill\ndescription: ambient\n---\n\nAmbient.",
        )
        .await
        .unwrap();

        let source = resolve_repositories_with_roots(&SkillsConfig::default(), None, None, None)
            .await
            .unwrap()
            .unwrap();
        let skills = source.list(&SkillFilter::default()).await.unwrap();
        assert!(skills.iter().all(|s| s.id.0 != "ambient-skill"));
    }
}
