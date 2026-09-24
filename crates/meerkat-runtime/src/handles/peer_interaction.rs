//! Runtime impl of [`meerkat_core::handles::PeerInteractionHandle`] (W1-A).
//!
//! Routes peer request/response lifecycle events into the session's
//! MeerkatMachine DSL (`pending_peer_requests` / `inbound_peer_requests`
//! substate maps) and fans the emitted `PeerInteractionCleanup` effects
//! out to the installed shell-side observer, so the subscriber / stream
//! registries update causally — the map is a strict projection of DSL
//! truth, not shadow state that happens to be updated lexically near each
//! terminal transition.

use std::sync::{Arc, RwLock, Weak};

use meerkat_core::handles::{
    DslTransitionError, PeerInteractionCleanupObserver, PeerInteractionHandle,
    PeerTerminalDisposition as CorePeerDisposition,
};
use meerkat_core::interaction::{TerminalDisposition, TerminalityClass};
use meerkat_core::peer_correlation::{
    InboundPeerRequestState as CoreInboundState, OutboundPeerRequestState as CoreOutboundState,
    PeerCorrelationId,
};
use meerkat_core::types::HandlingMode;

use super::HandleDslAuthority;
use crate::meerkat_machine::dsl as mm_dsl;

/// Runtime-backed [`PeerInteractionHandle`] impl.
///
/// Every trait method routes to the corresponding DSL input on the session's
/// shared MeerkatMachine authority. After the transition lands, emitted
/// effects are scanned for `PeerInteractionCleanup` and dispatched to the
/// installed [`PeerInteractionCleanupObserver`] (if any) — closing the
/// "terminal transition → effect → shell projection cleanup" loop.
///
/// The cleanup observer is held as a `Weak` reference. In production the
/// observer is the session's `CommsRuntime`, which in turn holds a strong
/// `Arc<dyn PeerInteractionHandle>` to this struct; storing the observer
/// strongly would create a cycle that prevents `CommsRuntime::drop` from
/// firing on session teardown (dropped listeners, leaked session-identity
/// claims, zombie `InprocRegistry` entries). `Weak` breaks the cycle —
/// once the runtime drops, `upgrade()` returns `None` and subsequent
/// effect dispatches become no-ops, which is the desired semantics
/// post-teardown.
pub struct RuntimePeerInteractionHandle {
    dsl: Arc<HandleDslAuthority>,
    cleanup_observer: RwLock<Option<Weak<dyn PeerInteractionCleanupObserver>>>,
}

impl std::fmt::Debug for RuntimePeerInteractionHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let observer_tag = self
            .cleanup_observer
            .read()
            .ok()
            .as_deref()
            .and_then(|o| o.as_ref().map(|_| "<observer>"));
        f.debug_struct("RuntimePeerInteractionHandle")
            .field("dsl", &self.dsl)
            .field("cleanup_observer", &observer_tag)
            .finish()
    }
}

impl RuntimePeerInteractionHandle {
    /// Construct a handle backed by the session's shared DSL authority.
    pub fn new(dsl: Arc<HandleDslAuthority>) -> Self {
        Self {
            dsl,
            cleanup_observer: RwLock::new(None),
        }
    }

    /// Construct a handle backed by an ephemeral DSL authority (tests /
    /// legacy recovery paths).
    pub fn ephemeral() -> Self {
        Self::new(Arc::new(HandleDslAuthority::ephemeral()))
    }

