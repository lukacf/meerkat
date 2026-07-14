//! Skills configuration types.
//!
//! Defines `SkillsConfig` and repository configuration vocabulary for skill
//! sources. Realm configuration owns composition; repository I/O and
//! resolution live in `meerkat-skills`.

use serde::{Deserialize, Serialize};
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
        auth: Option<HttpSkillRepoAuth>,
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
        auth: Option<GitSkillRepoAuth>,
        #[serde(default = "default_refresh_seconds")]
        refresh_seconds: u64,
        #[serde(default = "default_clone_depth")]
        depth: Option<usize>,
    },
}

/// Typed HTTP skill-repository credential, parsed at config ingress.
///
/// Replaces the untagged `auth_header: Option<String>` + `auth_token:
/// Option<String>` string pair, whose half-set combinations were
/// unrepresentable as a real credential. The variant IS the auth scheme;
/// `meerkat-skills` folds it into its transport auth owner without
/// re-deriving meaning from optional strings. `Debug` redacts secret
/// material.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "scheme", rename_all = "kebab-case")]
pub enum HttpSkillRepoAuth {
    /// `Authorization: Bearer <token>`.
    Bearer { token: String },
    /// Custom header credential (`<name>: <value>`).
    Header { name: String, value: String },
}

impl std::fmt::Debug for HttpSkillRepoAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Bearer { .. } => f
                .debug_struct("Bearer")
                .field("token", &"<redacted>")
                .finish(),
            Self::Header { name, .. } => f
                .debug_struct("Header")
                .field("name", name)
                .field("value", &"<redacted>")
                .finish(),
        }
    }
}

/// Typed Git skill-repository credential, parsed at config ingress.
///
/// Replaces the untagged `auth_token: Option<String>` + `ssh_key:
/// Option<String>` string pair (both-set was ambiguous). `Debug` redacts the
/// token.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "scheme", rename_all = "kebab-case")]
pub enum GitSkillRepoAuth {
    /// HTTPS token auth.
    Token { token: String },
    /// SSH private-key path.
    SshKey { path: String },
}

impl std::fmt::Debug for GitSkillRepoAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Token { .. } => f
                .debug_struct("Token")
                .field("token", &"<redacted>")
                .finish(),
            Self::SshKey { path } => f.debug_struct("SshKey").field("path", path).finish(),
        }
    }
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

impl SkillsConfig {
    /// Canonical source identity records from built-ins + configured
    /// repositories. Callers should build a `SourceIdentityRegistry` from
    /// these before resolving source nodes.
    pub fn source_identity_records(&self) -> Vec<SourceIdentityRecord> {
        let mut records = default_source_identity_records();
        records.extend(self.repositories.iter().map(repository_to_identity_record));
        records
    }

    /// Build a source identity registry from repository config + governance overlays.
    pub fn build_source_identity_registry(&self) -> Result<SourceIdentityRegistry, SkillError> {
        SourceIdentityRegistry::build(
            self.source_identity_records(),
            self.identity.lineage.clone(),
            self.identity.remaps.clone(),
            self.identity.aliases.clone(),
        )
    }
}

