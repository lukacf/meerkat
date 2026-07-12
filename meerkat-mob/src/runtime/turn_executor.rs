use crate::error::MobError;
use crate::event::RemoteTurnObligationEvent;
use crate::ids::{AgentIdentity, FenceToken, Generation, RunId, StepId};
#[cfg(target_arch = "wasm32")]
use crate::tokio;
use async_trait::async_trait;
use meerkat_core::service::TurnToolOverlay;
use meerkat_core::types::ContentInput;
use serde_json::Value;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, oneshot};
use tokio::task::JoinHandle;

pub struct ActorTurnTicket {
    pub(crate) run_id: RunId,
    pub(crate) completion_rx: Mutex<Option<oneshot::Receiver<FlowTurnOutcome>>>,
    pub(crate) bridge_handle: Mutex<Option<JoinHandle<()>>>,
}

/// Ticket for a REMOTE (placed-member) flow turn (§18 O1/O2, phase 6).
///
/// The terminal is resolved by the `RemoteFlowTicketRegistry` — fed by the
/// poll pump with the member's durable event rows and the journal
/// `turn_outcomes` sidecar — never by a local subscription bridge; there is
/// no `bridge_handle` to join, the watcher IS the registry.
pub struct RemoteTurnTicket {
    pub(crate) run_id: RunId,
    pub(crate) step_id: StepId,
    pub(crate) target: AgentIdentity,
    pub(crate) host_id: String,
    pub(crate) host_binding_generation: u64,
    pub(crate) member_session_id: String,
    pub(crate) generation: Generation,
    pub(crate) fence_token: FenceToken,
    pub(crate) dispatch_sequence: u64,
    /// The delivery / journal / obligation key (caller-minted uuid v7).
    pub(crate) input_id: String,
    pub(crate) completion_rx: Mutex<Option<oneshot::Receiver<FlowTurnOutcome>>>,
}

#[derive(Clone)]
pub enum FlowTurnTicket {
    Actor(Arc<ActorTurnTicket>),
    Remote(Arc<RemoteTurnTicket>),
}

impl FlowTurnTicket {
    #[must_use]
    pub(crate) fn remote_obligation(&self) -> Option<RemoteTurnObligationEvent> {
        match self {
            Self::Actor(_) => None,
            Self::Remote(ticket) => Some(RemoteTurnObligationEvent {
                agent_identity: ticket.target.clone(),
                host_id: ticket.host_id.clone(),
                host_binding_generation: ticket.host_binding_generation,
                member_session_id: ticket.member_session_id.clone(),
                generation: ticket.generation,
                fence_token: ticket.fence_token,
                dispatch_sequence: ticket.dispatch_sequence,
                input_id: ticket.input_id.clone(),
                run_id: ticket.run_id.clone(),
                step_id: ticket.step_id.clone(),
            }),
        }
    }
}

impl fmt::Debug for FlowTurnTicket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Actor(_) => f.write_str("FlowTurnTicket::Actor(..)"),
            Self::Remote(ticket) => f
                .debug_struct("FlowTurnTicket::Remote")
                .field("target", &ticket.target)
                .field("input_id", &ticket.input_id)
                .finish_non_exhaustive(),
        }
    }
}

#[derive(Debug)]
pub enum FlowTurnOutcome {
    Completed {
        output: String,
        structured_output: Option<Value>,
    },
    Failed {
        reason: String,
        disposition: FlowTurnFailureDisposition,
    },
    Canceled,
}

/// Proof carried by a failed attempt. Only these two classes may consume a
/// retry budget: an observed host/local terminal, or an authenticated
/// rejection whose pre-admission/cancel path certifies no effect. Ambiguous
/// transport loss never becomes a `Failed` outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FlowTurnFailureDisposition {
    ObservedTerminal,
    CertifiedNoEffect {
        cause: meerkat_contracts::wire::supervisor_bridge::BridgeDeliveryRejectionCause,
    },
}

/// The shared meerkat-core classifier payload lowers 1:1 onto the flow-turn
/// outcome — the trivial From DEC-P6F-8's consumer #1 rides.
impl From<meerkat_core::turn_terminal::TurnTerminalOutcome> for FlowTurnOutcome {
    fn from(outcome: meerkat_core::turn_terminal::TurnTerminalOutcome) -> Self {
        match outcome {
            meerkat_core::turn_terminal::TurnTerminalOutcome::Completed {
                output,
                structured_output,
            } => Self::Completed {
                output,
                structured_output,
            },
            meerkat_core::turn_terminal::TurnTerminalOutcome::Failed { reason } => Self::Failed {
                reason,
                disposition: FlowTurnFailureDisposition::ObservedTerminal,
            },
        }
    }
}

#[derive(Debug)]
pub enum TimeoutDisposition {
    Detached,
    Canceled,
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait FlowTurnExecutor: Send + Sync {
    async fn dispatch(
        &self,
        run_id: &RunId,
        step_id: &StepId,
        target: &AgentIdentity,
        message: ContentInput,
        turn_tool_overlay: Option<TurnToolOverlay>,
    ) -> Result<FlowTurnTicket, MobError>;

    async fn await_terminal(
        &self,
        ticket: FlowTurnTicket,
        timeout: Duration,
    ) -> Result<FlowTurnOutcome, MobError>;

    async fn on_timeout(&self, ticket: FlowTurnTicket) -> Result<TimeoutDisposition, MobError>;
}
