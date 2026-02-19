use crate::model::{MobRunStatus, MobSpecRevision};
use meerkat::SessionError;
use serde_json::Value;
use std::io;

pub type MobResult<T> = Result<T, MobError>;

#[derive(Debug, thiserror::Error)]
pub enum MobError {
    #[error("spec parse failed: {0}")]
    SpecParse(String),

    #[error("spec validation failed: {0}")]
    SpecValidation(String),

    #[error("spec not found: {mob_id}")]
    SpecNotFound { mob_id: String },

    #[error(
        "spec revision conflict for mob '{mob_id}': expected {expected:?}, current {current:?}"
    )]
    SpecRevisionConflict {
        mob_id: String,
        expected: Option<MobSpecRevision>,
        current: Option<MobSpecRevision>,
    },

    #[error("flow not found: {flow_id}")]
    FlowNotFound { flow_id: String },

    #[error("run not found: {run_id}")]
    RunNotFound { run_id: String },

    #[error("run canceled: {run_id}")]
    RunCanceled { run_id: String },

    #[error("invalid run transition for {run_id}: {from:?} -> {to:?}")]
    InvalidRunTransition {
        run_id: String,
        from: MobRunStatus,
        to: MobRunStatus,
    },

    #[error("reset barrier tripped for mob '{mob_id}': expected epoch {expected}, current {current}")]
    ResetBarrier {
        mob_id: String,
        expected: u64,
        current: u64,
    },

    #[error("store error: {0}")]
    Store(#[source] Box<dyn std::error::Error + Send + Sync>),

    #[error("session error: {0}")]
    Session(#[from] SessionError),

    #[error("comms error: {0}")]
    Comms(String),

    #[error("dispatch timeout after {timeout_ms}ms")]
    DispatchTimeout { timeout_ms: u64 },

    #[error("resolver not found: {resolver_id}")]
    ResolverNotFound { resolver_id: String },

    #[error("resolver failed: {0}")]
    Resolver(String),

    #[error("tool bundle failed: {0}")]
    ToolBundle(String),

    #[error("schema validation failed: {0}")]
    SchemaValidation(String),

    #[error("runtime root missing")]
    RuntimeRootMissing,

    #[error("context root missing")]
    ContextRootMissing,

    #[error("internal error: {0}")]
    Internal(String),
}

impl MobError {
    pub fn validation(field: &str, message: &str) -> Self {
        Self::SpecValidation(format!("{field}: {message}"))
    }

    pub fn with_data(self, data: Value) -> Self {
        Self::Internal(format!("{} | data={}", self, data))
    }

    pub fn store(message: impl Into<String>) -> Self {
        Self::Store(Box::new(io::Error::other(message.into())))
    }
}
