//! Typed payloads for one cooperative model-boundary delivery.
//!
//! A live Steer input that reaches an active turn is delivered at the runner's
//! next exact `CallingLlm` boundary. Two delivery classes exist, and the
//! runtime machine (not the shell) chooses between them at admission:
//!
//! - [`TurnBoundaryDelivery::RequestOnly`]: request-local context projected
//!   into exactly one model request. It is never Session state.
//! - [`TurnBoundaryDelivery::DurableAppends`]: typed conversation appends the
//!   runner writes into the running turn's Session, visible to that request and
//!   every later request, and committed with the run exactly like tool
//!   results. The runtime consumes the input only with the run terminal.
//!
//! A durable delivery carries a [`CoreBoundaryDeliveryWitness`]: the one typed
//! fact the runtime reads at the run terminal to decide whether the append is
//! in the session image that survives the run.

use std::sync::{Arc, Mutex};

use super::identifiers::InputId;
use super::run_primitive::{ConversationAppend, ConversationAppendRole, TurnRequestContext};
use crate::types::{ContentInput, Message, TranscriptMessageIdentity};

/// One typed delivery offered to the next exact cooperative model boundary.
#[derive(Debug, Clone)]
pub enum TurnBoundaryDelivery {
    /// Request-local context for exactly one model request.
    RequestOnly(Vec<TurnRequestContext>),
    /// Typed conversation appends written into the running turn's Session.
    DurableAppends(DurableTurnBoundaryAppends),
}

impl TurnBoundaryDelivery {
    /// Whether this delivery writes Session state.
    #[must_use]
    pub fn is_durable(&self) -> bool {
        matches!(self, Self::DurableAppends(_))
    }
}

/// Why typed appends cannot be delivered into a running turn.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DurableTurnBoundaryAppendsError {
    /// A durable delivery needs at least one append.
    #[error("a durable boundary delivery needs at least one conversation append")]
    Empty,
    /// The role is not accepted in the middle of a conversation by every
    /// provider (`System`), or it would forge model or tool output
    /// (`Assistant`, `Tool`).
    #[error("conversation append role {role:?} cannot join a running turn")]
    RoleNotInTurnEligible { role: ConversationAppendRole },
    /// Only ordinary System appends may carry an append identity.
    #[error("conversation append identity is valid only for role=system")]
    IdentityOnNonSystemRole,
    /// The notice is a synthetic refresh projection
    /// ([`crate::types::SystemNoticeMessage::is_synthetic_refresh_projection`]):
    /// the runner's next boundary notice refresh replaces every notice of
    /// that kind, so it would not survive in the running turn's transcript.
    #[error(
        "system notice kind {kind:?} is a synthetic refresh projection and cannot join a running turn"
    )]
    RefreshProjectionNotice {
        kind: crate::types::SystemNoticeKind,
    },
    /// The append content cannot be lowered into its transcript role.
    #[error("{0}")]
    Lowering(String),
}

/// Whether one typed conversation append may be written into a running turn.
///
/// Reads typed structure only: the role, the identity slot and, for a
/// `SystemNotice`, the typed refresh-projection predicate. The accepted roles
/// are exactly those every provider accepts in the middle of a conversation
/// (`SystemNotice`, `User`, `InjectedContext`), without an append identity
/// (reserved for `System` rows). A `SystemNotice` that is a synthetic refresh
/// projection is refused: the next boundary's notice refresh would remove it
/// from the running turn after it was delivered. Runtime admission classifies
/// with this predicate, and [`DurableTurnBoundaryAppends::try_new`] enforces
/// the same rule, so the two can never disagree.
#[must_use]
pub fn conversation_append_joins_running_turn(append: &ConversationAppend) -> bool {
    in_turn_append_refusal(append).is_none()
}

