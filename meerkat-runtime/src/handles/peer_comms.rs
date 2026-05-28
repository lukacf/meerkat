//! Runtime impl of [`meerkat_core::handles::PeerCommsHandle`].

use std::sync::Arc;

use meerkat_core::handles::{DslTransitionError, PeerCommsHandle};
use meerkat_core::interaction::{
    PeerIngressAdmission, PeerIngressDequeueAuthority, PeerIngressDequeueFacts,
    PeerIngressEnvelopeFacts, PeerIngressPlainEventFacts, PeerIngressReceiveAuthority,
    PeerIngressReceiveFacts,
};

use super::HandleDslAuthority;
use crate::meerkat_machine::dsl as mm_dsl;

/// Runtime-backed [`PeerCommsHandle`] impl.
///
/// Routes every trait method to the corresponding DSL signal / input on a
/// dedicated per-session MeerkatMachine DSL authority.
#[derive(Debug)]
pub struct RuntimePeerCommsHandle {
    dsl: Arc<HandleDslAuthority>,
}

impl RuntimePeerCommsHandle {
    /// Construct a handle backed by the session's shared DSL authority.
    pub fn new(dsl: Arc<HandleDslAuthority>) -> Self {
        Self { dsl }
    }

    /// Install a generated peer-comms handle and its matching trust owner.
    ///
    /// The owner token stays on the generated install path; callers receive no
    /// reusable token that could be copied into a handwritten handle.
    #[doc(hidden)]
    pub fn generated_install_factory(
        dsl: Arc<HandleDslAuthority>,
    ) -> Result<meerkat_core::handles::GeneratedPeerCommsInstallFactory, String> {
        let owner = dsl.generated_authority_owner_token();
        crate::protocol_comms_trust_reconcile::generated_peer_comms_install_factory(
            Arc::new(Self::new(dsl)),
            owner,
        )
    }

