use crate::types::{WorkAttentionBindingId, WorkItemId, WorkNamespace};

#[derive(Debug, thiserror::Error)]
pub enum WorkGraphError {
    #[error("work item {id} not found in realm '{realm_id}' namespace '{namespace}'")]
    NotFound {
        realm_id: String,
        namespace: WorkNamespace,
        id: WorkItemId,
    },
    #[error(
        "work attention binding {binding_id} not found in realm '{realm_id}' namespace '{namespace}'"
    )]
    AttentionNotFound {
        realm_id: String,
        namespace: WorkNamespace,
        binding_id: WorkAttentionBindingId,
    },
    #[error("stale work item revision for {id}: expected {expected}, actual {actual}")]
    StaleRevision {
        id: WorkItemId,
        expected: u64,
        actual: u64,
    },
    #[error("conflicting work graph mutation: {0}")]
    Conflict(String),
    #[error("invalid work graph transition: {0}")]
    InvalidTransition(String),
    #[error("invalid work graph input: {0}")]
    InvalidInput(String),
    #[error("work graph timestamp `{field}` cannot be represented as unsigned millis: {millis}")]
    InvalidTimestampMillis { field: &'static str, millis: i64 },
    #[error("work graph store error: {0}")]
    Store(String),
    #[error("work graph backend '{0}' does not support this operation")]
    UnsupportedBackend(String),
}

impl WorkGraphError {
    pub fn not_found(realm_id: String, namespace: WorkNamespace, id: WorkItemId) -> Self {
        Self::NotFound {
            realm_id,
            namespace,
            id,
        }
    }

    pub fn attention_not_found(
        realm_id: String,
        namespace: WorkNamespace,
        binding_id: WorkAttentionBindingId,
    ) -> Self {
        Self::AttentionNotFound {
            realm_id,
            namespace,
            binding_id,
        }
    }
}
