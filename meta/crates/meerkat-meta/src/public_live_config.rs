//! Explicit server-side public OpenAI Live registration shared by gateways.
//! Parsing configuration does not establish authenticated readiness.

use meerkat_core::{AuthBindingRef, BindingId, BindingOrigin, ProfileId, RealmId};
use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicLiveRegistration {
    pub principal: String,
    pub realm: RealmId,
    pub binding: AuthBindingRef,
    pub voice: String,
    pub session_instructions: Option<String>,
    /// Bounds and model for the concurrent context summary.
    pub summary: ConsoleVoiceSummaryConfig,
}

/// Default input window for the context summary: the most recent 64 KiB of
/// the serialized transcript. Older turns are dropped whole; this is a size
/// bound, not a content heuristic. 64 KiB is roughly 16k tokens, enough for
/// several dozen ordinary turns while keeping the summary request's input
/// cost and time-to-first-token bounded on large histories.
pub const DEFAULT_SUMMARY_MAX_INPUT_BYTES: usize = 64 * 1024;

/// Default output cap for the context summary: 4 KiB of UTF-8. The public
/// GPT Live transport delivers the summary in fragments of at most 500 bytes,
/// each waiting for its own provider receipt, so 4 KiB bounds delivery at 9
/// fragments (at most 8 full fragments and one remainder) while leaving room
/// for the 200-word factual notes the summary prompt asks for.
pub const DEFAULT_SUMMARY_MAX_OUTPUT_BYTES: usize = 4 * 1024;

/// Context summary configuration on the console voice registration.
///
/// The summary is a tool-free request made with the background agent's
/// credentials. `model` defaults to the background agent's own text model
/// because Meerkat's model catalog exposes a support tier
/// (`ModelTier::Recommended`/`Supported`), not a speed or cost tier, so no
/// "fastest model of the same provider" can be derived from catalog
/// authority. Operators who want a faster summariser name it here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsoleVoiceSummaryConfig {
    /// Model for the summary request, on the background agent's provider and
    /// credentials. `None` uses the agent's own model.
    pub model: Option<String>,
    /// Most recent serialized transcript bytes handed to the summariser.
    pub max_input_bytes: usize,
    /// Hard UTF-8 cap on the produced summary.
    pub max_output_bytes: usize,
}

