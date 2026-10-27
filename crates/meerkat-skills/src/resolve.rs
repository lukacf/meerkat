//! Skill repository resolution.

use std::path::Path;
use std::sync::Arc;
#[cfg(not(target_arch = "wasm32"))]
use std::time::Duration;

#[cfg(not(target_arch = "wasm32"))]
use meerkat_core::skills::SkillScope;
use meerkat_core::skills::{
    ResolveError, SkillError, SkillKey, SkillName, SourceIdentityRecord, SourceIdentityRegistry,
    SourceUuid,
};
#[cfg(not(target_arch = "wasm32"))]
use meerkat_core::skills_config::GitRefType;
#[cfg(not(target_arch = "wasm32"))]
use meerkat_core::skills_config::GitSkillRepoAuth;
#[cfg(all(feature = "skills-http", not(target_arch = "wasm32")))]
use meerkat_core::skills_config::HttpSkillRepoAuth;
use meerkat_core::skills_config::{SkillRepoTransport, SkillsConfig};

use crate::source::composite::NamedSource;
#[cfg(not(target_arch = "wasm32"))]
use crate::source::git::{GitRef, GitSkillAuth, GitSkillConfig, GitSkillSource};
#[cfg(all(feature = "skills-http", not(target_arch = "wasm32")))]
use crate::source::http::{HttpSkillAuth, HttpSkillSource};
use crate::source::{CompositeSkillSource, EmbeddedSkillSource, SourceNode};
#[cfg(not(target_arch = "wasm32"))]
use crate::source::{ExternalSkillSource, FilesystemSkillSource, StdioExternalClient};

#[deprecated(
    since = "0.8.4",
    note = "ambient $HOME resolution is deprecated; use \
            `resolve_repositories_with_roots` with explicit roots"
)]
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
    resolve_repositories_with_roots_trusting(
        config,
        context_root,
        user_root,
        cache_root,
        &SkillHostBoundary::trusted_host(),
    )
    .await
}

/// Whether a confined process could change what the host reads at a path
/// (`true`: anything under it as well).
pub type UntrustedPaths = Arc<dyn Fn(&Path, bool) -> bool + Send + Sync>;

/// How skill repository resolution meets a host that confines its agents'
/// launches: the paths its confined processes could change, and how a stdio
/// skill source's process launches. The default is the trusted host: every
/// path is trusted and stdio skill sources run as the host.
#[derive(Clone, Default)]
pub struct SkillHostBoundary {
    #[cfg(not(target_arch = "wasm32"))]
    untrusted: Option<UntrustedPaths>,
    #[cfg(not(target_arch = "wasm32"))]
    stdio_runner: Option<Arc<dyn crate::source::StdioSkillRunner>>,
}

impl SkillHostBoundary {
    /// The trusted host.
    #[must_use]
    pub fn trusted_host() -> Self {
        Self::default()
    }

    /// Do not trust the paths `untrusted` names: a skills directory or the
    /// Git skill cache a confined process could change is refused, a skill
    /// linked to such a path is not read, and `git` runs from a `PATH`
    /// without them.
    #[cfg(not(target_arch = "wasm32"))]
    #[must_use]
    pub fn with_untrusted_paths(mut self, untrusted: UntrustedPaths) -> Self {
        self.untrusted = Some(untrusted);
        self
    }

