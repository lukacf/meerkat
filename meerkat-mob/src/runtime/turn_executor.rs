use crate::error::MobError;
use crate::ids::{MeerkatId, RunId, StepId};
use async_trait::async_trait;
use std::any::Any;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

#[derive(Clone)]
pub struct FlowTurnTicket(Arc<dyn Any + Send + Sync>);

impl FlowTurnTicket {
    pub fn new<T>(ticket: T) -> Self
    where
        T: Any + Send + Sync,
    {
        Self(Arc::new(ticket))
    }

    pub fn downcast<T>(self) -> Result<Arc<T>, Self>
    where
        T: Any + Send + Sync,
    {
        match Arc::downcast::<T>(self.0) {
            Ok(ticket) => Ok(ticket),
            Err(ticket) => Err(Self(ticket)),
        }
    }
}

impl fmt::Debug for FlowTurnTicket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("FlowTurnTicket(..)")
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

#[async_trait]
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
