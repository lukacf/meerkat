use super::handle::MobHandle;
use super::provisioner::MobProvisioner;
use super::remote_flow_ticket::{
    MobHandleObligationResolver, REMATERIALIZED_STEP_FAILURE_REASON, RemoteFlowTicketRegistry,
    RemoteTurnObligationPump, TurnDirectiveDelivery,
};
use super::turn_executor::{
    ActorTurnTicket, FlowTurnExecutor, FlowTurnFailureDisposition, FlowTurnOutcome, FlowTurnTicket,
    RemoteTurnTicket, TimeoutDisposition,
};
use crate::error::{FlowStepDispatchRejectKind, MobError};
use crate::event::MemberRef;
use crate::ids::{AgentIdentity, RunId, StepId};
use crate::machines::mob_machine as mob_dsl;
#[cfg(target_arch = "wasm32")]
use crate::tokio;
use async_trait::async_trait;
use futures::FutureExt;
use meerkat_core::EventEnvelope;
use meerkat_core::event::{AgentEvent, ScopedAgentEvent, StreamScopeFrame};
use meerkat_core::service::{StartTurnRequest, TurnToolOverlay};
use meerkat_core::turn_terminal::TurnTerminalClassifier;
use meerkat_core::types::{ContentInput, SessionId};
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, mpsc, oneshot};

#[derive(Clone)]
pub struct ActorFlowTurnExecutor {
    handle: MobHandle,
    provisioner: Arc<dyn MobProvisioner>,
    /// One registry per mob (DEC-P6F-6): the executor arms/disarms; the
    /// events-lane poll pump feeds it through the same Arc (obtained via
    /// [`Self::remote_flow_ticket_registry`]).
    registry: Arc<RemoteFlowTicketRegistry>,
    /// A17 pump-liveness hook, installed by the events lane at mob build
    /// (`install_remote_turn_obligation_pump`). Shared across executor
    /// clones so the wiring survives the builder's clone-then-erase.
    obligation_pump: Arc<std::sync::OnceLock<Arc<dyn RemoteTurnObligationPump>>>,
}

impl ActorFlowTurnExecutor {
    pub fn new(handle: MobHandle, provisioner: Arc<dyn MobProvisioner>) -> Self {
        let registry = Arc::new(RemoteFlowTicketRegistry::new(Arc::new(
            MobHandleObligationResolver {
                handle: handle.clone(),
            },
        )));
        Self {
            handle,
            provisioner,
            registry,
            obligation_pump: Arc::new(std::sync::OnceLock::new()),
        }
    }

    /// The ONE registry instance shared by this executor and the member
    /// event pump (named hand-off hook, FLAG-P6F-5).
    pub fn remote_flow_ticket_registry(&self) -> Arc<RemoteFlowTicketRegistry> {
        Arc::clone(&self.registry)
    }