    /// Launch stdio skill sources through `runner` (a host passes one that
    /// applies its `mcp_stdio` confinement).
    #[cfg(not(target_arch = "wasm32"))]
    #[must_use]
    pub fn with_stdio_runner(mut self, runner: Arc<dyn crate::source::StdioSkillRunner>) -> Self {
        self.stdio_runner = Some(runner);
        self
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn untrusted(&self, path: &Path, subtree: bool) -> bool {
        self.untrusted
            .as_ref()
            .is_some_and(|untrusted| untrusted(path, subtree))
    }
}

/// [`resolve_repositories_with_roots`] for a host that confines its agents'
/// launches ([`SkillHostBoundary`]). Each skills directory `host` does not
/// trust is refused: a filesystem repository's directory, the local cache
/// Git repositories are fetched into, and the project skills directory
/// (`<context>/.rkat/skills`). `git`, and every program it starts by name,
/// runs from a `PATH` without the directories `host` does not trust. A
/// skill linked out of a trusted directory is judged where the link leads;
/// one linked out of a Git checkout is not read. Stdio skill sources launch through the host's
/// runner. Each refusal is logged; the embedded skills and every other
/// source still load.
pub async fn resolve_repositories_with_roots_trusting(
    config: &SkillsConfig,
    context_root: Option<&Path>,
    user_root: Option<&Path>,
    cache_root: Option<&Path>,
    host: &SkillHostBoundary,
) -> Result<Option<CompositeSkillSource>, SkillError> {
    if !config.enabled {
        return Ok(None);
    }

    let registry = Arc::new(config.build_source_identity_registry()?);
    let records = config.source_identity_records();

    #[cfg(target_arch = "wasm32")]
    {
        let _ = (context_root, user_root, cache_root, host);
        if let Some(repo) = config.repositories.first() {
            let error = match &repo.transport {
                SkillRepoTransport::Filesystem { .. } => unavailable_on_wasm("filesystem"),
                SkillRepoTransport::Http { .. } => unavailable_on_wasm("HTTP"),
                SkillRepoTransport::Stdio { .. } => unavailable_on_wasm("stdio"),
                SkillRepoTransport::Git { .. } => unavailable_on_wasm("Git"),
            };
            return Err(error);
        }
        let builtin_identity = active_source_identity(&registry, &records, &SourceUuid::builtin())?;
        Ok(Some(CompositeSkillSource::from_named_with_registry(
            vec![NamedSource::new(
                builtin_identity,
                SourceNode::Embedded(EmbeddedSkillSource::new()),
            )],
            registry,
        )))
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        let builtin_identity = active_source_identity(&registry, &records, &SourceUuid::builtin())?;
        let mut sources: Vec<NamedSource> = vec![NamedSource::new(
            builtin_identity,
            SourceNode::Embedded(EmbeddedSkillSource::new()),
        )];
        for repo in &config.repositories {
            let source_identity = active_source_identity(&registry, &records, &repo.source_uuid)?;
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
                    if host.untrusted(&full_path, true) {
                        refuse_untrusted_skills_directory(&full_path);
                        continue;
                    }
                    sources.push(NamedSource::new(
                        source_identity,
                        SourceNode::Filesystem(judged_links(
                            FilesystemSkillSource::new_with_identity(
                                full_path,
                                SkillScope::Project,
                                repo.source_uuid.clone(),
                                config.health_thresholds,
                            ),
                            host,
                        )),
                    ));
                }
                SkillRepoTransport::Http {
                    url,
                    auth,
                    refresh_seconds,
                    timeout_seconds,
                } => {
                    #[cfg(all(feature = "skills-http", not(target_arch = "wasm32")))]
                    {
                        let auth = auth.as_ref().map(|auth| match auth {
                            HttpSkillRepoAuth::Bearer { token } => {
                                HttpSkillAuth::Bearer(token.clone())
                            }
                            HttpSkillRepoAuth::Header { name, value } => HttpSkillAuth::Header {
                                name: name.clone(),
                                value: value.clone(),
                            },
                        });
                        sources.push(NamedSource::new(
                            source_identity,
                            SourceNode::Http(Box::new(HttpSkillSource::new_with_thresholds(
                                repo.source_uuid.clone(),
                                url.clone(),
                                auth,
                                Duration::from_secs(*refresh_seconds),
                                Duration::from_secs(*timeout_seconds),
                                config.health_thresholds,
                            ))),
                        ));
                    }
                    #[cfg(any(not(feature = "skills-http"), target_arch = "wasm32"))]
                    {
                        let _ = (url, auth, refresh_seconds, timeout_seconds);
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
                    let mut client = StdioExternalClient::new_with_timeout(
                        command.clone(),
                        args.clone(),
                        env.clone(),
                        resolved_cwd,
                        Duration::from_secs(*timeout_seconds),
                    );
                    if let Some(runner) = &host.stdio_runner {
                        client = client.with_runner(Arc::clone(runner));
                    }
                    sources.push(NamedSource::new(
                        source_identity,
                        SourceNode::External(ExternalSkillSource::new_with_source_uuid(
                            client,
                            repo.source_uuid.clone(),
                            Duration::from_mins(5),
                            config.health_thresholds,
                        )),
                    ));
                }
                SkillRepoTransport::Git {
                    url,
                    git_ref,
                    ref_type,
                    skills_root,
                    auth,
                    refresh_seconds,
                    depth,
                } => {
                    let cache_base = cache_root
                        .or(context_root)
                        .or(user_root)
                        .unwrap_or_else(|| Path::new("."))
                        .join(".rkat/skill-cache/git");
                    if host.untrusted(&cache_base, true) {
                        refuse_untrusted_skills_directory(&cache_base);
                        continue;
                    }
                    let git_ref = match ref_type {
                        GitRefType::Branch => GitRef::Branch(git_ref.clone()),
                        GitRefType::Tag => GitRef::Tag(git_ref.clone()),
                        GitRefType::Commit => GitRef::Commit(git_ref.clone()),
                    };
                    let auth = auth.as_ref().map(|auth| match auth {
                        GitSkillRepoAuth::Token { token } => {
                            GitSkillAuth::HttpsToken(token.clone())
                        }
                        GitSkillRepoAuth::SshKey { path } => GitSkillAuth::SshKey(path.clone()),
                    });
                    let mut git = GitSkillSource::new(GitSkillConfig {
                        repo_url: url.clone(),
                        git_ref,
                        cache_dir: cache_base,
                        skills_root: skills_root.clone(),
                        refresh_interval: Duration::from_secs(*refresh_seconds),
                        auth,
                        depth: *depth,
                        source_uuid: repo.source_uuid.clone(),
                        health_thresholds: config.health_thresholds,
                    });
                    if let Some(untrusted) = &host.untrusted {
                        git = git.with_untrusted_paths(Arc::clone(untrusted));
                    }
                    sources.push(NamedSource::new(
                        source_identity,
                        SourceNode::Git(Box::new(git)),
                    ));
                }
            }
        }

        if let Some(root) = context_root {
            let default_project_skills = root.join(".rkat/skills");
            if default_project_skills.is_dir() && host.untrusted(&default_project_skills, true) {
                refuse_untrusted_skills_directory(&default_project_skills);
            } else if default_project_skills.is_dir() {
                let project_identity =
                    active_source_identity(&registry, &records, &SourceUuid::project_local())?;
                sources.push(NamedSource::new(
                    project_identity,
                    SourceNode::Filesystem(judged_links(
                        FilesystemSkillSource::new_with_identity(
                            default_project_skills,
                            SkillScope::Project,
                            SourceUuid::project_local(),
                            config.health_thresholds,
                        ),
                        host,
                    )),
                ));
            }
        }

        Ok(Some(CompositeSkillSource::from_named_with_registry(
            sources, registry,
        )))
    }
}

