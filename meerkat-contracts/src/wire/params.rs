//! Composable request fragments.
//!
//! Protocol crates inline the fields they support and provide accessor
//! methods returning the fragment type. No `#[serde(flatten)]` —
//! explicit delegation to avoid known serde/schemars issues.

use serde::{Deserialize, Serialize};

use meerkat_core::{
    HookRunOverrides, OutputSchema, PeerMeta, Provider,
    skills::{SkillId, SkillKey, SkillRef, SourceIdentityRegistry},
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
    #[serde(default)]
    pub host_mode: bool,
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
/// `preload_skills`: Pre-load these skills at session creation.
/// `None` or empty = inventory-only mode (no pre-loading).
/// `Some([])` is normalized to `None` to prevent silent misconfiguration.
///
/// `skill_references`: Skill IDs to resolve and inject for this turn.
/// `None` or empty = no per-turn skill injection.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SkillsParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preload_skills: Option<Vec<String>>,
    /// Structured refs for Skills V2.1. Supports legacy strings via `SkillRef::Legacy`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skill_refs: Option<Vec<SkillRef>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skill_references: Option<Vec<String>>,
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
        if let Some(ref v) = self.skill_references
            && v.is_empty()
        {
            self.skill_references = None;
        }
    }

    /// Canonicalize boundary refs by merging structured `skill_refs` and
    /// compatibility `skill_references`.
    pub fn canonical_skill_refs(&self) -> Option<Vec<SkillRef>> {
        let mut refs = Vec::new();

        if let Some(structured) = &self.skill_refs {
            refs.extend(structured.iter().cloned());
        }
        if let Some(legacy) = &self.skill_references {
            refs.extend(legacy.iter().cloned().map(SkillRef::Legacy));
        }

        if refs.is_empty() { None } else { Some(refs) }
    }

    /// Canonicalize to typed `SkillId`s.
    pub fn canonical_skill_ids(&self) -> Option<Vec<SkillId>> {
        self.canonical_skill_refs().map(|refs| {
            refs.into_iter()
                .map(|r| match r {
                    SkillRef::Legacy(id) => SkillId(id),
                    SkillRef::Structured(key) => SourceIdentityRegistry::canonical_skill_id(&key),
                })
                .collect()
        })
    }

    /// Canonicalize through the source-identity resolver boundary, producing
    /// typed canonical `SkillKey` values.
    pub fn canonical_skill_keys_with_registry(
        &self,
        registry: &SourceIdentityRegistry,
    ) -> Result<Option<Vec<SkillKey>>, meerkat_core::skills::SkillError> {
        let Some(refs) = self.canonical_skill_refs() else {
            return Ok(None);
        };

        let mut keys = Vec::with_capacity(refs.len());
        for reference in refs {
            keys.push(registry.resolve_skill_ref(&reference)?);
        }

        Ok(Some(keys))
    }

    /// Canonicalize through the source-identity resolver boundary and down-convert
    /// to canonical `SkillId` strings for legacy callers.
    pub fn canonical_skill_ids_with_registry(
        &self,
        registry: &SourceIdentityRegistry,
    ) -> Result<Option<Vec<SkillId>>, meerkat_core::skills::SkillError> {
        Ok(self
            .canonical_skill_keys_with_registry(registry)?
            .map(|keys| {
                keys.into_iter()
                    .map(|key| SourceIdentityRegistry::canonical_skill_id(&key))
                    .collect()
            }))
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::redundant_clone)]
mod tests {
    use super::*;

    #[test]
    fn test_skills_params_none_serde() -> Result<(), serde_json::Error> {
        let params = SkillsParams {
            preload_skills: None,
            skill_refs: None,
            skill_references: None,
        };
        let json = serde_json::to_string(&params)?;
        assert_eq!(json, "{}");

        let parsed: SkillsParams = serde_json::from_str("{}")?;
        assert!(parsed.preload_skills.is_none());
        assert!(parsed.skill_refs.is_none());
        assert!(parsed.skill_references.is_none());
        Ok(())
    }

    #[test]
    fn test_skills_params_empty_normalizes() {
        let mut params = SkillsParams {
            preload_skills: Some(vec![]),
            skill_refs: Some(vec![]),
            skill_references: Some(vec![]),
        };
        params.normalize();
        assert!(params.preload_skills.is_none());
        assert!(params.skill_refs.is_none());
        assert!(params.skill_references.is_none());
    }

    #[test]
    fn test_skills_params_with_ids() -> Result<(), serde_json::Error> {
        let params = SkillsParams {
            preload_skills: Some(vec!["a/b".into()]),
            skill_refs: Some(vec![SkillRef::Legacy("a/b".to_string())]),
            skill_references: Some(vec!["c/d".into()]),
        };
        let json = serde_json::to_string(&params)?;
        let parsed: SkillsParams = serde_json::from_str(&json)?;
        assert_eq!(parsed.preload_skills, Some(vec!["a/b".to_string()]));
        assert_eq!(
            parsed.skill_refs,
            Some(vec![SkillRef::Legacy("a/b".to_string())])
        );
        assert_eq!(parsed.skill_references, Some(vec!["c/d".to_string()]));
        Ok(())
    }