    fn apply_input_and_dispatch_cleanup(
        &self,
        input: mm_dsl::MeerkatMachineInput,
        context: &'static str,
    ) -> Result<(), DslTransitionError> {
        // Sample the cleanup observer slot UNDER the DSL lock so any
        // concurrent `install_cleanup_observer` is totally ordered vs
        // this transition (same pattern as `session_context.rs` closes
        // for PR #286's race). The observer callback runs OUTSIDE the
        // lock to avoid reentrancy with any shell-side state that calls
        // back into the handle. Each cleanup target is carried as an
        // `Ok(core_id)` / `Err(raw)` pair so the invalid-UUID diagnostic
        // still fires with the raw DSL string when dispatch happens
        // post-lock.
        type CleanupTarget = Result<PeerCorrelationId, String>;
        let dispatch: Option<(Arc<dyn PeerInteractionCleanupObserver>, Vec<CleanupTarget>)> = self
            .dsl
            .apply_input_with_effects_and_sample(input, context, |effects| {
                let observer_opt = self
                    .cleanup_observer
                    .read()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .as_ref()
                    .and_then(Weak::upgrade);
                let observer = observer_opt?;
                let targets: Vec<CleanupTarget> = effects
                    .iter()
                    .filter_map(|effect| match effect {
                        mm_dsl::MeerkatMachineEffect::PeerInteractionCleanup { corr_id } => {
                            Some(match dsl_corr_id_to_core(corr_id.clone()) {
                                Some(core_id) => Ok(core_id),
                                None => Err(corr_id.0.clone()),
                            })
                        }
                        _ => None,
                    })
                    .collect();
                Some((observer, targets))
            })?;
        if let Some((observer, targets)) = dispatch {
            for target in targets {
                match target {
                    Ok(core_id) => observer.on_peer_interaction_cleanup(core_id),
                    Err(raw) => tracing::error!(
                        raw = %raw,
                        context = context,
                        "PeerInteractionCleanup: DSL emitted a corr_id that is not a valid UUID — broken invariant; skipping observer dispatch"
                    ),
                }
            }
        }
        Ok(())
    }
}