    /// Install a generated peer-comms handle and its matching trust owner.
    ///
    /// The owner token stays on the generated install path; callers receive no
    /// reusable token that could be copied into a handwritten handle.
    pub fn install_generated_on(
        dsl: Arc<HandleDslAuthority>,
        target: &(dyn meerkat_core::handles::PeerCommsInstallTarget + '_),
    ) -> Result<(), String> {
        Self::generated_install_factory(dsl)?.install_on_target(target)
    }

    /// Construct a handle backed by an ephemeral DSL authority.
    ///
    /// See [`RuntimeTurnStateHandle::ephemeral`].
    pub fn ephemeral() -> Self {
        Self::new(Arc::new(HandleDslAuthority::ephemeral()))
    }
}

fn lifecycle_to_dsl(
    kind: meerkat_core::comms::PeerLifecycleKind,
) -> mm_dsl::PeerIngressLifecycleClass {
    match kind {
        meerkat_core::comms::PeerLifecycleKind::PeerAdded => {
            mm_dsl::PeerIngressLifecycleClass::PeerAdded
        }
        meerkat_core::comms::PeerLifecycleKind::PeerRetired => {
            mm_dsl::PeerIngressLifecycleClass::PeerRetired
        }
        meerkat_core::comms::PeerLifecycleKind::PeerUnwired => {
            mm_dsl::PeerIngressLifecycleClass::PeerUnwired
        }
    }
}

fn lifecycle_from_dsl(
    kind: mm_dsl::PeerIngressLifecycleClass,
) -> meerkat_core::comms::PeerLifecycleKind {
    match kind {
        mm_dsl::PeerIngressLifecycleClass::PeerAdded => {
            meerkat_core::comms::PeerLifecycleKind::PeerAdded
        }
        mm_dsl::PeerIngressLifecycleClass::PeerRetired => {
            meerkat_core::comms::PeerLifecycleKind::PeerRetired
        }
        mm_dsl::PeerIngressLifecycleClass::PeerUnwired => {
            meerkat_core::comms::PeerLifecycleKind::PeerUnwired
        }
    }
}

fn response_status_to_dsl(
    status: meerkat_core::ResponseStatus,
) -> mm_dsl::PeerIngressResponseStatus {
    match status {
        meerkat_core::ResponseStatus::Accepted => mm_dsl::PeerIngressResponseStatus::Accepted,
        meerkat_core::ResponseStatus::Completed => mm_dsl::PeerIngressResponseStatus::Completed,
        meerkat_core::ResponseStatus::Failed => mm_dsl::PeerIngressResponseStatus::Failed,
    }
}

fn lifecycle_peer_param(params: &serde_json::Value) -> Option<String> {
    params
        .get("peer")
        .and_then(serde_json::Value::as_str)
        .map(ToOwned::to_owned)
}

fn terminality_from_dsl(
    terminality: mm_dsl::PeerIngressResponseTerminality,
) -> meerkat_core::TerminalityClass {
    match terminality {
        mm_dsl::PeerIngressResponseTerminality::Progress => {
            meerkat_core::TerminalityClass::Progress
        }
        mm_dsl::PeerIngressResponseTerminality::TerminalCompleted => {
            meerkat_core::TerminalityClass::Terminal {
                disposition: meerkat_core::TerminalDisposition::Completed,
            }
        }
        mm_dsl::PeerIngressResponseTerminality::TerminalFailed => {
            meerkat_core::TerminalityClass::Terminal {
                disposition: meerkat_core::TerminalDisposition::Failed,
            }
        }
    }
}

fn phase_from_dsl(
    phase: mm_dsl::PeerIngressAuthorityPhaseClass,
) -> meerkat_core::PeerIngressAuthorityPhase {
    phase.into()
}

fn kind_to_dsl(kind: meerkat_core::PeerIngressKind) -> mm_dsl::PeerIngressAdmittedKind {
    match kind {
        meerkat_core::PeerIngressKind::Message => mm_dsl::PeerIngressAdmittedKind::Message,
        meerkat_core::PeerIngressKind::Request => mm_dsl::PeerIngressAdmittedKind::Request,
        meerkat_core::PeerIngressKind::Response => mm_dsl::PeerIngressAdmittedKind::Response,
        meerkat_core::PeerIngressKind::Ack => mm_dsl::PeerIngressAdmittedKind::Ack,
        meerkat_core::PeerIngressKind::PlainEvent => mm_dsl::PeerIngressAdmittedKind::PlainEvent,
    }
}

fn auth_to_dsl(auth: meerkat_core::PeerIngressAuthDecision) -> mm_dsl::PeerIngressAuthClass {
    match auth {
        meerkat_core::PeerIngressAuthDecision::Required => mm_dsl::PeerIngressAuthClass::Required,
        meerkat_core::PeerIngressAuthDecision::Exempt(
            meerkat_core::PeerIngressAuthExemption::SupervisorBridge,
        ) => mm_dsl::PeerIngressAuthClass::SupervisorBridgeExempt,
    }
}

fn external_envelope_signal(facts: &PeerIngressEnvelopeFacts) -> mm_dsl::MeerkatMachineSignal {
    let (
        envelope_kind,
        request_intent,
        lifecycle_kind,
        lifecycle_peer_param,
        response_status,
        in_reply_to,
    ) = match &facts.kind {
        meerkat_core::PeerIngressEnvelopeKind::Message { .. } => (
            mm_dsl::PeerIngressEnvelopeClass::Message,
            String::new(),
            mm_dsl::PeerIngressLifecycleClass::PeerAdded,
            None,
            mm_dsl::PeerIngressResponseStatus::Accepted,
            String::new(),
        ),
        meerkat_core::PeerIngressEnvelopeKind::Request { intent, params } => (
            mm_dsl::PeerIngressEnvelopeClass::Request,
            intent.clone(),
            mm_dsl::PeerIngressLifecycleClass::PeerAdded,
            lifecycle_peer_param(params),
            mm_dsl::PeerIngressResponseStatus::Accepted,
            String::new(),
        ),
        meerkat_core::PeerIngressEnvelopeKind::Lifecycle { kind, params } => (
            mm_dsl::PeerIngressEnvelopeClass::Lifecycle,
            String::new(),
            lifecycle_to_dsl(*kind),
            lifecycle_peer_param(params),
            mm_dsl::PeerIngressResponseStatus::Accepted,
            String::new(),
        ),
        meerkat_core::PeerIngressEnvelopeKind::Response {
            in_reply_to: reply_to,
            status,
            ..
        } => (
            mm_dsl::PeerIngressEnvelopeClass::Response,
            String::new(),
            mm_dsl::PeerIngressLifecycleClass::PeerAdded,
            None,
            response_status_to_dsl(*status),
            reply_to.clone(),
        ),
        meerkat_core::PeerIngressEnvelopeKind::Ack {
            in_reply_to: reply_to,
        } => (
            mm_dsl::PeerIngressEnvelopeClass::Ack,
            String::new(),
            mm_dsl::PeerIngressLifecycleClass::PeerAdded,
            None,
            mm_dsl::PeerIngressResponseStatus::Accepted,
            reply_to.clone(),
        ),
    };

    mm_dsl::MeerkatMachineSignal::ClassifyExternalEnvelope {
        item_id: facts.item_id.clone(),
        from_peer: facts.from_peer.clone(),
        envelope_kind,
        request_intent,
        lifecycle_kind,
        lifecycle_peer_param,
        response_status,
        in_reply_to,
    }
}

struct PeerIngressClassifiedEffect {
    class: mm_dsl::PeerIngressInputClass,
    kind: mm_dsl::PeerIngressAdmittedKind,
    auth: mm_dsl::PeerIngressAuthClass,
    lifecycle_kind: Option<mm_dsl::PeerIngressLifecycleClass>,
    lifecycle_peer: Option<String>,
    request_id: Option<String>,
    response_terminality: Option<mm_dsl::PeerIngressResponseTerminality>,
}

fn classified_effect(
    effects: Vec<mm_dsl::MeerkatMachineEffect>,
    context: &'static str,
) -> Result<PeerIngressClassifiedEffect, DslTransitionError> {
    effects
        .into_iter()
        .find_map(|effect| match effect {
            mm_dsl::MeerkatMachineEffect::PeerIngressClassified {
                class,
                kind,
                auth,
                lifecycle_kind,
                lifecycle_peer,
                request_id,
                response_terminality,
            } => Some(PeerIngressClassifiedEffect {
                class,
                kind,
                auth,
                lifecycle_kind,
                lifecycle_peer,
                request_id,
                response_terminality,
            }),
            _ => None,
        })
        .ok_or_else(|| {
            DslTransitionError::guard_rejected(
                context,
                "machine transition did not emit PeerIngressClassified",
            )
        })
}

struct PeerIngressReceiveResolvedEffect {
    outcome: mm_dsl::PeerIngressReceiveOutcomeClass,
    admission_diagnostic: Option<mm_dsl::PeerIngressAdmissionDiagnosticClass>,
    phase: mm_dsl::PeerIngressAuthorityPhaseClass,
}

fn receive_resolved_effect(
    effects: Vec<mm_dsl::MeerkatMachineEffect>,
    context: &'static str,
) -> Result<PeerIngressReceiveResolvedEffect, DslTransitionError> {
    effects
        .into_iter()
        .find_map(|effect| match effect {
            mm_dsl::MeerkatMachineEffect::PeerIngressReceiveResolved {
                outcome,
                admission_diagnostic,
                phase,
            } => Some(PeerIngressReceiveResolvedEffect {
                outcome,
                admission_diagnostic,
                phase,
            }),
            _ => None,
        })
        .ok_or_else(|| {
            DslTransitionError::guard_rejected(
                context,
                "machine transition did not emit PeerIngressReceiveResolved",
            )
        })
}

fn dequeue_resolved_effect(
    effects: Vec<mm_dsl::MeerkatMachineEffect>,
    context: &'static str,
) -> Result<mm_dsl::PeerIngressAuthorityPhaseClass, DslTransitionError> {
    effects
        .into_iter()
        .find_map(|effect| match effect {
            mm_dsl::MeerkatMachineEffect::PeerIngressDequeueResolved { phase } => Some(phase),
            _ => None,
        })
        .ok_or_else(|| {
            DslTransitionError::guard_rejected(
                context,
                "machine transition did not emit PeerIngressDequeueResolved",
            )
        })
}

fn classification_from_effect(
    effect: &PeerIngressClassifiedEffect,
) -> meerkat_core::PeerIngressClassification {
    let class = match effect.class {
        mm_dsl::PeerIngressInputClass::ActionableMessage => {
            meerkat_core::PeerInputClass::ActionableMessage
        }
        mm_dsl::PeerIngressInputClass::ActionableRequest => {
            meerkat_core::PeerInputClass::ActionableRequest
        }
        mm_dsl::PeerIngressInputClass::ResponseProgress => {
            meerkat_core::PeerInputClass::ResponseProgress
        }
        mm_dsl::PeerIngressInputClass::ResponseTerminal => {
            meerkat_core::PeerInputClass::ResponseTerminal
        }
        mm_dsl::PeerIngressInputClass::PeerLifecycleAdded => {
            meerkat_core::PeerInputClass::PeerLifecycleAdded
        }
        mm_dsl::PeerIngressInputClass::PeerLifecycleRetired => {
            meerkat_core::PeerInputClass::PeerLifecycleRetired
        }
        mm_dsl::PeerIngressInputClass::PeerLifecycleUnwired => {
            meerkat_core::PeerInputClass::PeerLifecycleUnwired
        }
        mm_dsl::PeerIngressInputClass::SilentRequest => meerkat_core::PeerInputClass::SilentRequest,
        mm_dsl::PeerIngressInputClass::Ack => meerkat_core::PeerInputClass::Ack,
        mm_dsl::PeerIngressInputClass::PlainEvent => meerkat_core::PeerInputClass::PlainEvent,
    };
    let kind = match effect.kind {
        mm_dsl::PeerIngressAdmittedKind::Message => meerkat_core::PeerIngressKind::Message,
        mm_dsl::PeerIngressAdmittedKind::Request => meerkat_core::PeerIngressKind::Request,
        mm_dsl::PeerIngressAdmittedKind::Response => meerkat_core::PeerIngressKind::Response,
        mm_dsl::PeerIngressAdmittedKind::Ack => meerkat_core::PeerIngressKind::Ack,
        mm_dsl::PeerIngressAdmittedKind::PlainEvent => meerkat_core::PeerIngressKind::PlainEvent,
    };
    let auth = match effect.auth {
        mm_dsl::PeerIngressAuthClass::Required => meerkat_core::PeerIngressAuthDecision::Required,
        mm_dsl::PeerIngressAuthClass::SupervisorBridgeExempt => {
            meerkat_core::PeerIngressAuthDecision::Exempt(
                meerkat_core::PeerIngressAuthExemption::SupervisorBridge,
            )
        }
    };

    meerkat_core::PeerIngressClassification {
        class,
        kind,
        auth,
        lifecycle_kind: effect.lifecycle_kind.map(lifecycle_from_dsl),
        response_terminality: effect.response_terminality.map(terminality_from_dsl),
    }
}

impl PeerCommsHandle for RuntimePeerCommsHandle {
    fn classify_external_envelope(
        &self,
        facts: PeerIngressEnvelopeFacts,
    ) -> Result<PeerIngressAdmission, DslTransitionError> {
        let context = "PeerCommsHandle::classify_external_envelope";
        let effects = self
            .dsl
            .apply_signal_with_effects(external_envelope_signal(&facts), context)?;
        let effect = classified_effect(effects, context)?;
        let classification = classification_from_effect(&effect);
        Ok(PeerIngressAdmission {
            rendered_text: meerkat_core::render_peer_ingress_admitted_text(&facts, &classification),
            classification,
            lifecycle_peer: effect.lifecycle_peer,
            request_id: effect.request_id,
        })
    }

