//! Typed deploy policy parsed from a mobpack's `config/defaults.toml`.
//!
//! A mobpack is untrusted-by-default: deploying it must not let pack-authored
//! TOML reach arbitrary runtime configuration. The old path merged the pack's
//! raw `config/defaults.toml` bytes into [`meerkat_core::Config`] as a base
//! layer with no allow-list, so a pack could silently set realm connection
//! sets, self-hosted provider endpoints, provider-tool keys, comms credentials,
//! shell security mode, or hook commands.
//!
//! [`MobpackDeployPolicy`] closes that hole. It is the single typed owner of
//! what a pack may declare as deploy defaults:
//!
//! * Parsing uses `deny_unknown_fields`, so any section outside the allow-list
//!   (`realm`, `self_hosted`, `provider_tools`, `comms`, `hooks`, `shell`,
//!   `tools`, `agent`, `storage`, `retry`, …) fails closed at archive load.
//! * Each declared knob carries a backing [`CapabilityId`] gate. The deploying
//!   surface resolves the policy through the same capability allow-list as
//!   [`crate::manifest::RequiresSection`] (see `validate_required_capabilities`
//!   on the CLI), so a knob only applies when its capability is both declared
//!   by the manifest and present in the runtime.
//!
//! The deploying surface maps the validated policy onto its concrete `Config`
//! *after* realm and environment layering — the pack contributes deploy
//! defaults, never overrides operator-owned truth.

use std::str::FromStr;

use serde::{Deserialize, Serialize};

use meerkat_contracts::capability::CapabilityId;

use crate::vocabulary::ModelRef;

/// Error raised when a mobpack deploy policy fails to parse or validate.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DeployPolicyError {
    /// The `config/defaults.toml` document is not valid TOML or contains a knob
    /// outside the deploy allow-list.
    #[error("mobpack config/defaults.toml is invalid: {0}")]
    Invalid(String),
    /// A declared knob is gated by a capability that the manifest did not
    /// declare under `[requires]`.
    #[error(
        "mobpack deploy knob `{knob}` requires capability `{capability}` which the manifest does not declare"
    )]
    UndeclaredCapability { knob: String, capability: String },
    /// A declared knob is gated by a capability that the runtime does not
    /// provide for the target surface.
    #[error(
        "mobpack deploy knob `{knob}` requires capability `{capability}` which the runtime does not provide"
    )]
    RuntimeCapabilityMissing { knob: String, capability: String },
}

/// Default provider models a pack may pin for its deployed mob.
///
/// Each field is optional so a pack only sets the providers it cares about.
/// Pinning a default model is always allowed (the `core` deploy surface owns
/// the model registry), so this group carries no extra capability gate.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeployModelDefaults {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anthropic: Option<ModelRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub openai: Option<ModelRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gemini: Option<ModelRef>,
}

impl DeployModelDefaults {
    fn is_empty(&self) -> bool {
        self.anthropic.is_none() && self.openai.is_none() && self.gemini.is_none()
    }
}

/// Budget caps a pack may pin for its deployed mob. Tightening budget is a
/// `core` deploy knob.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeployBudget {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_duration_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tool_calls: Option<usize>,
}

impl DeployBudget {
    fn is_empty(&self) -> bool {
        self.max_tokens.is_none()
            && self.max_duration_secs.is_none()
            && self.max_tool_calls.is_none()
    }
}

/// Compaction tuning a pack may pin. Gated by the `session_compaction`
/// capability: a pack only gets to tune compaction when the surface actually
/// runs a compactor.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeployCompaction {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_compact_threshold: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recent_turn_budget: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_summary_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_turns_between_compactions: Option<u32>,
}

impl DeployCompaction {
    fn is_empty(&self) -> bool {
        self.auto_compact_threshold.is_none()
            && self.recent_turn_budget.is_none()
            && self.max_summary_tokens.is_none()
            && self.min_turns_between_compactions.is_none()
    }
}

/// The typed, capability-gated deploy defaults a mobpack may declare.
///
/// Parsed once at archive load via [`MobpackDeployPolicy::parse`]; an absent
/// `config/defaults.toml` yields the empty default policy. Unknown sections
/// fail closed, so this struct is an exhaustive allow-list of pack-settable
/// deploy knobs.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MobpackDeployPolicy {
    /// Per-turn output token cap (`max_tokens` top-level knob).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "DeployModelDefaults::is_empty")]
    pub models: DeployModelDefaults,
    #[serde(default, skip_serializing_if = "DeployBudget::is_empty")]
    pub budget: DeployBudget,
    #[serde(default, skip_serializing_if = "DeployCompaction::is_empty")]
    pub compaction: DeployCompaction,
}