fn dsl_corr_id_to_core(dsl_id: mm_dsl::PeerCorrelationId) -> Option<PeerCorrelationId> {
    // The DSL key is always produced by `From<PeerCorrelationId> for
    // mm_dsl::PeerCorrelationId`, which stringifies a UUID. Parse must
    // succeed on every canonical path; a parse failure here is a broken
    // invariant, not a recoverable condition. Return `None` so the caller
    // skips observer dispatch and logs — silently substituting nil would
    // cross-contaminate any real `corr_id 0` event.
    uuid::Uuid::parse_str(&dsl_id.0)
        .ok()
        .map(PeerCorrelationId::from_uuid)
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

fn terminality_from_dsl(terminality: mm_dsl::PeerIngressResponseTerminality) -> TerminalityClass {
    match terminality {
        mm_dsl::PeerIngressResponseTerminality::Progress => TerminalityClass::Progress,
        mm_dsl::PeerIngressResponseTerminality::TerminalCompleted => TerminalityClass::Terminal {
            disposition: TerminalDisposition::Completed,
        },
        mm_dsl::PeerIngressResponseTerminality::TerminalFailed => TerminalityClass::Terminal {
            disposition: TerminalDisposition::Failed,
        },
    }
}

fn handling_mode_from_dsl(lane: mm_dsl::InputLane) -> HandlingMode {
    match lane {
        mm_dsl::InputLane::Queue => HandlingMode::Queue,
        mm_dsl::InputLane::Steer => HandlingMode::Steer,
    }
}

fn peer_reply_classified_effect(
    effects: Vec<mm_dsl::MeerkatMachineEffect>,
    context: &'static str,
) -> Result<TerminalityClass, DslTransitionError> {
    effects
        .into_iter()
        .find_map(|effect| match effect {
            mm_dsl::MeerkatMachineEffect::PeerResponseReplyClassified {
                response_terminality,
            } => Some(terminality_from_dsl(response_terminality)),
            _ => None,
        })
        .ok_or_else(|| {
            DslTransitionError::guard_rejected(
                context,
                "machine transition did not emit PeerResponseReplyClassified",
            )
        })
}

impl PeerInteractionHandle for RuntimePeerInteractionHandle {
    fn request_sent(&self, corr_id: PeerCorrelationId) -> Result<(), DslTransitionError> {
        self.apply_input_and_dispatch_cleanup(
            mm_dsl::MeerkatMachineInput::PeerRequestSent {
                corr_id: corr_id.into(),
            },
            "PeerInteractionHandle::request_sent",
        )
    }

    fn response_progress(&self, corr_id: PeerCorrelationId) -> Result<(), DslTransitionError> {
        self.apply_input_and_dispatch_cleanup(
            mm_dsl::MeerkatMachineInput::PeerResponseProgressArrived {
                corr_id: corr_id.into(),
            },
            "PeerInteractionHandle::response_progress",
        )
    }

    fn response_terminal(
        &self,
        corr_id: PeerCorrelationId,
        disposition: CorePeerDisposition,
    ) -> Result<(), DslTransitionError> {
        self.apply_input_and_dispatch_cleanup(
            mm_dsl::MeerkatMachineInput::PeerResponseTerminalArrived {
                corr_id: corr_id.into(),
                disposition: disposition.into(),
            },
            "PeerInteractionHandle::response_terminal",
        )
    }

    fn response_rejected(&self, corr_id: PeerCorrelationId) -> Result<(), DslTransitionError> {
        self.apply_input_and_dispatch_cleanup(
            mm_dsl::MeerkatMachineInput::PeerResponseRejected {
                corr_id: corr_id.into(),
            },
            "PeerInteractionHandle::response_rejected",
        )
    }

    fn request_timed_out(&self, corr_id: PeerCorrelationId) -> Result<(), DslTransitionError> {
        self.apply_input_and_dispatch_cleanup(
            mm_dsl::MeerkatMachineInput::PeerRequestTimedOut {
                corr_id: corr_id.into(),
            },
            "PeerInteractionHandle::request_timed_out",
        )
    }

    fn request_send_failed(&self, corr_id: PeerCorrelationId) -> Result<(), DslTransitionError> {
        self.apply_input_and_dispatch_cleanup(
            mm_dsl::MeerkatMachineInput::PeerRequestSendFailed {
                corr_id: corr_id.into(),
            },
            "PeerInteractionHandle::request_send_failed",
        )
    }

    fn request_received(
        &self,
        corr_id: PeerCorrelationId,
        handling_mode: HandlingMode,
    ) -> Result<(), DslTransitionError> {
        self.apply_input_and_dispatch_cleanup(
            mm_dsl::MeerkatMachineInput::PeerRequestReceived {
                corr_id: corr_id.into(),
                handling_mode: mm_dsl::InputLane::from(handling_mode),
            },
            "PeerInteractionHandle::request_received",
        )
    }

    fn classify_response_reply(
        &self,
        status: meerkat_core::ResponseStatus,
    ) -> Result<TerminalityClass, DslTransitionError> {
        let context = "PeerInteractionHandle::classify_response_reply";
        let effects = self.dsl.apply_signal_with_effects(
            mm_dsl::MeerkatMachineSignal::ClassifyPeerResponseReply {
                status: response_status_to_dsl(status),
            },
            context,
        )?;
        peer_reply_classified_effect(effects, context)
    }

    fn response_replied(&self, corr_id: PeerCorrelationId) -> Result<(), DslTransitionError> {
        self.apply_input_and_dispatch_cleanup(
            mm_dsl::MeerkatMachineInput::PeerResponseReplied {
                corr_id: corr_id.into(),
            },
            "PeerInteractionHandle::response_replied",
        )
    }

    fn outbound_state(&self, corr_id: PeerCorrelationId) -> Option<CoreOutboundState> {
        let dsl_key: mm_dsl::PeerCorrelationId = corr_id.into();
        self.dsl
            .snapshot_state()
            .pending_peer_requests
            .get(&dsl_key)
            .copied()
            .map(Into::into)
    }

    fn inbound_state(&self, corr_id: PeerCorrelationId) -> Option<CoreInboundState> {
        let dsl_key: mm_dsl::PeerCorrelationId = corr_id.into();
        self.dsl
            .snapshot_state()
            .inbound_peer_requests
            .get(&dsl_key)
            .copied()
            .map(Into::into)
    }

    fn inbound_handling_mode(&self, corr_id: PeerCorrelationId) -> Option<HandlingMode> {
        let dsl_key: mm_dsl::PeerCorrelationId = corr_id.into();
        self.dsl
            .snapshot_state()
            .inbound_peer_request_lanes
            .get(&dsl_key)
            .copied()
            .map(handling_mode_from_dsl)
    }

    fn install_cleanup_observer(&self, observer: Arc<dyn PeerInteractionCleanupObserver>) {
        // Downgrade to a `Weak` so this handle does not keep the observer
        // (typically the session's `CommsRuntime`) alive. The caller retains
        // the canonical strong `Arc` via its own field; when the runtime is
        // dropped, the weak here fails to upgrade and cleanup dispatch
        // becomes a no-op — matching the "post-teardown, no more work"
        // semantics the shell-side projection expects.
        *self
            .cleanup_observer
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::downgrade(&observer));
    }
}
