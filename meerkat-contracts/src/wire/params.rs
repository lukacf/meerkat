//! Composable request fragments.
//!
//! Protocol crates inline the fields they support and provide accessor
//! methods returning the fragment type. No `#[serde(flatten)]` —
//! explicit delegation to avoid known serde/schemars issues.

use serde::{Deserialize, Serialize};

use super::runtime::WireRuntimeTurnMetadata;
use meerkat_core::{
    HookRunOverrides, OutputSchema, PeerMeta, SurfaceMetadata, SurfaceMetadataError,
};

/// Core session creation parameters.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct CoreCreateParams {
    pub prompt: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_metadata: Option<WireRuntimeTurnMetadata>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labels: Option<std::collections::BTreeMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_context: Option<serde_json::Value>,
    /// Per-agent environment variables injected into shell tool subprocesses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shell_env: Option<std::collections::HashMap<String, String>>,
}

impl CoreCreateParams {
    /// Compose the existing `labels` and `app_context` fields into the shared
    /// surface metadata contract without changing the JSON wire shape.
    #[must_use]
    pub fn surface_metadata(&self) -> SurfaceMetadata {
        SurfaceMetadata::from_optional_parts(self.labels.clone(), self.app_context.clone())
    }

    /// Validate caller-supplied metadata for public create surfaces.
    pub fn validate_public_surface_metadata(&self) -> Result<(), SurfaceMetadataError> {
        self.surface_metadata().validate_public()
    }
}

/// Structured output parameters.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StructuredOutputParams {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<OutputSchema>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub structured_output_retries: Option<u32>,
}

/// Comms parameters.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct CommsParams {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comms_name: Option<String>,
    /// Friendly metadata for peer discovery.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peer_meta: Option<PeerMeta>,
}

/// Hook parameters.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct HookParams {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hooks_override: Option<HookRunOverrides>,
}

/// Skills parameters for session/turn requests.
///
/// Skill references flow through the canonical runtime turn metadata carrier.
/// Legacy top-level `preload_skills` and `skill_refs` are retired.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct SkillsParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_metadata: Option<WireRuntimeTurnMetadata>,
}

impl SkillsParams {
    /// Normalize: empty metadata carriers become `None`.
    pub fn normalize(&mut self) {
        if let Some(metadata) = self.turn_metadata.as_mut()
            && metadata
                .skill_references
                .as_ref()
                .is_some_and(Vec::is_empty)
        {
            metadata.skill_references = None;
        }
        if self
            .turn_metadata
            .as_ref()
            .is_some_and(|metadata| metadata == &WireRuntimeTurnMetadata::default())
        {
            self.turn_metadata = None;
        }
    }

    /// Return canonical skill keys from the runtime turn metadata carrier.
    pub fn canonical_skill_keys(&self) -> Option<Vec<meerkat_core::skills::SkillKey>> {
        self.turn_metadata
            .as_ref()
            .and_then(|metadata| metadata.skill_references.clone())
            .filter(|keys| !keys.is_empty())
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::redundant_clone)]
mod tests {
    use super::*;
    use meerkat_core::skills::{SkillKey, SkillName, SourceUuid};

    fn test_key(skill: &str) -> SkillKey {
        SkillKey {
            source_uuid: SourceUuid::parse("dc256086-0d2f-4f61-a307-320d4148107f")
                .expect("valid uuid"),
            skill_name: SkillName::parse(skill).expect("valid skill slug"),
        }
    }

    #[test]
    fn test_skills_params_none_serde() -> Result<(), serde_json::Error> {
        let params = SkillsParams {
            turn_metadata: None,
        };
        let json = serde_json::to_string(&params)?;
        assert_eq!(json, "{}");

        let parsed: SkillsParams = serde_json::from_str("{}")?;
        assert!(parsed.turn_metadata.is_none());
        Ok(())
    }

    #[test]
    fn test_skills_params_empty_normalizes() {
        let mut params = SkillsParams {
            turn_metadata: Some(WireRuntimeTurnMetadata {
                skill_references: Some(vec![]),
                ..Default::default()
            }),
        };
        params.normalize();
        assert!(params.turn_metadata.is_none());
    }

    #[test]
    fn test_skills_params_with_keys_roundtrip() -> Result<(), serde_json::Error> {
        let key = test_key("email-extractor");
        let params = SkillsParams {
            turn_metadata: Some(WireRuntimeTurnMetadata {
                skill_references: Some(vec![key.clone()]),
                ..Default::default()
            }),
        };
        let json = serde_json::to_string(&params)?;
        let parsed: SkillsParams = serde_json::from_str(&json)?;
        assert_eq!(parsed, params);
        assert_eq!(parsed.canonical_skill_keys(), Some(vec![key]));
        Ok(())
    }

    #[test]
    fn test_skill_refs_rejects_legacy_split_fields() {
        for legacy_json in [
            r#"{"preload_skills":[{"source_uuid":"dc256086-0d2f-4f61-a307-320d4148107f","skill_name":"email-extractor"}]}"#,
            r#"{"skill_refs":[{"kind":"structured","source_uuid":"dc256086-0d2f-4f61-a307-320d4148107f","skill_name":"email-extractor"}]}"#,
            r#"{"skill_refs":["dc256086-0d2f-4f61-a307-320d4148107f/email-extractor"]}"#,
        ] {
            let parsed: Result<SkillsParams, _> = serde_json::from_str(legacy_json);
            assert!(parsed.is_err(), "legacy split skill field must be rejected");
        }
    }