    fn classify_plain_event(
        &self,
        facts: PeerIngressPlainEventFacts,
    ) -> Result<PeerIngressAdmission, DslTransitionError> {
        let context = "PeerCommsHandle::classify_plain_event";
        let effects = self.dsl.apply_signal_with_effects(
            mm_dsl::MeerkatMachineSignal::ClassifyPlainEvent {
                source_name: facts.source_name.clone(),
            },
            context,
        )?;
        let effect = classified_effect(effects, context)?;
        Ok(PeerIngressAdmission {
            classification: classification_from_effect(&effect),
            lifecycle_peer: effect.lifecycle_peer,
            request_id: effect.request_id,
            rendered_text: meerkat_core::interaction::format_external_event_projection(
                &facts.source_name,
                Some(&facts.body),
            ),
        })
    }

    fn resolve_peer_ingress_receive(
        &self,
        facts: PeerIngressReceiveFacts,
    ) -> Result<PeerIngressReceiveAuthority, DslTransitionError> {
        let context = "PeerCommsHandle::resolve_peer_ingress_receive";
        let effects = self.dsl.apply_input_with_effects(
            mm_dsl::MeerkatMachineInput::ResolvePeerIngressReceive {
                kind: kind_to_dsl(facts.kind),
                auth_required: facts.auth_required,
                auth_exempt: facts.auth_exempt,
                trusted: facts.trusted,
                queued_work_present: facts.queued_work_present,
                queue_closed: facts.queue_closed,
                queue_capacity_available: facts.queue_capacity_available,
            },
            context,
        )?;
        let effect = receive_resolved_effect(effects, context)?;
        Ok(PeerIngressReceiveAuthority {
            outcome: effect.outcome.into(),
            admission_diagnostic: effect.admission_diagnostic.map(Into::into),
            authority_phase: phase_from_dsl(effect.phase),
        })
    }

