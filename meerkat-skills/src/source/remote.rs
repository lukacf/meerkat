use std::collections::BTreeMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use indexmap::IndexMap;
use meerkat_core::skills::{
    SkillDescriptor, SkillDocument, SkillError, SkillFilter, SkillKey, SkillName,
    SkillQuarantineDiagnostic, SkillScope, SourceHealthSnapshot, SourceHealthThresholds,
    SourceUuid, apply_filter, classify_source_health,
};
use serde::Deserialize;

use crate::parser::parse_skill_md;

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum RemoteSkillCatalog {
    Wrapped {
        source_uuid: Option<String>,
        skills: Vec<RemoteSkillEntry>,
    },
    Direct(Vec<RemoteSkillEntry>),
}

#[derive(Debug, Clone, Deserialize)]
pub struct RemoteSkillEntry {
    pub name: String,
    #[serde(default)]
    pub source_uuid: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default, alias = "content", alias = "skill_md")]
    pub body: Option<String>,
    #[serde(default)]
    pub metadata: IndexMap<String, String>,
}

#[derive(Debug, Clone, Default)]
pub struct RemoteCache {
    pub descriptors: Vec<SkillDescriptor>,
    pub documents: BTreeMap<SkillKey, SkillDocument>,
    pub quarantined: Vec<SkillQuarantineDiagnostic>,
    pub refreshed_at: Option<SystemTime>,
}

impl RemoteCache {
    pub fn is_fresh(&self, ttl: Duration) -> bool {
        let Some(refreshed_at) = self.refreshed_at else {
            return false;
        };
        refreshed_at.elapsed().is_ok_and(|elapsed| elapsed < ttl)
    }

    pub fn has_data(&self) -> bool {
        !self.descriptors.is_empty() || !self.documents.is_empty()
    }
}

#[derive(Debug, Clone)]
pub struct ParsedRemoteCatalog {
    pub descriptors: Vec<SkillDescriptor>,
    pub documents: BTreeMap<SkillKey, SkillDocument>,
    pub quarantined: Vec<SkillQuarantineDiagnostic>,
}

pub fn parse_remote_catalog(
    raw: &str,
    configured_source_uuid: &SourceUuid,
    scope: SkillScope,
    location: &str,
) -> Result<ParsedRemoteCatalog, SkillError> {
    let catalog: RemoteSkillCatalog = serde_json::from_str(raw)
        .map_err(|e| SkillError::Parse(format!("remote skill catalog parse error: {e}").into()))?;
    let (catalog_source_uuid, entries) = match catalog {
        RemoteSkillCatalog::Wrapped {
            source_uuid,
            skills,
        } => (source_uuid, skills),
        RemoteSkillCatalog::Direct(skills) => (None, skills),
    };
    if let Some(raw_uuid) = catalog_source_uuid {
        let parsed = SourceUuid::parse(&raw_uuid)?;
        if &parsed != configured_source_uuid {
            return Err(SkillError::Load(
                format!(
                    "remote source_uuid mismatch at {location}: configured {configured_source_uuid}, remote {parsed}"
                )
                .into(),
            ));
        }
    }

    let mut descriptors = Vec::new();
    let mut documents = BTreeMap::new();
    let mut quarantined = Vec::new();
    for entry in entries {
        let entry_location = format!("{location}:{}", entry.name);
        match parse_remote_entry(entry, configured_source_uuid, scope) {
            Ok((descriptor, document)) => {
                if let Some(doc) = document {
                    documents.insert(doc.descriptor.key.clone(), doc);
                }
                descriptors.push(descriptor);
            }
            Err((key, message)) => {
                if let Some(key) = key {
                    quarantined.push(quarantine(
                        key,
                        entry_location,
                        "invalid_remote_skill",
                        "parse",
                        message,
                    ));
                }
            }
        }
    }
    Ok(ParsedRemoteCatalog {
        descriptors,
        documents,
        quarantined,
    })
}

