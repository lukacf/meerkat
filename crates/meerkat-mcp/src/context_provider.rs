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
    /// The generation number, for a nonsecret operation target fact.
    pub(crate) const fn generation(self) -> u64 {
        self.0
    }

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

/// A resource read on the exact existing MCP connection. It is not a tool
/// invocation and never borrows a tool's prepared metadata or approval.
pub struct McpResourceTarget<'a> {
    pub config: &'a McpServerConfig,
    pub connection_id: McpConnectionId,
    pub uri: &'a str,
}

/// Trusted process-local preparation, separate from serialized agent config.
/// A provider must select the complete destination, not its display name.
/// `None` leaves an unselected call's wire payload unchanged. An error refuses
/// the call before transport. Context is the current dispatch context; it is
/// never captured from the agent that first constructed a shared connection.
///
/// Core owns some `_meta` names: `progressToken`,
/// [`meerkat_core::CALL_ORIGIN_META_KEY`] and
/// [`meerkat_core::TOOL_CALL_DEDUPE_KEY_META_KEY`], the call's dedupe key,
/// which Core sends itself beside the prepared metadata on every call that
/// has one. A preparation whose metadata names any of them is refused
/// ([`McpCallContextError::Denied`]) before transport.
#[async_trait]
pub trait McpCallContextProvider: Send + Sync {
    /// Hosts that install call preparation must explicitly opt resource reads
    /// into their credential and member policy. No tool-shaped fallback exists.
    async fn prepare_resource(
        &self,
        _target: McpResourceTarget<'_>,
        _context: &ToolDispatchContext,
    ) -> Result<Option<McpCallContext>, McpCallContextError> {
        Err(McpCallContextError::Unavailable)
    }

    /// Actual native resource authority for governed reads. A server name is
    /// not a principal; only the configured host can supply these coordinates.
    fn resource_authorization_target(
        &self,
        _target: McpResourceTarget<'_>,
    ) -> Option<meerkat_core::authorization::OwnerQualifiedTarget> {
        None
    }

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

/// What a passed final check keeps held through the call's native entry and
/// local handoff (it travels with the request to the send owner).
pub type McpCallFinalCustody = Box<dyn Send + Sync>;

/// A synchronous check run at the call's final native entry.
pub type McpCallFinalCheck =
    Box<dyn FnOnce() -> Result<McpCallFinalCustody, McpCallContextError> + Send + Sync>;

/// Metadata plus a call-lifetime lease. Dropping the call future drops the
/// lease, including on cancellation or refusal. Neither part is logged.
pub struct McpCallContext {
    pub(crate) metadata: Map<String, Value>,
    pub(crate) guard: Box<dyn Send + Sync>,
    pub(crate) final_check: Option<McpCallFinalCheck>,
}

impl McpCallContext {
    pub fn new(metadata: Map<String, Value>, guard: impl Send + Sync + 'static) -> Self {
        Self {
            metadata,
            guard: Box::new(guard),
            final_check: None,
        }
    }

    /// Run `check` at the call's final native entry, before the single
    /// consuming reviewed entry, with no await before the request is handed
    /// to the transport. A refusal there sends nothing and spends no review.
    /// What a passed check returns stays held through the reviewed entry and
    /// the local handoff, travelling with the request to the send owner, so
    /// what it guards (for example a credential's lifecycle) cannot change
    /// before the request has left the caller.
    #[must_use]
    pub fn with_final_check<C: Send + Sync + 'static>(
        mut self,
        check: impl FnOnce() -> Result<C, McpCallContextError> + Send + Sync + 'static,
    ) -> Self {
        self.final_check = Some(Box::new(move || {
            check().map(|custody| Box::new(custody) as McpCallFinalCustody)
        }));
        self
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
