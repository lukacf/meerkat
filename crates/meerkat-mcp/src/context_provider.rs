//! Process-local host preparation for one call to an exact MCP destination.

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use meerkat_core::types::ToolCallView;
use meerkat_core::{McpServerConfig, ToolDispatchContext, ToolMutationClass};
use serde_json::{Map, Value};

/// One physical connection generation, never a durable agent identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct McpConnectionId(u64);

impl McpConnectionId {
    pub(crate) fn allocate() -> Result<Self, McpCallContextError> {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        NEXT.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |next| {
            next.checked_add(1)
        })
        .map(Self)
        .map_err(|_| McpCallContextError::Unavailable)
    }
}

impl fmt::Display for McpConnectionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Exact connected destination and raw server operation. Config may contain
/// credentials, so this host-only value deliberately has no Debug projection.
pub struct McpCallTarget<'a> {
    pub config: &'a McpServerConfig,
    pub connection_id: McpConnectionId,
    pub raw_operation: &'a str,
    /// The same native projection inserted into the wire metadata.
    pub origin: &'a meerkat_core::WireCallOrigin,
}

/// Trusted process-local preparation, separate from serialized agent config.
/// A provider must select the complete destination, not its display name.
/// `None` leaves an unselected call's wire payload unchanged. An error refuses
/// the call before transport. Context is the current dispatch context; it is
/// never captured from the agent that first constructed a shared connection.
#[async_trait]
pub trait McpCallContextProvider: Send + Sync {
    async fn prepare(
        &self,
        target: McpCallTarget<'_>,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
    ) -> Result<Option<McpCallContext>, McpCallContextError>;

    /// Host declaration for this exact destination and raw operation. Server
    /// annotations and display names never establish read-only authority.
    fn tool_mutation_class(
        &self,
        _config: &McpServerConfig,
        _raw_operation: &str,
    ) -> ToolMutationClass {
        ToolMutationClass::Unknown
    }
}

/// Metadata plus a call-lifetime lease. Dropping the call future drops the
/// lease, including on cancellation or refusal. Neither part is logged.
pub struct McpCallContext {
    pub(crate) metadata: Map<String, Value>,
    pub(crate) guard: Box<dyn Send + Sync>,
}

impl McpCallContext {
    pub fn new(metadata: Map<String, Value>, guard: impl Send + Sync + 'static) -> Self {
        Self {
            metadata,
            guard: Box::new(guard),
        }
    }
}

impl fmt::Debug for McpCallContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("McpCallContext").finish_non_exhaustive()
    }
}

/// Fixed diagnostics never include provider-owned values or an error chain.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum McpCallContextError {
    #[error("MCP call context unavailable")]
    Unavailable,
    #[error("MCP call context denied")]
    Denied,
}
