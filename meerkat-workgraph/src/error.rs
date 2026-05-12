use crate::types::{WorkItemId, WorkNamespace};

#[derive(Debug, thiserror::Error)]
pub enum WorkGraphError {
    #[error("work item {id} not found in realm '{realm_id}' namespace '{namespace}'")]
    NotFound {
        realm_id: String,
        namespace: WorkNamespace,
        id: WorkItemId,
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
}
