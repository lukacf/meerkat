//! Skills configuration types.
//!
//! Defines `SkillsConfig` and repository configuration for skill sources.
//! Lives in `meerkat-core` following the `McpServerConfig` precedent.
//! Resolution logic is in `meerkat-skills::resolve`.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::{fmt::Write as _, hash::Hasher};

use crate::skills::{
    SkillAlias, SkillError, SkillKeyRemap, SourceHealthThresholds, SourceIdentityLineage,
    SourceIdentityRecord, SourceIdentityRegistry, SourceIdentityStatus, SourceTransportKind,
    SourceUuid,
};

// ---------------------------------------------------------------------------
// SkillsConfig
// ---------------------------------------------------------------------------

/// Complete skills configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SkillsConfig {
    /// Whether skills are enabled.
    pub enabled: bool,
    /// Maximum injection content size in bytes.
    pub max_injection_bytes: usize,
    /// Inventory mode threshold. When total skill count <= this value,
    /// the system prompt uses flat skill listing. When > this value,
    /// it uses collection summary mode. Default: 12.
    pub inventory_threshold: usize,
    /// Skill repository configurations (precedence = order).
    #[serde(default)]
    pub repositories: Vec<SkillRepositoryConfig>,
    /// Health classification thresholds for source state transitions.
    #[serde(default)]
    pub health_thresholds: SourceHealthThresholds,
    /// Source identity governance overlays (lineage/remaps/aliases).
    #[serde(default)]
    pub identity: SkillsIdentityConfig,
}

impl Default for SkillsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_injection_bytes: 32 * 1024,
            inventory_threshold: 12,
            repositories: Vec::new(),
            health_thresholds: SourceHealthThresholds::default(),
            identity: SkillsIdentityConfig::default(),
        }
    }
}

/// Source identity governance controls.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct SkillsIdentityConfig {
    pub lineage: Vec<SourceIdentityLineage>,
    pub remaps: Vec<SkillKeyRemap>,
    pub aliases: Vec<SkillAlias>,
}

// ---------------------------------------------------------------------------
// Repository config
// ---------------------------------------------------------------------------

/// A named skill repository.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillRepositoryConfig {
    /// Human-readable name (used in browsing, tracing, shadowing logs).
    pub name: String,
    /// Stable UUID for source governance.
    pub source_uuid: SourceUuid,
    /// Repository type and transport-specific config.
    #[serde(flatten)]
    pub transport: SkillRepoTransport,
}

/// Transport configuration for a skill repository.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum SkillRepoTransport {
    Filesystem {
        path: String,
    },
    Stdio {
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        cwd: Option<String>,
        #[serde(default)]
        env: std::collections::BTreeMap<String, String>,
        #[serde(default = "default_external_timeout_seconds")]
        timeout_seconds: u64,
    },
    Http {
        url: String,
        #[serde(default)]
        auth_header: Option<String>,
        #[serde(default)]
        auth_token: Option<String>,
        #[serde(default = "default_refresh_seconds")]
        refresh_seconds: u64,
        #[serde(default = "default_external_timeout_seconds")]
        timeout_seconds: u64,
    },
    Git {
        url: String,
        #[serde(default = "default_git_ref")]
        git_ref: String,
        #[serde(default)]
        ref_type: GitRefType,
        #[serde(default)]
        skills_root: Option<String>,
        #[serde(default)]
        auth_token: Option<String>,
        #[serde(default)]
        ssh_key: Option<String>,
        #[serde(default = "default_refresh_seconds")]
        refresh_seconds: u64,
        #[serde(default = "default_clone_depth")]
        depth: Option<usize>,
    },
}

fn default_refresh_seconds() -> u64 {
    300
}

fn default_external_timeout_seconds() -> u64 {
    15
}

fn default_git_ref() -> String {
    "main".to_string()
}

fn default_clone_depth() -> Option<usize> {
    Some(1)
}

/// Git ref type for version pinning.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GitRefType {
    #[default]
    Branch,
    Tag,
    Commit,
}

// ---------------------------------------------------------------------------
// Config errors
// ---------------------------------------------------------------------------