fn in_turn_append_refusal(append: &ConversationAppend) -> Option<DurableTurnBoundaryAppendsError> {
    if append.identity.is_some() {
        return Some(DurableTurnBoundaryAppendsError::IdentityOnNonSystemRole);
    }
    match append.role {
        ConversationAppendRole::SystemNotice => {
            let notice = append.content.clone().into_system_notice_message();
            notice.is_synthetic_refresh_projection().then_some(
                DurableTurnBoundaryAppendsError::RefreshProjectionNotice { kind: notice.kind },
            )
        }
        ConversationAppendRole::User | ConversationAppendRole::InjectedContext => None,
        role @ (ConversationAppendRole::System
        | ConversationAppendRole::Assistant
        | ConversationAppendRole::Tool) => {
            Some(DurableTurnBoundaryAppendsError::RoleNotInTurnEligible { role })
        }
    }
}

/// Typed conversation appends of one runtime input, already lowered into the
/// exact transcript messages the runner will push.
///
/// Construction runs the same pure lowering the turn-start path performs for
/// these roles, so the runner-side write cannot fail after the runtime has
/// joined the input to the run. The accepted roles are exactly those every
/// provider accepts in the middle of a conversation: `SystemNotice`, `User`
/// and `InjectedContext`.
#[derive(Debug, Clone)]
pub struct DurableTurnBoundaryAppends {
    input_id: InputId,
    messages: Vec<Message>,
    model_projection: ContentInput,
    /// `PeerContentIngested` facts for the incoming comms blocks the appends
    /// carry, projected before lowering exactly as the turn-start path does,
    /// so both delivery paths publish the same ingestion events.
    peer_ingested_events: Vec<crate::event::AgentEvent>,
}

impl DurableTurnBoundaryAppends {
    /// Lower `appends` for delivery into a running turn.
    ///
    /// `transcript_identity` is the late input's own transcript identity (the
    /// identity its fallback follow-up turn would stamp on `User` and
    /// `InjectedContext` rows), so both delivery paths attribute those rows to
    /// the input rather than to the batch the run started with. The runner
    /// stamps `SystemNotice` application origin when it writes these ordered
    /// rows into the exact running session image.
    pub fn try_new(
        input_id: InputId,
        appends: Vec<ConversationAppend>,
        transcript_identity: Option<TranscriptMessageIdentity>,
    ) -> Result<Self, DurableTurnBoundaryAppendsError> {
        if appends.is_empty() {
            return Err(DurableTurnBoundaryAppendsError::Empty);
        }
        let model_projection =
            super::run_primitive::model_projection_content_input_from_conversation_appends(
                &appends,
            );
        let mut messages = Vec::with_capacity(appends.len());
        let mut peer_ingested_events = Vec::new();
        for append in appends {
            if let Some(refusal) = in_turn_append_refusal(&append) {
                return Err(refusal);
            }
            peer_ingested_events
                .extend(crate::event::peer_content_ingested_events(&append.content));
            let message = match append.role {
                ConversationAppendRole::SystemNotice => {
                    Message::SystemNotice(append.content.into_system_notice_message())
                }
                ConversationAppendRole::User => {
                    let mut message =
                        crate::agent::user_message_from_operator_renderable(append.content)
                            .map_err(|error| {
                                DurableTurnBoundaryAppendsError::Lowering(error.to_string())
                            })?;
                    if let Some(identity) = transcript_identity.as_ref() {
                        message.identity = identity.clone();
                    }
                    Message::User(message)
                }
                ConversationAppendRole::InjectedContext => {
                    let mut message =
                        crate::agent::injected_context_message_from_operator_renderable(
                            append.content,
                        )
                        .map_err(|error| {
                            DurableTurnBoundaryAppendsError::Lowering(error.to_string())
                        })?;
                    if let Some(identity) = transcript_identity.as_ref() {
                        message.identity = identity.clone();
                    }
                    Message::User(message)
                }
                role @ (ConversationAppendRole::System
                | ConversationAppendRole::Assistant
                | ConversationAppendRole::Tool) => {
                    return Err(DurableTurnBoundaryAppendsError::RoleNotInTurnEligible { role });
                }
            };
            messages.push(message);
        }
        Ok(Self {
            input_id,
            messages,
            model_projection,
            peer_ingested_events,
        })
    }

    /// The runtime input these appends belong to.
    #[must_use]
    pub fn input_id(&self) -> &InputId {
        &self.input_id
    }

