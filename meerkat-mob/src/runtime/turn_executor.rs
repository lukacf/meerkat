use crate::error::MobError;
use crate::ids::{MeerkatId, RunId, StepId};
use async_trait::async_trait;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;
#[cfg(target_arch = "wasm32")]
use crate::tokio;
use tokio::sync::{Mutex, oneshot};
use tokio::task::JoinHandle;

pub struct ActorTurnTicket {
    pub(crate) run_id: RunId,
    pub(crate) completion_rx: Mutex<Option<oneshot::Receiver<FlowTurnOutcome>>>,
    pub(crate) bridge_handle: Mutex<Option<JoinHandle<()>>>,
}

#[derive(Clone)]
pub enum FlowTurnTicket {
    Actor(Arc<ActorTurnTicket>),
}

impl fmt::Debug for FlowTurnTicket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Actor(_) => f.write_str("FlowTurnTicket::Actor(..)"),
        }
    }
}

#[derive(Debug)]
pub enum FlowTurnOutcome {
    Completed { output: String },
    Failed { reason: String },
    Canceled,
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
        target: &MeerkatId,
        message: String,
    ) -> Result<FlowTurnTicket, MobError>;

    async fn await_terminal(
        &self,
        ticket: FlowTurnTicket,
        timeout: Duration,
    ) -> Result<FlowTurnOutcome, MobError>;

    async fn on_timeout(&self, ticket: FlowTurnTicket) -> Result<TimeoutDisposition, MobError>;
}