/// `source`, judging each skill a link leads out of its directory by the
/// paths `host` does not trust.
#[cfg(not(target_arch = "wasm32"))]
fn judged_links(source: FilesystemSkillSource, host: &SkillHostBoundary) -> FilesystemSkillSource {
    match &host.untrusted {
        Some(untrusted) => source.with_untrusted_targets(Arc::clone(untrusted)),
        None => source,
    }
}

/// Log a skills directory the host does not trust, whose skills are not
/// loaded.
#[cfg(not(target_arch = "wasm32"))]
fn refuse_untrusted_skills_directory(directory: &Path) {
    tracing::warn!(
        directory = %directory.display(),
        "skills directory not trusted: a confined process can write it, so its skills are not loaded"
    );
}

fn active_source_identity(
    registry: &SourceIdentityRegistry,
    records: &[SourceIdentityRecord],
    source_uuid: &SourceUuid,
) -> Result<SourceIdentityRecord, SkillError> {
    let probe = SkillKey::new(
        source_uuid.clone(),
        SkillName::parse("source-identity-probe")?,
    );
    let resolved = registry
        .resolve(&probe)
        .map_err(source_identity_resolution_error)?;
    records
        .iter()
        .find(|record| record.source_uuid == resolved.source.source_uuid)
        .cloned()
        .ok_or_else(|| {
            SkillError::Load(
                format!(
                    "source identity registry resolved {source_uuid} without a canonical record"
                )
                .into(),
            )
        })
}

fn source_identity_resolution_error(error: ResolveError) -> SkillError {
    SkillError::Load(format!("source identity resolution failed: {error}").into())
}

#[cfg(target_arch = "wasm32")]
fn unavailable_on_wasm(transport: &str) -> SkillError {
    SkillError::Load(
        format!("{transport} skill repository configured but unavailable on wasm").into(),
    )
}
