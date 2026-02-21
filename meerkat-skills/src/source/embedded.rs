//! Embedded skill source (from inventory registrations).

use indexmap::IndexMap;
use meerkat_core::skills::{
    SkillDescriptor, SkillDocument, SkillError, SkillFilter, SkillId, SkillName, SkillSource,
    apply_filter,
};

use crate::registration::{SkillRegistration, collect_registered_skills};

/// Convert a static `SkillRegistration` to a `SkillDescriptor`.
fn registration_to_descriptor(reg: &SkillRegistration) -> Result<SkillDescriptor, SkillError> {
    let id = SkillId(reg.id.to_string());
    let canonical_name = SkillName::parse(id.skill_name())?;
    Ok(SkillDescriptor {
        id,
        // Canonical runtime name is slug; human-readable name is preserved in metadata.
        name: canonical_name.to_string(),
        description: reg.description.to_string(),
        scope: reg.scope,
        requires_capabilities: reg
            .requires_capabilities
            .iter()
            .map(|s| s.to_string())
            .collect(),
        metadata: IndexMap::from([("display_name".to_string(), reg.name.to_string())]),
        ..Default::default()
    })
}

/// Convert a static `SkillRegistration` to a `SkillDocument`.
fn registration_to_document(reg: &SkillRegistration) -> Result<SkillDocument, SkillError> {
    Ok(SkillDocument {
        descriptor: registration_to_descriptor(reg)?,
        body: reg.body.to_string(),
        extensions: reg
            .extensions
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
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

    async fn load(&self, id: &SkillId) -> Result<SkillDocument, SkillError> {
        collect_registered_skills()
            .into_iter()
            .find(|r| r.id == id.0)
            .map(registration_to_document)
            .transpose()?
            .ok_or_else(|| SkillError::NotFound { id: id.clone() })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use meerkat_core::skills::SkillScope;

    #[test]
    fn registration_descriptor_uses_canonical_slug_and_preserves_display_name() {
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
        assert_eq!(descriptor.id.0, "collection/email-extractor");
        assert_eq!(descriptor.name, "email-extractor");
        assert_eq!(
            descriptor.metadata.get("display_name"),
            Some(&"Email Extractor".to_string())
        );
    }

    #[test]
    fn registration_document_preserves_canonical_descriptor_and_metadata() {
        let reg = SkillRegistration {
            id: "root/pdf-processing",
            name: "PDF Processing",
            description: "Process PDF files",
            scope: SkillScope::Builtin,
            requires_capabilities: &[],
            body: "Content",
            extensions: &[("vendor.version", "1")],
        };

        let doc = registration_to_document(&reg).unwrap();
        assert_eq!(doc.descriptor.id.0, "root/pdf-processing");
        assert_eq!(doc.descriptor.name, "pdf-processing");
        assert_eq!(
            doc.descriptor.metadata.get("display_name"),
            Some(&"PDF Processing".to_string())
        );
        assert_eq!(doc.extensions.get("vendor.version"), Some(&"1".to_string()));
    }
}