impl Default for ConsoleVoiceSummaryConfig {
    fn default() -> Self {
        Self {
            model: None,
            max_input_bytes: DEFAULT_SUMMARY_MAX_INPUT_BYTES,
            max_output_bytes: DEFAULT_SUMMARY_MAX_OUTPUT_BYTES,
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RegistrationWire {
    principal: String,
    realm: String,
    auth_binding: BindingWire,
    voice: String,
    session_instructions: Option<String>,
    summary: Option<SummaryWire>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SummaryWire {
    model: Option<String>,
    max_input_bytes: Option<usize>,
    max_output_bytes: Option<usize>,
}

impl SummaryWire {
    fn resolve(self) -> Result<ConsoleVoiceSummaryConfig, String> {
        let defaults = ConsoleVoiceSummaryConfig::default();
        let config = ConsoleVoiceSummaryConfig {
            model: self
                .model
                .map(|model| nonempty(&model, "summary.model"))
                .transpose()?,
            max_input_bytes: self.max_input_bytes.unwrap_or(defaults.max_input_bytes),
            max_output_bytes: self.max_output_bytes.unwrap_or(defaults.max_output_bytes),
        };
        if config.max_input_bytes == 0 {
            return Err("summary.max_input_bytes must be positive".to_string());
        }
        if config.max_output_bytes == 0 {
            return Err("summary.max_output_bytes must be positive".to_string());
        }
        Ok(config)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BindingWire {
    realm: String,
    binding: String,
    profile: Option<String>,
}

fn nonempty(value: &str, field: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() {
        Err(format!("{field} must be a non-empty string"))
    } else {
        Ok(value.to_string())
    }
}

impl PublicLiveRegistration {
    /// Accept only a configured binding reference, never a browser-provided
    /// credential, model override, experimental profile, or provider.
    pub fn parse(value: &Value) -> Result<Self, String> {
        let wire: RegistrationWire =
            serde_json::from_value(value.clone()).map_err(|error| error.to_string())?;
        let principal = nonempty(&wire.principal, "principal")?;
        let realm = RealmId::parse(nonempty(&wire.realm, "realm")?)
            .map_err(|error| format!("realm is invalid: {error}"))?;
        let binding_realm =
            RealmId::parse(nonempty(&wire.auth_binding.realm, "auth_binding.realm")?)
                .map_err(|error| format!("auth_binding.realm is invalid: {error}"))?;
        if binding_realm != realm {
            return Err("auth_binding.realm must equal openai_live.realm".to_string());
        }
        let binding = BindingId::parse(nonempty(
            &wire.auth_binding.binding,
            "auth_binding.binding",
        )?)
        .map_err(|error| format!("auth_binding.binding is invalid: {error}"))?;
        let profile = wire
            .auth_binding
            .profile
            .map(|profile| {
                ProfileId::parse(nonempty(&profile, "auth_binding.profile")?)
                    .map_err(|error| format!("auth_binding.profile is invalid: {error}"))
            })
            .transpose()?;
        Ok(Self {
            principal,
            realm,
            binding: AuthBindingRef {
                realm: binding_realm,
                binding,
                profile,
                origin: BindingOrigin::Configured,
            },
            voice: nonempty(&wire.voice, "voice")?,
            session_instructions: wire
                .session_instructions
                .map(|instructions| nonempty(&instructions, "session_instructions"))
                .transpose()?,
            summary: wire
                .summary
                .map(SummaryWire::resolve)
                .transpose()?
                .unwrap_or_default(),
        })
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use serde_json::json;

    fn registration() -> Value {
        json!({
            "principal": "alice",
            "realm": "voice",
            "auth_binding": {"realm": "voice", "binding": "openai"},
            "voice": "marin"
        })
    }

    #[test]
    fn public_live_registration_preserves_exact_configured_binding() {
        let parsed = PublicLiveRegistration::parse(&registration()).expect("registration");
        assert_eq!(parsed.principal, "alice");
        assert_eq!(parsed.binding.origin, BindingOrigin::Configured);
        assert_eq!(parsed.binding.realm, parsed.realm);
        assert_eq!(parsed.binding.binding.as_str(), "openai");
        assert!(parsed.binding.profile.is_none());
        assert_eq!(parsed.summary, ConsoleVoiceSummaryConfig::default());
        assert_eq!(parsed.summary.max_input_bytes, 64 * 1024);
        assert_eq!(parsed.summary.max_output_bytes, 4 * 1024);
        assert!(parsed.summary.model.is_none());
    }

    #[test]
    fn public_live_registration_parses_the_shared_sdk_summary_fixture() {
        let contracts: Value =
            serde_json::from_str(include_str!("../tests/fixtures/live_contracts_v1.json"))
                .expect("live contracts fixture");
        let parsed =
            PublicLiveRegistration::parse(&contracts["openai_live_gateway_config_with_summary"])
                .expect("registration with summary");
        assert_eq!(parsed.summary.model.as_deref(), Some("gpt-5.4-mini"));
        assert_eq!(parsed.summary.max_input_bytes, 32768);
        assert_eq!(parsed.summary.max_output_bytes, 2048);
        let plain = PublicLiveRegistration::parse(&contracts["openai_live_gateway_config"])
            .expect("registration without summary");
        assert_eq!(plain.summary, ConsoleVoiceSummaryConfig::default());
    }

    #[test]
    fn public_live_registration_accepts_bounded_summary_configuration() {
        let mut configured = registration();
        configured["summary"] = json!({
            "model": "gpt-5.4-mini", "max_input_bytes": 32768, "max_output_bytes": 2048
        });
        let parsed = PublicLiveRegistration::parse(&configured).expect("registration");
        assert_eq!(parsed.summary.model.as_deref(), Some("gpt-5.4-mini"));
        assert_eq!(parsed.summary.max_input_bytes, 32768);
        assert_eq!(parsed.summary.max_output_bytes, 2048);
        let mut partial = registration();
        partial["summary"] = json!({ "max_output_bytes": 1024 });
        let parsed = PublicLiveRegistration::parse(&partial).expect("registration");
        assert_eq!(
            parsed.summary.max_input_bytes,
            DEFAULT_SUMMARY_MAX_INPUT_BYTES
        );
        assert_eq!(parsed.summary.max_output_bytes, 1024);
        for invalid in [
            json!({ "model": " " }),
            json!({ "max_input_bytes": 0 }),
            json!({ "max_output_bytes": 0 }),
            json!({ "api_key": "not-a-secret-fixture" }),
            json!({ "provider": "openai" }),
        ] {
            let mut rejected = registration();
            rejected["summary"] = invalid;
            assert!(PublicLiveRegistration::parse(&rejected).is_err());
        }
    }

    #[test]
    fn public_live_registration_rejects_secrets_overrides_and_cross_realm_binding() {
        for (field, value) in [
            ("api_key", json!("not-a-secret-fixture")),
            ("provider", json!("openai")),
            ("model", json!("gpt-live-1")),
            ("voice", json!(" ")),
            ("principal", json!("")),
        ] {
            let mut invalid = registration();
            invalid[field] = value;
            assert!(PublicLiveRegistration::parse(&invalid).is_err());
        }
        let mut wrong_realm = registration();
        wrong_realm["auth_binding"]["realm"] = json!("elsewhere");
        assert!(PublicLiveRegistration::parse(&wrong_realm).is_err());
        let mut secret = registration();
        secret["auth_binding"]["api_key"] = json!("not-a-secret-fixture");
        assert!(PublicLiveRegistration::parse(&secret).is_err());
    }
}