/// One declared knob paired with the capability that gates it.
///
/// Yielded by [`MobpackDeployPolicy::declared_knobs`] so the deploying surface
/// can run the same capability allow-list it already runs for manifest
/// `[requires]` entries, without re-deriving which knobs a pack set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeployKnobGate {
    /// Stable knob name for diagnostics (e.g. `compaction.recent_turn_budget`).
    pub knob: &'static str,
    /// The capability the runtime must provide for this knob to apply, or
    /// `None` when the knob is unconditionally part of the `core` deploy
    /// surface.
    pub capability: Option<CapabilityId>,
}

impl MobpackDeployPolicy {
    /// Parse the pack's `config/defaults.toml` bytes into a typed policy.
    ///
    /// An absent document (`None`) is the empty policy. Any TOML that is
    /// malformed, non-UTF-8, or contains a knob outside the allow-list fails
    /// closed with [`DeployPolicyError::Invalid`].
    pub fn parse(defaults_toml: Option<&[u8]>) -> Result<Self, DeployPolicyError> {
        let Some(bytes) = defaults_toml else {
            return Ok(Self::default());
        };
        let text = std::str::from_utf8(bytes)
            .map_err(|err| DeployPolicyError::Invalid(format!("not valid UTF-8: {err}")))?;
        Self::from_str(text)
    }

    /// The knobs this policy actually declares, each paired with its gating
    /// capability. Empty groups contribute no gate.
    pub fn declared_knobs(&self) -> Vec<DeployKnobGate> {
        let mut gates = Vec::new();
        if self.max_tokens.is_some() {
            gates.push(DeployKnobGate {
                knob: "max_tokens",
                capability: None,
            });
        }
        if !self.models.is_empty() {
            gates.push(DeployKnobGate {
                knob: "models",
                capability: None,
            });
        }
        if !self.budget.is_empty() {
            gates.push(DeployKnobGate {
                knob: "budget",
                capability: None,
            });
        }
        if !self.compaction.is_empty() {
            gates.push(DeployKnobGate {
                knob: "compaction",
                capability: Some(CapabilityId::SessionCompaction),
            });
        }
        gates
    }

    /// Resolve this policy against the manifest-declared capabilities and the
    /// runtime-provided capabilities, mirroring the manifest `[requires]`
    /// allow-list gate.
    ///
    /// Every declared knob whose gate names a capability must have that
    /// capability *both* declared by the manifest and present in the runtime;
    /// otherwise validation fails closed. Knobs with no gating capability are
    /// always permitted. Returns the same policy on success so the caller can
    /// apply it.
    pub fn validate<F, G>(
        &self,
        manifest_declares: F,
        runtime_provides: G,
    ) -> Result<&Self, DeployPolicyError>
    where
        F: Fn(CapabilityId) -> bool,
        G: Fn(CapabilityId) -> bool,
    {
        for gate in self.declared_knobs() {
            let Some(capability) = gate.capability else {
                continue;
            };
            if !manifest_declares(capability) {
                return Err(DeployPolicyError::UndeclaredCapability {
                    knob: gate.knob.to_string(),
                    capability: capability.to_string(),
                });
            }
            if !runtime_provides(capability) {
                return Err(DeployPolicyError::RuntimeCapabilityMissing {
                    knob: gate.knob.to_string(),
                    capability: capability.to_string(),
                });
            }
        }
        Ok(self)
    }
}

impl FromStr for MobpackDeployPolicy {
    type Err = DeployPolicyError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        toml::from_str(text).map_err(|err| DeployPolicyError::Invalid(err.to_string()))
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_document_is_default_policy() {
        assert_eq!(
            MobpackDeployPolicy::parse(None).unwrap(),
            MobpackDeployPolicy::default()
        );
        assert!(
            MobpackDeployPolicy::parse(None)
                .unwrap()
                .declared_knobs()
                .is_empty()
        );
    }

