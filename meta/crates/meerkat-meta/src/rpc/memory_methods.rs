//! Parameter parsing for memory RPC methods.

use super::*;
use crate::identity_first::{
    AgentIdentity, AgentMemoryRecallRequest, AgentMemorySelection, NewAgentMemory,
};
use crate::memory::records::ManifestTier;

const MEMORY_SUPPORTED_STORES: [&str; 5] = [
    "knowledge_graph",
    "vector",
    "timeline",
    "todo",
    "top_of_mind",
];
const DEFAULT_AGENT_MEMORY_MAX_ENTRIES: usize = 8;
const MAX_AGENT_MEMORY_MAX_ENTRIES: usize = 64;
const MAX_AGENT_MEMORY_TITLE_BYTES: usize = 200;
const MAX_AGENT_MEMORY_BODY_BYTES: usize = 64 * 1024;
const MAX_AGENT_MEMORY_TAGS: usize = 32;
const MAX_AGENT_MEMORY_TAG_BYTES: usize = 64;
const MAX_AGENT_MEMORY_QUERY_TEXT_BYTES: usize = 16 * 1024;
const DEFAULT_AGENT_MEMORY_MANIFEST_K: usize = 32;
const MAX_AGENT_MEMORY_MANIFEST_K: usize = 512;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MemoryParamsError {
    ParamsMustBeObject,
    EntityRequired,
    TopicRequired,
    StoreMustBeString,
    UnsupportedStore(String),
    FactMustBeString,
    MetadataMustBeJson,
    ConflictMustBeBoolean,
    ConflictReasonMustBeString,
    EntityMustBeString,
    TopicMustBeString,
    QueryMustBeString,
    IdentityRequired,
    RealmMustBeString,
    MemoryIdRequired,
    TitleRequired,
    BodyRequired,
    TagsMustBeArray,
    TagMustBeString,
    TitleTooLong,
    BodyTooLong,
    TooManyTags,
    TagTooLong,
    SelectionMustBeString,
    UnsupportedSelection(String),
    QueryTextMustBeString,
    QueryTextTooLong,
    QueryTermsMustBeArray,
    QueryTermMustBeString,
    MaxEntriesMustBePositiveInteger,
    MaxEntriesOutOfRange,
    TierMustBeString,
    UnsupportedTier(String),
    KMustBePositiveInteger,
    KOutOfRange,
    KRequiresWorkingSetTier,
    VerdictRequired,
    UnsupportedVerdict(String),
    ExpectedContentHashRequired,
    RationaleMustBeString,
    RationaleTooLong,
    Index(MemoryIndexError),
}

impl MemoryParamsError {
    pub(super) fn backend_message(error: &LocalJsonMemoryStoreError) -> String {
        match error {
            LocalJsonMemoryStoreError::InvalidConfig(reason)
            | LocalJsonMemoryStoreError::Io(reason)
            | LocalJsonMemoryStoreError::Serialize(reason)
            | LocalJsonMemoryStoreError::InvalidStoreData(reason)
            | LocalJsonMemoryStoreError::ExternalCallFailed(reason) => reason.clone(),
        }
    }

