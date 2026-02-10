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
                SkillRepoTransport::Http { .. } => {
                    // HTTP source requires `skills-http` feature (Phase 10).
                    // For now, log a warning and skip.
                    tracing::warn!(
                        "Skill repository '{}' uses HTTP transport which is not yet supported. Skipping.",
                        repo.name
                    );
                }
                SkillRepoTransport::Git { .. } => {
                    // Git source requires `skills-git` feature (Phase 11).
                    // For now, log a warning and skip.
                    tracing::warn!(
                        "Skill repository '{}' uses Git transport which is not yet supported. Skipping.",
                        repo.name
                    );
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
}
