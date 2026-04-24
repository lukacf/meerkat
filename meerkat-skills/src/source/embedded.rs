//! Embedded skill source (from inventory registrations).

use indexmap::IndexMap;
use meerkat_core::skills::{
    SkillDescriptor, SkillDocument, SkillError, SkillFilter, SkillKey, SkillName, SkillSource,
    SourceUuid, apply_filter,
};

use crate::registration::{SkillRegistration, collect_registered_skills};

/// Convert a static `SkillRegistration` to a `SkillDescriptor`.
///
/// Embedded skills are all rooted at `SourceUuid::builtin()`. The legacy
/// slash-delimited registration id is interpreted as "`skill_name`" (the
/// final path segment) — any prefix was purely a display convention and is
/// preserved via `metadata["display_id"]`.
fn registration_to_descriptor(reg: &SkillRegistration) -> Result<SkillDescriptor, SkillError> {
    let final_segment = reg.id.rsplit('/').next().unwrap_or(reg.id);
    let skill_name = SkillName::parse(final_segment)?;
    let key = SkillKey {
        source_uuid: SourceUuid::builtin(),
        skill_name,
    };

    let mut metadata: IndexMap<String, String> = IndexMap::new();
    metadata.insert("display_name".to_string(), reg.name.to_string());
    if reg.id != final_segment {
        metadata.insert("display_id".to_string(), reg.id.to_string());
    }

    let mut capability_requirements = Vec::new();
    for raw in reg.requires_capabilities {
        let cap = meerkat_core::skills::CapabilityId::parse(raw)?;
        capability_requirements.push(cap);
    }

    Ok(SkillDescriptor {
        key,
        name: final_segment.to_string(),
        description: reg.description.to_string(),
        scope: reg.scope,
        metadata,
        capability_requirements,
        source_name: String::new(),
    })
}

fn registration_to_document(reg: &SkillRegistration) -> Result<SkillDocument, SkillError> {
    Ok(SkillDocument {
        descriptor: registration_to_descriptor(reg)?,
        body: reg.body.to_string(),
        extensions: reg
            .extensions
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect::<IndexMap<_, _>>(),
    })
}

/// Skill source that reads from `inventory`-registered skills.
pub struct EmbeddedSkillSource;

impl EmbeddedSkillSource {
    pub fn new() -> Self {
        Self
    }
}

impl Default for EmbeddedSkillSource {
    fn default() -> Self {
        Self::new()
    }
}

impl SkillSource for EmbeddedSkillSource {
    async fn list(&self, filter: &SkillFilter) -> Result<Vec<SkillDescriptor>, SkillError> {
        let all: Vec<SkillDescriptor> = collect_registered_skills()
            .into_iter()
            .filter_map(|reg| match registration_to_descriptor(reg) {
                Ok(desc) => Some(desc),
                Err(err) => {
                    tracing::warn!(
                        skill_id = %reg.id,
                        "Skipping invalid embedded skill registration: {err}"
                    );
                    None
                }
            })
            .collect();
        Ok(apply_filter(&all, filter))
    }

    async fn load(&self, key: &SkillKey) -> Result<SkillDocument, SkillError> {
        if key.source_uuid != SourceUuid::builtin() {
            return Err(SkillError::NotFound { key: key.clone() });
        }
        let slug = key.skill_name.as_str();
        collect_registered_skills()
            .into_iter()
            .find(|r| {
                let final_segment = r.id.rsplit('/').next().unwrap_or(r.id);
                final_segment == slug
            })
            .map(registration_to_document)
            .transpose()?
            .ok_or_else(|| SkillError::NotFound { key: key.clone() })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use meerkat_core::skills::SkillScope;

    #[test]
    fn registration_descriptor_uses_builtin_source_uuid_and_slug() {
        let reg = SkillRegistration {
            id: "collection/email-extractor",
            name: "Email Extractor",
            description: "Extract email content",
            scope: SkillScope::Builtin,
            requires_capabilities: &[],
            body: "Body",
            extensions: &[],
        };

        let descriptor = registration_to_descriptor(&reg).unwrap();
        assert_eq!(descriptor.key.source_uuid, SourceUuid::builtin());
        assert_eq!(descriptor.key.skill_name.as_str(), "email-extractor");
        assert_eq!(
            descriptor.metadata.get("display_name"),
            Some(&"Email Extractor".to_string())
        );
        assert_eq!(
            descriptor.metadata.get("display_id"),
            Some(&"collection/email-extractor".to_string())
        );
    }

    #[test]
    fn registration_rejects_invalid_slug() {
        let reg = SkillRegistration {
            id: "EmailExtractor",
            name: "Email Extractor",
            description: "",
            scope: SkillScope::Builtin,
            requires_capabilities: &[],
            body: "",
            extensions: &[],
        };
        let err = registration_to_descriptor(&reg).unwrap_err();
        assert!(matches!(err, SkillError::Parse(_)));
    }

    #[test]
    fn registration_parses_typed_capabilities() {
        let reg = SkillRegistration {
            id: "task-workflow",
            name: "Task Workflow",
            description: "",
            scope: SkillScope::Builtin,
            requires_capabilities: &["builtins", "shell"],
            body: "",
            extensions: &[],
        };
        let descriptor = registration_to_descriptor(&reg).unwrap();
        let caps: Vec<&str> = descriptor
            .capability_requirements
            .iter()
            .map(meerkat_core::skills::CapabilityId::as_str)
            .collect();
        assert_eq!(caps, vec!["builtins", "shell"]);
    }
}
