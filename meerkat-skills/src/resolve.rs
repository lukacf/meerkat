//! Skill repository resolution.

use std::path::Path;
#[cfg(not(target_arch = "wasm32"))]
use std::time::Duration;

#[cfg(not(target_arch = "wasm32"))]
use meerkat_core::skills::SkillScope;
use meerkat_core::skills::{SkillError, SourceUuid};
#[cfg(not(target_arch = "wasm32"))]
use meerkat_core::skills_config::GitRefType;
use meerkat_core::skills_config::{SkillRepoTransport, SkillsConfig};

use crate::source::composite::NamedSource;
#[cfg(not(target_arch = "wasm32"))]
use crate::source::git::{GitRef, GitSkillAuth, GitSkillConfig, GitSkillSource};
#[cfg(all(feature = "skills-http", not(target_arch = "wasm32")))]
use crate::source::http::{HttpSkillAuth, HttpSkillSource};
use crate::source::{CompositeSkillSource, EmbeddedSkillSource, SourceNode};
#[cfg(not(target_arch = "wasm32"))]
use crate::source::{ExternalSkillSource, FilesystemSkillSource, StdioExternalClient};

pub async fn resolve_repositories(
    config: &SkillsConfig,
    project_root: Option<&Path>,
) -> Result<Option<CompositeSkillSource>, SkillError> {
    let user_root = std::env::var_os("HOME").map(std::path::PathBuf::from);
    resolve_repositories_with_roots(config, project_root, user_root.as_deref(), project_root).await
}

pub async fn resolve_repositories_with_roots(
    config: &SkillsConfig,
    context_root: Option<&Path>,
    user_root: Option<&Path>,
    cache_root: Option<&Path>,
) -> Result<Option<CompositeSkillSource>, SkillError> {
    if !config.enabled {
        return Ok(None);
    }

    #[cfg(target_arch = "wasm32")]
    {
        let _ = (context_root, user_root, cache_root);
        if let Some(repo) = config.repositories.first() {
            let error = match &repo.transport {
                SkillRepoTransport::Filesystem { .. } => unavailable_on_wasm("filesystem"),
                SkillRepoTransport::Http { .. } => unavailable_on_wasm("HTTP"),
                SkillRepoTransport::Stdio { .. } => unavailable_on_wasm("stdio"),
                SkillRepoTransport::Git { .. } => unavailable_on_wasm("Git"),
            };
            return Err(error);
        }
        Ok(Some(CompositeSkillSource::from_named(vec![NamedSource {
            name: "embedded".to_string(),
            source_uuid: SourceUuid::builtin(),
            source: SourceNode::Embedded(EmbeddedSkillSource::new()),
        }])))
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        let mut sources: Vec<NamedSource> = vec![NamedSource {
            name: "embedded".to_string(),
            source_uuid: SourceUuid::builtin(),
            source: SourceNode::Embedded(EmbeddedSkillSource::new()),
        }];
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
                        source_uuid: repo.source_uuid.clone(),
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
                    #[cfg(all(feature = "skills-http", not(target_arch = "wasm32")))]
                    {
                        let auth = match (auth_header, auth_token) {
                            (Some(name), Some(value)) => Some(HttpSkillAuth::Header {
                                name: name.clone(),
                                value: value.clone(),
                            }),
                            (None, Some(value)) => Some(HttpSkillAuth::Bearer(value.clone())),
                            _ => None,
                        };
                        sources.push(NamedSource {
                            name: repo.name.clone(),
                            source_uuid: repo.source_uuid.clone(),
                            source: SourceNode::Http(Box::new(
                                HttpSkillSource::new_with_thresholds(
                                    repo.source_uuid.clone(),
                                    url.clone(),
                                    auth,
                                    Duration::from_secs(*refresh_seconds),
                                    Duration::from_secs(*timeout_seconds),
                                    config.health_thresholds,
                                ),
                            )),
                        });
                    }
                    #[cfg(any(not(feature = "skills-http"), target_arch = "wasm32"))]
                    {
                        let _ = (
                            url,
                            auth_header,
                            auth_token,
                            refresh_seconds,
                            timeout_seconds,
                        );
                        #[cfg(target_arch = "wasm32")]
                        return Err(unavailable_on_wasm("HTTP"));
                        #[cfg(not(target_arch = "wasm32"))]
                        return Err(SkillError::Load(
                            "HTTP skill repository configured but skills-http feature is disabled"
                                .into(),
                        ));
                    }
                }
                SkillRepoTransport::Stdio {
                    command,
                    args,
                    cwd,
                    env,
                    timeout_seconds,
                } => {
                    let resolved_cwd = cwd.as_ref().map(|path| {
                        if Path::new(path).is_relative() {
                            context_root.unwrap_or_else(|| Path::new(".")).join(path)
                        } else {
                            path.into()
                        }
                    });
                    let client = StdioExternalClient::new_with_timeout(
                        command.clone(),
                        args.clone(),
                        env.clone(),
                        resolved_cwd,
                        Duration::from_secs(*timeout_seconds),
                    );
                    sources.push(NamedSource {
                        name: repo.name.clone(),
                        source_uuid: repo.source_uuid.clone(),
                        source: SourceNode::External(ExternalSkillSource::new_with_source_uuid(
                            client,
                            repo.source_uuid.clone(),
                            Duration::from_secs(300),
                            config.health_thresholds,
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
                    let cache_base = cache_root
                        .or(context_root)
                        .or(user_root)
                        .unwrap_or_else(|| Path::new("."))
                        .join(".rkat/skill-cache/git");
                    let git_ref = match ref_type {
                        GitRefType::Branch => GitRef::Branch(git_ref.clone()),
                        GitRefType::Tag => GitRef::Tag(git_ref.clone()),
                        GitRefType::Commit => GitRef::Commit(git_ref.clone()),
                    };
                    let auth = auth_token
                        .as_ref()
                        .map(|token| GitSkillAuth::HttpsToken(token.clone()))
                        .or_else(|| {
                            ssh_key
                                .as_ref()
                                .map(|key| GitSkillAuth::SshKey(key.clone()))
                        });
                    sources.push(NamedSource {
                        name: repo.name.clone(),
                        source_uuid: repo.source_uuid.clone(),
                        source: SourceNode::Git(Box::new(GitSkillSource::new(GitSkillConfig {
                            repo_url: url.clone(),
                            git_ref,
                            cache_dir: cache_base,
                            skills_root: skills_root.clone(),
                            refresh_interval: Duration::from_secs(*refresh_seconds),
                            auth,
                            depth: *depth,
                            source_uuid: repo.source_uuid.clone(),
                            health_thresholds: config.health_thresholds,
                        }))),
                    });
                }
            }
        }

        if let Some(root) = context_root {
            let default_project_skills = root.join(".rkat/skills");
            if default_project_skills.is_dir() {
                sources.push(NamedSource {
                    name: "project".to_string(),
                    source_uuid: SourceUuid::project_local(),
                    source: SourceNode::Filesystem(FilesystemSkillSource::new_with_identity(
                        default_project_skills,
                        SkillScope::Project,
                        SourceUuid::project_local(),
                        config.health_thresholds,
                    )),
                });
            }
        }

        Ok(Some(CompositeSkillSource::from_named(sources)))
    }
}

#[cfg(target_arch = "wasm32")]
fn unavailable_on_wasm(transport: &str) -> SkillError {
    SkillError::Load(
        format!("{transport} skill repository configured but unavailable on wasm").into(),
    )
}