    /// Install the pump-liveness hook (events lane wires this at mob build;
    /// a second install is a no-op — the first wiring wins).
    pub fn install_remote_turn_obligation_pump(&self, pump: Arc<dyn RemoteTurnObligationPump>) {
        let _ = self.obligation_pump.set(pump);
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
        // Thin shell over the ONE shared terminal classifier (DEC-P6F-8
        // consumer #1): forward every event for display, feed the fold, send
        // the classified terminal. Terminal semantics live in meerkat-core;
        // a second terminal-event list here would be the split-terminality
        // bug class by construction (gotcha 15).
        tokio::spawn(async move {
            let mut classifier = TurnTerminalClassifier::new();
            while let Some(event) = events.recv().await {
                let payload = extract_payload(event);
                if let (Some(tx), Some(frame)) = (&scoped_event_tx, &scoped_frame) {
                    let scoped = ScopedAgentEvent::new(vec![frame.clone()], payload.clone());
                    let _ = tx.send(scoped).await;
                }
                if let Some(terminal) = classifier.observe(&payload) {
                    let _ = completion_tx.send(FlowTurnOutcome::from(terminal.outcome));
                    return;
                }
            }

            let _ = completion_tx.send(FlowTurnOutcome::from(classifier.close().outcome));
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
        target: &AgentIdentity,
        message: ContentInput,
        turn_tool_overlay: Option<TurnToolOverlay>,
    ) -> Result<FlowTurnTicket, MobError> {
        let entry = self
            .handle
            .get_member(target)
            .await?
            .ok_or_else(|| MobError::MemberNotFound(target.clone()))?;

        // DEC-P6F-1: consult the machine BEFORE any delivery. This is a
        // mailbox command round-trip that COMPLETES before any bridge send
        // begins; `dispatch` itself runs on the flow-run task (spawned by
        // `FlowEngine`), never on the actor loop (ADJ-P4-12).
        let effects = self
            .handle
            .apply_machine_input_effects(mob_dsl::MobMachineInput::ClassifyFlowStepDispatch {
                run_id: mob_dsl::RunId::from(run_id.to_string()),
                step_id: mob_dsl::StepId::from(step_id.to_string()),
                target: mob_dsl::AgentIdentity(target.to_string()),
                overlay_present: turn_tool_overlay.is_some(),
            })
            .await?;
        let dispatch = effects
            .into_iter()
            .find_map(|effect| match effect {
                mob_dsl::MobMachineEffect::FlowStepDispatchClassified { dispatch, .. } => {
                    Some(dispatch)
                }
                _ => None,
            })
            .ok_or_else(|| {
                MobError::Internal(
                    "MobMachine accepted flow-step dispatch classification but emitted no verdict"
                        .into(),
                )
            })?;

        match dispatch {
            mob_dsl::FlowStepDispatchKind::Local => {
                self.dispatch_local(entry, run_id, step_id, target, message, turn_tool_overlay)
                    .await
            }
            mob_dsl::FlowStepDispatchKind::RemoteTurnDirective => {
                self.dispatch_remote_turn_directive(
                    entry,
                    run_id,
                    step_id,
                    target,
                    message,
                    turn_tool_overlay,
                )
                .await
            }
            // Typed step failure AT DISPATCH — no delivery is ever sent; one
            // machine arm, one error variant, identical local and remote
            // (§18.8:1000/:1001). The old shell overlay re-check is DELETED:
            // the `not_overlay_autonomous` guards on the admitting arms make
            // an overlay+autonomous combination unreachable (DEC-P6F-2).
            mob_dsl::FlowStepDispatchKind::RejectedOverlayAutonomous => {
                Err(MobError::FlowStepDispatchRejected {
                    target: target.clone(),
                    kind: FlowStepDispatchRejectKind::OverlayAutonomous,
                })
            }
            mob_dsl::FlowStepDispatchKind::RejectedHostIncapable => {
                Err(MobError::FlowStepDispatchRejected {
                    target: target.clone(),
                    kind: FlowStepDispatchRejectKind::HostIncapable,
                })
            }
        }
    }

    async fn await_terminal(
        &self,
        ticket: FlowTurnTicket,
        timeout: Duration,
    ) -> Result<FlowTurnOutcome, MobError> {
        match ticket {
            FlowTurnTicket::Actor(ticket) => self.await_actor_terminal(ticket, timeout).await,
            FlowTurnTicket::Remote(ticket) => self.await_remote_terminal(ticket, timeout).await,
        }
    }

    async fn on_timeout(&self, ticket: FlowTurnTicket) -> Result<TimeoutDisposition, MobError> {
        match ticket {
            FlowTurnTicket::Actor(ticket) => self.on_actor_timeout(ticket).await,
            FlowTurnTicket::Remote(ticket) => self.on_remote_timeout(ticket).await,
        }
    }
}

impl ActorFlowTurnExecutor {
    /// Today's local dispatch path, byte-for-byte minus the deleted shell
    /// overlay reject (DEC-P6F-2 — the machine classification owns it).
    async fn dispatch_local(
        &self,
        entry: crate::roster::RosterEntry,
        run_id: &RunId,
        step_id: &StepId,
        target: &AgentIdentity,
        message: ContentInput,
        turn_tool_overlay: Option<TurnToolOverlay>,
    ) -> Result<FlowTurnTicket, MobError> {
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
                            injected_context: Vec::new(),
                            prompt: message,
                            system_prompt: None,
                            event_tx: Some(event_tx),
                            runtime: meerkat_core::service::StartTurnRuntimeSemantics::new(
                                meerkat_core::types::HandlingMode::Queue,
                                turn_tool_overlay,
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

    /// Remote (placed-member) dispatch: the ONE delivery command upgraded
    /// with the typed turn directive (§18 O1), obligation choreography per
    /// DEC-P6F-7 (record → arm → send; unwind on failure).
    async fn dispatch_remote_turn_directive(
        &self,
        entry: crate::roster::RosterEntry,
        run_id: &RunId,
        step_id: &StepId,
        target: &AgentIdentity,
        message: ContentInput,
        turn_tool_overlay: Option<TurnToolOverlay>,
    ) -> Result<FlowTurnTicket, MobError> {
        // Fail closed BEFORE recording anything: a remote step without the
        // A17 pump keep-alive could never observe its terminal.
        let pump = self.obligation_pump.get().cloned().ok_or_else(|| {
            MobError::Internal(
                "remote flow dispatch requires an installed member event pump (A17); \
                 no pump hook is wired on this composition"
                    .to_string(),
            )
        })?;

        // Fail-closed overlay projection (FLAG-P6F-8 / ADJ-P6-12):
        // flow-lowered overlays always carry an empty dispatch_context, so
        // this is lossless for flow steps; anything else is a typed error.
        let tool_overlay = match turn_tool_overlay {
            Some(overlay) => Some(overlay.into_public().map_err(|error| {
                MobError::Internal(format!(
                    "flow-lowered turn tool overlay failed public projection for '{target}': {error}"
                ))
            })?),
            None => None,
        };
        let directive = super::bridge_protocol::BridgeTurnDirective {
            correlation: super::bridge_protocol::BridgeTurnCorrelation {
                run_id: run_id.to_string(),
                step_id: step_id.to_string(),
            },
            tool_overlay,
        };
        // The caller mints and owns input_id (DEC-P6F-4): it is the
        // obligation key, the journal key, and the ticket key.
        let input_id = meerkat_core::time_compat::new_uuid_v7().to_string();
        // Sequence allocation and custody admission are one actor-serialized
        // operation. Fan-out dispatches never race a watch projection or
        // exhaust a retry cap.
        let intent = crate::run::MobRunRemoteTurnIntent {
            obligation: crate::event::RemoteTurnObligationEvent {
                agent_identity: target.clone(),
                // Actor fills exact machine-owned placement and sequence.
                host_id: String::new(),
                host_binding_generation: 0,
                member_session_id: String::new(),
                generation: entry.generation,
                fence_token: entry.fence_token,
                dispatch_sequence: 0,
                input_id: input_id.clone(),
                run_id: run_id.clone(),
                step_id: step_id.clone(),
            },
            content: message.clone(),
            handling_mode: meerkat_core::types::HandlingMode::Queue,
            expected_member: Default::default(),
            directive: directive.clone(),
        };
        let reserved = self.handle.reserve_remote_turn_obligation(intent).await?;
        let obligation = &reserved.intent.obligation;
        // The actor command above durably recorded the obligation before
        // committing Pending custody. It continues to completion even if this
        // flow future is canceled while awaiting its reply.
        // Arm the ticket, then keep the pump alive for the obligation.
        let completion_rx = self.registry.arm_exact(
            target,
            &input_id,
            run_id,
            step_id,
            &reserved.intent.expected_member,
        );
        pump.ensure_pump_for_obligation(target);

        // The directive send (the bridge send runs HERE, on the flow
        //    task — never awaited on the actor loop; the machine round-trips
        //    above already completed through the mailbox).
        match self
            .provisioner
            .deliver_turn_directive(
                &reserved.member_ref,
                TurnDirectiveDelivery {
                    input_id: obligation.input_id.clone(),
                    content: reserved.intent.content.clone(),
                    handling_mode: reserved.intent.handling_mode,
                    expected_member: reserved.intent.expected_member.clone(),
                    directive: reserved.intent.directive.clone(),
                },
            )
            .await
        {
            Ok(_receipt) => Ok(FlowTurnTicket::Remote(Arc::new(RemoteTurnTicket {
                run_id: run_id.clone(),
                step_id: step_id.clone(),
                target: target.clone(),
                host_id: obligation.host_id.clone(),
                host_binding_generation: obligation.host_binding_generation,
                member_session_id: obligation.member_session_id.clone(),
                generation: obligation.generation,
                fence_token: obligation.fence_token,
                dispatch_sequence: obligation.dispatch_sequence,
                input_id,
                completion_rx: Mutex::new(Some(completion_rx)),
            }))),
            Err(error) => {
                // A timed-out send may still have admitted the turn. Keep
                // pending custody and return a terminal ticket so FlowEngine
                // first appends StepTargetFailed and only then commits it.
                // Controlling-caused fault attribution (T-F6): the placed
                // respawn marks the lane STRICTLY BEFORE its remote release
                // tears the member transport, so a dispatch unwinding on
                // that teardown is the respawn's own doing — surface the
                // SAME typed generation failure the pump's generation watch
                // produces. An unmarked lane keeps the raw error (no
                // blanket rewriting of comms faults).
                if let MobError::BridgeDeliveryRejected { cause, .. } = &error {
                    self.registry.fail_armed_certified_no_effect(
                        target,
                        &input_id,
                        error.to_string(),
                        cause.as_ref().clone(),
                    );
                } else if self.registry.is_member_rematerializing(target) {
                    tracing::warn!(
                        target = %target,
                        input_id = %input_id,
                        error = %error,
                        "remote dispatch unwound under a rematerializing mark; attributing typed"
                    );
                    self.registry.fail_armed(
                        target,
                        &input_id,
                        REMATERIALIZED_STEP_FAILURE_REASON.to_string(),
                    );
                } else {
                    tracing::warn!(
                        target = %target,
                        input_id = %input_id,
                        error = %error,
                        "remote delivery outcome is ambiguous; retaining the exact armed intent for same-id reconciliation"
                    );
                }
                Ok(FlowTurnTicket::Remote(Arc::new(RemoteTurnTicket {
                    run_id: run_id.clone(),
                    step_id: step_id.clone(),
                    target: target.clone(),
                    host_id: obligation.host_id.clone(),
                    host_binding_generation: obligation.host_binding_generation,
                    member_session_id: obligation.member_session_id.clone(),
                    generation: obligation.generation,
                    fence_token: obligation.fence_token,
                    dispatch_sequence: obligation.dispatch_sequence,
                    input_id,
                    completion_rx: Mutex::new(Some(completion_rx)),
                })))
            }
        }
    }

    async fn await_actor_terminal(
        &self,
        ticket: Arc<ActorTurnTicket>,
        timeout: Duration,
    ) -> Result<FlowTurnOutcome, MobError> {
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
                    disposition: FlowTurnFailureDisposition::ObservedTerminal,
                })
            }
            Err(_) => Err(MobError::FlowTurnTimedOut),
        }
    }

