//! The governed peer receive-admission seam: Core's request to the trusted
//! host's binder to bind one peer delivery Core verified, and its answer.
//!
//! On a governed runtime a peer delivery carries no authority of its own:
//! Core projects the delivery to an unbound input, and the [`PeerIngressBinder`]
//! the trusted composition installed on the machine
//! (`MeerkatMachine::install_peer_ingress_binder`) binds it, or refuses. Core
//! then checks the binding ([`super::check_bound_original`]) before ordinary
//! admission decides it again against current owners
//! (`NativeWorkAuthorizationHost::authenticate_association`).

use meerkat_authorization_contracts::evidence::EvidenceId;
use meerkat_contracts::wire::supervisor_bridge::BridgeMemberIncarnation;
use meerkat_core::comms::PeerId;
use meerkat_core::interaction::{
    InteractionId, PeerIngressAuthDecision, PeerIngressKind, PeerIngressSenderAuthentication,
    PeerInputClass,
};
use meerkat_core::types::SessionId;
use sha2::{Digest, Sha256};

use crate::identifiers::{IdempotencyKey, LogicalRuntimeId};
use crate::input::Input;
use crate::input_admission_custody::NativeInputAdmissionCustody;

/// Where a governed peer delivery came from, as Core established it before it
/// asks the installed binder to bind the delivery. Facts only: nothing here is
/// an authority, a grant, or a requester the host may adopt as the
/// receiver's. The host derives the receiver's authority from its own owners.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum PeerIngressOrigin {
    /// A comms message, request, or response (progress or terminal) whose
    /// sender the transport established.
    Comms(PeerIngressCommsOrigin),
    /// A supervisor-bridge member delivery (`DeliverMemberInput`), relayed by
    /// the mob's authorized supervisor. The supervisor is a relay fact: the
    /// delivery is bound under the receiving member's own owner-derived
    /// association, never under the supervisor's authority, and never as a
    /// member-to-member grant (the bridge payload names no originating
    /// member, and its content can be member-derived).
    SupervisorDelivery(PeerIngressSupervisorOrigin),
}

impl PeerIngressOrigin {
    /// The origin's own tag in [`PeerIngressBindRequest::delivery_work`], so
    /// a binding minted for one origin never passes another origin's pin.
    fn work_tag(&self) -> &'static str {
        match self {
            Self::Comms(_) => "comms",
            Self::SupervisorDelivery(_) => "supervisor",
        }
    }
}

/// A comms delivery's facts, as the comms layer admitted it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct PeerIngressCommsOrigin {
    /// The sender's canonical peer id, as the transport established it.
    pub sender: PeerId,
    /// The sender's signing key, when the envelope named one.
    pub signing_key: Option<[u8; 32]>,
    /// The sender's route label, for diagnostics only.
    pub route_label: Option<String>,
    /// How the transport established the sender. Core offers the binder only
    /// a sender the transport established (a verified signature or an
    /// in-process sender), never an open-auth envelope's claimed `from`.
    pub sender_authentication: PeerIngressSenderAuthentication,
    /// The ingress trust decision the delivery was admitted under.
    pub auth: PeerIngressAuthDecision,
    pub class: PeerInputClass,
    pub kind: PeerIngressKind,
    /// The delivery's interaction id.
    pub interaction_id: InteractionId,
    /// For a response (progress or terminal): the request it answers.
    pub in_reply_to: Option<InteractionId>,
    pub claim: PeerIngressClaimKind,
}

/// How Core holds a comms delivery while it binds it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PeerIngressClaimKind {
    /// A durable delivery: Core keeps the queue claim until its admission
    /// receipt, and the sender learns the claim's terminal from it. A pending
    /// answer releases the claim, which stays the session's FIFO head.
    Durable { raw_item_id: InteractionId },
    /// A volatile delivery, handed off when it was routed: nothing is kept to
    /// retry, so a pending answer admits nothing.
    Volatile,
}

/// A supervisor-bridge member delivery's facts.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct PeerIngressSupervisorOrigin {
    /// The supervisor that relayed the delivery: the bridge's bound
    /// supervisor, whose identity the transport established.
    pub supervisor: PeerId,
    /// How the transport established the supervisor.
    pub sender_authentication: PeerIngressSenderAuthentication,
    /// The bridge request's interaction id.
    pub interaction_id: InteractionId,
    /// Whether the delivery asked for tracked terminal custody.
    pub tracked: bool,
}