/// Errors from skills config operations.
#[derive(Debug, thiserror::Error)]
pub enum SkillsConfigError {
    #[error("IO error: {0}")]
    Io(String),
    #[error("Parse error in {path}: {message}")]
    Parse { path: String, message: String },
    #[error("Missing environment variable '{var}' referenced in {field}")]
    MissingEnvVar { field: String, var: String },
    #[error("Invalid environment variable reference in {field}: '{value}'")]
    InvalidEnvVarSyntax { field: String, value: String },
}

// ---------------------------------------------------------------------------
// Config loading
// ---------------------------------------------------------------------------

impl SkillsConfig {
    /// Load from user + project config, project wins.
    pub async fn load() -> Result<Self, SkillsConfigError> {
        let user = user_skills_path();
        let project = project_skills_path();
        Self::load_from_paths(user.as_deref(), project.as_deref()).await
    }

    /// Load from explicit paths (useful for testing).
    pub async fn load_from_paths(
        user_path: Option<&Path>,
        project_path: Option<&Path>,
    ) -> Result<Self, SkillsConfigError> {
        let user_cfg = read_skills_file(user_path)
            .await?
            .unwrap_or_else(SkillsConfig::default);
        let (project_cfg, project_has_file) = match read_skills_file(project_path).await? {
            Some(cfg) => (cfg, true),
            None => (SkillsConfig::default(), false),
        };
        Ok(merge_project_over_user(
            user_cfg,
            project_cfg,
            project_has_file,
        ))
    }

    /// Build a source identity registry from repository config + governance overlays.
    pub fn build_source_identity_registry(&self) -> Result<SourceIdentityRegistry, SkillError> {
        let records = self
            .repositories
            .iter()
            .map(repository_to_identity_record)
            .collect();
        SourceIdentityRegistry::build(
            records,
            self.identity.lineage.clone(),
            self.identity.remaps.clone(),
            self.identity.aliases.clone(),
        )
    }
}

fn repository_to_identity_record(repo: &SkillRepositoryConfig) -> SourceIdentityRecord {
    SourceIdentityRecord {
        source_uuid: repo.source_uuid.clone(),
        display_name: repo.name.clone(),
        transport_kind: repository_transport_kind(&repo.transport),
        fingerprint: repository_fingerprint(repo),
        status: SourceIdentityStatus::Active,
    }
}

fn repository_transport_kind(transport: &SkillRepoTransport) -> SourceTransportKind {
    match transport {
        SkillRepoTransport::Filesystem { .. } => SourceTransportKind::Filesystem,
        SkillRepoTransport::Stdio { .. } => SourceTransportKind::Stdio,
        SkillRepoTransport::Http { .. } => SourceTransportKind::Http,
        SkillRepoTransport::Git { .. } => SourceTransportKind::Git,
    }
}

fn repository_fingerprint(repo: &SkillRepositoryConfig) -> String {
    // Deterministic fingerprint derived from the configured source location/transport.
    // Keep this stable to ensure deterministic governance behavior across restarts.
    let material = match &repo.transport {
        SkillRepoTransport::Filesystem { path } => format!("filesystem:{path}"),
        SkillRepoTransport::Stdio {
            command, cwd, env, ..
        } => format!("stdio:{command}:{cwd:?}:{:?}", env),
        SkillRepoTransport::Http { url, .. } => format!("http:{url}"),
        SkillRepoTransport::Git {
            url,
            git_ref,
            ref_type,
            skills_root,
            ..
        } => format!("git:{url}:{git_ref}:{ref_type:?}:{:?}", skills_root),
    };
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    hasher.write(material.as_bytes());
    let hash = hasher.finish();
    let mut fp = String::with_capacity(19);
    fp.push_str("repo-");
    let _ = write!(&mut fp, "{hash:016x}");
    fp
}