    #[test]
    fn test_valid_allow_listed_knobs_parse_into_typed_policy() {
        let toml = br#"
max_tokens = 4096

[models]
anthropic = "claude-opus-4-8"
openai = "gpt-5.4"

[budget]
max_tokens = 200000
max_tool_calls = 50

[compaction]
recent_turn_budget = 6
max_summary_tokens = 2048
"#;
        let policy = MobpackDeployPolicy::parse(Some(toml)).expect("valid policy");
        assert_eq!(policy.max_tokens, Some(4096));
        assert_eq!(
            policy.models.anthropic.as_ref().map(ModelRef::as_str),
            Some("claude-opus-4-8")
        );
        assert_eq!(
            policy.models.openai.as_ref().map(ModelRef::as_str),
            Some("gpt-5.4")
        );
        assert_eq!(policy.budget.max_tokens, Some(200_000));
        assert_eq!(policy.budget.max_tool_calls, Some(50));
        assert_eq!(policy.compaction.recent_turn_budget, Some(6));
        assert_eq!(policy.compaction.max_summary_tokens, Some(2048));

        // The compaction group declares its session_compaction gate; the
        // always-on knobs declare no capability.
        let gates = policy.declared_knobs();
        assert!(gates.iter().any(
            |g| g.knob == "compaction" && g.capability == Some(CapabilityId::SessionCompaction)
        ));
        assert!(
            gates
                .iter()
                .any(|g| g.knob == "max_tokens" && g.capability.is_none())
        );
    }

    #[test]
    fn test_disallowed_section_is_rejected_at_parse() {
        // A pack must not be able to set realm connection sets, self-hosted
        // provider endpoints, provider-tool keys, comms credentials, shell
        // security mode, or hook commands through its deploy defaults.
        for disallowed in [
            "[realm.prod]\nbackend_profile = \"x\"\n",
            "[self_hosted]\nbase_url = \"http://evil\"\n",
            "[provider_tools]\nweb_search = true\n",
            "[comms]\nrealm = \"x\"\n",
            "[hooks]\npre_turn = \"rm -rf /\"\n",
            "[shell]\nsecurity_mode = \"unrestricted\"\n",
            "[tools]\nmob_enabled = true\n",
            "[agent]\nmax_tokens_per_turn = 1\n",
        ] {
            let err = MobpackDeployPolicy::parse(Some(disallowed.as_bytes()))
                .expect_err("disallowed knob must fail closed");
            assert!(
                matches!(err, DeployPolicyError::Invalid(_)),
                "expected Invalid for {disallowed:?}, got {err:?}"
            );
        }
    }

    #[test]
    fn test_unknown_field_in_allow_listed_section_is_rejected() {
        let err = MobpackDeployPolicy::parse(Some(b"[budget]\nbogus = 1\n"))
            .expect_err("unknown field inside an allow-listed section must fail closed");
        assert!(matches!(err, DeployPolicyError::Invalid(_)));
    }

    #[test]
    fn test_validate_requires_declared_and_runtime_capability_for_gated_knob() {
        let policy =
            MobpackDeployPolicy::parse(Some(b"[compaction]\nrecent_turn_budget = 4\n")).unwrap();

        // Manifest did not declare session_compaction -> rejected.
        let err = policy
            .validate(|_| false, |_| true)
            .expect_err("undeclared gating capability must reject");
        assert!(matches!(
            err,
            DeployPolicyError::UndeclaredCapability { ref capability, .. }
                if capability == "session_compaction"
        ));

        // Declared by manifest but runtime does not provide -> rejected.
        let err = policy
            .validate(|c| c == CapabilityId::SessionCompaction, |_| false)
            .expect_err("missing runtime capability must reject");
        assert!(matches!(
            err,
            DeployPolicyError::RuntimeCapabilityMissing { ref capability, .. }
                if capability == "session_compaction"
        ));

        // Both declared and provided -> accepted.
        policy
            .validate(
                |c| c == CapabilityId::SessionCompaction,
                |c| c == CapabilityId::SessionCompaction,
            )
            .expect("gated knob applies when capability declared and provided");
    }

    #[test]
    fn test_validate_allows_ungated_knobs_without_capabilities() {
        let policy = MobpackDeployPolicy::parse(Some(
            b"max_tokens = 1024\n[models]\nanthropic = \"claude-opus-4-8\"\n",
        ))
        .unwrap();
        // No capability declared or provided, yet always-on knobs validate.
        policy
            .validate(|_| false, |_| false)
            .expect("ungated knobs validate without any capability");
    }
}