/// Core's request to the installed [`PeerIngressBinder`] to bind one governed
/// peer delivery for the receiving session. Built only by Core.
#[non_exhaustive]
pub struct PeerIngressBindRequest {
    /// The receiving session's runtime.
    pub runtime_id: LogicalRuntimeId,
    /// The receiving session.
    pub session_id: SessionId,
    /// The member incarnation the delivery is fenced to, when it is fenced.
    pub expected_member: Option<BridgeMemberIncarnation>,
    pub origin: PeerIngressOrigin,
    /// Core's admission key for this delivery: the input's idempotency key.
    /// Pass it as the event key of `bind_scoped_original`.
    pub event_key: IdempotencyKey,
    /// Core's work id for this delivery, derived from the origin and the
    /// event key. The binding's `original_work.work` must equal it: Core
    /// refuses a binding that names any other work, so an association
    /// minted for one delivery cannot be bound to another.
    pub delivery_work: EvidenceId,
    /// Core's exact projected input. The binding sets only its authority
    /// association, its idempotency key (to `event_key`) and its trusted
    /// ingress observation; Core refuses any other change.
    pub input: Input,
}

impl PeerIngressBindRequest {
    pub(crate) fn new(
        runtime_id: LogicalRuntimeId,
        session_id: SessionId,
        expected_member: Option<BridgeMemberIncarnation>,
        origin: PeerIngressOrigin,
        event_key: IdempotencyKey,
        input: Input,
    ) -> Result<Self, String> {
        let delivery_work = delivery_work(&origin, &event_key)?;
        Ok(Self {
            runtime_id,
            session_id,
            expected_member,
            origin,
            event_key,
            delivery_work,
            input,
        })
    }
}

impl std::fmt::Debug for PeerIngressBindRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PeerIngressBindRequest")
            .field("runtime_id", &self.runtime_id)
            .field("session_id", &self.session_id)
            .field("origin", &self.origin)
            .field("input", &self.input.id())
            .finish_non_exhaustive()
    }
}

/// Core's work id for one delivery: `peer-ingress-delivery:<origin>:` and the
/// SHA-256 of the event key, so it is bounded and names nothing but the
/// delivery.
fn delivery_work(
    origin: &PeerIngressOrigin,
    event_key: &IdempotencyKey,
) -> Result<EvidenceId, String> {
    let digest = Sha256::digest(event_key.to_string().as_bytes());
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        hex.push(char::from(b"0123456789abcdef"[usize::from(byte >> 4)]));
        hex.push(char::from(b"0123456789abcdef"[usize::from(byte & 0x0f)]));
    }
    EvidenceId::new(format!("peer-ingress-delivery:{}:{hex}", origin.work_tag()))
        .map_err(|error| format!("peer delivery work id: {error}"))
}

/// The id Core gives a governed peer delivery's input: one id per delivery
/// (origin and event key), the same across every redelivery of it, so a
/// delivery the store already admitted is found again by its id whatever the
/// host binds the next time.
pub(crate) fn delivery_input_id(
    origin: &PeerIngressOrigin,
    event_key: &IdempotencyKey,
) -> meerkat_core::lifecycle::InputId {
    let mut digest = Sha256::new();
    digest.update(b"meerkat.peer-ingress-delivery-input.v1\0");
    digest.update(origin.work_tag().as_bytes());
    digest.update(b"\0");
    digest.update(event_key.to_string().as_bytes());
    let digest = digest.finalize();
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    meerkat_core::lifecycle::InputId::from_uuid(uuid::Builder::from_custom_bytes(bytes).into_uuid())
}

/// The binder's binding of one governed peer delivery: Core's exact input with
/// the association and the trusted ingress observation set, and optionally
/// the host's custody of the enrolled original, which native admission
/// settles exactly once (as for a bound host original).
#[non_exhaustive]
pub struct PeerIngressBinding {
    input: Input,
    custody: Option<Box<dyn NativeInputAdmissionCustody>>,
}

impl PeerIngressBinding {
    /// A binding of `input`, Core's input with the association and the
    /// trusted ingress observation set (for example through
    /// `bind_scoped_original`).
    #[must_use]
    pub fn new(input: Input) -> Self {
        Self {
            input,
            custody: None,
        }
    }

    /// The same binding with the host's custody of the original it enrolled
    /// for this delivery. Core settles it exactly once: `NotAdmitted` when
    /// Core refuses the binding before admission (a malformed binding, a
    /// replay conflict, a refused fence) or admission admits nothing,
    /// `Admitted` when native admission accepts the input (or verifies an
    /// exact replay), `Uncertain` when the outcome is unknown. Without
    /// custody, the host learns the outcome from the runtime's own record.
    #[must_use]
    pub fn with_admission_custody(mut self, custody: Box<dyn NativeInputAdmissionCustody>) -> Self {
        self.custody = Some(custody);
        self
    }

    /// The bound input.
    pub fn input(&self) -> &Input {
        &self.input
    }

    pub(crate) fn into_parts(self) -> (Input, Option<Box<dyn NativeInputAdmissionCustody>>) {
        (self.input, self.custody)
    }
}

impl std::fmt::Debug for PeerIngressBinding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PeerIngressBinding")
            .field("input", &self.input.id())
            .field("custody", &self.custody.is_some())
            .finish()
    }
}