    pub(crate) fn message(&self) -> String {
        match self {
            MemoryParamsError::ParamsMustBeObject => "params must be a JSON object".to_string(),
            MemoryParamsError::EntityRequired => "entity must be a non-empty string".to_string(),
            MemoryParamsError::TopicRequired => "topic must be a non-empty string".to_string(),
            MemoryParamsError::StoreMustBeString => {
                "store must be a non-empty string when provided".to_string()
            }
            MemoryParamsError::UnsupportedStore(store) => format!(
                "store '{store}' is unsupported (allowed: knowledge_graph, vector, timeline, todo, top_of_mind)"
            ),
            MemoryParamsError::FactMustBeString => {
                "fact must be a non-empty string when provided".to_string()
            }
            MemoryParamsError::MetadataMustBeJson => {
                "metadata must be a JSON object when provided".to_string()
            }
            MemoryParamsError::ConflictMustBeBoolean => {
                "conflict must be a boolean when provided".to_string()
            }
            MemoryParamsError::ConflictReasonMustBeString => {
                "conflict_reason must be a string when provided".to_string()
            }
            MemoryParamsError::EntityMustBeString => "entity filter must be a string".to_string(),
            MemoryParamsError::TopicMustBeString => "topic filter must be a string".to_string(),
            MemoryParamsError::QueryMustBeString => "query filter must be a string".to_string(),
            MemoryParamsError::IdentityRequired => {
                "identity must be a valid non-empty string".to_string()
            }
            MemoryParamsError::RealmMustBeString => {
                "realm must be a non-empty string when provided".to_string()
            }
            MemoryParamsError::MemoryIdRequired => {
                "memory_id must be a non-empty string".to_string()
            }
            MemoryParamsError::TitleRequired => "title must be a non-empty string".to_string(),
            MemoryParamsError::BodyRequired => "body must be a non-empty string".to_string(),
            MemoryParamsError::TagsMustBeArray => "tags must be an array when provided".to_string(),
            MemoryParamsError::TagMustBeString => {
                "tags must contain only non-empty strings".to_string()
            }
            MemoryParamsError::TitleTooLong => "title must be at most 200 bytes".to_string(),
            MemoryParamsError::BodyTooLong => "body must be at most 65536 bytes".to_string(),
            MemoryParamsError::TooManyTags => "tags must contain at most 32 entries".to_string(),
            MemoryParamsError::TagTooLong => "tags must be at most 64 bytes".to_string(),
            MemoryParamsError::SelectionMustBeString => {
                "selection must be 'always' or 'contextual' when provided".to_string()
            }
            MemoryParamsError::UnsupportedSelection(selection) => {
                format!("selection must be 'always' or 'contextual' (got '{selection}')")
            }
            MemoryParamsError::QueryTextMustBeString => {
                "query_text must be a non-empty string when provided".to_string()
            }
            MemoryParamsError::QueryTextTooLong => {
                "query_text must be at most 16384 bytes".to_string()
            }
            MemoryParamsError::QueryTermsMustBeArray => {
                "query_terms must be an array when provided".to_string()
            }
            MemoryParamsError::QueryTermMustBeString => {
                "query_terms must contain only non-empty strings".to_string()
            }
            MemoryParamsError::MaxEntriesMustBePositiveInteger => {
                "max_entries must be a positive integer when provided".to_string()
            }
            MemoryParamsError::MaxEntriesOutOfRange => {
                "max_entries must be between 1 and 64".to_string()
            }
            MemoryParamsError::TierMustBeString => {
                "tier must be 'working_set' or 'full' when provided".to_string()
            }
            MemoryParamsError::UnsupportedTier(tier) => {
                format!("tier must be 'working_set' or 'full' (got '{tier}')")
            }
            MemoryParamsError::KMustBePositiveInteger => {
                "k must be a positive integer when provided".to_string()
            }
            MemoryParamsError::KOutOfRange => {
                format!("k must be between 1 and {MAX_AGENT_MEMORY_MANIFEST_K}")
            }
            MemoryParamsError::KRequiresWorkingSetTier => {
                "k is only valid with tier 'working_set'".to_string()
            }
            MemoryParamsError::VerdictRequired => {
                "verdict must be 'release' or 'tombstone'".to_string()
            }
            MemoryParamsError::UnsupportedVerdict(verdict) => {
                format!("verdict must be 'release' or 'tombstone' (got '{verdict}')")
            }
            MemoryParamsError::ExpectedContentHashRequired => {
                "expected_content_hash must be the non-empty content_hash of the reviewed record"
                    .to_string()
            }
            MemoryParamsError::RationaleMustBeString => {
                "rationale must be a string when provided".to_string()
            }
            MemoryParamsError::RationaleTooLong => format!(
                "rationale must be at most {} bytes",
                crate::memory::review::MAX_REVIEW_RATIONALE_BYTES
            ),
            MemoryParamsError::Index(MemoryIndexError::EntityRequired) => {
                "entity must be a non-empty string".to_string()
            }
            MemoryParamsError::Index(MemoryIndexError::TopicRequired) => {
                "topic must be a non-empty string".to_string()
            }
            MemoryParamsError::Index(MemoryIndexError::UnsupportedStore(store)) => format!(
                "store '{store}' is unsupported (allowed: knowledge_graph, vector, timeline, todo, top_of_mind)"
            ),
            MemoryParamsError::Index(MemoryIndexError::FactRequiredWhenConflictUnset) => {
                "fact is required unless conflict=true".to_string()
            }
            MemoryParamsError::Index(MemoryIndexError::BackendPersistFailed(error)) => {
                format!(
                    "memory backend persistence failed: {}",
                    Self::backend_message(error)
                )
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AgentMemoryRememberRpcRequest {
    pub(crate) identity: AgentIdentity,
    pub(crate) realm: String,
    pub(crate) memory: NewAgentMemory,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AgentMemoryForgetRpcRequest {
    pub(crate) identity: AgentIdentity,
    pub(crate) realm: String,
    pub(crate) memory_id: String,
}

/// `meta/memory/quarantine/decide`: one review decision over a
/// quarantined record in `identity`'s scope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MemoryQuarantineDecideRpcRequest {
    pub(crate) identity: AgentIdentity,
    pub(crate) realm: String,
    pub(crate) memory_id: String,
    pub(crate) decision: crate::memory::review::QuarantineDecision,
    pub(crate) expected_content_hash: String,
    pub(crate) rationale: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AgentMemoryRecallRpcRequest {
    pub(crate) request: AgentMemoryRecallRequest,
}

/// `meta/agent_memory/update` — supersede within a record's lineage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AgentMemoryUpdateRpcRequest {
    pub(crate) identity: AgentIdentity,
    pub(crate) realm: String,
    pub(crate) memory_id: String,
    pub(crate) memory: NewAgentMemory,
}

/// `meta/agent_memory/manifest` — tiered metadata index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AgentMemoryManifestRpcRequest {
    pub(crate) identity: AgentIdentity,
    pub(crate) realm: String,
    pub(crate) tier: ManifestTier,
}

pub(super) fn parse_memory_stores_params(params: &Value) -> Result<(), MemoryParamsError> {
    if params.is_null() || params.is_object() {
        return Ok(());
    }
    Err(MemoryParamsError::ParamsMustBeObject)
}

pub(crate) fn parse_agent_memory_remember_params(
    params: &Value,
) -> Result<AgentMemoryRememberRpcRequest, MemoryParamsError> {
    let object = params
        .as_object()
        .ok_or(MemoryParamsError::ParamsMustBeObject)?;
    let identity = parse_agent_memory_identity(object)?;
    let realm = parse_agent_memory_realm(object)?;
    let title = object
        .get("title")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or(MemoryParamsError::TitleRequired)?
        .to_string();
    if title.len() > MAX_AGENT_MEMORY_TITLE_BYTES {
        return Err(MemoryParamsError::TitleTooLong);
    }
    let body = object
        .get("body")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or(MemoryParamsError::BodyRequired)?
        .to_string();
    if body.len() > MAX_AGENT_MEMORY_BODY_BYTES {
        return Err(MemoryParamsError::BodyTooLong);
    }
    let tags = match object.get("tags") {
        None => Vec::new(),
        Some(value) => {
            let tags = value.as_array().ok_or(MemoryParamsError::TagsMustBeArray)?;
            if tags.len() > MAX_AGENT_MEMORY_TAGS {
                return Err(MemoryParamsError::TooManyTags);
            }
            tags.iter()
                .map(|tag| {
                    let tag = tag
                        .as_str()
                        .ok_or(MemoryParamsError::TagMustBeString)?
                        .trim();
                    if tag.is_empty() {
                        return Err(MemoryParamsError::TagMustBeString);
                    }
                    if tag.len() > MAX_AGENT_MEMORY_TAG_BYTES {
                        return Err(MemoryParamsError::TagTooLong);
                    }
                    Ok(tag.to_string())
                })
                .collect::<Result<Vec<_>, _>>()?
        }
    };
    Ok(AgentMemoryRememberRpcRequest {
        identity,
        realm,
        memory: NewAgentMemory { title, body, tags },
    })
}

pub(crate) fn parse_agent_memory_update_params(
    params: &Value,
) -> Result<AgentMemoryUpdateRpcRequest, MemoryParamsError> {
    let object = params
        .as_object()
        .ok_or(MemoryParamsError::ParamsMustBeObject)?;
    let identity = parse_agent_memory_identity(object)?;
    let realm = parse_agent_memory_realm(object)?;
    let memory_id = object
        .get("memory_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or(MemoryParamsError::MemoryIdRequired)?
        .to_string();
    // Same title/body/tags shape and caps as remember, wire-compatibly.
    let remember = parse_agent_memory_remember_params(params)?;
    Ok(AgentMemoryUpdateRpcRequest {
        identity,
        realm,
        memory_id,
        memory: remember.memory,
    })
}

pub(crate) fn parse_agent_memory_manifest_params(
    params: &Value,
) -> Result<AgentMemoryManifestRpcRequest, MemoryParamsError> {
    let object = params
        .as_object()
        .ok_or(MemoryParamsError::ParamsMustBeObject)?;
    let identity = parse_agent_memory_identity(object)?;
    let realm = parse_agent_memory_realm(object)?;
    let tier_name = match object.get("tier") {
        None => "working_set".to_string(),
        Some(value) => {
            let tier = value
                .as_str()
                .ok_or(MemoryParamsError::TierMustBeString)?
                .trim()
                .to_ascii_lowercase();
            match tier.as_str() {
                "working_set" | "full" => tier,
                _ => return Err(MemoryParamsError::UnsupportedTier(tier)),
            }
        }
    };
    let k = match object.get("k") {
        None => None,
        Some(value) => {
            let k = value
                .as_u64()
                .filter(|k| *k > 0)
                .ok_or(MemoryParamsError::KMustBePositiveInteger)?;
            let k = usize::try_from(k).map_err(|_| MemoryParamsError::KOutOfRange)?;
            if k > MAX_AGENT_MEMORY_MANIFEST_K {
                return Err(MemoryParamsError::KOutOfRange);
            }
            Some(k)
        }
    };
    let tier = match tier_name.as_str() {
        "full" => {
            if k.is_some() {
                return Err(MemoryParamsError::KRequiresWorkingSetTier);
            }
            ManifestTier::Full
        }
        _ => ManifestTier::WorkingSet(k.unwrap_or(DEFAULT_AGENT_MEMORY_MANIFEST_K)),
    };
    Ok(AgentMemoryManifestRpcRequest {
        identity,
        realm,
        tier,
    })
}

pub(crate) fn parse_agent_memory_forget_params(
    params: &Value,
) -> Result<AgentMemoryForgetRpcRequest, MemoryParamsError> {
    let object = params
        .as_object()
        .ok_or(MemoryParamsError::ParamsMustBeObject)?;
    let identity = parse_agent_memory_identity(object)?;
    let realm = parse_agent_memory_realm(object)?;
    let memory_id = object
        .get("memory_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or(MemoryParamsError::MemoryIdRequired)?
        .to_string();
    Ok(AgentMemoryForgetRpcRequest {
        identity,
        realm,
        memory_id,
    })
}

pub(crate) fn parse_memory_quarantine_decide_params(
    params: &Value,
) -> Result<MemoryQuarantineDecideRpcRequest, MemoryParamsError> {
    let object = params
        .as_object()
        .ok_or(MemoryParamsError::ParamsMustBeObject)?;
    let identity = parse_agent_memory_identity(object)?;
    let realm = parse_agent_memory_realm(object)?;
    let memory_id = object
        .get("memory_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or(MemoryParamsError::MemoryIdRequired)?
        .to_string();
    let verdict = object
        .get("verdict")
        .and_then(Value::as_str)
        .map(str::trim)
        .ok_or(MemoryParamsError::VerdictRequired)?;
    let decision = crate::memory::review::QuarantineDecision::parse(verdict)
        .ok_or_else(|| MemoryParamsError::UnsupportedVerdict(verdict.to_string()))?;
    let expected_content_hash = object
        .get("expected_content_hash")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or(MemoryParamsError::ExpectedContentHashRequired)?
        .to_string();
    let rationale = match object.get("rationale") {
        None | Some(Value::Null) => None,
        Some(value) => {
            let rationale = value
                .as_str()
                .ok_or(MemoryParamsError::RationaleMustBeString)?
                .trim();
            if rationale.len() > crate::memory::review::MAX_REVIEW_RATIONALE_BYTES {
                return Err(MemoryParamsError::RationaleTooLong);
            }
            (!rationale.is_empty()).then(|| rationale.to_string())
        }
    };
    Ok(MemoryQuarantineDecideRpcRequest {
        identity,
        realm,
        memory_id,
        decision,
        expected_content_hash,
        rationale,
    })
}

pub(crate) fn parse_agent_memory_recall_params(
    params: &Value,
) -> Result<AgentMemoryRecallRpcRequest, MemoryParamsError> {
    let object = params
        .as_object()
        .ok_or(MemoryParamsError::ParamsMustBeObject)?;
    let identity = parse_agent_memory_identity(object)?;
    let realm = parse_agent_memory_realm(object)?;
    let selection = match object.get("selection") {
        None => AgentMemorySelection::Contextual,
        Some(value) => {
            let selection = value
                .as_str()
                .ok_or(MemoryParamsError::SelectionMustBeString)?
                .trim()
                .to_ascii_lowercase();
            match selection.as_str() {
                "always" => AgentMemorySelection::Always,
                "contextual" => AgentMemorySelection::Contextual,
                _ => return Err(MemoryParamsError::UnsupportedSelection(selection)),
            }
        }
    };
    let max_entries = match object.get("max_entries") {
        None => DEFAULT_AGENT_MEMORY_MAX_ENTRIES,
        Some(value) => {
            let entries = value
                .as_u64()
                .ok_or(MemoryParamsError::MaxEntriesMustBePositiveInteger)?;
            if entries == 0 {
                return Err(MemoryParamsError::MaxEntriesMustBePositiveInteger);
            }
            let entries =
                usize::try_from(entries).map_err(|_| MemoryParamsError::MaxEntriesOutOfRange)?;
            if entries > MAX_AGENT_MEMORY_MAX_ENTRIES {
                return Err(MemoryParamsError::MaxEntriesOutOfRange);
            }
            entries
        }
    };
    let query_terms = match object.get("query_terms") {
        None => Vec::new(),
        Some(value) => value
            .as_array()
            .ok_or(MemoryParamsError::QueryTermsMustBeArray)?
            .iter()
            .map(|term| {
                let term = term
                    .as_str()
                    .ok_or(MemoryParamsError::QueryTermMustBeString)?
                    .trim();
                if term.is_empty() {
                    return Err(MemoryParamsError::QueryTermMustBeString);
                }
                Ok(term.to_string())
            })
            .collect::<Result<Vec<_>, _>>()?,
    };
    let query_text = match object.get("query_text") {
        None => None,
        Some(value) => {
            let query_text = value
                .as_str()
                .ok_or(MemoryParamsError::QueryTextMustBeString)?
                .trim();
            if query_text.is_empty() {
                return Err(MemoryParamsError::QueryTextMustBeString);
            }
            if query_text.len() > MAX_AGENT_MEMORY_QUERY_TEXT_BYTES {
                return Err(MemoryParamsError::QueryTextTooLong);
            }
            Some(query_text.to_string())
        }
    };

    Ok(AgentMemoryRecallRpcRequest {
        request: AgentMemoryRecallRequest {
            identity,
            realm,
            query_text,
            query_terms,
            selection,
            max_entries,
        },
    })
}

fn parse_agent_memory_identity(
    object: &serde_json::Map<String, Value>,
) -> Result<AgentIdentity, MemoryParamsError> {
    object
        .get("identity")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or(MemoryParamsError::IdentityRequired)
        .and_then(|value| {
            AgentIdentity::parse(value).map_err(|_| MemoryParamsError::IdentityRequired)
        })
}

fn parse_agent_memory_realm(
    object: &serde_json::Map<String, Value>,
) -> Result<String, MemoryParamsError> {
    match object.get("realm") {
        None => Ok("default".to_string()),
        Some(value) => {
            let realm = value
                .as_str()
                .ok_or(MemoryParamsError::RealmMustBeString)?
                .trim();
            if realm.is_empty() {
                return Err(MemoryParamsError::RealmMustBeString);
            }
            Ok(realm.to_string())
        }
    }
}

fn parse_memory_store_field(value: &Value) -> Result<String, MemoryParamsError> {
    let store = value.as_str().ok_or(MemoryParamsError::StoreMustBeString)?;
    let canonical = store.trim().to_ascii_lowercase();
    if canonical.is_empty() {
        return Err(MemoryParamsError::StoreMustBeString);
    }
    if MEMORY_SUPPORTED_STORES.contains(&canonical.as_str()) {
        Ok(canonical)
    } else {
        Err(MemoryParamsError::UnsupportedStore(canonical))
    }
}

pub(super) fn parse_memory_index_params(
    params: &Value,
) -> Result<MemoryIndexRequest, MemoryParamsError> {
    let object = params
        .as_object()
        .ok_or(MemoryParamsError::ParamsMustBeObject)?;
    let entity = object
        .get("entity")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or(MemoryParamsError::EntityRequired)?;
    let topic = object
        .get("topic")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or(MemoryParamsError::TopicRequired)?;
    let store = match object.get("store") {
        None => None,
        Some(value) => Some(parse_memory_store_field(value)?),
    };
    let fact = match object.get("fact") {
        None => None,
        Some(value) => Some(
            value
                .as_str()
                .ok_or(MemoryParamsError::FactMustBeString)?
                .trim()
                .to_string(),
        ),
    };
    if fact.as_deref().is_some_and(str::is_empty) {
        return Err(MemoryParamsError::FactMustBeString);
    }
    let metadata = match object.get("metadata") {
        None => None,
        Some(value) => {
            if !value.is_object() {
                return Err(MemoryParamsError::MetadataMustBeJson);
            }
            Some(value.clone())
        }
    };
    let conflict = match object.get("conflict") {
        None => None,
        Some(value) => Some(
            value
                .as_bool()
                .ok_or(MemoryParamsError::ConflictMustBeBoolean)?,
        ),
    };
    let conflict_reason = match object.get("conflict_reason") {
        None => None,
        Some(value) => Some(
            value
                .as_str()
                .ok_or(MemoryParamsError::ConflictReasonMustBeString)?
                .to_string(),
        ),
    };

    Ok(MemoryIndexRequest {
        entity: entity.to_string(),
        topic: topic.to_string(),
        store,
        fact,
        metadata,
        conflict,
        conflict_reason,
    })
}

pub(super) fn parse_memory_query_params(
    params: &Value,
) -> Result<MemoryQueryRequest, MemoryParamsError> {
    if params.is_null() {
        return Ok(MemoryQueryRequest {
            entity: None,
            topic: None,
            store: None,
            query: None,
        });
    }
    let object = params
        .as_object()
        .ok_or(MemoryParamsError::ParamsMustBeObject)?;
    let entity = match object.get("entity") {
        None => None,
        Some(value) => Some(
            value
                .as_str()
                .ok_or(MemoryParamsError::EntityMustBeString)?
                .to_string(),
        ),
    };
    let topic = match object.get("topic") {
        None => None,
        Some(value) => Some(
            value
                .as_str()
                .ok_or(MemoryParamsError::TopicMustBeString)?
                .to_string(),
        ),
    };
    let store = match object.get("store") {
        None => None,
        Some(value) => Some(parse_memory_store_field(value)?),
    };
    let query = match object.get("query") {
        None => None,
        Some(value) => Some(
            value
                .as_str()
                .ok_or(MemoryParamsError::QueryMustBeString)?
                .to_string(),
        ),
    };
    Ok(MemoryQueryRequest {
        entity,
        topic,
        store,
        query,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::error::Error;

    #[test]
    fn agent_memory_recall_defaults_to_contextual_selection() -> Result<(), Box<dyn Error>> {
        let parsed = parse_agent_memory_recall_params(&json!({
            "identity": "identity:luka",
            "query_text": "Where is my passport?",
            "query_terms": ["passport"]
        }))
        .map_err(|err| std::io::Error::other(err.message()))?;

        assert_eq!(parsed.request.selection, AgentMemorySelection::Contextual);
        assert_eq!(parsed.request.realm, "default");
        assert_eq!(
            parsed.request.query_text,
            Some("Where is my passport?".to_string())
        );
        assert_eq!(parsed.request.query_terms, vec!["passport".to_string()]);
        Ok(())
    }

    #[test]
    fn memory_quarantine_decide_params_bind_scope_verdict_and_content() -> Result<(), Box<dyn Error>>
    {
        let parsed = parse_memory_quarantine_decide_params(&json!({
            "identity": " lead:main ",
            "memory_id": "mem-1",
            "verdict": "release",
            "expected_content_hash": "abc123",
            "rationale": "  confirmed with the reader  "
        }))
        .map_err(|err| std::io::Error::other(err.message()))?;
        assert_eq!(parsed.identity.as_str(), "lead:main");
        assert_eq!(parsed.realm, "default");
        assert_eq!(parsed.memory_id, "mem-1");
        assert_eq!(
            parsed.decision,
            crate::memory::review::QuarantineDecision::Release
        );
        assert_eq!(parsed.expected_content_hash, "abc123");
        assert_eq!(
            parsed.rationale.as_deref(),
            Some("confirmed with the reader")
        );

        let base = json!({
            "identity": "lead:main",
            "memory_id": "mem-1",
            "verdict": "tombstone",
            "expected_content_hash": "abc123"
        });
        let with = |key: &str, value: serde_json::Value| {
            let mut params = base.clone();
            params[key] = value;
            parse_memory_quarantine_decide_params(&params).err()
        };
        assert_eq!(
            parse_memory_quarantine_decide_params(&base)
                .map_err(|err| std::io::Error::other(err.message()))?
                .rationale,
            None
        );
        assert_eq!(
            with("verdict", json!("hold")),
            Some(MemoryParamsError::UnsupportedVerdict("hold".to_string()))
        );
        assert_eq!(
            with("verdict", json!(1)),
            Some(MemoryParamsError::VerdictRequired)
        );
        assert_eq!(
            with("expected_content_hash", json!(" ")),
            Some(MemoryParamsError::ExpectedContentHashRequired)
        );
        assert_eq!(
            with("rationale", json!(7)),
            Some(MemoryParamsError::RationaleMustBeString)
        );
        assert_eq!(
            with(
                "rationale",
                json!("r".repeat(crate::memory::review::MAX_REVIEW_RATIONALE_BYTES + 1))
            ),
            Some(MemoryParamsError::RationaleTooLong)
        );
        assert_eq!(
            with("memory_id", json!("")),
            Some(MemoryParamsError::MemoryIdRequired)
        );
        Ok(())
    }

    #[test]
    fn agent_memory_recall_rejects_empty_query_text() {
        let err = parse_agent_memory_recall_params(&json!({
            "identity": "identity:luka",
            "query_text": "   "
        }))
        .err();

        assert_eq!(err, Some(MemoryParamsError::QueryTextMustBeString));
    }

    #[test]
    fn agent_memory_recall_preserves_explicit_always_selection() -> Result<(), Box<dyn Error>> {
        let parsed = parse_agent_memory_recall_params(&json!({
            "identity": "identity:luka",
            "selection": "always"
        }))
        .map_err(|err| std::io::Error::other(err.message()))?;

        assert_eq!(parsed.request.selection, AgentMemorySelection::Always);
        Ok(())
    }

    #[test]
    fn memory_query_params_accept_legacy_query_string() -> Result<(), Box<dyn Error>> {
        let parsed = parse_memory_query_params(&json!({
            "query": "recipient consent",
            "store": "todo"
        }))
        .map_err(|err| std::io::Error::other(err.message()))?;

        assert_eq!(parsed.query, Some("recipient consent".to_string()));
        assert_eq!(parsed.store, Some("todo".to_string()));
        assert_eq!(parsed.entity, None);
        assert_eq!(parsed.topic, None);
        Ok(())
    }

    #[test]
    fn memory_query_params_reject_non_string_query() {
        let err = parse_memory_query_params(&json!({ "query": 42 })).err();
        assert_eq!(err, Some(MemoryParamsError::QueryMustBeString));
    }

    #[test]
    fn agent_memory_update_parses_full_shape() -> Result<(), Box<dyn Error>> {
        let parsed = parse_agent_memory_update_params(&json!({
            "identity": "identity:luka",
            "realm": "team",
            "memory_id": "mem-1",
            "title": "Weekly standup",
            "body": "Corrected standup time.",
            "tags": ["team"]
        }))
        .map_err(|err| std::io::Error::other(err.message()))?;

        assert_eq!(parsed.identity.as_str(), "identity:luka");
        assert_eq!(parsed.realm, "team");
        assert_eq!(parsed.memory_id, "mem-1");
        assert_eq!(parsed.memory.title, "Weekly standup");
        assert_eq!(parsed.memory.body, "Corrected standup time.");
        assert_eq!(parsed.memory.tags, vec!["team".to_string()]);
        Ok(())
    }

    #[test]
    fn agent_memory_update_requires_memory_id_and_body() {
        let missing_id = parse_agent_memory_update_params(&json!({
            "identity": "identity:luka",
            "title": "T",
            "body": "B"
        }))
        .err();
        assert_eq!(missing_id, Some(MemoryParamsError::MemoryIdRequired));

        let missing_body = parse_agent_memory_update_params(&json!({
            "identity": "identity:luka",
            "memory_id": "mem-1",
            "title": "T"
        }))
        .err();
        assert_eq!(missing_body, Some(MemoryParamsError::BodyRequired));
    }

    #[test]
    fn agent_memory_manifest_defaults_to_working_set() -> Result<(), Box<dyn Error>> {
        let parsed = parse_agent_memory_manifest_params(&json!({
            "identity": "identity:luka"
        }))
        .map_err(|err| std::io::Error::other(err.message()))?;

        assert_eq!(parsed.realm, "default");
        assert_eq!(
            parsed.tier,
            ManifestTier::WorkingSet(DEFAULT_AGENT_MEMORY_MANIFEST_K)
        );

        let with_k = parse_agent_memory_manifest_params(&json!({
            "identity": "identity:luka",
            "tier": "working_set",
            "k": 4
        }))
        .map_err(|err| std::io::Error::other(err.message()))?;
        assert_eq!(with_k.tier, ManifestTier::WorkingSet(4));

        let full = parse_agent_memory_manifest_params(&json!({
            "identity": "identity:luka",
            "tier": "full"
        }))
        .map_err(|err| std::io::Error::other(err.message()))?;
        assert_eq!(full.tier, ManifestTier::Full);
        Ok(())
    }

    #[test]
    fn agent_memory_manifest_rejects_bad_tier_and_k() {
        let bad_tier = parse_agent_memory_manifest_params(&json!({
            "identity": "identity:luka",
            "tier": "vector"
        }))
        .err();
        assert_eq!(
            bad_tier,
            Some(MemoryParamsError::UnsupportedTier("vector".to_string()))
        );

        let zero_k = parse_agent_memory_manifest_params(&json!({
            "identity": "identity:luka",
            "k": 0
        }))
        .err();
        assert_eq!(zero_k, Some(MemoryParamsError::KMustBePositiveInteger));

        let huge_k = parse_agent_memory_manifest_params(&json!({
            "identity": "identity:luka",
            "k": 100000
        }))
        .err();
        assert_eq!(huge_k, Some(MemoryParamsError::KOutOfRange));

        let k_with_full = parse_agent_memory_manifest_params(&json!({
            "identity": "identity:luka",
            "tier": "full",
            "k": 8
        }))
        .err();
        assert_eq!(
            k_with_full,
            Some(MemoryParamsError::KRequiresWorkingSetTier)
        );
    }

    #[test]
    fn agent_memory_forget_requires_memory_id() {
        let err = parse_agent_memory_forget_params(&json!({
            "identity": "identity:luka"
        }))
        .err();

        assert_eq!(err, Some(MemoryParamsError::MemoryIdRequired));
    }

    #[test]
    fn agent_memory_forget_parses_identity_realm_and_memory_id() -> Result<(), Box<dyn Error>> {
        let parsed = parse_agent_memory_forget_params(&json!({
            "identity": "identity:luka",
            "realm": "team",
            "memory_id": "mem-1"
        }))
        .map_err(|err| std::io::Error::other(err.message()))?;

        assert_eq!(parsed.identity.as_str(), "identity:luka");
        assert_eq!(parsed.realm, "team");
        assert_eq!(parsed.memory_id, "mem-1");
        Ok(())
    }
}
