//! Composable request fragments.
//!
//! Protocol crates inline the fields they support and provide accessor
//! methods returning the fragment type. No `#[serde(flatten)]` —
//! explicit delegation to avoid known serde/schemars issues.

use serde::{Deserialize, Serialize};

use meerkat_core::{
    HookRunOverrides, OutputSchema, PeerMeta, Provider, SurfaceMetadata, SurfaceMetadataError,
    skills::{SkillKey, SkillRef},
};

/// Core session creation parameters.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct CoreCreateParams {
    pub prompt: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<Provider>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labels: Option<std::collections::BTreeMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub additional_instructions: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_context: Option<serde_json::Value>,
    /// Per-agent environment variables injected into shell tool subprocesses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shell_env: Option<std::collections::HashMap<String, String>>,
    /// Phase 4c — realm-scoped binding reference. When set, the session
    /// is built through the realm connection set; when omitted, the
    /// legacy flat `provider + api_key` path is used.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_binding: Option<super::connection::WireAuthBindingRef>,
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
pub struct CommsParams {
    /// None = inherit persisted session intent, Some(true) = enable, Some(false) = disable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep_alive: Option<bool>,
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
/// `preload_skills` and `skill_refs` both carry typed `SkillKey`s — there is
/// no legacy string path. Wire ingress parses JSON objects directly to
/// `SkillKey` (`{"source_uuid":"…","skill_name":"…"}`); malformed input is a
/// typed ingress error, not a legacy-upgrade.
///
/// `Some([])` is normalized to `None` to prevent silent misconfiguration.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SkillsParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preload_skills: Option<Vec<SkillKey>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skill_refs: Option<Vec<SkillRef>>,
}

impl SkillsParams {
    /// Normalize: `Some([])` → `None` for both fields.
    pub fn normalize(&mut self) {
        if let Some(ref v) = self.preload_skills
            && v.is_empty()
        {
            self.preload_skills = None;
        }
        if let Some(ref v) = self.skill_refs
            && v.is_empty()
        {
            self.skill_refs = None;
        }
    }

    /// Flatten all ingress refs into a single typed `Vec<SkillKey>`.
    ///
    /// No legacy string path, no `SourceIdentityRegistry` round-trip at the
    /// wire boundary — the registry applies remaps at resolution time, not
    /// here. Callers that need registry canonicalization call
    /// `SourceIdentityRegistry::canonical_skill_key` directly.
    pub fn canonical_skill_keys(&self) -> Option<Vec<SkillKey>> {
        let mut keys: Vec<SkillKey> = Vec::new();
        if let Some(refs) = &self.skill_refs {
            keys.extend(refs.iter().map(|r| r.key().clone()));
        }
        if keys.is_empty() { None } else { Some(keys) }
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
            preload_skills: None,
            skill_refs: None,
        };
        let json = serde_json::to_string(&params)?;
        assert_eq!(json, "{}");

