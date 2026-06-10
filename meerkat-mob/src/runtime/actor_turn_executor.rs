use super::handle::MobHandle;
use super::provisioner::MobProvisioner;
use super::turn_executor::{
    ActorTurnTicket, FlowTurnExecutor, FlowTurnOutcome, FlowTurnTicket, TimeoutDisposition,
};
use crate::error::MobError;
use crate::event::MemberRef;
use crate::ids::{AgentIdentity, MeerkatId, RunId, StepId};
use crate::machines::mob_machine as mob_dsl;
#[cfg(target_arch = "wasm32")]
use crate::tokio;
use async_trait::async_trait;
use futures::FutureExt;
use meerkat_core::EventEnvelope;
use meerkat_core::event::{AgentEvent, ScopedAgentEvent, StreamScopeFrame};
use meerkat_core::service::{StartTurnRequest, TurnToolOverlay};
use meerkat_core::types::{ContentInput, SessionId};
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, mpsc, oneshot};

#[derive(Clone)]
pub struct ActorFlowTurnExecutor {
    handle: MobHandle,
    provisioner: Arc<dyn MobProvisioner>,
}

impl ActorFlowTurnExecutor {
    pub fn new(handle: MobHandle, provisioner: Arc<dyn MobProvisioner>) -> Self {
        Self {
            handle,
            provisioner,
        }
    }

    fn actor_ticket(ticket: FlowTurnTicket) -> Arc<ActorTurnTicket> {
        match ticket {
            FlowTurnTicket::Actor(ticket) => ticket,
        }
    }

    fn project_member_ref_session_binding(
        member_ref: &MemberRef,
        current_bridge_session_id: Option<SessionId>,
    ) -> Option<MemberRef> {
        match member_ref {
            MemberRef::Session { .. } => {
                current_bridge_session_id.map(MemberRef::from_bridge_session_id)
            }
            MemberRef::BackendPeer {
                peer_id,
                address,
                pubkey,
                bootstrap_token,
                ..
            } => Some(MemberRef::BackendPeer {
                peer_id: peer_id.clone(),
                address: address.clone(),
                pubkey: *pubkey,
                bootstrap_token: bootstrap_token.clone(),
                session_id: current_bridge_session_id,
            }),
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
                            });
                        }
                        return;
                    }
                    AgentEvent::RunFailed { error, .. } => {
                        if let Some(tx) = completion_tx.take() {
                            let _ = tx.send(FlowTurnOutcome::Failed { reason: error });
                        }
                        return;
                    }
                    AgentEvent::InteractionFailed { reason, .. } => {
                        if let Some(tx) = completion_tx.take() {
                            let _ = tx.send(FlowTurnOutcome::Failed {
                                reason: reason.to_string(),
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
                });
            }
        })
    }

    /// Join a detached timed-out turn's subscription bridge. The orphan budget
    /// is MobMachine-owned (decremented at `ClassifyTurnTimeoutDisposition`),
    /// so this is now a pure bridge-join with no shell-side budget accounting.
    async fn reconcile_detached_turn(bridge_handle: tokio::task::JoinHandle<()>, run_id: RunId) {
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
        };

        if AssertUnwindSafe(reconcile).catch_unwind().await.is_err() {
            tracing::error!(
                run_id = %run_id,
                "panic while joining detached timed-out turn"
            );
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
            .await?
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
                let bridge_session_id = self
                    .handle
                    .resolve_bridge_session_id(&AgentIdentity::from(target.as_str()))
                    .await
                    .ok_or_else(|| {
                    MobError::Internal(format!(
                        "autonomous flow dispatch requires MobMachine session binding for '{target}'"
                    ))
                })?;
                let injector = self
                    .provisioner
                    .interaction_event_injector(&bridge_session_id)
                    .await
                    .ok_or_else(|| MobError::MissingMemberCapability {
                        member_id: target.clone(),
                        capability: crate::error::MobMemberCapability::InteractionEventInjector,
                        context: "autonomous flow turn delivery",
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
                let bridge_session_id = self
                    .handle
                    .resolve_bridge_session_id(&AgentIdentity::from(target.as_str()))
                    .await;
                let member_ref = match Self::project_member_ref_session_binding(
                    &entry.member_ref,
                    bridge_session_id,
                ) {
                    Some(member_ref) => member_ref,
                    None => {
                        return Err(MobError::Internal(format!(
                            "turn-driven flow dispatch requires MobMachine session binding for '{target}'"
                        )));
                    }
                };
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
                        &member_ref,
                        run_id,
                        step_id,
                        StartTurnRequest {
                            prompt: message,
                            system_prompt: None,
                            event_tx: Some(event_tx),
                            runtime: meerkat_core::service::StartTurnRuntimeSemantics::new(
                                None,
                                meerkat_core::types::HandlingMode::Queue,
                                None,
                                flow_tool_overlay,
                                Vec::new(),
                                None,
                            ),
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

        // Hard flow turn timeouts are never retryable; MobMachine owns the
        // detach-vs-cancel decision (and the orphan budget) from here.
        let effects = self
            .handle
            .apply_machine_input_effects(mob_dsl::MobMachineInput::ClassifyTurnTimeoutDisposition {
                timed_out_run_id: mob_dsl::RunId::from(run_id.to_string()),
                retryable: false,
            })
            .await?;
        let disposition = effects
            .into_iter()
            .find_map(|effect| match effect {
                mob_dsl::MobMachineEffect::TurnTimeoutDispositionClassified {
                    disposition, ..
                } => Some(disposition),
                _ => None,
            })
            .ok_or_else(|| {
                MobError::Internal(
                    "MobMachine accepted turn-timeout observation but emitted no disposition"
                        .into(),
                )
            })?;

        match disposition {
            mob_dsl::TurnTimeoutDisposition::Detached => {
                tokio::spawn(async move {
                    Self::reconcile_detached_turn(bridge_handle, run_id).await;
                });
                Ok(TimeoutDisposition::Detached)
            }
            // A non-retryable timeout without budget cancels. `Retryable` is
            // unreachable here (retryable=false) but is a hard cancel for the
            // shell's binary `TimeoutDisposition`: aborting the bridge is the
            // only safe shell action for a non-detached hard timeout.
            mob_dsl::TurnTimeoutDisposition::Canceled
            | mob_dsl::TurnTimeoutDisposition::Retryable => {
                bridge_handle.abort();
                Ok(TimeoutDisposition::Canceled)
            }
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
    use tokio::sync::oneshot;

    #[tokio::test]
    async fn test_reconcile_detached_turn_joins_bridge_even_when_bridge_panics() {
        // The orphan budget is now MobMachine-owned; detached-turn reconciliation
        // is a pure bridge-join that must not itself panic when the bridge does.
        let run_id = RunId::new();
        let bridge_handle = tokio::spawn(async move {
            panic!("detached bridge panic");
        });

        ActorFlowTurnExecutor::reconcile_detached_turn(bridge_handle, run_id).await;
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
    async fn test_subscription_bridge_keeps_run_failed_metadata_display_only() {
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
            FlowTurnOutcome::Failed { reason } => {
                assert_eq!(reason, "LLM failure terminal turn");
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
            FlowTurnOutcome::Failed { reason } => {
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
