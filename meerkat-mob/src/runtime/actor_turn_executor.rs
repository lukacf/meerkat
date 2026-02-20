use super::handle::MobHandle;
use super::provisioner::MobProvisioner;
use super::turn_executor::{FlowTurnExecutor, FlowTurnOutcome, FlowTurnTicket, TimeoutDisposition};
use crate::error::MobError;
use crate::ids::{MeerkatId, RunId, StepId};
use async_trait::async_trait;
use meerkat_core::event::AgentEvent;
use meerkat_core::service::StartTurnRequest;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::sync::{Mutex, mpsc, oneshot};
use tokio::task::JoinHandle;

struct ActorTurnTicket {
    completion_rx: Mutex<Option<oneshot::Receiver<FlowTurnOutcome>>>,
    bridge_handle: Mutex<Option<JoinHandle<()>>>,
}

#[derive(Clone)]
pub struct ActorFlowTurnExecutor {
    handle: MobHandle,
    provisioner: Arc<dyn MobProvisioner>,
    orphan_budget: Arc<AtomicUsize>,
}

impl ActorFlowTurnExecutor {
    pub fn new(
        handle: MobHandle,
        provisioner: Arc<dyn MobProvisioner>,
        orphan_budget: usize,
    ) -> Self {
        Self {
            handle,
            provisioner,
            orphan_budget: Arc::new(AtomicUsize::new(orphan_budget)),
        }
    }

    fn reserve_orphan_slot(&self) -> bool {
        let mut observed = self.orphan_budget.load(Ordering::Acquire);
        loop {
            if observed == 0 {
                return false;
            }
            match self.orphan_budget.compare_exchange(
                observed,
                observed - 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return true,
                Err(next) => observed = next,
            }
        }
    }

    fn downcast_ticket(ticket: FlowTurnTicket) -> Result<Arc<ActorTurnTicket>, MobError> {
        ticket.downcast::<ActorTurnTicket>().map_err(|_| {
            MobError::Internal("flow turn ticket type mismatch for actor executor".to_string())
        })
    }
}

#[async_trait]
impl FlowTurnExecutor for ActorFlowTurnExecutor {
    async fn dispatch(
        &self,
        _run_id: &RunId,
        _step_id: &StepId,
        target: &MeerkatId,
        message: String,
    ) -> Result<FlowTurnTicket, MobError> {
        let entry = self
            .handle
            .get_meerkat(target)
            .await
            .ok_or_else(|| MobError::MeerkatNotFound(target.to_string()))?;

        let (event_tx, mut event_rx) = mpsc::channel::<AgentEvent>(8);
        let (completion_tx, completion_rx) = oneshot::channel::<FlowTurnOutcome>();

        let bridge_handle = tokio::spawn(async move {
            let mut completion_tx = Some(completion_tx);
            while let Some(event) = event_rx.recv().await {
                match event {
                    AgentEvent::RunCompleted { result, .. } => {
                        if let Some(tx) = completion_tx.take() {
                            let _ = tx.send(FlowTurnOutcome::Completed { output: result });
                        }
                        return;
                    }
                    AgentEvent::RunFailed { error, .. } => {
                        if let Some(tx) = completion_tx.take() {
                            let _ = tx.send(FlowTurnOutcome::Failed { reason: error });
                        }
                        return;
                    }
                    _ => {}
                }
            }

            if let Some(tx) = completion_tx {
                let _ = tx.send(FlowTurnOutcome::Failed {
                    reason: "turn event stream closed before terminal outcome".to_string(),
                });
            }
        });

        let start_result = self
            .provisioner
            .start_turn(
                &entry.member_ref,
                StartTurnRequest {
                    prompt: message,
                    event_tx: Some(event_tx),
                    host_mode: false,
                    skill_references: None,
                },
            )
            .await;

        if let Err(error) = start_result {
            bridge_handle.abort();
            return Err(error);
        }

        Ok(FlowTurnTicket::new(ActorTurnTicket {
            completion_rx: Mutex::new(Some(completion_rx)),
            bridge_handle: Mutex::new(Some(bridge_handle)),
        }))
    }

    async fn await_terminal(
        &self,
        ticket: FlowTurnTicket,
        timeout: Duration,
    ) -> Result<FlowTurnOutcome, MobError> {
        let ticket = Self::downcast_ticket(ticket)?;
        let completion_rx = {
            let mut lock = ticket.completion_rx.lock().await;
            lock.take().ok_or_else(|| {
                MobError::Internal("flow turn ticket cannot be awaited twice".to_string())
            })?
        };

        match tokio::time::timeout(timeout, completion_rx).await {
            Ok(Ok(outcome)) => {
                if let Some(handle) = ticket.bridge_handle.lock().await.take() {
                    let _ = handle.await;
                }
                Ok(outcome)
            }
            Ok(Err(_)) => {
                if let Some(handle) = ticket.bridge_handle.lock().await.take() {
                    let _ = handle.await;
                }
                Ok(FlowTurnOutcome::Failed {
                    reason: "turn completion channel closed".to_string(),
                })
            }
            Err(_) => Err(MobError::FlowTurnTimedOut),
        }
    }

    async fn on_timeout(&self, ticket: FlowTurnTicket) -> Result<TimeoutDisposition, MobError> {
        let ticket = Self::downcast_ticket(ticket)?;
        let Some(bridge_handle) = ticket.bridge_handle.lock().await.take() else {
            return Ok(TimeoutDisposition::Canceled);
        };

        if self.reserve_orphan_slot() {
            let orphan_budget = self.orphan_budget.clone();
            tokio::spawn(async move {
                let _ = bridge_handle.await;
                orphan_budget.fetch_add(1, Ordering::Release);
            });
            Ok(TimeoutDisposition::Detached)
        } else {
            bridge_handle.abort();
            Err(MobError::Internal(
                "flow turn timeout with exhausted orphan budget".to_string(),
            ))
        }
    }
}
