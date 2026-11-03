//! Policy decision framework — auth, console access, metrics, and runtime ops.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::console_config::ConsoleUiConfig;
use crate::types::{ModuleConfig, RestartPolicy};

pub const REQUIRED_RELEASE_TARGETS: &[&str] = &["crates.io", "npm", "pypi", "github-releases"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecisionPolicyError {
    EmptyBigQueryDataset,
    EmptyBigQueryTable,
    InvalidBigQueryName(String),
    /// A policy document failed to parse. Carried for both the trust manifest
    /// (TOML) and the release metadata (JSON); the message names which
    /// document and format failed. The variant name predates the JSON use and
    /// is kept because this enum is not `#[non_exhaustive]`.
    TomlParse(String),
    MissingModuleId,
    MissingModuleCommand,
    AuthProviderMismatch,
    AuthProviderNotSupported,
    EmailNotAllowlisted,
    InvalidServiceIdentity,
    ServiceIdentityNotAllowlisted,
    ReplicaCountMustBeOne(u16),
    SloTargetsNotSupportedV01,
    MissingReleaseTarget(String),
    DuplicateReleaseTarget(String),
    InvalidSupportMatrix(String),
    InvalidTrustedAuthConfig(String),
}

impl std::fmt::Display for DecisionPolicyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyBigQueryDataset => write!(f, "empty BigQuery dataset"),
            Self::EmptyBigQueryTable => write!(f, "empty BigQuery table"),
            Self::InvalidBigQueryName(name) => write!(f, "invalid BigQuery name: {name}"),
            Self::TomlParse(msg) => write!(f, "parse error: {msg}"),
            Self::MissingModuleId => write!(f, "missing module id"),
            Self::MissingModuleCommand => write!(f, "missing module command"),
            Self::AuthProviderMismatch => write!(f, "auth provider mismatch"),
            Self::AuthProviderNotSupported => write!(f, "auth provider not supported"),
            Self::EmailNotAllowlisted => write!(f, "email not allowlisted"),
            Self::InvalidServiceIdentity => write!(f, "invalid service identity"),
            Self::ServiceIdentityNotAllowlisted => write!(f, "service identity not allowlisted"),
            Self::ReplicaCountMustBeOne(count) => {
                write!(f, "replica count must be 1, got {count}")
            }
            Self::SloTargetsNotSupportedV01 => {
                write!(f, "SLO targets not supported in v0.1")
            }
            Self::MissingReleaseTarget(target) => {
                write!(f, "missing release target: {target}")
            }
            Self::DuplicateReleaseTarget(target) => {
                write!(f, "duplicate release target: {target}")
            }
            Self::InvalidSupportMatrix(matrix) => {
                write!(f, "invalid support matrix: {matrix}")
            }
            Self::InvalidTrustedAuthConfig(msg) => {
                write!(f, "invalid trusted auth config: {msg}")
            }
        }
    }
}

