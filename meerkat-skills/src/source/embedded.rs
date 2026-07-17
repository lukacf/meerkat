//! Embedded skill source (from inventory registrations).

use std::collections::HashSet;

use indexmap::IndexMap;
use meerkat_core::skills::{
    SkillDescriptor, SkillDocument, SkillError, SkillExtensionKey, SkillFilter, SkillKey,
    SkillSource, SourceHealthSnapshot, SourceHealthThresholds, SourceUuid, apply_filter,
    classify_source_health,
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
    let mut extensions: IndexMap<SkillExtensionKey, String> =
        IndexMap::with_capacity(reg.extensions.len());
    for (k, v) in reg.extensions {
        // Parse each embedded extension key through the typed `namespace.key`
        // owner, fail-closed: a malformed key surfaces here rather than
        // silently surviving as a re-split string.
        extensions.insert(SkillExtensionKey::parse(k)?, (*v).to_string());
    }
    Ok(SkillDocument {
        descriptor: registration_to_descriptor(reg)?,
        body: reg.body.to_string(),
        extensions,
    })
}

/// Outcome of scanning the embedded inventory: the valid descriptors plus the
/// counts that feed source-health classification (invalid = unparsable
/// registrations and id collisions; total = every registration considered).
struct EmbeddedScan<'a> {
    descriptors: Vec<SkillDescriptor>,
    registrations: IndexMap<SkillKey, &'a SkillRegistration>,
    ambiguous_keys: HashSet<SkillKey>,
    invalid_count: u32,
    total_count: u32,
}

impl EmbeddedScan<'_> {
    /// Resolve one registration from the same collision-aware projection used
    /// by list and health. An ambiguous key is a load error, never a
    /// link-order-dependent `NotFound` or silently selected owner.
    fn load(&self, key: &SkillKey) -> Result<SkillDocument, SkillError> {
        if self.ambiguous_keys.contains(key) {
            return Err(SkillError::Load(
                format!(
                    "ambiguous embedded skill registration for {key}: every colliding owner is excluded"
                )
                .into(),
            ));
        }
        self.registrations
            .get(key)
            .copied()
            .map(registration_to_document)
            .transpose()?
            .ok_or_else(|| SkillError::NotFound { key: key.clone() })
    }
}

/// Scan the ambient `inventory` registry once into a typed [`EmbeddedScan`].
fn scan_registrations() -> EmbeddedScan<'static> {
    scan_registration_iter(collect_registered_skills())
}