    fn resolve_peer_ingress_dequeue(
        &self,
        facts: PeerIngressDequeueFacts,
    ) -> Result<PeerIngressDequeueAuthority, DslTransitionError> {
        let context = "PeerCommsHandle::resolve_peer_ingress_dequeue";
        let effects = self.dsl.apply_input_with_effects(
            mm_dsl::MeerkatMachineInput::ResolvePeerIngressDequeue {
                kind: kind_to_dsl(facts.kind),
                auth: auth_to_dsl(facts.auth),
                queued_work_remaining: facts.queued_work_remaining,
            },
            context,
        )?;
        let phase = dequeue_resolved_effect(effects, context)?;
        Ok(PeerIngressDequeueAuthority {
            authority_phase: phase_from_dsl(phase),
        })
    }

    fn set_peer_ingress_context(&self, keep_alive: bool) -> Result<(), DslTransitionError> {
        // intra-machine: no route; dispatcher not applicable (handle targets the meerkat DSL directly, not a CompositionDispatcher seam)
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::SetPeerIngressContext { keep_alive },
            "PeerCommsHandle::set_peer_ingress_context",
        )
    }

    fn install_generated_peer_comms_on_target(
        &self,
        expected_owner: &meerkat_core::comms::GeneratedPeerCommsOwnerToken,
        target: &(dyn meerkat_core::handles::PeerCommsInstallTarget + '_),
    ) -> Result<(), String> {
        let target_endpoint = target.generated_peer_comms_target_endpoint()?;
        let mut guard = self
            .dsl
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let previous_endpoint = guard.state().local_endpoint.clone();
        let publish_input = mm_dsl::MeerkatMachineInput::PublishLocalEndpoint {
            endpoint: mm_dsl::PeerEndpoint::from(&target_endpoint),
        };
        crate::meerkat_machine_types::MeerkatMachineFieldlessRuntimeInternalInput::reject_raw_dsl_input(
            &publish_input,
        )
        .map_err(|reason| {
            DslTransitionError::no_matching(
                "PeerCommsHandle::install_generated_peer_comms_on_target",
                reason,
            )
            .to_string()
        })?;
        mm_dsl::MeerkatMachineMutator::apply(&mut *guard, publish_input).map_err(|error| {
            super::map_kernel_error(
                error,
                "PeerCommsHandle::install_generated_peer_comms_on_target",
            )
            .to_string()
        })?;
        let approved_peer_id = guard
            .state()
            .local_endpoint
            .as_ref()
            .map(|endpoint| endpoint.peer_id.clone())
            .ok_or_else(|| {
                "generated peer-comms install target authority did not publish local endpoint"
                    .to_string()
            })?;
        let approved_peer_id = meerkat_core::comms::PeerId::parse(approved_peer_id.as_str())
            .map_err(|error| {
                format!("generated peer-comms install target peer_id invalid: {error}")
            })?;
        let install = crate::protocol_comms_trust_reconcile::generated_peer_comms_install(
            Arc::new(Self::new(Arc::clone(&self.dsl))),
            guard.generated_authority_owner_token(),
            approved_peer_id,
        )?;
        if !install.owner_token().same_owner(expected_owner) {
            restore_local_endpoint(&mut guard, previous_endpoint)?;
            return Err(
                "generated peer-comms install came from a different MeerkatMachine owner"
                    .to_string(),
            );
        }
        if let Err(error) = target.install_generated_peer_comms_handle(install) {
            let rollback = restore_local_endpoint(&mut guard, previous_endpoint);
            if let Err(rollback_error) = rollback {
                return Err(format!(
                    "{error}; generated local endpoint rollback failed: {rollback_error}"
                ));
            }
            return Err(error);
        }
        Ok(())
    }
}

fn restore_local_endpoint(
    guard: &mut mm_dsl::MeerkatMachineAuthority,
    previous_endpoint: Option<mm_dsl::PeerEndpoint>,
) -> Result<(), String> {
    let input = match previous_endpoint {
        Some(endpoint) => mm_dsl::MeerkatMachineInput::PublishLocalEndpoint { endpoint },
        None => mm_dsl::MeerkatMachineInput::ClearLocalEndpoint,
    };
    crate::meerkat_machine_types::MeerkatMachineFieldlessRuntimeInternalInput::reject_raw_dsl_input(
        &input,
    )
    .map_err(|reason| {
        DslTransitionError::no_matching("PeerCommsHandle::restore_local_endpoint", reason)
            .to_string()
    })?;
    mm_dsl::MeerkatMachineMutator::apply(guard, input)
        .map(|_| ())
        .map_err(|error| {
            super::map_kernel_error(error, "PeerCommsHandle::restore_local_endpoint").to_string()
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::sync::Mutex;

    struct RejectingPeerCommsInstallTarget {
        descriptor: meerkat_core::comms::TrustedPeerDescriptor,
        notify: Arc<tokio::sync::Notify>,
    }

    impl RejectingPeerCommsInstallTarget {
        fn new(descriptor: meerkat_core::comms::TrustedPeerDescriptor) -> Self {
            Self {
                descriptor,
                notify: Arc::new(tokio::sync::Notify::new()),
            }
        }
    }

    #[async_trait::async_trait]
    impl meerkat_core::agent::CommsRuntime for RejectingPeerCommsInstallTarget {
        async fn drain_messages(&self) -> Vec<String> {
            Vec::new()
        }

        fn inbox_notify(&self) -> Arc<tokio::sync::Notify> {
            Arc::clone(&self.notify)
        }

        fn peer_id(&self) -> Option<meerkat_core::comms::PeerId> {
            Some(self.descriptor.peer_id)
        }

        fn public_key_bytes(&self) -> Option<[u8; 32]> {
            Some(self.descriptor.pubkey)
        }

        fn comms_name(&self) -> Option<String> {
            Some(self.descriptor.name.as_str().to_string())
        }

        fn advertised_address(&self) -> Option<String> {
            Some(self.descriptor.address.to_string())
        }
    }

    impl meerkat_core::handles::PeerCommsInstallTarget for RejectingPeerCommsInstallTarget {
        fn install_generated_peer_comms_handle(
            &self,
            _install: meerkat_core::handles::GeneratedPeerCommsInstall,
        ) -> Result<(), String> {
            Err("target rejected install".to_string())
        }
    }

    fn peer_descriptor(name: &str, seed: u8) -> meerkat_core::comms::TrustedPeerDescriptor {
        let pubkey = [seed; 32];
        let peer_id = meerkat_core::comms::PeerId::from_ed25519_pubkey(&pubkey);
        meerkat_core::comms::TrustedPeerDescriptor::unsigned_with_pubkey(
            name,
            peer_id.to_string(),
            pubkey,
            format!("inproc://{name}"),
        )
        .expect("valid peer descriptor")
    }

    fn handle_for_phase(phase: mm_dsl::MeerkatPhase) -> RuntimePeerCommsHandle {
        let state = mm_dsl::MeerkatMachineState {
            lifecycle_phase: phase,
            session_id: Some(mm_dsl::SessionId("session-1".to_string())),
            ..Default::default()
        };
        let authority = Arc::new(Mutex::new(
            mm_dsl::MeerkatMachineAuthority::recover_from_state(state)
                .expect("test MeerkatMachine state must be recoverable"),
        ));
        RuntimePeerCommsHandle::new(Arc::new(HandleDslAuthority::from_shared(authority)))
    }

    #[test]
    fn peer_comms_install_rejection_restores_generated_local_endpoint() {
        for previous in [None, Some(peer_descriptor("previous", 7))] {
            let rejected = peer_descriptor("rejected", 9);
            let expected_previous = previous.as_ref().map(mm_dsl::PeerEndpoint::from);
            let state = mm_dsl::MeerkatMachineState {
                lifecycle_phase: mm_dsl::MeerkatPhase::Attached,
                session_id: Some(mm_dsl::SessionId("session-1".to_string())),
                local_endpoint: expected_previous.clone(),
                ..Default::default()
            };
            let authority = Arc::new(Mutex::new(
                mm_dsl::MeerkatMachineAuthority::recover_from_state(state)
                    .expect("test MeerkatMachine state must be recoverable"),
            ));
            let dsl = Arc::new(HandleDslAuthority::from_shared(authority));
            let factory = RuntimePeerCommsHandle::generated_install_factory(Arc::clone(&dsl))
                .expect("generated peer-comms install factory");
            let target = RejectingPeerCommsInstallTarget::new(rejected);
            let target: &dyn meerkat_core::handles::PeerCommsInstallTarget = &target;

            let error = factory
                .install_on_target(target)
                .expect_err("target rejection should fail install");

            assert!(
                error.contains("target rejected install"),
                "unexpected install error: {error}"
            );
            assert_eq!(dsl.snapshot_state().local_endpoint, expected_previous);
        }
    }

    #[test]
    fn runtime_peer_comms_handle_classifies_from_dsl_silent_intents() {
        let state = mm_dsl::MeerkatMachineState {
            lifecycle_phase: mm_dsl::MeerkatPhase::Attached,
            session_id: Some(mm_dsl::SessionId("session-1".to_string())),
            silent_intent_overrides: BTreeSet::from(["probe.silent".to_string()]),
            ..Default::default()
        };
        let authority = Arc::new(Mutex::new(
            mm_dsl::MeerkatMachineAuthority::recover_from_state(state)
                .expect("test MeerkatMachine state must be recoverable"),
        ));
        let handle =
            RuntimePeerCommsHandle::new(Arc::new(HandleDslAuthority::from_shared(authority)));

        let admission = handle
            .classify_external_envelope(PeerIngressEnvelopeFacts {
                item_id: "request-1".to_string(),
                from_peer: "peer-1".to_string(),
                from_peer_id: meerkat_core::comms::PeerId::new(),
                kind: meerkat_core::PeerIngressEnvelopeKind::Request {
                    intent: "probe.silent".to_string(),
                    params: serde_json::json!({}),
                },
            })
            .expect("attached session should classify peer ingress");

        assert_eq!(
            admission.classification.class,
            meerkat_core::PeerInputClass::SilentRequest
        );
        assert_eq!(
            admission.classification.auth,
            meerkat_core::PeerIngressAuthDecision::Required
        );
        assert_eq!(admission.request_id.as_deref(), Some("request-1"));
    }

    #[test]
    fn runtime_peer_comms_handle_lifecycle_subject_is_selected_by_machine() {
        let state = mm_dsl::MeerkatMachineState {
            lifecycle_phase: mm_dsl::MeerkatPhase::Attached,
            session_id: Some(mm_dsl::SessionId("session-1".to_string())),
            ..Default::default()
        };
        let authority = Arc::new(Mutex::new(
            mm_dsl::MeerkatMachineAuthority::recover_from_state(state)
                .expect("test MeerkatMachine state must be recoverable"),
        ));
        let handle =
            RuntimePeerCommsHandle::new(Arc::new(HandleDslAuthority::from_shared(authority)));

        let with_param = handle
            .classify_external_envelope(PeerIngressEnvelopeFacts {
                item_id: "request-param".to_string(),
                from_peer: "orchestrator".to_string(),
                from_peer_id: meerkat_core::comms::PeerId::new(),
                kind: meerkat_core::PeerIngressEnvelopeKind::Request {
                    intent: "mob.peer_added".to_string(),
                    params: serde_json::json!({ "peer": "worker-1" }),
                },
            })
            .expect("machine should classify lifecycle request");
        assert_eq!(with_param.lifecycle_peer.as_deref(), Some("worker-1"));

        let without_param = handle
            .classify_external_envelope(PeerIngressEnvelopeFacts {
                item_id: "request-fallback".to_string(),
                from_peer: "orchestrator".to_string(),
                from_peer_id: meerkat_core::comms::PeerId::new(),
                kind: meerkat_core::PeerIngressEnvelopeKind::Request {
                    intent: "mob.peer_retired".to_string(),
                    params: serde_json::json!({}),
                },
            })
            .expect("machine should classify lifecycle request fallback");
        assert_eq!(
            without_param.lifecycle_peer.as_deref(),
            Some("orchestrator")
        );

        let empty_param = handle
            .classify_external_envelope(PeerIngressEnvelopeFacts {
                item_id: "lifecycle-empty".to_string(),
                from_peer: "orchestrator".to_string(),
                from_peer_id: meerkat_core::comms::PeerId::new(),
                kind: meerkat_core::PeerIngressEnvelopeKind::Lifecycle {
                    kind: meerkat_core::comms::PeerLifecycleKind::PeerUnwired,
                    params: serde_json::json!({ "peer": "" }),
                },
            })
            .expect("machine should classify lifecycle event fallback");
        assert_eq!(empty_param.lifecycle_peer.as_deref(), Some("orchestrator"));
    }

    #[test]
    fn runtime_peer_comms_handle_classifies_idle_lifecycle_without_opening_peer_work() {
        let handle = handle_for_phase(mm_dsl::MeerkatPhase::Idle);

        let retired_notice = handle
            .classify_external_envelope(PeerIngressEnvelopeFacts {
                item_id: "lifecycle-retired".to_string(),
                from_peer: "orchestrator".to_string(),
                from_peer_id: meerkat_core::comms::PeerId::new(),
                kind: meerkat_core::PeerIngressEnvelopeKind::Lifecycle {
                    kind: meerkat_core::comms::PeerLifecycleKind::PeerRetired,
                    params: serde_json::json!({ "peer": "worker-1" }),
                },
            })
            .expect("idle live session should classify mob lifecycle notices");
        assert_eq!(
            retired_notice.classification.class,
            meerkat_core::PeerInputClass::PeerLifecycleRetired
        );
        assert_eq!(
            retired_notice.classification.lifecycle_kind,
            Some(meerkat_core::comms::PeerLifecycleKind::PeerRetired)
        );
        assert_eq!(retired_notice.lifecycle_peer.as_deref(), Some("worker-1"));
        assert_eq!(retired_notice.request_id, None);

        let added_request = handle
            .classify_external_envelope(PeerIngressEnvelopeFacts {
                item_id: "request-added".to_string(),
                from_peer: "orchestrator".to_string(),
                from_peer_id: meerkat_core::comms::PeerId::new(),
                kind: meerkat_core::PeerIngressEnvelopeKind::Request {
                    intent: "mob.peer_added".to_string(),
                    params: serde_json::json!({ "peer": "worker-2" }),
                },
            })
            .expect("idle live session should classify lifecycle requests");
        assert_eq!(
            added_request.classification.class,
            meerkat_core::PeerInputClass::PeerLifecycleAdded
        );
        assert_eq!(added_request.lifecycle_peer.as_deref(), Some("worker-2"));
        assert_eq!(added_request.request_id.as_deref(), Some("request-added"));

        let work_admission = handle.classify_external_envelope(PeerIngressEnvelopeFacts {
            item_id: "message-1".to_string(),
            from_peer: "peer-1".to_string(),
            from_peer_id: meerkat_core::comms::PeerId::new(),
            kind: meerkat_core::PeerIngressEnvelopeKind::Message {
                body: "wake up".to_string(),
            },
        });
        assert!(
            work_admission.is_err(),
            "idle lifecycle admission must not reopen normal peer work ingress"
        );
    }

    #[test]
    fn runtime_peer_comms_handle_classifies_idle_supervisor_bridge() {
        let handle = handle_for_phase(mm_dsl::MeerkatPhase::Idle);

        let admission = handle
            .classify_external_envelope(PeerIngressEnvelopeFacts {
                item_id: "supervisor-request".to_string(),
                from_peer: "mob/__mob_supervisor__".to_string(),
                from_peer_id: meerkat_core::comms::PeerId::new(),
                kind: meerkat_core::PeerIngressEnvelopeKind::Request {
                    intent: "supervisor.bridge".to_string(),
                    params: serde_json::json!({}),
                },
            })
            .expect("idle session should classify supervisor bridge requests");
        assert_eq!(
            admission.classification.class,
            meerkat_core::PeerInputClass::ActionableRequest
        );
        assert_eq!(
            admission.classification.auth,
            meerkat_core::PeerIngressAuthDecision::Exempt(
                meerkat_core::PeerIngressAuthExemption::SupervisorBridge
            )
        );

        let state = mm_dsl::MeerkatMachineState {
            lifecycle_phase: mm_dsl::MeerkatPhase::Idle,
            session_id: Some(mm_dsl::SessionId("session-1".to_string())),
            silent_intent_overrides: BTreeSet::from(["supervisor.bridge".to_string()]),
            ..Default::default()
        };
        let authority = Arc::new(Mutex::new(
            mm_dsl::MeerkatMachineAuthority::recover_from_state(state)
                .expect("test MeerkatMachine state must be recoverable"),
        ));
        let silent_handle =
            RuntimePeerCommsHandle::new(Arc::new(HandleDslAuthority::from_shared(authority)));

        let silent = silent_handle
            .classify_external_envelope(PeerIngressEnvelopeFacts {
                item_id: "supervisor-silent".to_string(),
                from_peer: "mob/__mob_supervisor__".to_string(),
                from_peer_id: meerkat_core::comms::PeerId::new(),
                kind: meerkat_core::PeerIngressEnvelopeKind::Request {
                    intent: "supervisor.bridge".to_string(),
                    params: serde_json::json!({}),
                },
            })
            .expect("idle session should classify silent supervisor bridge requests");
        assert_eq!(
            silent.classification.class,
            meerkat_core::PeerInputClass::SilentRequest
        );
        assert_eq!(
            silent.classification.auth,
            meerkat_core::PeerIngressAuthDecision::Exempt(
                meerkat_core::PeerIngressAuthExemption::SupervisorBridge
            )
        );
    }

    #[test]
    fn runtime_peer_comms_handle_drains_terminal_cleanup_without_reopening_topology_adds() {
        for phase in [mm_dsl::MeerkatPhase::Retired, mm_dsl::MeerkatPhase::Stopped] {
            let handle = handle_for_phase(phase);

            let retired_notice = handle
                .classify_external_envelope(PeerIngressEnvelopeFacts {
                    item_id: "lifecycle-retired".to_string(),
                    from_peer: "orchestrator".to_string(),
                    from_peer_id: meerkat_core::comms::PeerId::new(),
                    kind: meerkat_core::PeerIngressEnvelopeKind::Lifecycle {
                        kind: meerkat_core::comms::PeerLifecycleKind::PeerRetired,
                        params: serde_json::json!({ "peer": "worker-1" }),
                    },
                })
                .expect("terminal sessions should drain peer-retired cleanup notices");
            assert_eq!(
                retired_notice.classification.class,
                meerkat_core::PeerInputClass::PeerLifecycleRetired
            );

            let unwired_request = handle
                .classify_external_envelope(PeerIngressEnvelopeFacts {
                    item_id: "request-unwired".to_string(),
                    from_peer: "orchestrator".to_string(),
                    from_peer_id: meerkat_core::comms::PeerId::new(),
                    kind: meerkat_core::PeerIngressEnvelopeKind::Request {
                        intent: "mob.peer_unwired".to_string(),
                        params: serde_json::json!({ "peer": "worker-2" }),
                    },
                })
                .expect("terminal sessions should drain peer-unwired cleanup requests");
            assert_eq!(
                unwired_request.classification.class,
                meerkat_core::PeerInputClass::PeerLifecycleUnwired
            );

            let added_notice = handle.classify_external_envelope(PeerIngressEnvelopeFacts {
                item_id: "lifecycle-added".to_string(),
                from_peer: "orchestrator".to_string(),
                from_peer_id: meerkat_core::comms::PeerId::new(),
                kind: meerkat_core::PeerIngressEnvelopeKind::Lifecycle {
                    kind: meerkat_core::comms::PeerLifecycleKind::PeerAdded,
                    params: serde_json::json!({ "peer": "worker-3" }),
                },
            });
            assert!(
                added_notice.is_err(),
                "terminal cleanup admission must not accept new peer topology"
            );
        }
    }

    #[test]
    fn runtime_peer_comms_handle_resolves_receive_authority_from_dsl() {
        let handle = handle_for_phase(mm_dsl::MeerkatPhase::Attached);

        let admitted = handle
            .resolve_peer_ingress_receive(PeerIngressReceiveFacts {
                kind: meerkat_core::PeerIngressKind::Request,
                current_phase: meerkat_core::PeerIngressAuthorityPhase::Absent,
                auth_required: true,
                auth_exempt: false,
                trusted: true,
                queued_work_present: false,
                queue_closed: false,
                queue_capacity_available: true,
            })
            .expect("trusted receive should resolve");
        assert_eq!(
            admitted.outcome,
            meerkat_core::PeerIngressReceiveOutcome::Admitted
        );
        assert_eq!(
            admitted.admission_diagnostic,
            Some(meerkat_core::PeerIngressAdmissionDiagnostic::TrustedAtAdmission)
        );
        assert_eq!(
            admitted.authority_phase,
            meerkat_core::PeerIngressAuthorityPhase::Received
        );

        let dropped = handle
            .resolve_peer_ingress_receive(PeerIngressReceiveFacts {
                kind: meerkat_core::PeerIngressKind::Request,
                current_phase: meerkat_core::PeerIngressAuthorityPhase::Absent,
                auth_required: true,
                auth_exempt: false,
                trusted: false,
                queued_work_present: false,
                queue_closed: false,
                queue_capacity_available: true,
            })
            .expect("untrusted receive should resolve as a typed drop");
        assert_eq!(
            dropped.outcome,
            meerkat_core::PeerIngressReceiveOutcome::DroppedUntrustedSender
        );
        assert_eq!(
            dropped.authority_phase,
            meerkat_core::PeerIngressAuthorityPhase::Dropped
        );
    }

    #[test]
    fn runtime_peer_comms_handle_resolves_dequeue_phase_from_dsl() {
        let handle = handle_for_phase(mm_dsl::MeerkatPhase::Attached);

        handle
            .resolve_peer_ingress_receive(PeerIngressReceiveFacts {
                kind: meerkat_core::PeerIngressKind::Request,
                current_phase: meerkat_core::PeerIngressAuthorityPhase::Absent,
                auth_required: true,
                auth_exempt: false,
                trusted: true,
                queued_work_present: false,
                queue_closed: false,
                queue_capacity_available: true,
            })
            .expect("trusted receive should seed Received phase");

        let retained = handle
            .resolve_peer_ingress_dequeue(PeerIngressDequeueFacts {
                kind: meerkat_core::PeerIngressKind::Request,
                auth: meerkat_core::PeerIngressAuthDecision::Required,
                queued_work_remaining: true,
            })
            .expect("dequeue with queued work should resolve");
        assert_eq!(
            retained.authority_phase,
            meerkat_core::PeerIngressAuthorityPhase::Received
        );

        let delivered = handle
            .resolve_peer_ingress_dequeue(PeerIngressDequeueFacts {
                kind: meerkat_core::PeerIngressKind::Request,
                auth: meerkat_core::PeerIngressAuthDecision::Required,
                queued_work_remaining: false,
            })
            .expect("empty dequeue should resolve");
        assert_eq!(
            delivered.authority_phase,
            meerkat_core::PeerIngressAuthorityPhase::Delivered
        );
    }

    #[test]
    fn runtime_signal_builder_does_not_preselect_lifecycle_subject() {
        let source = include_str!("peer_comms.rs");
        let signal_builder = source
            .split("fn external_envelope_signal")
            .nth(1)
            .expect("signal builder should exist")
            .split("struct PeerIngressClassifiedEffect")
            .next()
            .expect("classified effect should follow signal builder");
        let forbidden_helper = ["peer", "lifecycle", "subject"].join("_");

        assert!(
            signal_builder.contains("lifecycle_peer_param"),
            "runtime should pass the parsed lifecycle peer candidate"
        );
        assert!(
            !signal_builder.contains(&forbidden_helper),
            "runtime must not call the lifecycle subject selector before the machine"
        );
        assert!(
            !signal_builder.contains("facts.from_peer.as_str()"),
            "fallback peer must remain a machine input fact, not a preselected subject"
        );
        assert!(
            !signal_builder.contains("unwrap_or"),
            "runtime must not choose a lifecycle subject fallback before the machine"
        );
    }
}
