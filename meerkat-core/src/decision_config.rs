//! Configuration vocabulary for the optional decision service.
//!
//! Core owns only the declarative shape of the `[decision]` realm-config
//! table, exactly as it does for `[skills]` and `[model_fallback]`. The
//! evaluation semantics live in the `meerkat-decision` feature crate and the
//! route/credential composition lives in the facade. Nothing here reads the
//! environment, opens a connection, or decides an answer.

use serde::{Deserialize, Serialize};

#[cfg(feature = "schema")]
use schemars::JsonSchema;

use crate::config::ConfigError;
use crate::connection::CredentialSourceSpec;

/// Realm-config `[decision]` table.
///
/// The agent-callable `decide` tool is switched on separately through
/// `tools.decision_enabled`; this table selects which backend serves
/// evaluations and how tightly each request is bounded. A child realm that
/// writes any part of this table replaces the inherited table as a whole.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(default, deny_unknown_fields)]
pub struct DecisionConfig {
    /// Backend that serves decision requests.
    pub backend: DecisionBackendSelection,
    /// Jev backend connection facts. Required when `backend = "jev"`; its
    /// mere presence never selects Jev and never triggers a key lookup.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub jev: Option<JevBackendConfig>,
    /// Bounded request, candidate, deadline, attempt, and output limits.
    pub limits: DecisionLimitsConfig,
}

impl DecisionConfig {
    /// Validate configuration invariants for this table.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.backend == DecisionBackendSelection::Jev && self.jev.is_none() {
            return Err(ConfigError::Validation(
                "decision.backend = \"jev\" requires a [decision.jev] table".into(),
            ));
        }
        if let Some(jev) = self.jev.as_ref() {
            jev.validate()?;
        }
        self.limits.validate()
    }
}

/// Which backend evaluates decision requests.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum DecisionBackendSelection {
    /// One bounded, tool-free structured request through the session's
    /// already-admitted LLM route. No new account, credential, or endpoint.
    #[default]
    SessionLlm,
    /// The explicitly configured Jev (TypeSafe) evaluation endpoint.
    Jev,
}

/// Connection facts for the Jev backend.
///
/// Credentials have exactly one owner: the typed [`CredentialSourceSpec`]
/// resolved through the shared credential resolver at composition time. The
/// adapter itself never reads the environment or persists tokens.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(default, deny_unknown_fields)]
pub struct JevBackendConfig {
    /// Evaluation endpoint URL.
    pub endpoint: String,
    /// Requested Jev model alias or pinned version.
    pub model: String,
    /// Where the bearer credential comes from.
    pub credential: CredentialSourceSpec,
    /// Explicit host permission to disclose admitted decision inputs to this
    /// external destination. Access to content does not authorize sending it
    /// to a new vendor; this flag is that authorization. Defaults to `false`.
    pub allow_disclosure: bool,
}

/// Default TypeSafe evaluation endpoint.
pub const DEFAULT_JEV_ENDPOINT: &str = "https://api.typesafe.ai/v1/systemone";
/// Default Jev model alias.
pub const DEFAULT_JEV_MODEL: &str = "jev-latest";

impl Default for JevBackendConfig {
    fn default() -> Self {
        Self {
            endpoint: DEFAULT_JEV_ENDPOINT.to_string(),
            model: DEFAULT_JEV_MODEL.to_string(),
            credential: CredentialSourceSpec::Env {
                env: "JEV_API_KEY".to_string(),
                fallback: Vec::new(),
            },
            allow_disclosure: false,
        }
    }
}

impl JevBackendConfig {
    /// Validate connection facts.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.endpoint.trim().is_empty() {
            return Err(ConfigError::Validation(
                "decision.jev.endpoint must not be empty".into(),
            ));
        }
        if !(self.endpoint.starts_with("https://") || self.endpoint.starts_with("http://")) {
            return Err(ConfigError::Validation(
                "decision.jev.endpoint must be an http(s) URL".into(),
            ));
        }
        if self.model.trim().is_empty() {
            return Err(ConfigError::Validation(
                "decision.jev.model must not be empty".into(),
            ));
        }
        Ok(())
    }
}