    /// Remote await mirrors the Actor arm: oneshot under timeout; there is
    /// no local bridge to join — the watcher IS the registry (DEC-P6F-5).
    async fn await_remote_terminal(
        &self,
        ticket: Arc<RemoteTurnTicket>,
        timeout: Duration,
    ) -> Result<FlowTurnOutcome, MobError> {
        let completion_rx = {
            let mut lock = ticket.completion_rx.lock().await;
            lock.take().ok_or_else(|| {
                MobError::Internal("flow turn ticket cannot be awaited twice".to_string())
            })?
        };

        match tokio::time::timeout(timeout, completion_rx).await {
            Ok(Ok(outcome)) => Ok(outcome),
            Ok(Err(_)) => Ok(FlowTurnOutcome::Failed {
                reason: "turn completion channel closed".to_string(),
                disposition: FlowTurnFailureDisposition::ObservedTerminal,
            }),
            Err(_) => Err(MobError::FlowTurnTimedOut),
        }
    }

    async fn on_actor_timeout(
        &self,
        ticket: Arc<ActorTurnTicket>,
    ) -> Result<TimeoutDisposition, MobError> {
        let Some(bridge_handle) = ticket.bridge_handle.lock().await.take() else {
            return Ok(TimeoutDisposition::Canceled);
        };
        let run_id = ticket.run_id.clone();

        // Hard flow turn timeouts are never retryable; MobMachine owns the
        // detach-vs-cancel decision (and the orphan budget) from here.
        let disposition = self.classify_turn_timeout_disposition(&run_id).await?;

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

    /// Remote timeout = LOCAL PARITY (ADJ-P6-9 / FLAG-P6F-4): for BOTH
    /// dispositions, disarm the ticket and resolve the obligation — the
    /// timeout ladder disposes the step. Neither disposition sends
    /// `HardCancelMember` (the local Canceled arm only aborts the local
    /// watcher and never cancels the member's turn; the remote arm mirrors
    /// that honestly — hard cancel stays an explicit verb). The remote turn
    /// keeps running on its host; its journal record, when it lands, is
    /// consumed as a display-only row.
    async fn on_remote_timeout(
        &self,
        ticket: Arc<RemoteTurnTicket>,
    ) -> Result<TimeoutDisposition, MobError> {
        let disposition = self
            .classify_turn_timeout_disposition(&ticket.run_id)
            .await?;

        self.registry.disarm(&ticket.target, &ticket.input_id);
        match disposition {
            mob_dsl::TurnTimeoutDisposition::Detached => Ok(TimeoutDisposition::Detached),
            mob_dsl::TurnTimeoutDisposition::Canceled
            | mob_dsl::TurnTimeoutDisposition::Retryable => Ok(TimeoutDisposition::Canceled),
        }
    }

    async fn classify_turn_timeout_disposition(
        &self,
        run_id: &RunId,
    ) -> Result<mob_dsl::TurnTimeoutDisposition, MobError> {
        let effects = self
            .handle
            .apply_machine_input_effects(mob_dsl::MobMachineInput::ClassifyTurnTimeoutDisposition {
                timed_out_run_id: mob_dsl::RunId::from(run_id.to_string()),
                retryable: false,
            })
            .await?;
        effects
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
            })
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
                terminal_cause_kind: Some(meerkat_core::TurnTerminalCauseKind::LlmFailure),
                error_report: report.clone(),
            })
            .await
            .expect("send event");

        match completion_rx.await.expect("completion outcome") {
            FlowTurnOutcome::Failed { reason, .. } => {
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
