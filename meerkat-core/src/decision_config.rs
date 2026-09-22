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
    /// Explicit LLM route for host invocations on the `llm` backend. The
    /// agent tool never reads it; a host without one cannot evaluate.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host_route: Option<DecisionHostRoute>,
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
        if let Some(route) = self.host_route.as_ref() {
            route.validate()?;
        }
        self.limits.validate()
    }
}

/// Which backend evaluates decision requests.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum DecisionBackendSelection {
    /// One bounded, tool-free structured request through an admitted LLM
    /// route. The agent-callable tool inherits the session's already-admitted
    /// route; host invocations (gateways, feature policies) use the explicit
    /// `[decision.host_route]`. No new account, credential, or endpoint.
    #[default]
    Llm,
    /// The explicitly configured Jev (TypeSafe) evaluation endpoint.
    Jev,
}

/// Explicit LLM route for host invocations that have no admitted session.
///
/// The host names the exact provider and model (and optionally the realm
/// auth binding); nothing here is elected by a leaf, SDK, or backend.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct DecisionHostRoute {
    pub provider: crate::Provider,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_binding: Option<crate::connection::AuthBindingRef>,
}

impl DecisionHostRoute {
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.model.trim().is_empty() {
            return Err(ConfigError::Validation(
                "decision.host_route.model must not be empty".into(),
            ));
        }
        Ok(())
    }
}

/// Connection facts for the Jev backend.
///
/// Core carries only the shape: the host names the endpoint, the model, and
/// the typed [`CredentialSourceSpec`] explicitly, and the feature crate owns
/// any vendor defaults it documents. Credentials have exactly one owner (the
/// realm credential resolver); the adapter never reads the environment or
/// persists tokens.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
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
    #[serde(default)]
    pub allow_disclosure: bool,
}