/// Bounded limits applied to every decision request.
///
/// Retries consume the same deadline; there is no unbounded dimension.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(default, deny_unknown_fields)]
pub struct DecisionLimitsConfig {
    /// Maximum questions in one batched request.
    pub max_questions: usize,
    /// Maximum options in one choose-one question.
    pub max_options_per_choice: usize,
    /// Maximum ordered levels in one grade question.
    pub max_grade_levels: usize,
    /// Maximum serialized bytes of the supplied state.
    pub max_state_bytes: usize,
    /// Maximum serialized bytes of one instruction, criterion, option, or
    /// level description.
    pub max_instruction_bytes: usize,
    /// Total deadline for one evaluation, including retries and repair.
    pub deadline_ms: u64,
    /// Maximum backend attempts within the deadline (format repair or
    /// transient backoff). Never a retry-until-agreement loop.
    pub max_attempts: u32,
    /// Output-token allowance for the LLM backend's single structured request.
    pub max_output_tokens: u32,
}

impl Default for DecisionLimitsConfig {
    fn default() -> Self {
        Self {
            max_questions: 32,
            max_options_per_choice: 32,
            max_grade_levels: 10,
            max_state_bytes: 64 * 1024,
            max_instruction_bytes: 4 * 1024,
            deadline_ms: 30_000,
            max_attempts: 2,
            max_output_tokens: 1024,
        }
    }
}

impl DecisionLimitsConfig {
    /// Validate that every bound is positive and internally consistent.
    pub fn validate(&self) -> Result<(), ConfigError> {
        let positive = [
            ("decision.limits.max_questions", self.max_questions),
            (
                "decision.limits.max_options_per_choice",
                self.max_options_per_choice,
            ),
            ("decision.limits.max_state_bytes", self.max_state_bytes),
            (
                "decision.limits.max_instruction_bytes",
                self.max_instruction_bytes,
            ),
        ];
        for (name, value) in positive {
            if value == 0 {
                return Err(ConfigError::Validation(format!(
                    "{name} must be greater than 0"
                )));
            }
        }
        if self.max_grade_levels < 2 {
            return Err(ConfigError::Validation(
                "decision.limits.max_grade_levels must be at least 2".into(),
            ));
        }
        if self.deadline_ms == 0 {
            return Err(ConfigError::Validation(
                "decision.limits.deadline_ms must be greater than 0".into(),
            ));
        }
        if self.max_attempts == 0 {
            return Err(ConfigError::Validation(
                "decision.limits.max_attempts must be at least 1".into(),
            ));
        }
        if self.max_output_tokens == 0 {
            return Err(ConfigError::Validation(
                "decision.limits.max_output_tokens must be greater than 0".into(),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn default_table_selects_session_llm_without_jev_facts() {
        let config = DecisionConfig::default();
        assert_eq!(config.backend, DecisionBackendSelection::SessionLlm);
        assert!(config.jev.is_none());
        config.validate().unwrap();
    }

    #[test]
    fn jev_backend_requires_its_table() {
        let config = DecisionConfig {
            backend: DecisionBackendSelection::Jev,
            jev: None,
            limits: DecisionLimitsConfig::default(),
        };
        let error = config.validate().unwrap_err();
        assert!(error.to_string().contains("[decision.jev]"));
    }

    #[test]
    fn jev_table_parses_with_typed_credential_source() {
        let parsed: DecisionConfig = toml::from_str(
            r#"
backend = "jev"

[jev]
endpoint = "https://api.typesafe.ai/v1/systemone"
model = "jev-latest"
allow_disclosure = true
credential = { kind = "env", env = "JEV_API_KEY" }
"#,
        )
        .unwrap();
        parsed.validate().unwrap();
        let jev = parsed.jev.unwrap();
        assert!(jev.allow_disclosure);
        assert!(matches!(
            jev.credential,
            CredentialSourceSpec::Env { ref env, .. } if env == "JEV_API_KEY"
        ));
    }

    #[test]
    fn unknown_keys_are_rejected_at_parse() {
        let error = toml::from_str::<DecisionConfig>("bearer_token = \"x\"\n").unwrap_err();
        assert!(error.to_string().contains("bearer_token"));
    }

    #[test]
    fn limits_fail_closed_on_zero_and_degenerate_grades() {
        let degenerate_grades = DecisionLimitsConfig {
            max_grade_levels: 1,
            ..DecisionLimitsConfig::default()
        };
        assert!(degenerate_grades.validate().is_err());
        let no_attempts = DecisionLimitsConfig {
            max_attempts: 0,
            ..DecisionLimitsConfig::default()
        };
        assert!(no_attempts.validate().is_err());
        let no_deadline = DecisionLimitsConfig {
            deadline_ms: 0,
            ..DecisionLimitsConfig::default()
        };
        assert!(no_deadline.validate().is_err());
    }
}