    #[test]
    fn test_skill_refs_parse_from_turn_metadata_only() -> Result<(), serde_json::Error> {
        let structured_json = r#"{
            "turn_metadata": {
                "skill_references":[{
                    "source_uuid":"dc256086-0d2f-4f61-a307-320d4148107f",
                    "skill_name":"email-extractor"
                }]
            }
        }"#;
        let parsed: SkillsParams = serde_json::from_str(structured_json)?;
        let canonical = parsed.canonical_skill_keys().expect("keys");
        assert_eq!(canonical, vec![test_key("email-extractor")]);
        Ok(())
    }

    #[test]
    fn test_core_create_params_all_fields_roundtrip() -> Result<(), serde_json::Error> {
        use super::super::runtime::{WireTurnInstruction, WireTurnInstructionKind};

        let mut labels = std::collections::BTreeMap::new();
        labels.insert("env".to_string(), "prod".to_string());
        labels.insert("team".to_string(), "infra".to_string());

        let params = CoreCreateParams {
            prompt: "hello".to_string(),
            turn_metadata: Some(WireRuntimeTurnMetadata {
                model: Some("claude-opus-4-6".to_string()),
                additional_instructions: Some(vec![
                    WireTurnInstruction {
                        kind: WireTurnInstructionKind::Host,
                        body: "Be concise.".to_string(),
                    },
                    WireTurnInstruction {
                        kind: WireTurnInstructionKind::Host,
                        body: "Use JSON output.".to_string(),
                    },
                ]),
                ..Default::default()
            }),
            max_tokens: Some(1024),
            system_prompt: Some("You are helpful.".to_string()),
            labels: Some(labels.clone()),
            app_context: Some(serde_json::json!({"org_id": "acme", "tier": "premium"})),
            shell_env: None,
        };
        let json = serde_json::to_string(&params)?;
        let parsed: CoreCreateParams = serde_json::from_str(&json)?;
        assert_eq!(parsed.prompt, "hello");
        assert_eq!(parsed.labels, Some(labels));
        assert_eq!(
            parsed
                .turn_metadata
                .as_ref()
                .and_then(|metadata| metadata.model.as_deref()),
            Some("claude-opus-4-6")
        );
        assert!(parsed.app_context.is_some());
        assert_eq!(
            parsed
                .surface_metadata()
                .labels
                .get("team")
                .map(String::as_str),
            Some("infra")
        );
        assert_eq!(
            parsed.surface_metadata().app_context,
            Some(serde_json::json!({"org_id": "acme", "tier": "premium"}))
        );
        Ok(())
    }

    #[test]
    fn test_core_create_params_surface_metadata_rejects_reserved_keys() {
        let params = CoreCreateParams {
            prompt: "hello".to_string(),
            turn_metadata: None,
            max_tokens: None,
            system_prompt: None,
            labels: Some(std::collections::BTreeMap::from([(
                "meerkat.runtime_id".to_string(),
                "spoof".to_string(),
            )])),
            app_context: None,
            shell_env: None,
        };

        assert!(params.validate_public_surface_metadata().is_err());
    }

    #[test]
    fn test_core_create_params_defaults_backward_compat() -> Result<(), serde_json::Error> {
        let json = r#"{"prompt": "hello"}"#;
        let parsed: CoreCreateParams = serde_json::from_str(json)?;
        assert_eq!(parsed.prompt, "hello");
        assert!(parsed.turn_metadata.is_none());
        assert!(parsed.labels.is_none());
        assert!(parsed.app_context.is_none());
        Ok(())
    }

    #[test]
    fn test_core_create_params_none_fields_omitted() -> Result<(), serde_json::Error> {
        let params = CoreCreateParams {
            prompt: "hello".to_string(),
            turn_metadata: None,
            max_tokens: None,
            system_prompt: None,
            labels: None,
            app_context: None,
            shell_env: None,
        };
        let json = serde_json::to_string(&params)?;
        assert!(!json.contains("\"labels\""));
        assert!(!json.contains("\"turn_metadata\""));
        assert!(!json.contains("\"app_context\""));
        assert!(!json.contains("\"shell_env\""));
        Ok(())
    }

    #[test]
    fn test_core_create_params_rejects_split_metadata_fields() {
        for (field, value) in [
            ("model", serde_json::json!("claude-opus-4-6")),
            ("provider", serde_json::json!("anthropic")),
            (
                "additional_instructions",
                serde_json::json!(["Be concise."]),
            ),
            (
                "connection_ref",
                serde_json::json!({ "realm": "dev", "binding": "default_openai" }),
            ),
        ] {
            let parsed: Result<CoreCreateParams, _> = serde_json::from_value(serde_json::json!({
                "prompt": "hello",
                field: value
            }));
            assert!(
                parsed.is_err(),
                "CoreCreateParams must reject split field {field}"
            );
        }
    }

    #[test]
    fn test_comms_params_rejects_split_keep_alive() {
        let parsed: Result<CommsParams, _> =
            serde_json::from_str(r#"{"comms_name":"agent-a","keep_alive":true}"#);
        assert!(parsed.is_err(), "CommsParams must reject split keep_alive");
    }
}
