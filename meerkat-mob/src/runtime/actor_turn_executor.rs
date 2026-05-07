use super::handle::MobHandle;
use super::provisioner::MobProvisioner;
use super::turn_executor::{
    ActorTurnTicket, FlowTurnExecutor, FlowTurnOutcome, FlowTurnTicket, TimeoutDisposition,
};
use crate::error::MobError;
use crate::ids::{MeerkatId, RunId, StepId};
#[cfg(target_arch = "wasm32")]
use crate::tokio;
use async_trait::async_trait;
use futures::FutureExt;
use meerkat_core::EventEnvelope;
use meerkat_core::event::{AgentEvent, ScopedAgentEvent, StreamScopeFrame, TurnErrorMetadata};
use meerkat_core::service::{StartTurnRequest, TurnToolOverlay};
use meerkat_core::types::ContentInput;
use std::collections::BTreeMap;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::sync::{Mutex, mpsc, oneshot};

#[derive(Clone)]
pub struct ActorFlowTurnExecutor {
    handle: MobHandle,
    provisioner: Arc<dyn MobProvisioner>,
    orphan_budget: Arc<AtomicUsize>,
    orphan_budget_max: usize,
    per_run_orphan_limit: usize,
    per_run_orphans: Arc<Mutex<BTreeMap<RunId, usize>>>,
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
            orphan_budget_max: orphan_budget,
            per_run_orphan_limit: orphan_budget.saturating_div(2).max(1),
            per_run_orphans: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    fn reserve_orphan_slot(&self) -> bool {
        let mut observed = self.orphan_budget.load(Ordering::Acquire);
        loop {
            if observed == 0 {
                tracing::warn!(
                    budget_max = self.orphan_budget_max,
                    "flow timeout orphan budget exhausted"
                );
                return false;
            }
            let next = observed - 1;
            match self.orphan_budget.compare_exchange(
                observed,
                next,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    if next == 0 {
                        tracing::warn!(
                            budget_max = self.orphan_budget_max,
                            "flow timeout orphan budget reached zero"
                        );
                    }
                    return true;
                }
                Err(next) => observed = next,
            }
        }
    }

    fn release_orphan_slot(orphan_budget: &AtomicUsize, orphan_budget_max: usize) {
        let mut observed = orphan_budget.load(Ordering::Acquire);
        loop {
            if observed >= orphan_budget_max {
                tracing::warn!(
                    observed,
                    budget_max = orphan_budget_max,
                    "orphan budget release attempted above configured maximum; ignoring"
                );
                return;
            }
            let next = observed + 1;
            match orphan_budget.compare_exchange(
                observed,
                next,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return,
                Err(current) => observed = current,
            }
        }
    }

    fn actor_ticket(ticket: FlowTurnTicket) -> Arc<ActorTurnTicket> {
        match ticket {
            FlowTurnTicket::Actor(ticket) => ticket,
        }
    }

    fn spawn_subscription_bridge(
        events: tokio::sync::mpsc::Receiver<AgentEvent>,
        completion_tx: oneshot::Sender<FlowTurnOutcome>,
        scoped_event_tx: Option<mpsc::Sender<ScopedAgentEvent>>,
        scoped_frame: Option<StreamScopeFrame>,
    ) -> tokio::task::JoinHandle<()> {
        // Autonomous-host injector subscriptions still emit raw AgentEvent.
        Self::spawn_subscription_bridge_impl(
            events,
            completion_tx,
            scoped_event_tx,
            scoped_frame,
            |payload| payload,
        )
    }

    fn spawn_subscription_bridge_enveloped(
        events: tokio::sync::mpsc::Receiver<EventEnvelope<AgentEvent>>,
        completion_tx: oneshot::Sender<FlowTurnOutcome>,
        scoped_event_tx: Option<mpsc::Sender<ScopedAgentEvent>>,
        scoped_frame: Option<StreamScopeFrame>,
    ) -> tokio::task::JoinHandle<()> {
        Self::spawn_subscription_bridge_impl(
            events,
            completion_tx,
            scoped_event_tx,
            scoped_frame,
            |event| event.payload,
        )
    }

    fn spawn_subscription_bridge_impl<E, F>(
        mut events: tokio::sync::mpsc::Receiver<E>,
        completion_tx: oneshot::Sender<FlowTurnOutcome>,
        scoped_event_tx: Option<mpsc::Sender<ScopedAgentEvent>>,
        scoped_frame: Option<StreamScopeFrame>,
        mut extract_payload: F,
    ) -> tokio::task::JoinHandle<()>
    where
        E: Send + 'static,
        F: FnMut(E) -> AgentEvent + Send + 'static,
    {
        tokio::spawn(async move {
            let mut completion_tx = Some(completion_tx);
            let mut pending_completed_run: Option<(String, Option<serde_json::Value>)> = None;
            while let Some(event) = events.recv().await {
                let payload = extract_payload(event);
                if let (Some(tx), Some(frame)) = (&scoped_event_tx, &scoped_frame) {
                    let scoped = ScopedAgentEvent::new(vec![frame.clone()], payload.clone());
                    let _ = tx.send(scoped).await;
                }
                match payload {
                    AgentEvent::RunCompleted {
                        result,
                        structured_output,
                        extraction_required,
                        ..
                    } => {
                        if extraction_required {
                            pending_completed_run = Some((result, structured_output));
                            continue;
                        }
                        if let Some(tx) = completion_tx.take() {
                            let _ = tx.send(FlowTurnOutcome::Completed {
                                output: result,
                                structured_output,
                            });
                        }
                        return;
                    }
                    AgentEvent::ExtractionSucceeded {
                        structured_output, ..
                    } => {
                        if let Some(tx) = completion_tx.take() {
                            let (output, _) = pending_completed_run
                                .take()
                                .unwrap_or_else(|| (structured_output.to_string(), None));
                            let _ = tx.send(FlowTurnOutcome::Completed {
                                output,
                                structured_output: Some(structured_output),
                            });
                        }
                        return;
                    }
                    AgentEvent::ExtractionFailed {
                        last_output,
                        attempts,
                        reason,
                        ..
                    } => {
                        if let Some(tx) = completion_tx.take() {
                            let (output, _) =
                                pending_completed_run.take().unwrap_or((last_output, None));
                            let _ = tx.send(FlowTurnOutcome::Failed {
                                reason: format!(
                                    "structured output extraction failed after {attempts} attempt(s): {reason}; last_output={output:?}"
                                ),
                                error_report: None,
                                error: Some(TurnErrorMetadata::terminal(
                                    meerkat_core::TurnTerminalCauseKind::StructuredOutputValidationFailed,
                                    meerkat_core::TurnTerminalOutcome::StructuredOutputValidationFailed,
                                    format!(
                                        "structured output extraction failed after {attempts} attempt(s): {reason}; last_output={output:?}"
                                    ),
                                )),
                            });
                        }
                        return;
                    }
                    AgentEvent::InteractionComplete {
                        result,
                        structured_output,
                        ..
                    } => {
                        if let Some(tx) = completion_tx.take() {
                            let _ = tx.send(FlowTurnOutcome::Completed {
                                output: result,
                                structured_output,
                            });
                        }
                        return;
                    }
                    AgentEvent::InteractionCallbackPending {
                        tool_name, args, ..
                    } => {
                        if let Some(tx) = completion_tx.take() {
                            let _ = tx.send(FlowTurnOutcome::Failed {
                                reason: format!("callback pending for tool '{tool_name}': {args}"),
                                error_report: None,
                                error: None,
                            });
                        }
                        return;
                    }
                    AgentEvent::RunFailed {
                        error,
                        error_report,
                        ..
                    } => {
                        if let Some(tx) = completion_tx.take() {
                            let metadata = error_report.as_ref().and_then(|report| {
                                TurnErrorMetadata::from_agent_error_report(report, error.clone())
                            });
                            let _ = tx.send(FlowTurnOutcome::Failed {
                                reason: error,
                                error_report,
                                error: metadata,
                            });
                        }
                        return;
                    }
                    AgentEvent::InteractionFailed { error, .. } => {
                        if let Some(tx) = completion_tx.take() {
                            let _ = tx.send(FlowTurnOutcome::Failed {
                                reason: error,
                                error_report: None,
                                error: None,
                            });
                        }
                        return;
                    }
                    _ => {}
                }
            }

            if let Some(tx) = completion_tx {
                let _ = tx.send(FlowTurnOutcome::Failed {
                    reason: "turn event stream closed before terminal outcome".to_string(),
                    error_report: None,
                    error: None,
                });
            }
        })
    }

    async fn reserve_orphan_slot_for_run(&self, run_id: &RunId) -> bool {
        {
            let mut usage = self.per_run_orphans.lock().await;
            let used = *usage.get(run_id).unwrap_or(&0);
            if used >= self.per_run_orphan_limit {
                tracing::warn!(
                    run_id = %run_id,
                    per_run_orphan_limit = self.per_run_orphan_limit,
                    "flow timeout orphan budget reached per-flow limit"
                );
                return false;
            }
            usage.insert(run_id.clone(), used + 1);
        }

        if self.reserve_orphan_slot() {
            return true;
        }

        let mut usage = self.per_run_orphans.lock().await;
        match usage.get(run_id).copied() {
            Some(0) | None => {}
            Some(1) => {
                usage.remove(run_id);
            }
            Some(n) => {
                usage.insert(run_id.clone(), n - 1);
            }
        }
        false
    }

    async fn release_orphan_slot_for_run(
        per_run_orphans: &Mutex<BTreeMap<RunId, usize>>,
        run_id: &RunId,
    ) {
        let mut usage = per_run_orphans.lock().await;
        match usage.get(run_id).copied() {
            Some(0) | None => {}
            Some(1) => {
                usage.remove(run_id);
            }
            Some(n) => {
                usage.insert(run_id.clone(), n - 1);
            }
        }
    }

    async fn reconcile_detached_turn(
        bridge_handle: tokio::task::JoinHandle<()>,
        orphan_budget: Arc<AtomicUsize>,
        orphan_budget_max: usize,
        per_run_orphans: Arc<Mutex<BTreeMap<RunId, usize>>>,
        run_id: RunId,
    ) {
        let reconcile = async {
            match bridge_handle.await {
                Ok(()) => {
                    tracing::debug!(run_id = %run_id, "detached timed-out turn finished");
                }
                Err(error) => {
                    tracing::warn!(
                        run_id = %run_id,
                        error = %error,
                        "detached timed-out turn ended abnormally"
                    );
                }
            }
            Self::release_orphan_slot(orphan_budget.as_ref(), orphan_budget_max);
            Self::release_orphan_slot_for_run(per_run_orphans.as_ref(), &run_id).await;
        };

        if AssertUnwindSafe(reconcile).catch_unwind().await.is_err() {
            tracing::error!(
                run_id = %run_id,
                "panic while reconciling detached timed-out turn; forcing orphan budget release"
            );
            Self::release_orphan_slot(orphan_budget.as_ref(), orphan_budget_max);
            Self::release_orphan_slot_for_run(per_run_orphans.as_ref(), &run_id).await;
        }
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl FlowTurnExecutor for ActorFlowTurnExecutor {
    async fn dispatch(
        &self,
        run_id: &RunId,
        step_id: &StepId,
        target: &MeerkatId,
        message: ContentInput,
        flow_tool_overlay: Option<TurnToolOverlay>,
    ) -> Result<FlowTurnTicket, MobError> {
        let entry = self
            .handle
            .get_member_by_meerkat_id(target)
            .await
            .ok_or_else(|| MobError::MemberNotFound(target.clone()))?;

        let (completion_tx, completion_rx) = oneshot::channel::<FlowTurnOutcome>();
        let scoped_event_tx = self.handle.flow_streams.lock().await.get(run_id).cloned();
        let scope_frame = StreamScopeFrame::MobMember {
            flow_run_id: run_id.to_string(),
            agent_identity: entry.agent_identity.to_string(),
            agent_runtime_id: None,
            fence_token: None,
            generation: None,
        };
        let bridge_handle = match entry.runtime_mode {
            crate::MobRuntimeMode::AutonomousHost => {
                if flow_tool_overlay.is_some() {
                    return Err(MobError::Internal(format!(
                        "flow tool overlay cannot be enforced for autonomous host member '{target}'; \
                         use turn_driven runtime mode for steps with allowed_tools/blocked_tools"
                    )));
                }
                let bridge_session_id = entry.member_ref.bridge_session_id().ok_or_else(|| {
                    MobError::Internal(format!(
                        "autonomous flow dispatch requires session-backed member ref for '{target}'"
                    ))
                })?;
                let injector = self
                    .provisioner
                    .interaction_event_injector(bridge_session_id)
                    .await
                    .ok_or_else(|| {
                        MobError::Internal(format!(
                            "missing event injector for autonomous flow target '{target}'"
                        ))
                    })?;
                let subscription = injector
                    .inject_with_subscription(
                        message,
                        meerkat_core::PlainEventSource::Rpc,
                        meerkat_core::types::HandlingMode::Queue,
                        None,
                    )
                    .map_err(|error| {
                        MobError::Internal(format!(
                            "autonomous flow dispatch inject failed for '{target}': {error}"
                        ))
                    })?;
                Self::spawn_subscription_bridge(
                    subscription.events,
                    completion_tx,
                    scoped_event_tx.clone(),
                    Some(scope_frame.clone()),
                )
            }
            crate::MobRuntimeMode::TurnDriven => {
                let (event_tx, event_rx) = mpsc::channel::<EventEnvelope<AgentEvent>>(8);
                let bridge_handle = Self::spawn_subscription_bridge_enveloped(
                    event_rx,
                    completion_tx,
                    scoped_event_tx,
                    Some(scope_frame),
                );

                if let Err(error) = self
                    .provisioner
                    .start_flow_step(
                        &entry.member_ref,
                        run_id,
                        step_id,
                        StartTurnRequest {
                            prompt: message,
                            system_prompt: None,
                            render_metadata: None,
                            handling_mode: meerkat_core::types::HandlingMode::Queue,
                            event_tx: Some(event_tx),
                            skill_references: None,
                            flow_tool_overlay,
                            pre_turn_context_appends: Vec::new(),
                            turn_metadata: None,
                        },
                    )
                    .await
                {
                    bridge_handle.abort();
                    return Err(error);
                }
                bridge_handle
            }
        };

        Ok(FlowTurnTicket::Actor(Arc::new(ActorTurnTicket {
            run_id: run_id.clone(),
            completion_rx: Mutex::new(Some(completion_rx)),
            bridge_handle: Mutex::new(Some(bridge_handle)),
        })))
    }

    async fn await_terminal(
        &self,
        ticket: FlowTurnTicket,
        timeout: Duration,
    ) -> Result<FlowTurnOutcome, MobError> {
        let ticket = Self::actor_ticket(ticket);
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
                    error_report: None,
                    error: None,
                })
            }
            Err(_) => Err(MobError::FlowTurnTimedOut),
        }
    }

    async fn on_timeout(&self, ticket: FlowTurnTicket) -> Result<TimeoutDisposition, MobError> {
        let ticket = Self::actor_ticket(ticket);
        let Some(bridge_handle) = ticket.bridge_handle.lock().await.take() else {
            return Ok(TimeoutDisposition::Canceled);
        };
        let run_id = ticket.run_id.clone();

        if self.reserve_orphan_slot_for_run(&run_id).await {
            let orphan_budget = self.orphan_budget.clone();
            let orphan_budget_max = self.orphan_budget_max;
            let per_run_orphans = self.per_run_orphans.clone();
            tokio::spawn(async move {
                Self::reconcile_detached_turn(
                    bridge_handle,
                    orphan_budget,
                    orphan_budget_max,
                    per_run_orphans,
                    run_id,
                )
                .await;
            });
            Ok(TimeoutDisposition::Detached)
        } else {
            bridge_handle.abort();
            Err(MobError::Internal(
                "flow turn timeout with exhausted orphan budget or per-flow orphan limit"
                    .to_string(),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ActorFlowTurnExecutor;
    use crate::ids::RunId;
    use crate::runtime::FlowTurnOutcome;
    use meerkat_core::AgentEvent;
    use meerkat_core::event::{AgentErrorClass, AgentErrorReason, AgentErrorReport};
    use meerkat_core::interaction::InteractionId;
    use std::collections::BTreeMap;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::{Mutex, oneshot};

    #[test]
    fn test_release_orphan_slot_caps_at_max_budget() {
        let budget = AtomicUsize::new(2);
        ActorFlowTurnExecutor::release_orphan_slot(&budget, 2);
        assert_eq!(
            budget.load(Ordering::Acquire),
            2,
            "release should be capped at configured max budget"
        );
    }

    #[test]
    fn test_release_orphan_slot_increments_when_below_max() {
        let budget = AtomicUsize::new(1);
        ActorFlowTurnExecutor::release_orphan_slot(&budget, 2);
        assert_eq!(
            budget.load(Ordering::Acquire),
            2,
            "release should increment when a slot is available"
        );
    }

    #[tokio::test]
    async fn test_reconcile_detached_turn_releases_budget_when_bridge_panics() {
        let run_id = RunId::new();
        let budget = Arc::new(AtomicUsize::new(0));
        let per_run_orphans = Arc::new(Mutex::new(BTreeMap::from([(run_id.clone(), 1usize)])));
        let bridge_handle = tokio::spawn(async move {
            panic!("detached bridge panic");
        });

        ActorFlowTurnExecutor::reconcile_detached_turn(
            bridge_handle,
            budget.clone(),
            1,
            per_run_orphans.clone(),
            run_id.clone(),
        )
        .await;

        assert_eq!(
            budget.load(Ordering::Acquire),
            1,
            "detached panic path should release one global orphan budget slot"
        );
        assert!(
            !per_run_orphans.lock().await.contains_key(&run_id),
            "detached panic path should release per-run orphan accounting"
        );
    }

    #[tokio::test]
    async fn test_subscription_bridge_preserves_interaction_structured_output() {
        let (events_tx, events_rx) = tokio::sync::mpsc::channel(1);
        let (completion_tx, completion_rx) = oneshot::channel();
        let bridge =
            ActorFlowTurnExecutor::spawn_subscription_bridge(events_rx, completion_tx, None, None);

        events_tx
            .send(AgentEvent::InteractionComplete {
                interaction_id: InteractionId(uuid::Uuid::new_v4()),
                result: "{\"answer\":42}".to_string(),
                structured_output: Some(serde_json::json!({"answer": 42})),
            })
            .await
            .expect("send event");

        match completion_rx.await.expect("completion outcome") {
            FlowTurnOutcome::Completed {
                output,
                structured_output,
            } => {
                assert_eq!(output, "{\"answer\":42}");
                assert_eq!(structured_output, Some(serde_json::json!({"answer": 42})));
            }
            other => panic!("expected completed outcome, got {other:?}"),
        }
        bridge.await.expect("bridge exits after terminal event");
    }

    #[tokio::test]
    async fn test_subscription_bridge_preserves_run_structured_output() {
        let (events_tx, events_rx) = tokio::sync::mpsc::channel(1);
        let (completion_tx, completion_rx) = oneshot::channel();
        let bridge =
            ActorFlowTurnExecutor::spawn_subscription_bridge(events_rx, completion_tx, None, None);

        events_tx
            .send(AgentEvent::RunCompleted {
                session_id: meerkat_core::SessionId::new(),
                result: "{\"answer\":42}".to_string(),
                structured_output: Some(serde_json::json!({"answer": 42})),
                extraction_required: false,
                usage: meerkat_core::Usage::default(),
                terminal_cause_kind: None,
            })
            .await
            .expect("send event");

        match completion_rx.await.expect("completion outcome") {
            FlowTurnOutcome::Completed {
                output,
                structured_output,
            } => {
                assert_eq!(output, "{\"answer\":42}");
                assert_eq!(structured_output, Some(serde_json::json!({"answer": 42})));
            }
            other => panic!("expected completed outcome, got {other:?}"),
        }
        bridge.await.expect("bridge exits after terminal event");
    }

    #[tokio::test]
    async fn test_subscription_bridge_preserves_run_failed_typed_llm_error() {
        let (events_tx, events_rx) = tokio::sync::mpsc::channel(1);
        let (completion_tx, completion_rx) = oneshot::channel();
        let bridge =
            ActorFlowTurnExecutor::spawn_subscription_bridge(events_rx, completion_tx, None, None);

        let report = AgentErrorReport {
            class: AgentErrorClass::Terminal,
            reason: Some(AgentErrorReason::TurnTerminalCause {
                outcome: meerkat_core::TurnTerminalOutcome::Failed,
                cause_kind: meerkat_core::TurnTerminalCauseKind::LlmFailure,
            }),
            message: "LLM failure terminal turn".to_string(),
        };

        events_tx
            .send(AgentEvent::RunFailed {
                session_id: meerkat_core::SessionId::new(),
                error_class: AgentErrorClass::Terminal,
                error: "LLM failure terminal turn".to_string(),
                terminal_cause_kind: Some(meerkat_core::TurnTerminalCauseKind::LlmFailure),
                error_report: Some(report.clone()),
            })
            .await
            .expect("send event");

        match completion_rx.await.expect("completion outcome") {
            FlowTurnOutcome::Failed {
                reason,
                error_report,
                error,
            } => {
                assert_eq!(reason, "LLM failure terminal turn");
                assert_eq!(error_report, Some(report));
                let error = error.expect("typed turn error metadata");
                assert_eq!(error.kind, meerkat_core::TurnTerminalCauseKind::LlmFailure);
                assert!(error.terminal);
                assert_eq!(
                    error.outcome,
                    Some(meerkat_core::TurnTerminalOutcome::Failed)
                );
                assert_eq!(error.detail.as_deref(), Some("LLM failure terminal turn"));
            }
            other => panic!("expected failed outcome, got {other:?}"),
        }
        bridge.await.expect("bridge exits after terminal event");
    }

    #[tokio::test]
    async fn test_subscription_bridge_waits_for_required_extraction_success() {
        let (events_tx, events_rx) = tokio::sync::mpsc::channel(2);
        let (completion_tx, completion_rx) = oneshot::channel();
        let bridge =
            ActorFlowTurnExecutor::spawn_subscription_bridge(events_rx, completion_tx, None, None);

        let session_id = meerkat_core::SessionId::new();
        events_tx
            .send(AgentEvent::RunCompleted {
                session_id: session_id.clone(),
                result: "main answer".to_string(),
                structured_output: None,
                extraction_required: true,
                usage: meerkat_core::Usage::default(),
                terminal_cause_kind: None,
            })
            .await
            .expect("send run completed event");
        events_tx
            .send(AgentEvent::ExtractionSucceeded {
                session_id,
                structured_output: serde_json::json!({"answer": 42}),
                schema_warnings: None,
            })
            .await
            .expect("send extraction succeeded event");

        match completion_rx.await.expect("completion outcome") {
            FlowTurnOutcome::Completed {
                output,
                structured_output,
            } => {
                assert_eq!(output, "main answer");
                assert_eq!(structured_output, Some(serde_json::json!({"answer": 42})));
            }
            other => panic!("expected completed outcome, got {other:?}"),
        }
        bridge.await.expect("bridge exits after extraction event");
    }

    #[tokio::test]
    async fn test_subscription_bridge_treats_extraction_failure_as_failed_step() {
        let (events_tx, events_rx) = tokio::sync::mpsc::channel(2);
        let (completion_tx, completion_rx) = oneshot::channel();
        let bridge =
            ActorFlowTurnExecutor::spawn_subscription_bridge(events_rx, completion_tx, None, None);

        let session_id = meerkat_core::SessionId::new();
        events_tx
            .send(AgentEvent::RunCompleted {
                session_id: session_id.clone(),
                result: "main answer".to_string(),
                structured_output: None,
                extraction_required: true,
                usage: meerkat_core::Usage::default(),
                terminal_cause_kind: None,
            })
            .await
            .expect("send run completed event");
        events_tx
            .send(AgentEvent::ExtractionFailed {
                session_id,
                last_output: "main answer".to_string(),
                attempts: 2,
                reason: "Invalid JSON".to_string(),
            })
            .await
            .expect("send extraction failed event");

        match completion_rx.await.expect("completion outcome") {
            FlowTurnOutcome::Failed { reason, .. } => {
                assert!(
                    reason.contains("structured output extraction failed after 2 attempt(s)"),
                    "reason should preserve extraction attempts: {reason}"
                );
                assert!(
                    reason.contains("Invalid JSON"),
                    "reason should preserve extraction failure reason: {reason}"
                );
                assert!(
                    reason.contains("main answer"),
                    "reason should preserve main output context: {reason}"
                );
            }
            other => panic!("expected failed outcome, got {other:?}"),
        }
        bridge.await.expect("bridge exits after extraction event");
    }
}
