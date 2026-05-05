//! Runtime impl of [`meerkat_core::handles::PeerCommsHandle`].

use std::sync::Arc;

use meerkat_core::handles::{DslTransitionError, PeerCommsHandle};
use meerkat_core::interaction::{
    PeerIngressAdmission, PeerIngressEnvelopeFacts, PeerIngressPlainEventFacts,
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

    fn set_peer_ingress_context(&self, keep_alive: bool) -> Result<(), DslTransitionError> {
        // intra-machine: no route; dispatcher not applicable (handle targets the meerkat DSL directly, not a CompositionDispatcher seam)
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::SetPeerIngressContext { keep_alive },
            "PeerCommsHandle::set_peer_ingress_context",
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::sync::Mutex;

    fn handle_for_phase(phase: mm_dsl::MeerkatPhase) -> RuntimePeerCommsHandle {
        let state = mm_dsl::MeerkatMachineState {
            lifecycle_phase: phase,
            session_id: Some(mm_dsl::SessionId("session-1".to_string())),
            ..Default::default()
        };
        let authority = Arc::new(Mutex::new(mm_dsl::MeerkatMachineAuthority::from_state(
            state,
        )));
        RuntimePeerCommsHandle::new(Arc::new(HandleDslAuthority::from_shared(authority)))
    }

    #[test]
    fn runtime_peer_comms_handle_classifies_from_dsl_silent_intents() {
        let state = mm_dsl::MeerkatMachineState {
            lifecycle_phase: mm_dsl::MeerkatPhase::Attached,
            session_id: Some(mm_dsl::SessionId("session-1".to_string())),
            silent_intent_overrides: BTreeSet::from(["probe.silent".to_string()]),
            ..Default::default()
        };
        let authority = Arc::new(Mutex::new(mm_dsl::MeerkatMachineAuthority::from_state(
            state,
        )));
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
        let authority = Arc::new(Mutex::new(mm_dsl::MeerkatMachineAuthority::from_state(
            state,
        )));
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
