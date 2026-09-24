//! Runtime impl of [`meerkat_core::handles::McpServerLifecycleHandle`].
//!
//! Routes per-server MCP handshake events into the session's MeerkatMachine
//! DSL (`mcp_server_states` substate) and exposes the `PendingConnect` set
//! for the agent loop's `[MCP_PENDING]` system-notice toggle.

use std::collections::BTreeSet;
use std::sync::Arc;

use meerkat_core::handles::{DslTransitionError, McpServerLifecycleHandle};

use super::HandleDslAuthority;
use crate::meerkat_machine::dsl as mm_dsl;

/// Runtime-backed [`McpServerLifecycleHandle`] impl.
///
/// Routes every trait method to the corresponding DSL input on the session's
/// shared MeerkatMachine DSL authority.
#[derive(Debug)]
pub struct RuntimeMcpServerLifecycleHandle {
    dsl: Arc<HandleDslAuthority>,
}

impl RuntimeMcpServerLifecycleHandle {
    /// Construct a handle backed by the session's shared DSL authority.
    pub fn new(dsl: Arc<HandleDslAuthority>) -> Self {
        Self { dsl }
    }

    /// Construct a handle backed by an ephemeral DSL authority.
    pub fn ephemeral() -> Self {
        Self::new(Arc::new(HandleDslAuthority::ephemeral()))
    }
}

impl McpServerLifecycleHandle for RuntimeMcpServerLifecycleHandle {
    fn apply_connect_pending(&self, server_id: &str) -> Result<(), DslTransitionError> {
        // intra-machine: no route; dispatcher not applicable (handle targets the meerkat DSL directly, not a CompositionDispatcher seam)
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::McpServerConnectPending {
                server_id: mm_dsl::McpServerId::from(server_id.to_string()),
            },
            "McpServerLifecycleHandle::apply_connect_pending",
        )
    }

    fn apply_connected(&self, server_id: &str) -> Result<(), DslTransitionError> {
        // intra-machine: no route; dispatcher not applicable (handle targets the meerkat DSL directly, not a CompositionDispatcher seam)
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::McpServerConnected {
                server_id: mm_dsl::McpServerId::from(server_id.to_string()),
            },
            "McpServerLifecycleHandle::apply_connected",
        )
    }

    fn apply_failed(&self, server_id: &str, error: &str) -> Result<(), DslTransitionError> {
        // intra-machine: no route; dispatcher not applicable (handle targets the meerkat DSL directly, not a CompositionDispatcher seam)
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::McpServerFailed {
                server_id: mm_dsl::McpServerId::from(server_id.to_string()),
                error: error.to_string(),
            },
            "McpServerLifecycleHandle::apply_failed",
        )
    }

    fn apply_disconnected(&self, server_id: &str) -> Result<(), DslTransitionError> {
        // intra-machine: no route; dispatcher not applicable (handle targets the meerkat DSL directly, not a CompositionDispatcher seam)
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::McpServerDisconnected {
                server_id: mm_dsl::McpServerId::from(server_id.to_string()),
            },
            "McpServerLifecycleHandle::apply_disconnected",
        )
    }

    fn apply_reload(&self, server_id: &str) -> Result<(), DslTransitionError> {
        // intra-machine: no route; dispatcher not applicable (handle targets the meerkat DSL directly, not a CompositionDispatcher seam)
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::McpServerReload {
                server_id: mm_dsl::McpServerId::from(server_id.to_string()),
            },
            "McpServerLifecycleHandle::apply_reload",
        )
    }

    fn pending_server_ids(&self) -> BTreeSet<String> {
        self.dsl
            .snapshot_state()
            .mcp_server_states
            .into_iter()
            .filter_map(|(server_id, state)| {
                if matches!(state, mm_dsl::McpServerState::PendingConnect) {
                    Some(server_id.0)
                } else {
                    None
                }
            })
            .collect()
    }
}
