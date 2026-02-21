use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use super::{
    SkillError, SkillKey, SkillName, SkillRef, SourceIdentityLineage, SourceIdentityLineageEvent,
    SourceIdentityRecord, SourceUuid,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SkillAlias {
    pub alias: String,
    pub to: SkillKey,
}

#[derive(Debug, Clone, Default)]
pub struct SourceIdentityRegistry {
    aliases: HashMap<String, SkillKey>,
    remaps: HashMap<SkillKey, SkillKey>,
}

impl SourceIdentityRegistry {
    pub fn build(
        records: Vec<SourceIdentityRecord>,
        lineage: Vec<SourceIdentityLineage>,
        remaps: Vec<super::SkillKeyRemap>,
        aliases: Vec<SkillAlias>,
    ) -> Result<Self, SkillError> {
        let mut fingerprints_by_uuid: HashMap<SourceUuid, String> = HashMap::new();
        let mut uuids_by_fingerprint: HashMap<String, HashSet<SourceUuid>> = HashMap::new();

        for record in &records {
            if let Some(existing) = fingerprints_by_uuid.get(&record.source_uuid)
                && existing != &record.fingerprint
            {
                return Err(SkillError::SourceUuidCollision {
                    source_uuid: record.source_uuid.to_string(),
                    existing_fingerprint: existing.clone(),
                    new_fingerprint: record.fingerprint.clone(),
                });
            }

            fingerprints_by_uuid
                .entry(record.source_uuid.clone())
                .or_insert_with(|| record.fingerprint.clone());
            uuids_by_fingerprint
                .entry(record.fingerprint.clone())
                .or_default()
                .insert(record.source_uuid.clone());
        }

        let edges = collect_lineage_edges(&lineage);
        for (fingerprint, uuids) in &uuids_by_fingerprint {
            if uuids.len() <= 1 {
                continue;
            }
            let mut iter = uuids.iter();
            let Some(first) = iter.next() else {
                continue;
            };
            for other in iter {
                if !lineage_related(first, other, &edges) {
                    return Err(SkillError::SourceUuidMutationWithoutLineage {
                        fingerprint: fingerprint.clone(),
                        existing_source_uuid: first.to_string(),
                        mutated_source_uuid: other.to_string(),
                    });
                }
            }
        }

        for event in &lineage {
            if let Some((event_kind, from_set, to_set, required_skills)) =
                remap_required_sets(event)
            {
                if required_skills.is_empty() {
                    return Err(SkillError::MissingSkillRemaps {
                        event_id: event.event_id.clone(),
                        event_kind,
                    });
                }
                if !remaps_cover_required_sets(&remaps, &from_set, &to_set, &required_skills) {
                    return Err(SkillError::MissingSkillRemaps {
                        event_id: event.event_id.clone(),
                        event_kind,
                    });
                }
            }
        }

        let mut remap_index: HashMap<SkillKey, SkillKey> = HashMap::new();
        for remap in remaps {
            if remap.from.source_uuid != remap.to.source_uuid
                && !lineage_allows(&remap.from.source_uuid, &remap.to.source_uuid, &edges)
            {
                return Err(SkillError::RemapWithoutLineage {
                    from_source_uuid: remap.from.source_uuid.to_string(),
                    from_skill_name: remap.from.skill_name.to_string(),
                    to_source_uuid: remap.to.source_uuid.to_string(),
                    to_skill_name: remap.to.skill_name.to_string(),
                });
            }
            remap_index.insert(remap.from, remap.to);
        }

        let mut alias_index = HashMap::new();
        for alias in aliases {
            alias_index.insert(alias.alias, alias.to);
        }

        Ok(Self {
            aliases: alias_index,
            remaps: remap_index,
        })
    }

    pub fn resolve_skill_ref(&self, reference: &SkillRef) -> Result<SkillKey, SkillError> {
        match reference {
            SkillRef::Structured(key) => self.apply_remaps(key.clone()),
            SkillRef::Legacy(legacy) => {
                let key = match parse_legacy_as_key(legacy) {
                    Ok(parsed) => parsed,
                    Err(_) => self.aliases.get(legacy).cloned().ok_or_else(|| {
                        SkillError::UnknownSkillAlias {
                            alias: legacy.clone(),
                        }
                    })?,
                };
                self.apply_remaps(key)
            }
        }
    }

    pub fn canonical_skill_id(key: &SkillKey) -> super::SkillId {
        super::SkillId(format!("{}/{}", key.source_uuid, key.skill_name))
    }

    fn apply_remaps(&self, mut key: SkillKey) -> Result<SkillKey, SkillError> {
        let mut visited = HashSet::new();
        while let Some(next) = self.remaps.get(&key).cloned() {
            if !visited.insert(key.clone()) {
                return Err(SkillError::RemapCycle {
                    source_uuid: key.source_uuid.to_string(),
                    skill_name: key.skill_name.to_string(),
                });
            }
            key = next;
        }
        Ok(key)
    }
}

fn collect_lineage_edges(lineage: &[SourceIdentityLineage]) -> HashSet<(SourceUuid, SourceUuid)> {
    let mut edges = HashSet::new();
    for event in lineage {
        match &event.event {
            SourceIdentityLineageEvent::RenameOrRelocate { from, to }
            | SourceIdentityLineageEvent::Rotate { from, to } => {
                edges.insert((from.clone(), to.clone()));
            }
            SourceIdentityLineageEvent::Split { from, into } => {
                for target in into {
                    edges.insert((from.clone(), target.clone()));
                }
            }
            SourceIdentityLineageEvent::Merge { from, to } => {
                for origin in from {
                    edges.insert((origin.clone(), to.clone()));
                }
            }
        }
    }
    edges
}

fn lineage_allows(
    from: &SourceUuid,
    to: &SourceUuid,
    edges: &HashSet<(SourceUuid, SourceUuid)>,
) -> bool {
    if from == to {
        return true;
    }
    edges.contains(&(from.clone(), to.clone()))
}

fn lineage_related(
    a: &SourceUuid,
    b: &SourceUuid,
    edges: &HashSet<(SourceUuid, SourceUuid)>,
) -> bool {
    if a == b {
        return true;
    }

    let mut visited: HashSet<SourceUuid> = HashSet::new();
    let mut stack = vec![a.clone()];

    while let Some(current) = stack.pop() {
        if current == *b {
            return true;
        }
        if !visited.insert(current.clone()) {
            continue;
        }

        for (from, to) in edges {
            if *from == current && !visited.contains(to) {
                stack.push(to.clone());
            }
            if *to == current && !visited.contains(from) {
                stack.push(from.clone());
            }
        }
    }

    false
}

fn remap_required_sets(event: &SourceIdentityLineage) -> Option<RemapRequiredSets> {
    match &event.event {
        SourceIdentityLineageEvent::Rotate { from, to } => Some((
            "rotate",
            HashSet::from([from.clone()]),
            HashSet::from([to.clone()]),
            event.required_from_skills.iter().cloned().collect(),
        )),
        SourceIdentityLineageEvent::Split { from, into } => Some((
            "split",
            HashSet::from([from.clone()]),
            into.iter().cloned().collect(),
            event.required_from_skills.iter().cloned().collect(),
        )),
        SourceIdentityLineageEvent::Merge { from, to } => Some((
            "merge",
            from.iter().cloned().collect(),
            HashSet::from([to.clone()]),
            event.required_from_skills.iter().cloned().collect(),
        )),
        SourceIdentityLineageEvent::RenameOrRelocate { .. } => None,
    }
}

type RemapRequiredSets = (
    &'static str,
    HashSet<SourceUuid>,
    HashSet<SourceUuid>,
    HashSet<SkillName>,
);

fn remaps_cover_required_sets(
    remaps: &[super::SkillKeyRemap],
    from_set: &HashSet<SourceUuid>,
    to_set: &HashSet<SourceUuid>,
    required_skills: &HashSet<SkillName>,
) -> bool {
    let mut covered_from: HashSet<SourceUuid> = HashSet::new();
    let mut covered_to: HashSet<SourceUuid> = HashSet::new();
    for remap in remaps {
        if from_set.contains(&remap.from.source_uuid) && to_set.contains(&remap.to.source_uuid) {
            covered_from.insert(remap.from.source_uuid.clone());
            covered_to.insert(remap.to.source_uuid.clone());
        }
    }
    let source_coverage_ok =
        covered_from.len() == from_set.len() && covered_to.len() == to_set.len();
    if !source_coverage_ok {
        return false;
    }

    if required_skills.is_empty() {
        return true;
    }

    from_set.iter().all(|source_uuid| {
        required_skills.iter().all(|skill_name| {
            remaps.iter().any(|remap| {
                remap.from.source_uuid == *source_uuid
                    && remap.from.skill_name == *skill_name
                    && to_set.contains(&remap.to.source_uuid)
            })
        })
    })
}

fn parse_legacy_as_key(reference: &str) -> Result<SkillKey, SkillError> {
    let mut parts = reference.split('/');
    let Some(source_uuid_raw) = parts.next() else {
        return Err(SkillError::InvalidLegacySkillRefFormat {
            reference: reference.to_string(),
        });
    };
    let Some(skill_name_raw) = parts.next() else {
        return Err(SkillError::InvalidLegacySkillRefFormat {
            reference: reference.to_string(),
        });
    };
    if parts.next().is_some() {
        return Err(SkillError::InvalidLegacySkillRefFormat {
            reference: reference.to_string(),
        });
    }

    let source_uuid = SourceUuid::parse(source_uuid_raw).map_err(|_| {
        SkillError::InvalidLegacySkillRefFormat {
            reference: reference.to_string(),
        }
    })?;
    let skill_name =
        SkillName::parse(skill_name_raw).map_err(|_| SkillError::InvalidLegacySkillRefFormat {
            reference: reference.to_string(),
        })?;
    Ok(SkillKey {
        source_uuid,
        skill_name,
    })
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use crate::skills::{SkillKeyRemap, SourceIdentityStatus, SourceTransportKind};

    fn source_uuid(raw: &str) -> SourceUuid {
        SourceUuid::parse(raw).expect("valid source uuid")
    }

    fn parse_skill_name(raw: &str) -> SkillName {
        SkillName::parse(raw).expect("valid skill name")
    }

    fn key(source_raw: &str, skill_slug: &str) -> SkillKey {
        SkillKey {
            source_uuid: source_uuid(source_raw),
            skill_name: parse_skill_name(skill_slug),
        }
    }

    fn record(source_raw: &str, fingerprint: &str) -> SourceIdentityRecord {
        SourceIdentityRecord {
            source_uuid: source_uuid(source_raw),
            display_name: "test".to_string(),
            transport_kind: SourceTransportKind::Filesystem,
            fingerprint: fingerprint.to_string(),
            status: SourceIdentityStatus::Active,
        }
    }

    fn lineage_rotate(event_id: &str, from: &str, to: &str) -> SourceIdentityLineage {
        SourceIdentityLineage {
            event_id: event_id.to_string(),
            recorded_at_unix_secs: 1,
            required_from_skills: vec![parse_skill_name("email-extractor")],
            event: SourceIdentityLineageEvent::Rotate {
                from: source_uuid(from),
                to: source_uuid(to),
            },
        }
    }

    #[test]
    fn registry_rejects_uuid_collision_with_conflicting_fingerprint() {
        let result = SourceIdentityRegistry::build(
            vec![
                record("dc256086-0d2f-4f61-a307-320d4148107f", "fp-a"),
                record("dc256086-0d2f-4f61-a307-320d4148107f", "fp-b"),
            ],
            vec![],
            vec![],
            vec![],
        );
        assert!(matches!(
            result,
            Err(SkillError::SourceUuidCollision { .. })
        ));
    }

    #[test]
    fn registry_rejects_uuid_mutation_without_lineage() {
        let result = SourceIdentityRegistry::build(
            vec![
                record("dc256086-0d2f-4f61-a307-320d4148107f", "fp-a"),
                record("a93d587d-8f44-438f-8189-6e8cf549f6e7", "fp-a"),
            ],
            vec![],
            vec![],
            vec![],
        );
        assert!(matches!(
            result,
            Err(SkillError::SourceUuidMutationWithoutLineage { .. })
        ));
    }

    #[test]
    fn registry_rejects_rotate_without_remaps() {
        let result = SourceIdentityRegistry::build(
            vec![
                record("dc256086-0d2f-4f61-a307-320d4148107f", "fp-a"),
                record("a93d587d-8f44-438f-8189-6e8cf549f6e7", "fp-a"),
            ],
            vec![lineage_rotate(
                "evt-rotate",
                "dc256086-0d2f-4f61-a307-320d4148107f",
                "a93d587d-8f44-438f-8189-6e8cf549f6e7",
            )],
            vec![],
            vec![],
        );
        assert!(matches!(result, Err(SkillError::MissingSkillRemaps { .. })));
    }

    #[test]
    fn registry_resolves_alias_and_applies_remap() {
        let registry = SourceIdentityRegistry::build(
            vec![
                record("dc256086-0d2f-4f61-a307-320d4148107f", "fp-a"),
                record("a93d587d-8f44-438f-8189-6e8cf549f6e7", "fp-a"),
            ],
            vec![lineage_rotate(
                "evt-rotate",
                "dc256086-0d2f-4f61-a307-320d4148107f",
                "a93d587d-8f44-438f-8189-6e8cf549f6e7",
            )],
            vec![SkillKeyRemap {
                from: key("dc256086-0d2f-4f61-a307-320d4148107f", "email-extractor"),
                to: key("a93d587d-8f44-438f-8189-6e8cf549f6e7", "mail-extractor"),
                reason: None,
            }],
            vec![SkillAlias {
                alias: "legacy/email".to_string(),
                to: key("dc256086-0d2f-4f61-a307-320d4148107f", "email-extractor"),
            }],
        )
        .expect("registry should build");

        let resolved = registry
            .resolve_skill_ref(&SkillRef::Legacy("legacy/email".to_string()))
            .expect("alias should resolve");
        assert_eq!(
            resolved,
            key("a93d587d-8f44-438f-8189-6e8cf549f6e7", "mail-extractor")
        );
    }

    #[test]
    fn registry_rejects_split_when_remaps_do_not_cover_all_targets() {
        let result = SourceIdentityRegistry::build(
            vec![
                record("dc256086-0d2f-4f61-a307-320d4148107f", "fp-a"),
                record("a93d587d-8f44-438f-8189-6e8cf549f6e7", "fp-a"),
                record("e8df561d-d38f-4242-af55-3a6efb34c950", "fp-a"),
            ],
            vec![SourceIdentityLineage {
                event_id: "evt-split".to_string(),
                recorded_at_unix_secs: 1,
                required_from_skills: vec![
                    parse_skill_name("email-extractor"),
                    parse_skill_name("pdf-processing"),
                ],
                event: SourceIdentityLineageEvent::Split {
                    from: source_uuid("dc256086-0d2f-4f61-a307-320d4148107f"),
                    into: vec![
                        source_uuid("a93d587d-8f44-438f-8189-6e8cf549f6e7"),
                        source_uuid("e8df561d-d38f-4242-af55-3a6efb34c950"),
                    ],
                },
            }],
            vec![SkillKeyRemap {
                from: key("dc256086-0d2f-4f61-a307-320d4148107f", "email-extractor"),
                to: key("a93d587d-8f44-438f-8189-6e8cf549f6e7", "mail-extractor"),
                reason: None,
            }],
            vec![],
        );
        assert!(matches!(result, Err(SkillError::MissingSkillRemaps { .. })));
    }

    #[test]
    fn registry_rejects_merge_when_remaps_do_not_cover_all_origins() {
        let result = SourceIdentityRegistry::build(
            vec![
                record("dc256086-0d2f-4f61-a307-320d4148107f", "fp-a"),
                record("a93d587d-8f44-438f-8189-6e8cf549f6e7", "fp-a"),
                record("e8df561d-d38f-4242-af55-3a6efb34c950", "fp-a"),
            ],
            vec![SourceIdentityLineage {
                event_id: "evt-merge".to_string(),
                recorded_at_unix_secs: 1,
                required_from_skills: vec![
                    parse_skill_name("email-extractor"),
                    parse_skill_name("pdf-processing"),
                ],
                event: SourceIdentityLineageEvent::Merge {
                    from: vec![
                        source_uuid("dc256086-0d2f-4f61-a307-320d4148107f"),
                        source_uuid("a93d587d-8f44-438f-8189-6e8cf549f6e7"),
                    ],
                    to: source_uuid("e8df561d-d38f-4242-af55-3a6efb34c950"),
                },
            }],
            vec![SkillKeyRemap {
                from: key("dc256086-0d2f-4f61-a307-320d4148107f", "email-extractor"),
                to: key("e8df561d-d38f-4242-af55-3a6efb34c950", "email-extractor"),
                reason: None,
            }],
            vec![],
        );
        assert!(matches!(result, Err(SkillError::MissingSkillRemaps { .. })));
    }

    #[test]
    fn registry_rejects_when_required_from_skill_is_missing() {
        let result = SourceIdentityRegistry::build(
            vec![
                record("dc256086-0d2f-4f61-a307-320d4148107f", "fp-a"),
                record("a93d587d-8f44-438f-8189-6e8cf549f6e7", "fp-a"),
            ],
            vec![SourceIdentityLineage {
                event_id: "evt-rotate-required".to_string(),
                recorded_at_unix_secs: 1,
                required_from_skills: vec![
                    parse_skill_name("email-extractor"),
                    parse_skill_name("pdf-processing"),
                ],
                event: SourceIdentityLineageEvent::Rotate {
                    from: source_uuid("dc256086-0d2f-4f61-a307-320d4148107f"),
                    to: source_uuid("a93d587d-8f44-438f-8189-6e8cf549f6e7"),
                },
            }],
            vec![SkillKeyRemap {
                from: key("dc256086-0d2f-4f61-a307-320d4148107f", "email-extractor"),
                to: key("a93d587d-8f44-438f-8189-6e8cf549f6e7", "mail-extractor"),
                reason: None,
            }],
            vec![],
        );
        assert!(matches!(result, Err(SkillError::MissingSkillRemaps { .. })));
    }
}