/// Canonical source identity records for the built-in and project-local
/// sources that exist independently of user repository configuration.
pub fn default_source_identity_records() -> Vec<SourceIdentityRecord> {
    vec![
        SourceIdentityRecord {
            source_uuid: SourceUuid::builtin(),
            display_name: "embedded".to_string(),
            transport_kind: SourceTransportKind::Embedded,
            fingerprint: "embedded:inventory".to_string(),
            status: SourceIdentityStatus::Active,
        },
        SourceIdentityRecord {
            source_uuid: SourceUuid::project_local(),
            display_name: "project".to_string(),
            transport_kind: SourceTransportKind::Filesystem,
            fingerprint: "filesystem:.rkat/skills".to_string(),
            status: SourceIdentityStatus::Active,
        },
    ]
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
        } => format!("stdio:{command}:{cwd:?}:{env:?}"),
        SkillRepoTransport::Http { url, .. } => format!("http:{url}"),
        SkillRepoTransport::Git {
            url,
            git_ref,
            ref_type,
            skills_root,
            ..
        } => format!("git:{url}:{git_ref}:{ref_type:?}:{skills_root:?}"),
    };
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    hasher.write(material.as_bytes());
    let hash = hasher.finish();
    let mut fp = String::with_capacity(19);
    fp.push_str("repo-");
    let _ = write!(&mut fp, "{hash:016x}");
    fp
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

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
refresh_seconds = 60
auth = { scheme = "header", name = "X-API-Key", value = "secret" }
"#;
        let config: SkillsConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.repositories.len(), 1);
        let SkillRepoTransport::Http {
            url,
            auth,
            refresh_seconds,
            ..
        } = &config.repositories[0].transport
        else {
            unreachable!("Expected Http transport");
        };
        assert_eq!(url, "http://localhost:8080/api");
        assert_eq!(
            auth,
            &Some(HttpSkillRepoAuth::Header {
                name: "X-API-Key".into(),
                value: "secret".into(),
            })
        );
        assert_eq!(*refresh_seconds, 60);
    }

    #[test]
    fn test_parse_http_repo_bearer_auth() {
        let toml = r#"
[[repositories]]
name = "elephant"
source_uuid = "dc256086-0d2f-4f61-a307-320d4148107f"
type = "http"
url = "https://skills.example/api"
auth = { scheme = "bearer", token = "secret" }
"#;
        let config: SkillsConfig = toml::from_str(toml).unwrap();
        let SkillRepoTransport::Http { auth, .. } = &config.repositories[0].transport else {
            unreachable!("Expected Http transport");
        };
        assert_eq!(
            auth,
            &Some(HttpSkillRepoAuth::Bearer {
                token: "secret".into()
            })
        );
    }

    #[test]
    fn test_repo_credentials_are_redacted_in_debug() {
        let http = HttpSkillRepoAuth::Bearer {
            token: "super-secret".into(),
        };
        let header = HttpSkillRepoAuth::Header {
            name: "X-API-Key".into(),
            value: "super-secret".into(),
        };
        let git = GitSkillRepoAuth::Token {
            token: "ghp_super_secret".into(),
        };
        for debug in [
            format!("{http:?}"),
            format!("{header:?}"),
            format!("{git:?}"),
        ] {
            assert!(
                !debug.contains("secret"),
                "credential material must not leak through Debug: {debug}"
            );
        }
    }

    #[test]
    fn test_partial_typed_credential_is_rejected_at_ingress() {
        // A typed credential must be complete: a bearer scheme without its
        // token (or a header scheme without name/value) fails closed at parse
        // time — no half-set credential state is representable.
        let toml = r#"
[[repositories]]
name = "elephant"
source_uuid = "dc256086-0d2f-4f61-a307-320d4148107f"
type = "http"
url = "https://skills.example/api"
auth = { scheme = "bearer" }
"#;
        assert!(
            toml::from_str::<SkillsConfig>(toml).is_err(),
            "incomplete typed credentials must be rejected"
        );
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
auth = { scheme = "token", token = "ghp_token" }
"#;
        let config: SkillsConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.repositories.len(), 1);
        let SkillRepoTransport::Git {
            url,
            git_ref,
            ref_type,
            auth,
            ..
        } = &config.repositories[0].transport
        else {
            unreachable!("Expected Git transport");
        };
        assert_eq!(url, "https://github.com/company/skills.git");
        assert_eq!(git_ref, "v1.2.0");
        assert!(matches!(ref_type, GitRefType::Tag));
        assert_eq!(
            auth,
            &Some(GitSkillRepoAuth::Token {
                token: "ghp_token".into()
            })
        );
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
