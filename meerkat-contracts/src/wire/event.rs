//! Wire event envelope.

use serde::{Deserialize, Serialize};

use crate::version::ContractVersion;
use meerkat_core::{AgentEvent, SessionId};

/// Canonical event envelope for wire protocol.
///
/// Wraps an [`AgentEvent`] with session context and contract version.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WireEvent {
    pub session_id: SessionId,
    pub sequence: u64,
    pub event: AgentEvent,
    pub contract_version: ContractVersion,
}