async fn read_skills_file(path: Option<&Path>) -> Result<Option<SkillsConfig>, SkillsConfigError> {
    let Some(path) = path else {
        return Ok(None);
    };
    if !tokio::fs::try_exists(path)
        .await
        .map_err(|e| SkillsConfigError::Io(e.to_string()))?
    {
        return Ok(None);
    }
    let contents = tokio::fs::read_to_string(path)
        .await
        .map_err(|e| SkillsConfigError::Io(e.to_string()))?;
    let mut parsed: SkillsConfig =
        toml::from_str(&contents).map_err(|e| SkillsConfigError::Parse {
            path: path.display().to_string(),
            message: e.to_string(),
        })?;
    expand_env_in_config(&mut parsed)?;
    Ok(Some(parsed))
}

fn merge_project_over_user(
    user: SkillsConfig,
    project: SkillsConfig,
    project_has_file: bool,
) -> SkillsConfig {
    let mut seen: HashSet<String> = HashSet::new();
    let mut merged_repos: Vec<SkillRepositoryConfig> = Vec::new();

    // Project first (highest precedence)
    for repo in project.repositories {
        if seen.insert(repo.name.clone()) {
            merged_repos.push(repo);
        }
    }
    // Then user repos not shadowed by project
    for repo in user.repositories {
        if seen.insert(repo.name.clone()) {
            merged_repos.push(repo);
        }
    }

    // Project scalars take precedence only when a project file was present.
    // `project_has_file` is true when the project config was loaded from disk
    // (not a default placeholder).
    SkillsConfig {
        enabled: if project_has_file {
            project.enabled
        } else {
            user.enabled
        },
        max_injection_bytes: if project_has_file {
            project.max_injection_bytes
        } else {
            user.max_injection_bytes
        },
        inventory_threshold: if project_has_file {
            project.inventory_threshold
        } else {
            user.inventory_threshold
        },
        health_thresholds: if project_has_file {
            project.health_thresholds
        } else {
            user.health_thresholds
        },
        repositories: merged_repos,
        identity: if project_has_file {
            project.identity
        } else {
            user.identity
        },
    }
}

// ---------------------------------------------------------------------------
// Env expansion
// ---------------------------------------------------------------------------

fn expand_env_in_config(config: &mut SkillsConfig) -> Result<(), SkillsConfigError> {
    for repo in &mut config.repositories {
        match &mut repo.transport {
            SkillRepoTransport::Stdio {
                command,
                args,
                cwd,
                env,
                ..
            } => {
                *command = expand_env_in_string(command, "repositories[].command")?;
                for arg in args {
                    *arg = expand_env_in_string(arg, "repositories[].args[]")?;
                }
                if let Some(dir) = cwd {
                    *dir = expand_env_in_string(dir, "repositories[].cwd")?;
                }
                for value in env.values_mut() {
                    *value = expand_env_in_string(value, "repositories[].env[]")?;
                }
            }
            SkillRepoTransport::Http {
                auth_token, url, ..
            } => {
                *url = expand_env_in_string(url, "repositories[].url")?;
                if let Some(token) = auth_token {
                    *token = expand_env_in_string(token, "repositories[].auth_token")?;
                }
            }
            SkillRepoTransport::Git {
                auth_token,
                url,
                ssh_key,
                ..
            } => {
                *url = expand_env_in_string(url, "repositories[].url")?;
                if let Some(token) = auth_token {
                    *token = expand_env_in_string(token, "repositories[].auth_token")?;
                }
                if let Some(key) = ssh_key {
                    *key = expand_env_in_string(key, "repositories[].ssh_key")?;
                }
            }
            SkillRepoTransport::Filesystem { .. } => {}
        }
    }
    Ok(())
}

fn expand_env_in_string(value: &str, field: &str) -> Result<String, SkillsConfigError> {
    let mut output = String::with_capacity(value.len());
    let mut remaining = value;
    while let Some(start) = remaining.find("${") {
        output.push_str(&remaining[..start]);
        let after = &remaining[start + 2..];
        let Some(end) = after.find('}') else {
            return Err(SkillsConfigError::InvalidEnvVarSyntax {
                field: field.to_string(),
                value: value.to_string(),
            });
        };
        let var_name = &after[..end];
        if var_name.is_empty() {
            return Err(SkillsConfigError::InvalidEnvVarSyntax {
                field: field.to_string(),
                value: value.to_string(),
            });
        }
        let var_value = std::env::var(var_name).map_err(|_| SkillsConfigError::MissingEnvVar {
            field: field.to_string(),
            var: var_name.to_string(),
        })?;
        output.push_str(&var_value);
        remaining = &after[end + 1..];
    }
    output.push_str(remaining);
    Ok(output)
}