    /// The lowered transcript messages, in append order.
    #[must_use]
    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    /// Model projection of the appends (the same projection a runtime input
    /// publishes in `RunStarted` when it starts a turn by itself).
    #[must_use]
    pub fn model_projection(&self) -> &ContentInput {
        &self.model_projection
    }

    /// `PeerContentIngested` events the runner publishes when it applies
    /// these appends (one per incoming comms block, in append order).
    #[must_use]
    pub fn peer_ingested_events(&self) -> &[crate::event::AgentEvent] {
        &self.peer_ingested_events
    }

    pub(crate) fn into_parts(self) -> DurableTurnBoundaryAppendParts {
        DurableTurnBoundaryAppendParts {
            input_id: self.input_id,
            messages: self.messages,
            model_projection: self.model_projection,
            peer_ingested_events: self.peer_ingested_events,
        }
    }
}

/// The owned pieces of one accepted durable delivery, split for the runner.
pub(crate) struct DurableTurnBoundaryAppendParts {
    pub(crate) input_id: InputId,
    pub(crate) messages: Vec<Message>,
    pub(crate) model_projection: ContentInput,
    pub(crate) peer_ingested_events: Vec<crate::event::AgentEvent>,
}

/// What became of one durable boundary delivery, as observed by the runtime
/// at the run terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoreBoundaryDeliveryOutcome {
    /// Not yet resolved. The run terminal never observes this value once the
    /// run guard has closed the run: every unresolved delivery is withdrawn.
    Pending,
    /// The runner wrote the appends into the running turn's Session.
    Applied,
    /// The runner never took the appends (the run ended, was cancelled, or
    /// refused durable deliveries at that boundary).
    Withdrawn,
    /// The runner wrote the appends, but the session image that carried them
    /// was discarded: an uncommitted image the owning service resyncs from
    /// durable authority, or a compaction rollback that restored a session
    /// captured before the append.
    Discarded,
}

#[derive(Debug)]
struct DeliveryWitnessState {
    outcome: CoreBoundaryDeliveryOutcome,
    /// Position of the apply in the actor's durable-apply order; `None` until
    /// the runner applies the delivery.
    apply_ordinal: Option<u64>,
}

/// Shared, monotonic witness of one durable boundary delivery.
///
/// `Pending` may become `Applied` (only by the runner) or `Withdrawn`;
/// `Applied` may become `Discarded`. `Withdrawn` and `Discarded` are final.
#[derive(Clone)]
pub struct CoreBoundaryDeliveryWitness {
    state: Arc<Mutex<DeliveryWitnessState>>,
}

impl std::fmt::Debug for CoreBoundaryDeliveryWitness {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CoreBoundaryDeliveryWitness")
            .field("outcome", &self.outcome())
            .finish()
    }
}

impl CoreBoundaryDeliveryWitness {
    pub(crate) fn pending() -> Self {
        Self {
            state: Arc::new(Mutex::new(DeliveryWitnessState {
                outcome: CoreBoundaryDeliveryOutcome::Pending,
                apply_ordinal: None,
            })),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, DeliveryWitnessState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Current outcome.
    #[must_use]
    pub fn outcome(&self) -> CoreBoundaryDeliveryOutcome {
        self.lock().outcome
    }

    /// Whether two handles observe the same delivery.
    #[must_use]
    pub fn same_delivery(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.state, &other.state)
    }

    pub(crate) fn mark_applied(&self, apply_ordinal: u64) -> bool {
        let mut state = self.lock();
        if state.outcome != CoreBoundaryDeliveryOutcome::Pending {
            return false;
        }
        state.outcome = CoreBoundaryDeliveryOutcome::Applied;
        state.apply_ordinal = Some(apply_ordinal);
        true
    }

    pub(crate) fn mark_withdrawn(&self) -> bool {
        let mut state = self.lock();
        if state.outcome != CoreBoundaryDeliveryOutcome::Pending {
            return false;
        }
        state.outcome = CoreBoundaryDeliveryOutcome::Withdrawn;
        true
    }

    /// Mark an applied delivery discarded when its apply came after
    /// `after_ordinal` (`None` discards every applied delivery).
    pub(crate) fn mark_discarded_if_applied_after(&self, after_ordinal: Option<u64>) -> bool {
        let mut state = self.lock();
        if state.outcome != CoreBoundaryDeliveryOutcome::Applied {
            return false;
        }
        let applied_after = match (state.apply_ordinal, after_ordinal) {
            (Some(applied), Some(floor)) => applied > floor,
            (_, None) => true,
            (None, Some(_)) => false,
        };
        if applied_after {
            state.outcome = CoreBoundaryDeliveryOutcome::Discarded;
        }
        applied_after
    }
}

/// Which delivery classes a runner accepts at one boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BoundaryDeliveryAcceptance {
    /// Ordinary committing turn boundary: both classes.
    RequestOnlyAndDurable,
    /// Extraction boundaries and noncommitting live-bridge runs: request-only
    /// context only. A registered durable delivery is refused before parking.
    RequestOnly,
}

