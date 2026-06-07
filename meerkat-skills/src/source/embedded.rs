//! Embedded skill source (from inventory registrations).

use std::collections::HashSet;

use indexmap::IndexMap;
use meerkat_core::skills::{
    SkillDescriptor, SkillDocument, SkillError, SkillFilter, SkillKey, SkillSource,
    SourceHealthSnapshot, SourceHealthThresholds, SourceUuid, apply_filter, classify_source_health,
};

use crate::registration::{RegistrationId, SkillRegistration, collect_registered_skills};

/// Convert a static `SkillRegistration` to a `SkillDescriptor`.
///
/// Embedded skills are all rooted at `SourceUuid::builtin()`. The full
/// registration id IS the canonical builtin identity (parsed via the typed
/// [`RegistrationId`], fail-closed) — there is no "trailing path segment"
/// convention that could collapse distinct ids to one loadable key.
fn registration_to_descriptor(reg: &SkillRegistration) -> Result<SkillDescriptor, SkillError> {
    let registration_id = RegistrationId::parse(reg.id)?;
    let skill_name = registration_id.skill_name().clone();
    let key = SkillKey {
        source_uuid: SourceUuid::builtin(),
        skill_name,
    };

    let mut metadata: IndexMap<String, String> = IndexMap::new();
    metadata.insert("display_name".to_string(), reg.name.to_string());

    let mut capability_requirements = Vec::new();
    for raw in reg.requires_capabilities {
        let cap = meerkat_core::skills::CapabilityId::parse(raw)?;
        capability_requirements.push(cap);
    }

    Ok(SkillDescriptor {
        key,
        name: registration_id.as_str().to_string(),
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

/// Outcome of scanning the embedded inventory: the valid descriptors plus the
/// counts that feed source-health classification (invalid = unparsable
/// registrations and id collisions; total = every registration considered).
struct EmbeddedScan {
    descriptors: Vec<SkillDescriptor>,
    invalid_count: u32,
    total_count: u32,
}

/// Scan the inventory once, recording every skipped/invalid/colliding
/// registration so inventory truth carries typed source-health instead of a
/// silently filtered best-effort list.
fn scan_registrations() -> EmbeddedScan {
    let mut descriptors: Vec<SkillDescriptor> = Vec::new();
    let mut seen: HashSet<SkillKey> = HashSet::new();
    let mut invalid_count: u32 = 0;
    let mut total_count: u32 = 0;

    for reg in collect_registered_skills() {
        total_count = total_count.saturating_add(1);
        match registration_to_descriptor(reg) {
            Ok(desc) => {
                if !seen.insert(desc.key.clone()) {
                    // Fail-closed collision: two distinct registrations resolved
                    // to one builtin key. Count it invalid rather than letting a
                    // later registration silently shadow an earlier one.
                    invalid_count = invalid_count.saturating_add(1);
                    tracing::warn!(
                        skill_id = %reg.id,
                        key = %desc.key,
                        "embedded skill registration collides with an existing builtin key"
                    );
                    continue;
                }
                descriptors.push(desc);
            }
            Err(err) => {
                invalid_count = invalid_count.saturating_add(1);
                tracing::warn!(
                    skill_id = %reg.id,
                    "Skipping invalid embedded skill registration: {err}"
                );
            }
        }
    }

    EmbeddedScan {
        descriptors,
        invalid_count,
        total_count,
    }
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
        let scan = scan_registrations();
        Ok(apply_filter(&scan.descriptors, filter))
    }

    async fn health_snapshot(&self) -> Result<SourceHealthSnapshot, SkillError> {
        let scan = scan_registrations();
        let invalid_ratio = if scan.total_count == 0 {
            0.0
        } else {
            scan.invalid_count as f32 / scan.total_count as f32
        };
        let thresholds = SourceHealthThresholds::default();
        Ok(SourceHealthSnapshot {
            state: classify_source_health(invalid_ratio, 0, false, thresholds),
            invalid_ratio,
            invalid_count: scan.invalid_count,
            total_count: scan.total_count,
            failure_streak: 0,
            handshake_failed: false,
        })
    }

    async fn load(&self, key: &SkillKey) -> Result<SkillDocument, SkillError> {
        if key.source_uuid != SourceUuid::builtin() {
            return Err(SkillError::NotFound { key: key.clone() });
        }
        let slug = key.skill_name.as_str();
        collect_registered_skills()
            .into_iter()
            .find(|r| match RegistrationId::parse(r.id) {
                // Match the FULL typed registration id, not a trailing path
                // segment, so distinct ids cannot resolve through one slug.
                Ok(registration_id) => registration_id.as_str() == slug,
                Err(_) => false,
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
    fn registration_descriptor_uses_builtin_source_uuid_and_full_id() {
        let reg = SkillRegistration {
            id: "email-extractor",
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
    }

    #[test]
    fn registration_rejects_slash_namespaced_id() {
        // Row #77: a slash-namespaced id must fail closed, not silently collapse
        // to its trailing path segment.
        let reg = SkillRegistration {
            id: "collection/email-extractor",
            name: "Email Extractor",
            description: "Extract email content",
            scope: SkillScope::Builtin,
            requires_capabilities: &[],
            body: "Body",
            extensions: &[],
        };
        let err = registration_to_descriptor(&reg).unwrap_err();
        assert!(matches!(err, SkillError::Parse(_)));
    }

    #[test]
    fn distinct_prefixed_ids_do_not_collapse_to_one_key() {
        // Row #77: two distinct registrations whose only difference is a slash
        // prefix must NOT collapse to a single loadable builtin key. With the
        // typed RegistrationId both are rejected (slash is not a valid slug),
        // so neither resolves to the shared trailing segment.
        assert!(RegistrationId::parse("a/foo").is_err());
        assert!(RegistrationId::parse("b/foo").is_err());
        // The valid bare slug remains addressable as itself.
        let id = RegistrationId::parse("foo").unwrap();
        assert_eq!(id.as_str(), "foo");
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

    #[tokio::test]
    async fn health_snapshot_reports_typed_counts_not_default() {
        // Row #220: embedded health must carry real inventory counts, not a
        // Default snapshot. The embedded inventory ships valid registrations,
        // so total_count is populated and the source classifies Healthy.
        let source = EmbeddedSkillSource::new();
        let listed = source.list(&SkillFilter::default()).await.unwrap();
        let health = source.health_snapshot().await.unwrap();

        assert!(
            health.total_count >= listed.len() as u32,
            "health total_count must account for every registration considered"
        );
        assert!(
            health.total_count > 0,
            "embedded inventory must report a non-empty total_count"
        );
        // All shipped builtin registrations are valid, so health is Healthy and
        // carries a zero invalid_count (not a Default placeholder).
        assert_eq!(health.invalid_count, 0);
        assert_eq!(
            health.state,
            meerkat_core::skills::SourceHealthState::Healthy
        );
    }
}
