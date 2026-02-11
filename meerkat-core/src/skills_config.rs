//! Skills configuration types.
//!
//! Defines `SkillsConfig` and repository configuration for skill sources.
//! Lives in `meerkat-core` following the `McpServerConfig` precedent.
//! Resolution logic is in `meerkat-skills::resolve`.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

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
}

impl Default for SkillsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_injection_bytes: 32 * 1024,
            inventory_threshold: 12,
            repositories: Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Repository config
// ---------------------------------------------------------------------------

/// A named skill repository.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillRepositoryConfig {
    /// Human-readable name (used in browsing, tracing, shadowing logs).
    pub name: String,
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
    Http {
        url: String,
        #[serde(default)]
        auth_header: Option<String>,
        #[serde(default)]
        auth_token: Option<String>,
        #[serde(default = "default_refresh_seconds")]
        refresh_seconds: u64,
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
        let user_cfg = read_skills_file(user_path).await?.unwrap_or_else(SkillsConfig::default);
        let (project_cfg, project_has_file) = match read_skills_file(project_path).await? {
            Some(cfg) => (cfg, true),
            None => (SkillsConfig::default(), false),
        };
        Ok(merge_project_over_user(user_cfg, project_cfg, project_has_file))
    }
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

fn merge_project_over_user(user: SkillsConfig, project: SkillsConfig, project_has_file: bool) -> SkillsConfig {
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
        enabled: if project_has_file { project.enabled } else { user.enabled },
        max_injection_bytes: if project_has_file { project.max_injection_bytes } else { user.max_injection_bytes },
        inventory_threshold: if project_has_file { project.inventory_threshold } else { user.inventory_threshold },
        repositories: merged_repos,
    }
}

// ---------------------------------------------------------------------------
// Env expansion
// ---------------------------------------------------------------------------

fn expand_env_in_config(config: &mut SkillsConfig) -> Result<(), SkillsConfigError> {
    for repo in &mut config.repositories {
        match &mut repo.transport {
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
        let var_value =
            std::env::var(var_name).map_err(|_| SkillsConfigError::MissingEnvVar {
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
type = "filesystem"
path = ".rkat/skills"
"#;
        let config: SkillsConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.repositories.len(), 1);
        assert_eq!(config.repositories[0].name, "project");
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
type = "http"
url = "http://localhost:8080/api"
auth_header = "X-API-Key"
auth_token = "secret"
refresh_seconds = 60
"#;
        let config: SkillsConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.repositories.len(), 1);
        match &config.repositories[0].transport {
            SkillRepoTransport::Http {
                url,
                auth_header,
                auth_token,
                refresh_seconds,
            } => {
                assert_eq!(url, "http://localhost:8080/api");
                assert_eq!(auth_header.as_deref(), Some("X-API-Key"));
                assert_eq!(auth_token.as_deref(), Some("secret"));
                assert_eq!(*refresh_seconds, 60);
            }
            other => panic!("Expected Http, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_git_repo() {
        let toml = r#"
[[repositories]]
name = "company"
type = "git"
url = "https://github.com/company/skills.git"
git_ref = "v1.2.0"
ref_type = "tag"
auth_token = "ghp_token"
"#;
        let config: SkillsConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.repositories.len(), 1);
        match &config.repositories[0].transport {
            SkillRepoTransport::Git {
                url,
                git_ref,
                ref_type,
                auth_token,
                ..
            } => {
                assert_eq!(url, "https://github.com/company/skills.git");
                assert_eq!(git_ref, "v1.2.0");
                assert!(matches!(ref_type, GitRefType::Tag));
                assert_eq!(auth_token.as_deref(), Some("ghp_token"));
            }
            other => panic!("Expected Git, got {other:?}"),
        }
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
        let result = expand_env_in_string(
            "${RKAT_NONEXISTENT_VAR_SKILLS_TEST}",
            "test_field",
        );
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
                    transport: SkillRepoTransport::Filesystem {
                        path: "user-path".into(),
                    },
                },
                SkillRepositoryConfig {
                    name: "personal".into(),
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
                    transport: SkillRepoTransport::Filesystem {
                        path: "project-path".into(),
                    },
                },
                SkillRepositoryConfig {
                    name: "company".into(),
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
        match &merged.repositories[1].transport {
            SkillRepoTransport::Filesystem { path } => {
                assert_eq!(path, "project-company-path");
            }
            other => panic!("Expected Filesystem, got {other:?}"),
        }
    }

    #[test]
    fn test_merge_project_shadows_user_repo() {
        let user = SkillsConfig {
            repositories: vec![SkillRepositoryConfig {
                name: "shared".into(),
                transport: SkillRepoTransport::Filesystem {
                    path: "user-shared".into(),
                },
            }],
            ..Default::default()
        };

        let project = SkillsConfig {
            repositories: vec![SkillRepositoryConfig {
                name: "shared".into(),
                transport: SkillRepoTransport::Filesystem {
                    path: "project-shared".into(),
                },
            }],
            ..Default::default()
        };

        let merged = merge_project_over_user(user, project, true);
        assert_eq!(merged.repositories.len(), 1);
        match &merged.repositories[0].transport {
            SkillRepoTransport::Filesystem { path } => {
                assert_eq!(path, "project-shared");
            }
            other => panic!("Expected Filesystem, got {other:?}"),
        }
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
}