impl std::error::Error for DecisionPolicyError {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BigQueryNaming {
    pub dataset: String,
    pub table: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct TrustedMetaToml {
    /// An empty manifest (no `[[modules]]` table at all) declares no trusted
    /// modules; hosts do not have to write a literal `modules = []`.
    #[serde(default)]
    pub modules: Vec<TrustedModuleDecl>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct TrustedModuleDecl {
    pub id: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    pub restart_policy: Option<RestartPolicy>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthProvider {
    GoogleOAuth,
    GitHubOAuth,
    GenericOidc,
    ServiceIdentity,
    TestProvider,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthPolicy {
    pub default_provider: AuthProvider,
    pub email_allowlist: Vec<String>,
}

impl Default for AuthPolicy {
    fn default() -> Self {
        Self {
            default_provider: AuthProvider::GoogleOAuth,
            email_allowlist: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsolePolicy {
    pub require_app_auth: bool,
    #[serde(default)]
    pub read_only: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fetch_timeout_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "ConsoleUiConfig::is_default")]
    pub ui: ConsoleUiConfig,
}

impl Default for ConsolePolicy {
    fn default() -> Self {
        Self {
            require_app_auth: true,
            read_only: false,
            fetch_timeout_ms: None,
            ui: ConsoleUiConfig::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsoleAccessRequest {
    pub provider: AuthProvider,
    pub email: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MetricsPolicy {
    pub enforce_slo_targets: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeOpsPolicy {
    pub replica_count: u16,
    pub metrics: MetricsPolicy,
}

impl Default for RuntimeOpsPolicy {
    fn default() -> Self {
        Self {
            replica_count: 1,
            metrics: MetricsPolicy {
                enforce_slo_targets: false,
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseMetadata {
    pub targets: Vec<String>,
    pub support_matrix: String,
}

pub fn validate_bigquery_naming(naming: &BigQueryNaming) -> Result<(), DecisionPolicyError> {
    if naming.dataset.trim().is_empty() {
        return Err(DecisionPolicyError::EmptyBigQueryDataset);
    }
    if naming.table.trim().is_empty() {
        return Err(DecisionPolicyError::EmptyBigQueryTable);
    }

    for value in [&naming.dataset, &naming.table] {
        if !value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
        {
            return Err(DecisionPolicyError::InvalidBigQueryName(value.clone()));
        }
    }

    Ok(())
}

pub fn load_trusted_meta_modules_from_toml(
    toml_text: &str,
) -> Result<Vec<ModuleConfig>, DecisionPolicyError> {
    let parsed: TrustedMetaToml = toml::from_str(toml_text)
        .map_err(|err| DecisionPolicyError::TomlParse(format!("trust manifest TOML: {err}")))?;

    parsed
        .modules
        .into_iter()
        .map(|module| {
            if module.id.trim().is_empty() {
                return Err(DecisionPolicyError::MissingModuleId);
            }
            if module.command.trim().is_empty() {
                return Err(DecisionPolicyError::MissingModuleCommand);
            }
            Ok(ModuleConfig {
                id: module.id,
                command: module.command,
                args: module.args,
                restart_policy: module.restart_policy.unwrap_or(RestartPolicy::OnFailure),
            })
        })
        .collect()
}

pub fn enforce_console_route_access(
    auth_policy: &AuthPolicy,
    console_policy: &ConsolePolicy,
    request: &ConsoleAccessRequest,
) -> Result<(), DecisionPolicyError> {
    if !console_policy.require_app_auth {
        return Ok(());
    }

    if request.provider == AuthProvider::ServiceIdentity {
        if !request.email.starts_with("svc:") || request.email.len() <= 4 {
            return Err(DecisionPolicyError::InvalidServiceIdentity);
        }
        if !auth_policy
            .email_allowlist
            .iter()
            .any(|principal| principal == &request.email)
        {
            return Err(DecisionPolicyError::ServiceIdentityNotAllowlisted);
        }
        return Ok(());
    }

    if request.provider != auth_policy.default_provider {
        return Err(DecisionPolicyError::AuthProviderMismatch);
    }

    if matches!(request.provider, AuthProvider::TestProvider) {
        return Err(DecisionPolicyError::AuthProviderNotSupported);
    }

    if !auth_policy
        .email_allowlist
        .iter()
        .any(|email| email == &request.email)
    {
        return Err(DecisionPolicyError::EmailNotAllowlisted);
    }

    Ok(())
}

pub fn validate_runtime_ops_policy(policy: &RuntimeOpsPolicy) -> Result<(), DecisionPolicyError> {
    if policy.replica_count != 1 {
        return Err(DecisionPolicyError::ReplicaCountMustBeOne(
            policy.replica_count,
        ));
    }
    if policy.metrics.enforce_slo_targets {
        return Err(DecisionPolicyError::SloTargetsNotSupportedV01);
    }
    Ok(())
}

/// Parses the release metadata document. A malformed document is reported as
/// `DecisionPolicyError::TomlParse` whose message names the release metadata
/// JSON, not TOML: the variant is shared with the trust manifest because the
/// enum is not `#[non_exhaustive]` and adding a variant would break exhaustive
/// matchers in hosts.
pub fn parse_release_metadata_json(
    json_text: &str,
) -> Result<ReleaseMetadata, DecisionPolicyError> {
    serde_json::from_str(json_text)
        .map_err(|err| DecisionPolicyError::TomlParse(format!("release metadata JSON: {err}")))
}

pub fn validate_release_metadata(metadata: &ReleaseMetadata) -> Result<(), DecisionPolicyError> {
    let mut seen = BTreeSet::new();
    for target in &metadata.targets {
        if !seen.insert(target.clone()) {
            return Err(DecisionPolicyError::DuplicateReleaseTarget(target.clone()));
        }
    }

    for required in REQUIRED_RELEASE_TARGETS {
        if !seen.contains(*required) {
            return Err(DecisionPolicyError::MissingReleaseTarget(
                (*required).to_string(),
            ));
        }
    }

    if metadata.support_matrix != "same-as-meerkat" {
        return Err(DecisionPolicyError::InvalidSupportMatrix(
            metadata.support_matrix.clone(),
        ));
    }

    Ok(())
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn empty_trust_manifest_declares_no_modules() {
        let modules = load_trusted_meta_modules_from_toml("").expect("empty manifest");

        assert!(modules.is_empty());
    }

    #[test]
    fn trust_manifest_without_modules_table_declares_no_modules() {
        let modules = load_trusted_meta_modules_from_toml("# no modules yet\n")
            .expect("comment-only manifest");

        assert!(modules.is_empty());
    }

    #[test]
    fn trust_manifest_parse_error_names_the_toml_document() {
        let err = load_trusted_meta_modules_from_toml("modules = [ this is not toml")
            .expect_err("malformed manifest must be refused");

        match &err {
            DecisionPolicyError::TomlParse(msg) => {
                assert!(msg.starts_with("trust manifest TOML: "), "{msg}");
            }
            other => panic!("expected TomlParse, got {other:?}"),
        }
        assert!(
            err.to_string()
                .starts_with("parse error: trust manifest TOML: ")
        );
    }

    #[test]
    fn release_metadata_parse_error_names_the_json_document() {
        let err = parse_release_metadata_json("{ not json")
            .expect_err("malformed release metadata must be refused");

        match &err {
            DecisionPolicyError::TomlParse(msg) => {
                assert!(msg.starts_with("release metadata JSON: "), "{msg}");
            }
            other => panic!("expected TomlParse, got {other:?}"),
        }
        let rendered = err.to_string();
        assert!(
            rendered.starts_with("parse error: release metadata JSON: "),
            "{rendered}"
        );
        assert!(!rendered.contains("TOML"), "{rendered}");
    }

    #[test]
    fn canonical_release_metadata_validates() {
        let metadata = ReleaseMetadata {
            targets: REQUIRED_RELEASE_TARGETS
                .iter()
                .map(|target| (*target).to_string())
                .collect(),
            support_matrix: "same-as-meerkat".to_string(),
        };

        validate_release_metadata(&metadata).expect("canonical release metadata");
    }

    #[test]
    fn release_metadata_missing_a_registry_is_refused() {
        let metadata = ReleaseMetadata {
            targets: vec!["crates.io".to_string()],
            support_matrix: "same-as-meerkat".to_string(),
        };

        assert_eq!(
            validate_release_metadata(&metadata),
            Err(DecisionPolicyError::MissingReleaseTarget("npm".to_string()))
        );
    }
}