/// A durable delivery the runner accepted and must apply in one synchronous
/// segment.
pub(crate) struct AcceptedDurableTurnAppends {
    pub(crate) appends: DurableTurnBoundaryAppends,
    pub(crate) witness: CoreBoundaryDeliveryWitness,
}

/// Everything published for one exact boundary.
#[derive(Default)]
pub(crate) struct TakenBoundaryDelivery {
    pub(crate) request_only: Vec<TurnRequestContext>,
    pub(crate) durable: Option<AcceptedDurableTurnAppends>,
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::lifecycle::run_primitive::CoreRenderable;
    use crate::types::{SystemNoticeBlock, SystemNoticeKind};

    fn notice(detail: &str) -> ConversationAppend {
        ConversationAppend {
            runtime_source: None,
            role: ConversationAppendRole::SystemNotice,
            content: CoreRenderable::SystemNotice {
                kind: SystemNoticeKind::Generic,
                body: Some(detail.to_string()),
                blocks: vec![SystemNoticeBlock::RuntimeNotice {
                    category: "test".to_string(),
                    detail: Some(detail.to_string()),
                    payload: None,
                }],
            },
            identity: None,
        }
    }

    #[test]
    fn durable_appends_lower_eligible_roles_and_stamp_the_input_identity() {
        let identity = TranscriptMessageIdentity {
            interaction_id: Some(crate::interaction::InteractionId(uuid::Uuid::new_v4())),
            ..Default::default()
        };
        let appends = DurableTurnBoundaryAppends::try_new(
            InputId::new(),
            vec![
                notice("n"),
                ConversationAppend {
                    runtime_source: None,
                    role: ConversationAppendRole::User,
                    content: CoreRenderable::Text {
                        text: "hello".to_string(),
                    },
                    identity: None,
                },
            ],
            Some(identity.clone()),
        )
        .expect("eligible appends lower");
        assert_eq!(appends.messages().len(), 2);
        assert!(matches!(appends.messages()[0], Message::SystemNotice(_)));
        match &appends.messages()[1] {
            Message::User(user) => assert_eq!(user.identity, identity),
            other => panic!("expected a user row, got {other:?}"),
        }
    }

    #[test]
    fn durable_appends_refuse_roles_that_cannot_join_a_running_turn() {
        for role in [
            ConversationAppendRole::System,
            ConversationAppendRole::Assistant,
            ConversationAppendRole::Tool,
        ] {
            let error = DurableTurnBoundaryAppends::try_new(
                InputId::new(),
                vec![ConversationAppend {
                    runtime_source: None,
                    role,
                    content: CoreRenderable::Text {
                        text: "x".to_string(),
                    },
                    identity: None,
                }],
                None,
            )
            .expect_err("ineligible role is refused");
            assert_eq!(
                error,
                DurableTurnBoundaryAppendsError::RoleNotInTurnEligible { role }
            );
        }
        assert_eq!(
            DurableTurnBoundaryAppends::try_new(InputId::new(), Vec::new(), None)
                .expect_err("empty is refused"),
            DurableTurnBoundaryAppendsError::Empty
        );
    }