pub fn parse_remote_document(
    raw: &str,
    key: &SkillKey,
    scope: SkillScope,
) -> Result<SkillDocument, SkillError> {
    if raw.trim_start().starts_with("---") {
        return parse_skill_md(key.clone(), scope, raw, Some(key.skill_name.as_str()));
    }
    let entry: RemoteSkillEntry = serde_json::from_str(raw)
        .map_err(|e| SkillError::Parse(format!("remote skill document parse error: {e}").into()))?;
    let (descriptor, document) =
        parse_remote_entry(entry, &key.source_uuid, scope).map_err(|(_, message)| {
            SkillError::Parse(format!("remote skill document invalid: {message}").into())
        })?;
    if &descriptor.key != key {
        return Err(SkillError::Load(
            format!(
                "remote skill document identity mismatch: requested {}, returned {}",
                key, descriptor.key
            )
            .into(),
        ));
    }
    document.ok_or_else(|| {
        SkillError::Load(
            format!(
                "remote skill document for {} contained descriptor '{}' but no body",
                key, descriptor.name
            )
            .into(),
        )
    })
}

fn parse_remote_entry(
    entry: RemoteSkillEntry,
    configured_source_uuid: &SourceUuid,
    scope: SkillScope,
) -> Result<(SkillDescriptor, Option<SkillDocument>), (Option<SkillKey>, String)> {
    let skill_name = SkillName::parse(&entry.name).map_err(|e| {
        (
            fallback_key(configured_source_uuid),
            format!("invalid remote skill name '{}': {e}", entry.name),
        )
    })?;
    let key = SkillKey::new(configured_source_uuid.clone(), skill_name.clone());
    if let Some(raw_uuid) = entry.source_uuid {
        let remote_uuid =
            SourceUuid::parse(&raw_uuid).map_err(|e| (Some(key.clone()), e.to_string()))?;
        if &remote_uuid != configured_source_uuid {
            return Err((
                Some(key),
                format!(
                    "remote entry source_uuid mismatch: configured {configured_source_uuid}, remote {remote_uuid}"
                ),
            ));
        }
    }

    if let Some(content) = entry.body {
        let doc = parse_skill_md(key.clone(), scope, &content, Some(skill_name.as_str()))
            .map_err(|e| (Some(key.clone()), e.to_string()))?;
        return Ok((doc.descriptor.clone(), Some(doc)));
    }

    let Some(description) = entry.description else {
        return Err((
            Some(key),
            "remote skill entry missing description or body".to_string(),
        ));
    };
    Ok((
        SkillDescriptor {
            key,
            name: skill_name.as_str().to_string(),
            description,
            scope,
            metadata: entry.metadata,
            capability_requirements: Vec::new(),
            source_name: String::new(),
        },
        None,
    ))
}

pub fn filter_cached(cache: &RemoteCache, filter: &SkillFilter) -> Vec<SkillDescriptor> {
    apply_filter(&cache.descriptors, filter)
}

pub fn health_from_cache(
    cache: &RemoteCache,
    thresholds: SourceHealthThresholds,
    failure_streak: u32,
    handshake_failed: bool,
) -> SourceHealthSnapshot {
    let invalid_count = cache.quarantined.len() as u32;
    let total_count = cache.descriptors.len() as u32 + invalid_count;
    let invalid_ratio = if total_count == 0 {
        0.0
    } else {
        invalid_count as f32 / total_count as f32
    };
    SourceHealthSnapshot {
        state: classify_source_health(invalid_ratio, failure_streak, handshake_failed, thresholds),
        invalid_ratio,
        invalid_count,
        total_count,
        failure_streak,
        handshake_failed,
    }
}

pub fn load_cached(cache: &RemoteCache, key: &SkillKey) -> Result<SkillDocument, SkillError> {
    cache
        .documents
        .get(key)
        .cloned()
        .ok_or_else(|| SkillError::NotFound { key: key.clone() })
}

fn fallback_key(source_uuid: &SourceUuid) -> Option<SkillKey> {
    SkillName::parse("invalid-skill")
        .ok()
        .map(|skill_name| SkillKey::new(source_uuid.clone(), skill_name))
}

fn quarantine(
    key: SkillKey,
    location: String,
    error_code: &str,
    error_class: &str,
    message: String,
) -> SkillQuarantineDiagnostic {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default();
    SkillQuarantineDiagnostic {
        key,
        location,
        error_code: error_code.to_string(),
        error_class: error_class.to_string(),
        message,
        first_seen_unix_secs: now,
        last_seen_unix_secs: now,
    }
}

pub fn cache_from_catalog(parsed: ParsedRemoteCatalog) -> RemoteCache {
    RemoteCache {
        descriptors: parsed.descriptors,
        documents: parsed.documents,
        quarantined: parsed.quarantined,
        refreshed_at: Some(SystemTime::now()),
    }
}