// ---------------------------------------------------------------------------
// Path helpers
// ---------------------------------------------------------------------------

/// Get the user-level skills config path: ~/.rkat/skills.toml
fn user_skills_path() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".rkat/skills.toml"))
}

/// Get the project-level skills config path: .rkat/skills.toml
fn project_skills_path() -> Option<PathBuf> {
    std::env::current_dir()
        .ok()
        .map(|cwd| cwd.join(".rkat/skills.toml"))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn test_source_uuid() -> &'static str {
        "dc256086-0d2f-4f61-a307-320d4148107f"
    }

    #[test]
    fn test_default_config() {
        let config = SkillsConfig::default();
        assert!(config.enabled);
        assert_eq!(config.max_injection_bytes, 32 * 1024);
        assert_eq!(config.inventory_threshold, 12);
        assert!(config.repositories.is_empty());
    }

    #[test]
    fn test_parse_filesystem_repo() {
        let toml = r#"
enabled = true

[[repositories]]
name = "project"
source_uuid = "dc256086-0d2f-4f61-a307-320d4148107f"
type = "filesystem"
path = ".rkat/skills"
"#;
        let config: SkillsConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.repositories.len(), 1);
        assert_eq!(config.repositories[0].name, "project");
        assert_eq!(
            config.repositories[0].source_uuid.to_string(),
            test_source_uuid()
        );
        assert!(matches!(
            &config.repositories[0].transport,
            SkillRepoTransport::Filesystem { path } if path == ".rkat/skills"
        ));
    }

    #[test]
    fn test_parse_http_repo() {
        let toml = r#"
[[repositories]]
name = "elephant"
source_uuid = "dc256086-0d2f-4f61-a307-320d4148107f"
type = "http"
url = "http://localhost:8080/api"
auth_header = "X-API-Key"
auth_token = "secret"
refresh_seconds = 60
"#;
        let config: SkillsConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.repositories.len(), 1);
        let SkillRepoTransport::Http {
            url,
            auth_header,
            auth_token,
            refresh_seconds,
            ..
        } = &config.repositories[0].transport
        else {
            unreachable!("Expected Http transport");
        };
        assert_eq!(url, "http://localhost:8080/api");
        assert_eq!(auth_header.as_deref(), Some("X-API-Key"));
        assert_eq!(auth_token.as_deref(), Some("secret"));
        assert_eq!(*refresh_seconds, 60);
    }

    #[test]
    fn test_parse_stdio_repo() {
        let toml = r#"
[[repositories]]
name = "external-stdio"
source_uuid = "dc256086-0d2f-4f61-a307-320d4148107f"
type = "stdio"
command = "node"
args = ["skills-server.js", "--mode", "stdio"]
cwd = ".rkat/skills"
timeout_seconds = 7
"#;
        let config: SkillsConfig = toml::from_str(toml).unwrap();
        let SkillRepoTransport::Stdio {
            command,
            args,
            cwd,
            timeout_seconds,
            ..
        } = &config.repositories[0].transport
        else {
            unreachable!("Expected Stdio transport");
        };
        assert_eq!(command, "node");
        assert_eq!(args.len(), 3);
        assert_eq!(cwd.as_deref(), Some(".rkat/skills"));
        assert_eq!(*timeout_seconds, 7);
    }

    #[test]
    fn test_parse_git_repo() {
        let toml = r#"
[[repositories]]
name = "company"
source_uuid = "dc256086-0d2f-4f61-a307-320d4148107f"
type = "git"
url = "https://github.com/company/skills.git"
git_ref = "v1.2.0"
ref_type = "tag"
auth_token = "ghp_token"
"#;
        let config: SkillsConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.repositories.len(), 1);
        let SkillRepoTransport::Git {
            url,
            git_ref,
            ref_type,
            auth_token,
            ..
        } = &config.repositories[0].transport
        else {
            unreachable!("Expected Git transport");
        };
        assert_eq!(url, "https://github.com/company/skills.git");
        assert_eq!(git_ref, "v1.2.0");
        assert!(matches!(ref_type, GitRefType::Tag));
        assert_eq!(auth_token.as_deref(), Some("ghp_token"));
    }

    #[test]
    fn test_env_expansion_in_auth_token() {
        // Test with a var that exists on all platforms
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
        let result = expand_env_in_string("path=${HOME}/skills", "test_field").unwrap();
        assert_eq!(result, format!("path={home}/skills"));
    }

    #[test]
    fn test_env_expansion_missing_var_errors() {
        let result = expand_env_in_string("${RKAT_NONEXISTENT_VAR_SKILLS_TEST}", "test_field");
        assert!(matches!(
            result,
            Err(SkillsConfigError::MissingEnvVar { .. })
        ));
    }

    #[test]
    fn test_merge_project_over_user() {
        let user = SkillsConfig {
            repositories: vec![
                SkillRepositoryConfig {
                    name: "company".into(),
                    source_uuid: SourceUuid::parse("00000000-0000-4000-8000-000000000001")
                        .expect("uuid"),
                    transport: SkillRepoTransport::Filesystem {
                        path: "user-path".into(),
                    },
                },
                SkillRepositoryConfig {
                    name: "personal".into(),
                    source_uuid: SourceUuid::parse("00000000-0000-4000-8000-000000000002")
                        .expect("uuid"),
                    transport: SkillRepoTransport::Filesystem {
                        path: "personal-path".into(),
                    },
                },
            ],
            ..Default::default()
        };

        let project = SkillsConfig {
            repositories: vec![
                SkillRepositoryConfig {
                    name: "project".into(),
                    source_uuid: SourceUuid::parse("00000000-0000-4000-8000-000000000003")
                        .expect("uuid"),
                    transport: SkillRepoTransport::Filesystem {
                        path: "project-path".into(),
                    },
                },
                SkillRepositoryConfig {
                    name: "company".into(),
                    source_uuid: SourceUuid::parse("00000000-0000-4000-8000-000000000004")
                        .expect("uuid"),
                    transport: SkillRepoTransport::Filesystem {
                        path: "project-company-path".into(),
                    },
                },
            ],
            ..Default::default()
        };

        let merged = merge_project_over_user(user, project, true);
        assert_eq!(merged.repositories.len(), 3);
        assert_eq!(merged.repositories[0].name, "project");
        assert_eq!(merged.repositories[1].name, "company");
        assert_eq!(merged.repositories[2].name, "personal");

        // Company should be from project (first wins)
        let SkillRepoTransport::Filesystem { path } = &merged.repositories[1].transport else {
            unreachable!("Expected Filesystem transport");
        };
        assert_eq!(path, "project-company-path");
    }

    #[test]
    fn test_merge_project_shadows_user_repo() {
        let user = SkillsConfig {
            repositories: vec![SkillRepositoryConfig {
                name: "shared".into(),
                source_uuid: SourceUuid::parse("00000000-0000-4000-8000-000000000005")
                    .expect("uuid"),
                transport: SkillRepoTransport::Filesystem {
                    path: "user-shared".into(),
                },
            }],
            ..Default::default()
        };

        let project = SkillsConfig {
            repositories: vec![SkillRepositoryConfig {
                name: "shared".into(),
                source_uuid: SourceUuid::parse("00000000-0000-4000-8000-000000000006")
                    .expect("uuid"),
                transport: SkillRepoTransport::Filesystem {
                    path: "project-shared".into(),
                },
            }],
            ..Default::default()
        };

        let merged = merge_project_over_user(user, project, true);
        assert_eq!(merged.repositories.len(), 1);
        let SkillRepoTransport::Filesystem { path } = &merged.repositories[0].transport else {
            unreachable!("Expected Filesystem transport");
        };
        assert_eq!(path, "project-shared");
    }

    #[tokio::test]
    async fn test_load_from_paths_no_files() {
        let config = SkillsConfig::load_from_paths(None, None).await.unwrap();
        assert!(config.enabled);
        assert!(config.repositories.is_empty());
    }

    #[tokio::test]
    async fn test_load_from_paths_with_file() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("skills.toml");
        tokio::fs::write(
            &file,
            r#"
enabled = true
[[repositories]]
name = "test"
source_uuid = "dc256086-0d2f-4f61-a307-320d4148107f"
type = "filesystem"
path = "/tmp/skills"
"#,
        )
        .await
        .unwrap();

        let config = SkillsConfig::load_from_paths(None, Some(&file))
            .await
            .unwrap();
        assert_eq!(config.repositories.len(), 1);
        assert_eq!(config.repositories[0].name, "test");
    }

    #[test]
    fn test_missing_source_uuid_is_rejected() {
        let toml = r#"
[[repositories]]
name = "project"
type = "filesystem"
path = ".rkat/skills"
"#;
        let result = toml::from_str::<SkillsConfig>(toml);
        assert!(result.is_err(), "source_uuid must be required");
    }

    #[test]
    fn test_build_identity_registry_from_config_rejects_partial_split_remap() {
        let cfg = SkillsConfig {
            repositories: vec![
                SkillRepositoryConfig {
                    name: "old".to_string(),
                    source_uuid: SourceUuid::parse("dc256086-0d2f-4f61-a307-320d4148107f")
                        .expect("uuid"),
                    transport: SkillRepoTransport::Filesystem {
                        path: ".rkat/skills-old".to_string(),
                    },
                },
                SkillRepositoryConfig {
                    name: "new-a".to_string(),
                    source_uuid: SourceUuid::parse("a93d587d-8f44-438f-8189-6e8cf549f6e7")
                        .expect("uuid"),
                    transport: SkillRepoTransport::Filesystem {
                        path: ".rkat/skills-a".to_string(),
                    },
                },
                SkillRepositoryConfig {
                    name: "new-b".to_string(),
                    source_uuid: SourceUuid::parse("e8df561d-d38f-4242-af55-3a6efb34c950")
                        .expect("uuid"),
                    transport: SkillRepoTransport::Filesystem {
                        path: ".rkat/skills-b".to_string(),
                    },
                },
            ],
            identity: SkillsIdentityConfig {
                lineage: vec![SourceIdentityLineage {
                    event_id: "split-1".to_string(),
                    recorded_at_unix_secs: 1,
                    required_from_skills: vec![
                        crate::skills::SkillName::parse("email-extractor").expect("skill"),
                        crate::skills::SkillName::parse("pdf-processing").expect("skill"),
                    ],
                    event: crate::skills::SourceIdentityLineageEvent::Split {
                        from: SourceUuid::parse("dc256086-0d2f-4f61-a307-320d4148107f")
                            .expect("uuid"),
                        into: vec![
                            SourceUuid::parse("a93d587d-8f44-438f-8189-6e8cf549f6e7")
                                .expect("uuid"),
                            SourceUuid::parse("e8df561d-d38f-4242-af55-3a6efb34c950")
                                .expect("uuid"),
                        ],
                    },
                }],
                remaps: vec![SkillKeyRemap {
                    from: crate::skills::SkillKey {
                        source_uuid: SourceUuid::parse("dc256086-0d2f-4f61-a307-320d4148107f")
                            .expect("uuid"),
                        skill_name: crate::skills::SkillName::parse("email-extractor")
                            .expect("skill"),
                    },
                    to: crate::skills::SkillKey {
                        source_uuid: SourceUuid::parse("a93d587d-8f44-438f-8189-6e8cf549f6e7")
                            .expect("uuid"),
                        skill_name: crate::skills::SkillName::parse("mail-extractor")
                            .expect("skill"),
                    },
                    reason: None,
                }],
                aliases: vec![],
            },
            ..SkillsConfig::default()
        };

        let result = cfg.build_source_identity_registry();
        assert!(matches!(result, Err(SkillError::MissingSkillRemaps { .. })));
    }
}