/// Scan a registration set once, recording every skipped/invalid/colliding
/// registration so inventory truth carries typed source-health instead of a
/// silently filtered best-effort list. Pure over its input so the counting is
/// exercisable independently of the ambient `inventory` registry.
fn scan_registration_iter<'a>(
    registrations: impl IntoIterator<Item = &'a SkillRegistration>,
) -> EmbeddedScan<'a> {
    let mut loadable: IndexMap<SkillKey, (&'a SkillRegistration, SkillDescriptor)> =
        IndexMap::new();
    let mut ambiguous_keys: HashSet<SkillKey> = HashSet::new();
    let mut invalid_count: u32 = 0;
    let mut total_count: u32 = 0;

    for reg in registrations {
        total_count = total_count.saturating_add(1);
        match registration_to_descriptor(reg) {
            Ok(desc) => {
                let key = desc.key.clone();
                if ambiguous_keys.contains(&key) {
                    // Every registration participating in an already-known
                    // collision is invalid. Never allow a third or later entry
                    // to repopulate the key after the first pair was removed.
                    invalid_count = invalid_count.saturating_add(1);
                    tracing::warn!(
                        skill_id = %reg.id,
                        key = %key,
                        "embedded skill registration participates in an ambiguous builtin key"
                    );
                    continue;
                }
                if loadable.shift_remove(&key).is_some() {
                    // The first and second registrations are BOTH invalid: the
                    // builtin key has no canonical owner. Remove the first entry
                    // and permanently mark the key ambiguous for this scan so
                    // link iteration order can never select a winner.
                    invalid_count = invalid_count.saturating_add(2);
                    ambiguous_keys.insert(key.clone());
                    tracing::warn!(
                        skill_id = %reg.id,
                        key = %key,
                        "embedded skill registration collides with an existing builtin key; excluding every owner"
                    );
                    continue;
                }
                loadable.insert(key, (reg, desc));
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

    let mut descriptors = Vec::with_capacity(loadable.len());
    let mut registrations = IndexMap::with_capacity(loadable.len());
    for (key, (registration, descriptor)) in loadable {
        descriptors.push(descriptor);
        registrations.insert(key, registration);
    }

    EmbeddedScan {
        descriptors,
        registrations,
        ambiguous_keys,
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
        scan_registrations().load(key)
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
    async fn health_snapshot_agrees_with_list_one_scan() {
        // Row #220: the live source's health is computed from the SAME scan as
        // `list()` — one truth, not two. `total_count` accounts for every
        // registration considered and the valid ones are exactly what `list()`
        // returns; the shipped builtins carry no parse/collision failures, so
        // `invalid_count` is zero and the source classifies Healthy. (The
        // ambient `inventory` registry may be empty in an isolated unit build;
        // this asserts the cross-read invariant, not a specific population.)
        let source = EmbeddedSkillSource::new();
        let listed = source.list(&SkillFilter::default()).await.unwrap();
        let health = source.health_snapshot().await.unwrap();

        assert_eq!(health.invalid_count, 0);
        assert_eq!(
            health.total_count,
            listed.len() as u32,
            "health total_count must equal the count of valid listed registrations"
        );
        assert_eq!(
            health.state,
            meerkat_core::skills::SourceHealthState::Healthy
        );
    }

    #[test]
    fn embedded_scan_reports_real_counts_not_default() {
        // Row #220: the scan is the typed source of health truth, not a Default
        // placeholder. Every registration considered is counted (`total`); a
        // builtin-key collision invalidates BOTH colliders; the parse failure is
        // independently invalid.
        // A `Default` snapshot would report (total 0, invalid 0); a real scan
        // over these known-bad registrations cannot.
        let valid = SkillRegistration {
            id: "task-workflow",
            name: "Task Workflow",
            description: "",
            scope: SkillScope::Builtin,
            requires_capabilities: &[],
            body: "",
            extensions: &[],
        };
        // Same id => same builtin key as `valid`: a fail-closed collision.
        let collision = SkillRegistration {
            id: "task-workflow",
            name: "Task Workflow (duplicate)",
            description: "",
            scope: SkillScope::Builtin,
            requires_capabilities: &[],
            body: "",
            extensions: &[],
        };
        // Capitalised id is not a canonical slug: a parse failure.
        let invalid_slug = SkillRegistration {
            id: "EmailExtractor",
            name: "Email Extractor",
            description: "",
            scope: SkillScope::Builtin,
            requires_capabilities: &[],
            body: "",
            extensions: &[],
        };

        let scan = scan_registration_iter([&valid, &collision, &invalid_slug]);

        assert_eq!(
            scan.total_count, 3,
            "every registration considered is counted"
        );
        assert_eq!(
            scan.invalid_count, 3,
            "every invalid registration is counted"
        );
        assert_eq!(
            scan.descriptors.len(),
            0,
            "no registration participating in a collision is listable"
        );
        assert!(scan.registrations.is_empty());
        assert_eq!(scan.ambiguous_keys.len(), 1);
    }

    #[test]
    fn colliding_builtin_key_has_no_link_order_winner() {
        let first = SkillRegistration {
            id: "task-workflow",
            name: "First owner",
            description: "first",
            scope: SkillScope::Builtin,
            requires_capabilities: &[],
            body: "first body",
            extensions: &[],
        };
        let second = SkillRegistration {
            id: "task-workflow",
            name: "Second owner",
            description: "second",
            scope: SkillScope::Builtin,
            requires_capabilities: &["shell"],
            body: "second body",
            extensions: &[],
        };
        let key =
            SkillKey::builtin(meerkat_core::skills::SkillName::parse("task-workflow").unwrap());

        for registrations in [[&first, &second], [&second, &first]] {
            let scan = scan_registration_iter(registrations);
            assert_eq!(scan.invalid_count, 2);
            assert_eq!(scan.total_count, 2);
            assert!(scan.descriptors.is_empty());
            assert!(!scan.registrations.contains_key(&key));
            assert!(scan.ambiguous_keys.contains(&key));
            assert!(matches!(scan.load(&key), Err(SkillError::Load(_))));
        }
    }

    #[test]
    fn every_registration_in_multiway_collision_is_invalid() {
        let registrations = ["first", "second", "third"].map(|name| SkillRegistration {
            id: "task-workflow",
            name,
            description: "",
            scope: SkillScope::Builtin,
            requires_capabilities: &[],
            body: name,
            extensions: &[],
        });
        let scan = scan_registration_iter(registrations.iter());

        assert_eq!(scan.total_count, 3);
        assert_eq!(scan.invalid_count, 3);
        assert!(scan.descriptors.is_empty());
        assert!(scan.registrations.is_empty());
    }
}
