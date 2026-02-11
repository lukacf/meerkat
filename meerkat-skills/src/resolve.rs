//! Skill repository resolution.
//!
//! Turns `SkillsConfig` into a composed `Arc<dyn SkillSource>`.
//! This is the single canonical path for all surfaces.

use std::path::Path;
use std::sync::Arc;

use meerkat_core::skills::{SkillError, SkillScope, SkillSource};
use meerkat_core::skills_config::{SkillRepoTransport, SkillsConfig};

use crate::source::composite::NamedSource;
use crate::source::{CompositeSkillSource, EmbeddedSkillSource, FilesystemSkillSource};

/// Resolve skill repository config into a composed `SkillSource`.
///
/// Returns `None` when `config.enabled == false`.
/// When `config.repositories` is empty, falls back to the default chain:
/// project FS → user FS → embedded.
pub async fn resolve_repositories(
    config: &SkillsConfig,
    project_root: Option<&Path>,
) -> Result<Option<Arc<dyn SkillSource>>, SkillError> {
    if !config.enabled {
        return Ok(None);
    }

    let mut sources: Vec<NamedSource> = Vec::new();

    if config.repositories.is_empty() {
        // Default chain: project FS → user FS → embedded
        let root = project_root.unwrap_or_else(|| Path::new("."));
        sources.push(NamedSource {
            name: "project".into(),
            source: Box::new(FilesystemSkillSource::new(
                root.join(".rkat/skills"),
                SkillScope::Project,
            )),
        });

        if let Some(home) = std::env::var_os("HOME") {
            sources.push(NamedSource {
                name: "user".into(),
                source: Box::new(FilesystemSkillSource::new(
                    std::path::PathBuf::from(home).join(".rkat/skills"),
                    SkillScope::User,
                )),
            });
        }
    } else {
        // Config-driven sources
        for repo in &config.repositories {
            match &repo.transport {
                SkillRepoTransport::Filesystem { path } => {
                    let full_path = if Path::new(path).is_relative() {
                        project_root
                            .unwrap_or_else(|| Path::new("."))
                            .join(path)
                    } else {
                        path.into()
                    };
                    sources.push(NamedSource {
                        name: repo.name.clone(),
                        source: Box::new(FilesystemSkillSource::new(
                            full_path,
                            SkillScope::Project,
                        )),
                    });
                }
                SkillRepoTransport::Http {
                    url,
                    auth_header,
                    auth_token,
                    refresh_seconds,
                } => {
                    #[cfg(feature = "skills-http")]
                    {
                        use crate::source::http::{HttpSkillAuth, HttpSkillSource};

                        let auth = match (auth_header.as_deref(), auth_token.as_deref()) {
                            (Some(header), Some(token)) => {
                                Some(HttpSkillAuth::Header {
                                    name: header.to_string(),
                                    value: token.to_string(),
                                })
                            }
                            (None, Some(token)) => {
                                Some(HttpSkillAuth::Bearer(token.to_string()))
                            }
                            _ => None,
                        };

                        sources.push(NamedSource {
                            name: repo.name.clone(),
                            source: Box::new(HttpSkillSource::new(
                                url.clone(),
                                auth,
                                std::time::Duration::from_secs(*refresh_seconds),
                            )),
                        });
                    }
                    #[cfg(not(feature = "skills-http"))]
                    {
                        let _ = (url, auth_header, auth_token, refresh_seconds);
                        tracing::warn!(
                            "Skill repository '{}' uses HTTP transport. \
                             Compile with `skills-http` feature to enable. Skipping.",
                            repo.name
                        );
                    }
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
                    use crate::source::git::{GitRef, GitSkillAuth, GitSkillConfig, GitSkillSource};

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
                    let cache_dir = project_root
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
                    };

                    sources.push(NamedSource {
                        name: repo.name.clone(),
                        source: Box::new(GitSkillSource::new(config)),
                    });
                }
            }
        }
    }

    // Embedded skills always appended as lowest precedence.
    sources.push(NamedSource {
        name: "embedded".into(),
        source: Box::new(EmbeddedSkillSource::new()),
    });

    Ok(Some(Arc::new(CompositeSkillSource::from_named(sources))))
}

/// Sanitize a repo URL into a valid directory name.
fn sanitize_repo_name(url: &str) -> String {
    url.chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use meerkat_core::skills::SkillFilter;
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
        let skills = source
            .unwrap()
            .list(&SkillFilter::default())
            .await
            .unwrap();
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
        crate::source::git::tests_support::init_test_repo(&repo_dir, &work_dir).await;

        let config = SkillsConfig {
            repositories: vec![meerkat_core::skills_config::SkillRepositoryConfig {
                name: "git-test".into(),
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
                    transport: SkillRepoTransport::Filesystem {
                        path: tmp.path().join("fs1").to_str().unwrap().into(),
                    },
                },
                meerkat_core::skills_config::SkillRepositoryConfig {
                    name: "second".into(),
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
}