impl JevBackendConfig {
    /// Validate connection facts.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.endpoint.trim().is_empty() {
            return Err(ConfigError::Validation(
                "decision.jev.endpoint must not be empty".into(),
            ));
        }
        // The bearer credential travels in this request; plaintext is
        // admitted only to loopback, which exists for local mock servers.
        let plaintext_loopback = self
            .endpoint
            .strip_prefix("http://")
            .map(|rest| {
                let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
                let host = authority
                    .strip_prefix('[')
                    .and_then(|v6| v6.split(']').next())
                    .unwrap_or_else(|| {
                        authority
                            .rsplit_once(':')
                            .map_or(authority, |(host, _)| host)
                    });
                matches!(host, "127.0.0.1" | "localhost" | "::1")
            })
            .unwrap_or(false);
        if !(self.endpoint.starts_with("https://") || plaintext_loopback) {
            return Err(ConfigError::Validation(
                "decision.jev.endpoint must be an https:// URL (http:// is admitted only for \
                 loopback hosts)"
                    .into(),
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
    /// Thinking models spend this allowance on reasoning before the answer
    /// envelope, so the default leaves generous headroom; a cut-off envelope
    /// is the typed `output_truncated` failure, never a guessed answer.
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
            max_output_tokens: 4096,
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
        assert_eq!(config.backend, DecisionBackendSelection::Llm);
        assert!(config.jev.is_none());
        config.validate().unwrap();
    }

    fn jev(allow_disclosure: bool) -> JevBackendConfig {
        JevBackendConfig {
            endpoint: "https://jev.example/v1".into(),
            model: "jev-latest".into(),
            credential: CredentialSourceSpec::Env {
                env: "JEV_API_KEY".into(),
                fallback: Vec::new(),
            },
            allow_disclosure,
        }
    }

    #[test]
    fn undeclared_table_is_not_written_back_but_a_declared_one_round_trips() {
        let rendered = toml::to_string(&crate::Config::default()).unwrap();
        assert!(
            !rendered.contains("[decision"),
            "an undeclared decision table must not be rendered: {rendered}"
        );
        assert_eq!(
            *crate::Config::default().decision_config(),
            DecisionConfig::default()
        );

        // A declared table round-trips even when it equals the defaults: the
        // declaration itself is the fact a child uses to revoke inheritance.
        let config = crate::Config {
            decision: Some(DecisionConfig::default()),
            ..crate::Config::default()
        };
        let rendered = toml::to_string(&config).unwrap();
        assert!(rendered.contains("[decision"), "{rendered}");
        let parsed: crate::Config = toml::from_str(&rendered).unwrap();
        assert_eq!(parsed.decision, Some(DecisionConfig::default()));
    }

    #[test]
    fn a_child_realm_can_revoke_an_inherited_jev_route_by_declaring_the_default() {
        let jev_parent = || crate::Config {
            decision: Some(DecisionConfig {
                backend: DecisionBackendSelection::Jev,
                jev: Some(jev(true)),
                ..DecisionConfig::default()
            }),
            ..crate::Config::default()
        };
        let mut parent = jev_parent();
        // The child writes only the default backend; that declaration wins.
        parent
            .merge_toml_str("[decision]\nbackend = \"llm\"\n")
            .unwrap();
        let effective = parent.decision_config();
        assert_eq!(effective.backend, DecisionBackendSelection::Llm);
        assert!(
            effective.jev.is_none(),
            "the inherited Jev table is revoked"
        );

        // A child that declares nothing inherits.
        let mut parent = jev_parent();
        parent
            .merge_toml_str("[tools]\ndecision_enabled = true\n")
            .unwrap();
        assert_eq!(
            parent.decision_config().backend,
            DecisionBackendSelection::Jev
        );
    }

    #[test]
    fn jev_endpoint_must_be_https_except_loopback() {
        for endpoint in [
            "https://api.typesafe.ai/v1/systemone",
            "http://127.0.0.1:8080/v1",
            "http://localhost/v1",
            "http://[::1]:9/v1",
        ] {
            let mut config = jev(true);
            config.endpoint = endpoint.into();
            assert!(config.validate().is_ok(), "{endpoint}");
        }
        for endpoint in [
            "http://api.typesafe.ai/v1/systemone",
            "http://10.0.0.5/v1",
            "http://localhost.evil.example/v1",
            "ftp://api.typesafe.ai/v1",
        ] {
            let mut config = jev(true);
            config.endpoint = endpoint.into();
            assert!(config.validate().is_err(), "{endpoint}");
        }
    }

    #[test]
    fn jev_backend_requires_its_table() {
        let config = DecisionConfig {
            backend: DecisionBackendSelection::Jev,
            jev: None,
            host_route: None,
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
    fn host_route_parses_and_validates() {
        let parsed: DecisionConfig = toml::from_str(
            r#"
backend = "llm"

[host_route]
provider = "anthropic"
model = "claude-sonnet-4-5"
"#,
        )
        .unwrap();
        parsed.validate().unwrap();
        let route = parsed.host_route.unwrap();
        assert_eq!(route.provider, crate::Provider::Anthropic);
        assert!(route.auth_binding.is_none());

        let empty_model = DecisionConfig {
            host_route: Some(DecisionHostRoute {
                provider: crate::Provider::OpenAI,
                model: "  ".into(),
                auth_binding: None,
            }),
            ..DecisionConfig::default()
        };
        assert!(empty_model.validate().is_err());
    }

    #[test]
    fn unknown_keys_are_rejected_at_parse() {
        let error = toml::from_str::<DecisionConfig>("bearer_token = \"x\"\n").unwrap_err();
        assert!(error.to_string().contains("bearer_token"));
    }

    #[test]
    fn jev_table_requires_explicit_endpoint_model_and_credential() {
        let error = toml::from_str::<DecisionConfig>(
            r#"
backend = "jev"

[jev]
model = "jev-latest"
"#,
        )
        .unwrap_err();
        assert!(error.to_string().contains("endpoint"), "{error}");
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