    #[test]
    fn test_skill_refs_structured_and_legacy_equivalence() -> Result<(), serde_json::Error> {
        let structured_json = r#"{
            "skill_refs":[{"source_uuid":"dc256086-0d2f-4f61-a307-320d4148107f","skill_name":"email-extractor"}]
        }"#;
        let legacy_json =
            r#"{"skill_references":["dc256086-0d2f-4f61-a307-320d4148107f/email-extractor"]}"#;

        let structured: SkillsParams = serde_json::from_str(structured_json)?;
        let legacy: SkillsParams = serde_json::from_str(legacy_json)?;

        assert_eq!(
            structured.canonical_skill_ids(),
            Some(vec![SkillId(
                "dc256086-0d2f-4f61-a307-320d4148107f/email-extractor".to_string()
            )])
        );
        assert_eq!(
            structured.canonical_skill_ids(),
            legacy.canonical_skill_ids()
        );
        Ok(())
    }

    #[test]
    fn test_skill_refs_canonical_mixed_order_is_deterministic() -> Result<(), serde_json::Error> {
        let mixed_json = r#"{
            "skill_refs":[{"source_uuid":"dc256086-0d2f-4f61-a307-320d4148107f","skill_name":"email-extractor"}],
            "skill_references":["legacy/skill"]
        }"#;
        let parsed: SkillsParams = serde_json::from_str(mixed_json)?;
        let canonical = parsed.canonical_skill_refs().expect("canonical refs");

        assert_eq!(canonical.len(), 2);
        assert!(matches!(canonical[0], SkillRef::Structured(_)));
        assert_eq!(canonical[1], SkillRef::Legacy("legacy/skill".to_string()));
        Ok(())
    }

    #[test]
    fn test_skill_refs_canonicalized_via_registry_remap() {
        use meerkat_core::skills::{
            SkillAlias, SkillKey, SkillKeyRemap, SkillName, SourceIdentityLineage,
            SourceIdentityLineageEvent, SourceIdentityRecord, SourceIdentityStatus,
            SourceTransportKind, SourceUuid,
        };

        let source_old = SourceUuid::parse("dc256086-0d2f-4f61-a307-320d4148107f").expect("uuid");
        let source_new = SourceUuid::parse("a93d587d-8f44-438f-8189-6e8cf549f6e7").expect("uuid");
        let old_name = SkillName::parse("email-extractor").expect("slug");
        let new_name = SkillName::parse("mail-extractor").expect("slug");

        let registry = SourceIdentityRegistry::build(
            vec![
                SourceIdentityRecord {
                    source_uuid: source_old.clone(),
                    display_name: "old".to_string(),
                    transport_kind: SourceTransportKind::Filesystem,
                    fingerprint: "fp-a".to_string(),
                    status: SourceIdentityStatus::Active,
                },
                SourceIdentityRecord {
                    source_uuid: source_new.clone(),
                    display_name: "new".to_string(),
                    transport_kind: SourceTransportKind::Filesystem,
                    fingerprint: "fp-a".to_string(),
                    status: SourceIdentityStatus::Active,
                },
            ],
            vec![SourceIdentityLineage {
                event_id: "rotate-1".to_string(),
                recorded_at_unix_secs: 1,
                required_from_skills: vec![old_name.clone()],
                event: SourceIdentityLineageEvent::Rotate {
                    from: source_old.clone(),
                    to: source_new.clone(),
                },
            }],
            vec![SkillKeyRemap {
                from: SkillKey {
                    source_uuid: source_old.clone(),
                    skill_name: old_name.clone(),
                },
                to: SkillKey {
                    source_uuid: source_new.clone(),
                    skill_name: new_name.clone(),
                },
                reason: None,
            }],
            vec![SkillAlias {
                alias: "legacy/email".to_string(),
                to: SkillKey {
                    source_uuid: source_old.clone(),
                    skill_name: old_name,
                },
            }],
        )
        .expect("registry");

        let params = SkillsParams {
            preload_skills: None,
            skill_refs: Some(vec![SkillRef::Structured(SkillKey {
                source_uuid: source_old,
                skill_name: SkillName::parse("email-extractor").expect("slug"),
            })]),
            skill_references: Some(vec!["legacy/email".to_string()]),
        };

        let canonical = params
            .canonical_skill_ids_with_registry(&registry)
            .expect("canonicalization should succeed")
            .expect("ids");
        assert_eq!(
            canonical,
            vec![
                SkillId("a93d587d-8f44-438f-8189-6e8cf549f6e7/mail-extractor".to_string()),
                SkillId("a93d587d-8f44-438f-8189-6e8cf549f6e7/mail-extractor".to_string())
            ]
        );
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
        Ok(())
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
        };
        let json = serde_json::to_string(&params)?;
        assert!(!json.contains("\"labels\""));
        assert!(!json.contains("\"additional_instructions\""));
        assert!(!json.contains("\"app_context\""));
        Ok(())
    }
}