        let parsed: SkillsParams = serde_json::from_str("{}")?;
        assert!(parsed.preload_skills.is_none());
        assert!(parsed.skill_refs.is_none());
        Ok(())
    }

    #[test]
    fn test_skills_params_empty_normalizes() {
        let mut params = SkillsParams {
            preload_skills: Some(vec![]),
            skill_refs: Some(vec![]),
        };
        params.normalize();
        assert!(params.preload_skills.is_none());
        assert!(params.skill_refs.is_none());
    }

    #[test]
    fn test_skills_params_with_keys_roundtrip() -> Result<(), serde_json::Error> {
        let key = test_key("email-extractor");
        let params = SkillsParams {
            preload_skills: Some(vec![key.clone()]),
            skill_refs: Some(vec![SkillRef::Structured(key.clone())]),
        };
        let json = serde_json::to_string(&params)?;
        let parsed: SkillsParams = serde_json::from_str(&json)?;
        assert_eq!(parsed, params);
        Ok(())
    }

    #[test]
    fn test_skill_refs_rejects_legacy_string_form() {
        // The legacy slash-delimited string shape ("uuid/skill-name") is no
        // longer accepted at the wire boundary.
        let legacy_json =
            r#"{"skill_refs":["dc256086-0d2f-4f61-a307-320d4148107f/email-extractor"]}"#;
        let parsed: Result<SkillsParams, _> = serde_json::from_str(legacy_json);
        assert!(parsed.is_err(), "legacy string form must be rejected");
    }

    #[test]
    fn test_skill_refs_parses_structured_only() -> Result<(), serde_json::Error> {
        let structured_json = r#"{
            "skill_refs":[{
                "kind":"structured",
                "source_uuid":"dc256086-0d2f-4f61-a307-320d4148107f",
                "skill_name":"email-extractor"
            }]
        }"#;
        let parsed: SkillsParams = serde_json::from_str(structured_json)?;
        let canonical = parsed.canonical_skill_keys().expect("keys");
        assert_eq!(canonical, vec![test_key("email-extractor")]);
        Ok(())
    }

    #[test]
    fn test_core_create_params_all_fields_roundtrip() -> Result<(), serde_json::Error> {
        let mut labels = std::collections::BTreeMap::new();
        labels.insert("env".to_string(), "prod".to_string());
        labels.insert("team".to_string(), "infra".to_string());

        let params = CoreCreateParams {
            prompt: "hello".to_string(),
            model: Some("claude-opus-4-6".to_string()),
            provider: Some(Provider::Anthropic),
            max_tokens: Some(1024),
            system_prompt: Some("You are helpful.".to_string()),
            labels: Some(labels.clone()),
            additional_instructions: Some(vec![
                "Be concise.".to_string(),
                "Use JSON output.".to_string(),
            ]),
            app_context: Some(serde_json::json!({"org_id": "acme", "tier": "premium"})),
            shell_env: None,
            auth_binding: None,
        };
        let json = serde_json::to_string(&params)?;
        let parsed: CoreCreateParams = serde_json::from_str(&json)?;
        assert_eq!(parsed.prompt, "hello");
        assert_eq!(parsed.labels, Some(labels));
        assert_eq!(
            parsed.additional_instructions,
            Some(vec![
                "Be concise.".to_string(),
                "Use JSON output.".to_string()
            ])
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
            model: None,
            provider: None,
            max_tokens: None,
            system_prompt: None,
            labels: Some(std::collections::BTreeMap::from([(
                "meerkat.runtime_id".to_string(),
                "spoof".to_string(),
            )])),
            additional_instructions: None,
            app_context: None,
            shell_env: None,
            auth_binding: None,
        };

        assert!(params.validate_public_surface_metadata().is_err());
    }

    #[test]
    fn test_core_create_params_defaults_backward_compat() -> Result<(), serde_json::Error> {
        let json = r#"{"prompt": "hello"}"#;
        let parsed: CoreCreateParams = serde_json::from_str(json)?;
        assert_eq!(parsed.prompt, "hello");
        assert!(parsed.model.is_none());
        assert!(parsed.labels.is_none());
        assert!(parsed.additional_instructions.is_none());
        assert!(parsed.app_context.is_none());
        Ok(())
    }

    #[test]
    fn test_core_create_params_none_fields_omitted() -> Result<(), serde_json::Error> {
        let params = CoreCreateParams {
            prompt: "hello".to_string(),
            model: None,
            provider: None,
            max_tokens: None,
            system_prompt: None,
            labels: None,
            additional_instructions: None,
            app_context: None,
            shell_env: None,
            auth_binding: None,
        };
        let json = serde_json::to_string(&params)?;
        assert!(!json.contains("\"labels\""));
        assert!(!json.contains("\"additional_instructions\""));
        assert!(!json.contains("\"app_context\""));
        assert!(!json.contains("\"shell_env\""));
        assert!(!json.contains("\"auth_binding\""));
        Ok(())
    }

    #[test]
    fn test_core_create_params_with_auth_binding() -> Result<(), Box<dyn std::error::Error>> {
        use crate::wire::WireAuthBindingRef;
        let params = CoreCreateParams {
            prompt: "hello".to_string(),
            model: None,
            provider: None,
            max_tokens: None,
            system_prompt: None,
            labels: None,
            additional_instructions: None,
            app_context: None,
            shell_env: None,
            auth_binding: Some(WireAuthBindingRef {
                realm: meerkat_core::connection::RealmId::parse("dev")?,
                binding: meerkat_core::connection::BindingId::parse("default_openai")?,
                profile: None,
            }),
        };
        let json = serde_json::to_string(&params)?;
        assert!(json.contains("\"auth_binding\""));
        assert!(json.contains("\"realm\":\"dev\""));
        let parsed: CoreCreateParams = serde_json::from_str(&json)?;
        assert_eq!(
            parsed.auth_binding.map(|r| r.binding.as_str().to_owned()),
            Some("default_openai".to_string())
        );
        Ok(())
    }
}