    fn notice_of(kind: SystemNoticeKind, blocks: Vec<SystemNoticeBlock>) -> ConversationAppend {
        ConversationAppend {
            runtime_source: None,
            role: ConversationAppendRole::SystemNotice,
            content: CoreRenderable::SystemNotice {
                kind,
                body: Some("notice".to_string()),
                blocks,
            },
            identity: None,
        }
    }

    fn mcp_block(persisted: bool) -> SystemNoticeBlock {
        SystemNoticeBlock::Mcp {
            server_id: Some("server".to_string()),
            operation: None,
            phase: None,
            persisted,
            detail: None,
            pending_sources: Vec::new(),
        }
    }

    #[test]
    fn durable_appends_refuse_refresh_projection_notices() {
        // The next boundary's synthetic-notice refresh strips every notice of
        // these kinds, so an in-turn write would be gone from later requests
        // and from the committed transcript while its witness said Applied.
        for (kind, blocks) in [
            (SystemNoticeKind::BackgroundJob, Vec::new()),
            (SystemNoticeKind::AuthReauthRequired, Vec::new()),
            (SystemNoticeKind::McpPending, vec![mcp_block(false)]),
        ] {
            let append = notice_of(kind, blocks);
            assert!(!conversation_append_joins_running_turn(&append));
            assert_eq!(
                DurableTurnBoundaryAppends::try_new(InputId::new(), vec![append], None)
                    .expect_err("a refresh projection never joins a running turn"),
                DurableTurnBoundaryAppendsError::RefreshProjectionNotice { kind }
            );
        }
        // A persisted pending-MCP notice is durable history: the typed notice
        // predicate, not the kind, decides.
        let persisted = notice_of(SystemNoticeKind::McpPending, vec![mcp_block(true)]);
        assert!(conversation_append_joins_running_turn(&persisted));
        assert!(DurableTurnBoundaryAppends::try_new(InputId::new(), vec![persisted], None).is_ok());
        assert!(conversation_append_joins_running_turn(&notice("generic")));
    }

    #[test]
    fn durable_appends_project_peer_ingestion_events_like_the_turn_start_path() {
        let comms = ConversationAppend {
            runtime_source: None,
            role: ConversationAppendRole::SystemNotice,
            content: CoreRenderable::SystemNotice {
                kind: SystemNoticeKind::Comms,
                body: None,
                blocks: vec![SystemNoticeBlock::Comms {
                    kind: crate::types::CommsNoticeKind::Message,
                    direction: crate::types::SystemNoticeDirection::Incoming,
                    peer: None,
                    sender_taint: None,
                    request_id: None,
                    intent: None,
                    status: None,
                    summary: None,
                    payload: None,
                    content: vec![crate::types::ContentBlock::Text {
                        text: "hello".to_string(),
                    }],
                }],
            },
            identity: None,
        };
        let expected = crate::event::peer_content_ingested_events(&comms.content);
        assert_eq!(expected.len(), 1);
        let appends =
            DurableTurnBoundaryAppends::try_new(InputId::new(), vec![notice("n"), comms], None)
                .expect("eligible appends lower");
        assert_eq!(
            format!("{:?}", appends.peer_ingested_events()),
            format!("{expected:?}")
        );
    }

    #[test]
    fn witness_is_monotonic() {
        let witness = CoreBoundaryDeliveryWitness::pending();
        assert_eq!(witness.outcome(), CoreBoundaryDeliveryOutcome::Pending);
        assert!(!witness.mark_discarded_if_applied_after(None));
        assert!(witness.mark_applied(3));
        assert!(!witness.mark_withdrawn());
        assert!(!witness.mark_discarded_if_applied_after(Some(3)));
        assert_eq!(witness.outcome(), CoreBoundaryDeliveryOutcome::Applied);
        assert!(witness.mark_discarded_if_applied_after(Some(2)));
        assert_eq!(witness.outcome(), CoreBoundaryDeliveryOutcome::Discarded);
        assert!(!witness.mark_applied(4));

        let withdrawn = CoreBoundaryDeliveryWitness::pending();
        assert!(withdrawn.mark_withdrawn());
        assert!(!withdrawn.mark_applied(1));
        assert_eq!(withdrawn.outcome(), CoreBoundaryDeliveryOutcome::Withdrawn);
    }
}