/// Why the binder bound nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum PeerIngressBindRefusal {
    /// The host's verdict: this delivery is never admitted for this receiver.
    /// Terminal: a durable claim ends `RuntimeRejected`, and a bridge delivery
    /// is rejected. The host records the verdict against the delivery's
    /// custody key, so a later bind of the same key answers `Denied` again.
    Denied,
    /// This binder does not bind this delivery. Core admits it unbound
    /// exactly as before the seam existed (as for a runtime with no binder),
    /// which a governed runtime refuses.
    NotOffered,
    /// Not answerable now, for a typed cause (a cold or held receiver, a
    /// controller not pinned yet, contention). Pending: a durable claim is
    /// released and stays the session's FIFO head until the host binds it.
    NotReady(meerkat_core::authorization::ControllerReadinessFailure),
    /// The host could not be read now. Pending, as `NotReady`.
    Unavailable(String),
}

/// The trusted host's binder of governed peer deliveries: on a governed
/// runtime, the one way a peer message, request, response or
/// supervisor-bridge member delivery is admitted. The trusted composition
/// installs it on the machine, once, before any session registers
/// (`MeerkatMachine::install_peer_ingress_binder`), beside the native work
/// authorization host whose owners decide the binding's admission again.
///
/// Core asks only for a delivery whose sender the transport established (a
/// signature the listener verified, an in-process sender, or the bridge's
/// bound supervisor so established), and never for a plain external event,
/// which has no sender. The request carries facts only; none of them is a
/// grant, and a sender's own tool authority is never the receiver's.
///
/// Contract:
/// - Bounded. Core awaits the binder holding no drain, session, runtime or
///   recovery lock: only the delivery's own queue claim. The comms drain runs
///   one task per receiving session, and supervisor deliveries are served in
///   the receiving member's own drain, so a slow binder holds back only its
///   own session's later deliveries. The binder must never await a member
///   turn, or anything that waits on the receiving session's ingress; what it
///   cannot answer now is `NotReady`. On the supervisor-bridge path Core
///   bounds the await below the supervisor's command timeout and reads expiry
///   as `NotReady`.
/// - It binds Core's exact `request.input`, setting only its authority
///   association, its idempotency key (to `request.event_key`) and its
///   trusted ingress observation, for example through `bind_scoped_original`.
///   The association names this runtime and `request.delivery_work` as its
///   `original_work.work`, a controller grant lineage and model; the ingress
///   names the association's requester, actor and realm with a controller
///   client of that model. Anything else is a malformed binding, which Core
///   refuses for good: the delivery ends rejected, and the next one proceeds.
/// - It enrolls the receiver's original in the host's own custody under the
///   event key before it returns, and keeps it at least as long as the sender
///   may redeliver. A later bind of the same event key returns that
///   association verbatim (also after Core refused the earlier one), or
///   refuses; it never mints a new association. A redelivery after an
///   admission is then deduplicated, and one that names another
///   qualification (requester, target or namespace) is refused as a replay
///   conflict. A `Denied` verdict is recorded against the key and answered
///   again.
/// - The custody's outcome reaches the host through
///   [`PeerIngressBinding::with_admission_custody`], settled exactly once
///   (`NotAdmitted` for every refusal of Core's own), or, without custody,
///   from the runtime's own record of the delivery's input.
/// - A supervisor delivery is bound under the receiving member's own
///   owner-derived association; the supervisor is a relay fact, never a
///   requester with member-to-member authority.
/// - Core never makes a governed session live: a cold receiver answers
///   `NotReady(ExecutorUnavailable)` until the host's own recovery attaches
///   it. A durable delivery's pending answer keeps it the session's FIFO
///   head, retried on the drain's re-arm (about once a second), with no give
///   up in Core: the sender's own wait bounds what the sender sees (an
///   in-process sender's park bound, a listener's ack timeout), and a resend
///   of the same envelope deduplicates once the host binds it.
/// - When the receiver's scoped destination changed since an admission, a
///   verbatim re-bind names the old destination and is refused before the
///   replay lookup: the redelivery ends rejected while the first admission
///   stands and runs once.
pub trait PeerIngressBinder: Send + Sync {
    /// Bind one governed peer delivery for its receiving session, or refuse.
    fn bind_peer_ingress(&self, request: PeerIngressBindRequest) -> PeerIngressBindFuture<'_>;
}

/// The binder's future: bounded, and never awaiting anything that waits on
/// the receiving session's ingress.
#[cfg(not(target_arch = "wasm32"))]
pub type PeerIngressBindFuture<'a> = std::pin::Pin<
    Box<
        dyn std::future::Future<Output = Result<PeerIngressBinding, PeerIngressBindRefusal>>
            + Send
            + 'a,
    >,
>;

/// The binder's future: bounded, and never awaiting anything that waits on
/// the receiving session's ingress.
#[cfg(target_arch = "wasm32")]
pub type PeerIngressBindFuture<'a> = std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<PeerIngressBinding, PeerIngressBindRefusal>> + 'a>,
>;
