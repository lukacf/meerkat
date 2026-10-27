//! The keyed inject plane of the cross-mob control channel.
//!
//! `inject` reaches a member of a peer gateway's mob, but carries no key, so
//! a lost answer leaves the caller unable to tell whether it landed.
//! `inject_keyed` delivers one host continuation to a durable identity a
//! peer gateway hosts under a delivery key, and `inject_status` reads that
//! key's native status back. A mob announcement batch for a member on a peer
//! gateway travels this way. Both verbs ride the signed control channel
//! ([`super::cross_mob_control`]):
//!
//! * The calling gateway signs every request with its own gateway key. The
//!   signature covers the member, the delivery key, the content digest, a
//!   digest of the body, the issue time, a fresh nonce and the serving
//!   gateway's mob id (the audience).
//! * The serving gateway admits a request only under a grant that names the
//!   verb: `verbs = ["*"]` never covers it, and a listener that enforces no
//!   grants refuses it. It refuses a request issued outside its freshness
//!   window, or before the serving process started, as stale, and otherwise
//!   submits or reads through its own continuation owner. A delivery it
//!   admitted has a deadline, its issue time plus the window: one it cannot
//!   commit by then is refused as stale too, and nothing is committed. Every
//!   stale refusal carries its typed [`StaleRequestCause`].
//! * Every delivery is scoped to the caller the grant admitted: its
//!   continuation key is the delivery key under the caller's scope, a digest
//!   of the grant's label ([`KeyedInjectCallerScope`]). A caller reaches only
//!   its own keys, so another granted caller can neither commit under a key
//!   it learned nor answer for a delivery that is not its own. Within one scope,
//!   an identical replay returns the original receipt and other content
//!   under the same key is refused: no replay can add an input.
//! * The serving gateway signs every answer, refusals included, over the
//!   exact request bytes. The caller requires that signature against the key
//!   it pinned for the peer. An unsigned or unverifiable answer, and any
//!   `Err` answer, is never evidence of anything.
//!
//! The serving side is a [`KeyedInjectReceiver`] behind a slot that the
//! gateway's composition seals exactly once, after its runtime is composed
//! ([`KeyedInjectServing`]). Until then both verbs answer
//! [`KeyedInjectRefusalCode::Unavailable`]; a gateway that does not serve
//! keyed injects answers [`KeyedInjectRefusalCode::NotServed`] with its
//! typed [`NotServedCause`].

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, RwLock};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::cross_mob_control::{
    AuthorizedCaller, ControlRequest, ControlResponse, push_signed_field, sha256_hex,
};

/// How far a request's issue time may lie from the serving gateway's clock,
/// in either direction, unless the gateway configures another window.
pub const DEFAULT_KEYED_INJECT_WINDOW_MS: u64 = 5 * 60 * 1_000;

/// The native owner's commit receipt for a delivery, as it issued it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeReceipt {
    pub delivery_id: String,
    pub address: String,
    pub submission_digest: String,
    pub committed_at_ms: u64,
}

/// An admitted input, as the native owner names it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdmittedInput {
    pub session_id: String,
    pub input_id: String,
}

/// A delivery's native status: the continuation owner's own status of the
/// delivery's key on the gateway that hosts the member. None of these says
/// that the member processed anything.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
#[non_exhaustive]
pub enum NativeStatus {
    /// The native owner holds no commit for the key.
    NotCommitted,
    /// Committed to the member's inbox, not yet applied.
    Pending {
        receipt: NativeReceipt,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        admitted: Option<AdmittedInput>,
    },
    /// Admitted as one input of the member's session.
    Applied {
        receipt: NativeReceipt,
        session_id: String,
        input_id: String,
    },
    /// Committed to an incarnation that retired before admitting it.
    Stranded { receipt: NativeReceipt },
    /// Settled as refused by the native owner.
    Refused {
        receipt: NativeReceipt,
        reason: String,
    },
}

/// Why a serving gateway refused a keyed inject request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum KeyedInjectRefusalCode {
    /// This gateway's keyed inject plane is not composed yet: it is still
    /// starting. Asking again later is right.
    Unavailable,
    /// The request was issued outside this gateway's freshness window.
    StaleRequest,
    /// The request is malformed: a member that is not a durable identity
    /// in its public form, an invalid delivery key, or a body that does not
    /// match its digest.
    InvalidRequest,
    /// The member is not an identity registered with this gateway.
    UnknownMember,
    /// The member is registered, but this gateway's runtime does not serve
    /// it (it is placed on a host, or bound to an external runtime).
    UnusableMember,
    /// This gateway's runtime admits work only through a native work
    /// authorization host, which refuses host continuations.
    Governed,
    /// The delivery key is already bound to other content.
    Conflict,
    /// The member has no live incarnation now; nothing was committed.
    OwnerRetired,
    /// A native read or write could not be made now; nothing is implied.
    NativeUnavailable,
    /// This gateway does not serve keyed injects. The refusal carries the
    /// typed [`NotServedCause`].
    NotServed,
    /// The member left this gateway for good: its durable identity departed.
    MemberDeparted,
}

impl KeyedInjectRefusalCode {
    /// Every code this gateway answers.
    pub const ALL: [Self; 11] = [
        Self::Unavailable,
        Self::StaleRequest,
        Self::InvalidRequest,
        Self::UnknownMember,
        Self::UnusableMember,
        Self::Governed,
        Self::Conflict,
        Self::OwnerRetired,
        Self::NativeUnavailable,
        Self::NotServed,
        Self::MemberDeparted,
    ];

    /// Stable wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unavailable => "keyed_inject_unavailable",
            Self::StaleRequest => "stale_request",
            Self::InvalidRequest => "invalid_request",
            Self::UnknownMember => "unknown_member",
            Self::UnusableMember => "unusable_member",
            Self::Governed => "governed",
            Self::Conflict => "conflict",
            Self::OwnerRetired => "owner_retired",
            Self::NativeUnavailable => "native_unavailable",
            Self::NotServed => "not_served",
            Self::MemberDeparted => "member_departed",
        }
    }

    /// Parse a wire spelling. `None` for a code this gateway does not know,
    /// which a caller treats as no evidence.
    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|code| code.as_str() == value)
    }
}

impl std::fmt::Display for KeyedInjectRefusalCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Why a gateway does not serve keyed injects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum NotServedCause {
    /// The gateway is not identity-first, so it has no durable identities to
    /// deliver to.
    NotIdentityFirst,
    /// The gateway's configuration turns keyed inject serving off.
    RuntimeDisabled,
    /// How a caller reads a `not_served` refusal that carries no cause, or a
    /// cause this build does not know. A gateway never answers it.
    Unknown,
}

impl NotServedCause {
    /// Every cause this gateway answers.
    pub const ALL: [Self; 2] = [Self::NotIdentityFirst, Self::RuntimeDisabled];

    /// Stable wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotIdentityFirst => "not_identity_first",
            Self::RuntimeDisabled => "runtime_disabled",
            Self::Unknown => "unknown",
        }
    }

    /// Parse a wire spelling. `None` for a cause this gateway does not
    /// answer.
    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|cause| cause.as_str() == value)
    }

    /// Read the cause a signed `not_served` refusal carries in its own
    /// field: [`Self::Unknown`] when it carries none, or one this build does
    /// not know. Never read from the refusal's message.
    pub fn read(value: Option<&str>) -> Self {
        value.and_then(Self::parse).unwrap_or(Self::Unknown)
    }
}

impl std::fmt::Display for NotServedCause {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Why a gateway refused a keyed inject request as stale. Each says that
/// nothing was committed for the request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum StaleRequestCause {
    /// The request's issue time lies further from this gateway's clock than
    /// its window: the two gateways' clocks disagree.
    OutsideWindow,
    /// The request was issued before this gateway's serving process
    /// started: it restarted while the request was on its way.
    IssuedBeforeStart,
    /// This gateway admitted the delivery but could not commit it by its
    /// deadline, the request's issue time plus the window.
    DeadlinePassed,
    /// How a caller reads a `stale_request` refusal that carries no cause, or
    /// a cause this build does not know. A gateway never answers it.
    Unknown,
}

impl StaleRequestCause {
    /// Every cause this gateway answers.
    pub const ALL: [Self; 3] = [
        Self::OutsideWindow,
        Self::IssuedBeforeStart,
        Self::DeadlinePassed,
    ];

    /// Stable wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OutsideWindow => "outside_window",
            Self::IssuedBeforeStart => "issued_before_start",
            Self::DeadlinePassed => "deadline_passed",
            Self::Unknown => "unknown",
        }
    }

    /// Parse a wire spelling. `None` for a cause this gateway does not
    /// answer.
    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|cause| cause.as_str() == value)
    }

    /// Read the cause a signed `stale_request` refusal carries in its own
    /// field: [`Self::Unknown`] when it carries none, or one this build does
    /// not know. Never read from the refusal's message.
    pub fn read(value: Option<&str>) -> Self {
        value.and_then(Self::parse).unwrap_or(Self::Unknown)
    }
}

impl std::fmt::Display for StaleRequestCause {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The typed cause a refusal carries next to its code, inside the signed
/// facts, for the codes that have one. A caller reads it from this field and
/// never from the message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum KeyedInjectRefusalCause {
    /// Why a gateway answering [`KeyedInjectRefusalCode::NotServed`] does not
    /// serve keyed injects.
    NotServed(NotServedCause),
    /// Why a gateway answering [`KeyedInjectRefusalCode::StaleRequest`]
    /// refused the request as stale.
    Stale(StaleRequestCause),
}

impl KeyedInjectRefusalCause {
    /// Stable wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotServed(cause) => cause.as_str(),
            Self::Stale(cause) => cause.as_str(),
        }
    }

    /// Parse the cause of a refusal with `code` from its wire spelling.
    /// `None` for a code without causes, or a cause this gateway does not
    /// know.
    pub fn parse(code: KeyedInjectRefusalCode, value: &str) -> Option<Self> {
        match code {
            KeyedInjectRefusalCode::NotServed => NotServedCause::parse(value).map(Self::NotServed),
            KeyedInjectRefusalCode::StaleRequest => {
                StaleRequestCause::parse(value).map(Self::Stale)
            }
            _ => None,
        }
    }
}

/// A typed refusal from the serving side.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyedInjectRefusal {
    pub code: KeyedInjectRefusalCode,
    pub message: String,
    /// The typed cause, for the codes that carry one.
    pub cause: Option<KeyedInjectRefusalCause>,
}

impl KeyedInjectRefusal {
    pub fn new(code: KeyedInjectRefusalCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            cause: None,
        }
    }

    /// The refusal of a gateway that does not serve keyed injects.
    pub fn not_served(cause: NotServedCause) -> Self {
        Self {
            code: KeyedInjectRefusalCode::NotServed,
            message: format!("this gateway does not serve keyed injects ({cause})"),
            cause: Some(KeyedInjectRefusalCause::NotServed(cause)),
        }
    }

    /// The refusal of a request this gateway will not serve for its timing.
    /// Nothing was committed for it.
    pub fn stale(cause: StaleRequestCause, message: impl Into<String>) -> Self {
        Self {
            code: KeyedInjectRefusalCode::StaleRequest,
            message: message.into(),
            cause: Some(KeyedInjectRefusalCause::Stale(cause)),
        }
    }
}

/// The refusal of a delivery this gateway admitted but did not commit by
/// `deadline_ms`, its issue time plus the window, when its clock read
/// `now_ms`. Nothing was committed.
pub fn deadline_passed(deadline_ms: u64, now_ms: u64) -> KeyedInjectRefusal {
    KeyedInjectRefusal::stale(
        StaleRequestCause::DeadlinePassed,
        format!(
            "the delivery was not committed by its deadline at {deadline_ms} ms, and this \
             gateway's clock reads {now_ms} ms; nothing was committed"
        ),
    )
}

impl std::fmt::Display for KeyedInjectRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

/// The caller a keyed inject was admitted for, as the serving side scopes
/// what it delivers: a digest of the label of the grant that admitted it.
/// Keyed by the label rather than the key, so a caller's key rotation under
/// the same grant label keeps its in-flight deliveries; two grants that share
/// a label are one caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyedInjectCallerScope {
    digest: String,
}

impl KeyedInjectCallerScope {
    /// The scope of the caller a grant labelled `label` admitted.
    pub fn for_grant_label(label: &str) -> Self {
        Self {
            digest: sha256_hex(label.as_bytes()),
        }
    }

    /// The native continuation key of `delivery_key` delivered by this caller.
    pub fn native_key(&self, delivery_key: &str) -> String {
        format!("keyed-inject:{}:{delivery_key}", self.digest)
    }
}

/// One delivery as the calling gateway sent it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyedInject<'a> {
    pub member: &'a str,
    pub delivery_key: &'a str,
    pub content_digest: &'a str,
    pub summary: &'a str,
    pub body: &'a str,
}

/// A status read as the serving side answers it: the native status and, when
/// asked for, the admitted input's own runtime state.
#[derive(Debug, Clone, PartialEq)]
pub struct KeyedInjectStatusAnswer {
    pub status: NativeStatus,
    pub input: Option<Value>,
}

/// A boxed future, so the receiver can live behind `dyn`.
pub type KeyedInjectFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// The serving side of the keyed inject plane: the gateway's own mob
/// announcement composition, which reaches its continuation owner.
///
/// The control handler checks the freshness window and the member's form
/// before it calls [`Self::deliver`] or [`Self::status`]; a receiver never
/// sees a request issued outside the window. Each delivery reaches it with
/// its deadline, which the receiver honours.
pub trait KeyedInjectReceiver: Send + Sync {
    /// How far a request's issue time may lie from [`Self::now_ms`], in
    /// either direction, in milliseconds.
    fn window_ms(&self) -> u64;

    /// This gateway's clock, in milliseconds since the Unix epoch.
    fn now_ms(&self) -> u64;

    /// When this serving process started, on [`Self::now_ms`]'s clock. A
    /// request issued earlier is refused as stale: nothing captured before a
    /// restart is admitted after it. Zero admits any issue time the window
    /// admits.
    fn started_at_ms(&self) -> u64 {
        0
    }

    /// Submit one delivery to the member's continuation owner, under the
    /// caller's scope, by `deadline_ms` on [`Self::now_ms`]'s clock: the
    /// request's issue time plus the window. Whatever must happen before the
    /// commit is bounded by the time left, and the deadline is checked again
    /// right before the commit; once it has passed the receiver answers
    /// [`deadline_passed`] and commits nothing. The commit itself is never
    /// cut short.
    fn deliver<'a>(
        &'a self,
        scope: &'a KeyedInjectCallerScope,
        delivery: KeyedInject<'a>,
        deadline_ms: u64,
    ) -> KeyedInjectFuture<'a, Result<NativeReceipt, KeyedInjectRefusal>>;

    /// Read the native status of a delivery's key, under the caller's scope,
    /// for the member.
    fn status<'a>(
        &'a self,
        scope: &'a KeyedInjectCallerScope,
        member: &'a str,
        delivery_key: &'a str,
        with_input: bool,
    ) -> KeyedInjectFuture<'a, Result<KeyedInjectStatusAnswer, KeyedInjectRefusal>>;
}

/// What a gateway's keyed inject plane serves. A gateway binds its control
/// listener after its identity roster pass but before its runtime is fully
/// composed, so the plane starts [`Self::Pending`], and the gateway's
/// composition seals it exactly once into [`Self::Serving`] or
/// [`Self::NotServed`] ([`seal_keyed_inject_slot`]). A request therefore
/// never meets an incomplete identity registry. The handler re-reads the
/// plane per request.
#[derive(Clone)]
#[non_exhaustive]
pub enum KeyedInjectServing {
    /// Not composed yet: the gateway is starting. Both verbs answer
    /// [`KeyedInjectRefusalCode::Unavailable`].
    Pending,
    /// Served from this receiver.
    Serving(Arc<dyn KeyedInjectReceiver>),
    /// Not served here, for this typed cause. Both verbs answer
    /// [`KeyedInjectRefusalCode::NotServed`].
    NotServed(NotServedCause),
}

impl std::fmt::Debug for KeyedInjectServing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pending => f.write_str("Pending"),
            Self::Serving(_) => f.write_str("Serving"),
            Self::NotServed(cause) => f.debug_tuple("NotServed").field(cause).finish(),
        }
    }
}

/// The keyed inject plane of one gateway, shared by its control listener and
/// its composition.
pub type KeyedInjectSlot = Arc<RwLock<KeyedInjectServing>>;

/// A plane the gateway has not composed yet.
pub fn pending_keyed_inject_slot() -> KeyedInjectSlot {
    Arc::new(RwLock::new(KeyedInjectServing::Pending))
}

/// Why a keyed inject plane was not sealed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum KeyedInjectSealError {
    /// The plane is already sealed: it is sealed exactly once.
    AlreadySealed,
    /// [`KeyedInjectServing::Pending`] is where a plane starts, never what it
    /// is sealed into.
    SealedPending,
}

impl std::fmt::Display for KeyedInjectSealError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::AlreadySealed => "the keyed inject plane is already sealed",
            Self::SealedPending => "a keyed inject plane is never sealed into pending",
        })
    }
}

impl std::error::Error for KeyedInjectSealError {}

/// Seal a pending plane into what it serves, exactly once.
pub fn seal_keyed_inject_slot(
    slot: &KeyedInjectSlot,
    serving: KeyedInjectServing,
) -> Result<(), KeyedInjectSealError> {
    if matches!(serving, KeyedInjectServing::Pending) {
        return Err(KeyedInjectSealError::SealedPending);
    }
    let mut current = slot
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if !matches!(*current, KeyedInjectServing::Pending) {
        return Err(KeyedInjectSealError::AlreadySealed);
    }
    *current = serving;
    Ok(())
}

/// Refuse a request issued further than `window_ms` from `now_ms`, in either
/// direction. A request exactly `window_ms` away is still fresh.
///
/// This bounds how long a captured request stays usable: the window plus the
/// clock skew between the two gateways. Within it a replay is harmless
/// anyway. What decides that is the serving runtime store's continuation key
/// ledger, not the calling gateway's store: the key (the delivery key
/// under the caller's scope) is bound to its first content, so the native
/// owner answers an identical replay with the original receipt and adds no
/// input.
pub fn check_freshness(
    issued_at_ms: u64,
    now_ms: u64,
    window_ms: u64,
) -> Result<(), KeyedInjectRefusal> {
    if now_ms.abs_diff(issued_at_ms) <= window_ms {
        return Ok(());
    }
    Err(KeyedInjectRefusal::stale(
        StaleRequestCause::OutsideWindow,
        format!(
            "the request was issued at {issued_at_ms} ms and this gateway's clock reads \
             {now_ms} ms, more than the {window_ms} ms window apart; check both gateways' \
             clocks"
        ),
    ))
}

/// A keyed inject member is a durable identity in its public form: the form a
/// grant's member scope is written in. Anything else is refused rather than
/// normalized, so the scope decides on exactly the identity the delivery
/// reaches.
fn check_member_form(member: &str) -> Result<(), KeyedInjectRefusal> {
    if member.is_empty()
        || crate::member_comms_id::runtime_alias_str(member) != member
        || crate::member_comms_id::is_reserved_generated_alias(member)
    {
        return Err(KeyedInjectRefusal::new(
            KeyedInjectRefusalCode::InvalidRequest,
            format!("'{member}' is not a durable identity in its public form"),
        ));
    }
    Ok(())
}

fn current_serving(slot: &KeyedInjectSlot) -> KeyedInjectServing {
    slot.read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

fn admit(
    slot: &KeyedInjectSlot,
    caller: Option<&AuthorizedCaller>,
    member: &str,
    issued_at_ms: u64,
) -> Result<(Arc<dyn KeyedInjectReceiver>, KeyedInjectCallerScope), KeyedInjectRefusal> {
    // The listener routes a keyed inject verb only under a grant naming it, so a
    // missing caller is a routing defect; nothing is served for it.
    let Some(caller) = caller else {
        return Err(KeyedInjectRefusal::new(
            KeyedInjectRefusalCode::Unavailable,
            "keyed inject verbs are served only to a caller a grant admitted",
        ));
    };
    let receiver = match current_serving(slot) {
        KeyedInjectServing::Serving(receiver) => receiver,
        KeyedInjectServing::Pending => {
            return Err(KeyedInjectRefusal::new(
                KeyedInjectRefusalCode::Unavailable,
                "this gateway's keyed inject plane is not composed yet; ask again later",
            ));
        }
        KeyedInjectServing::NotServed(cause) => {
            return Err(KeyedInjectRefusal::not_served(cause));
        }
    };
    check_freshness(issued_at_ms, receiver.now_ms(), receiver.window_ms())?;
    let started_at_ms = receiver.started_at_ms();
    if issued_at_ms < started_at_ms {
        return Err(KeyedInjectRefusal::stale(
            StaleRequestCause::IssuedBeforeStart,
            format!(
                "the request was issued at {issued_at_ms} ms, before this gateway started at \
                 {started_at_ms} ms"
            ),
        ));
    }
    check_member_form(member)?;
    Ok((
        receiver,
        KeyedInjectCallerScope::for_grant_label(&caller.label),
    ))
}

fn refused(refusal: KeyedInjectRefusal) -> ControlResponse {
    ControlResponse::InjectRefused {
        code: refusal.code.as_str().to_string(),
        message: refusal.message,
        cause: refusal.cause.map(|cause| cause.as_str().to_string()),
        sig_b64: None,
    }
}

/// Answer one keyed inject request from `slot` for the `caller` the
/// listener's grant admitted (`None` refuses it). The listener signs whatever
/// this returns. Any other verb is answered `Err`: it never belongs here.
pub(crate) async fn answer_keyed_inject_request(
    slot: &KeyedInjectSlot,
    request: ControlRequest,
    caller: Option<&AuthorizedCaller>,
) -> ControlResponse {
    match request {
        ControlRequest::InjectKeyed {
            remote_member,
            delivery_key,
            content_digest,
            summary,
            body,
            issued_at_ms,
            nonce: _,
            caller: _,
        } => {
            answer_deliver(
                slot,
                caller,
                KeyedInject {
                    member: &remote_member,
                    delivery_key: &delivery_key,
                    content_digest: &content_digest,
                    summary: &summary,
                    body: &body,
                },
                issued_at_ms,
            )
            .await
        }
        ControlRequest::InjectStatus {
            remote_member,
            delivery_key,
            with_input,
            issued_at_ms,
            nonce: _,
            caller: _,
        } => {
            answer_status(
                slot,
                caller,
                (&remote_member, &delivery_key, with_input),
                issued_at_ms,
            )
            .await
        }
        other => ControlResponse::Err {
            code: "not_keyed_inject".to_string(),
            message: format!("'{}' is not a keyed inject verb", other.verb()),
        },
    }
}

async fn answer_deliver(
    slot: &KeyedInjectSlot,
    caller: Option<&AuthorizedCaller>,
    delivery: KeyedInject<'_>,
    issued_at_ms: u64,
) -> ControlResponse {
    let (receiver, scope) = match admit(slot, caller, delivery.member, issued_at_ms) {
        Ok(admitted) => admitted,
        Err(refusal) => return refused(refusal),
    };
    let deadline_ms = issued_at_ms.saturating_add(receiver.window_ms());
    match receiver.deliver(&scope, delivery, deadline_ms).await {
        Ok(receipt) => ControlResponse::InjectReceipt {
            receipt,
            sig_b64: None,
        },
        Err(refusal) => refused(refusal),
    }
}

async fn answer_status(
    slot: &KeyedInjectSlot,
    caller: Option<&AuthorizedCaller>,
    (member, delivery_key, with_input): (&str, &str, bool),
    issued_at_ms: u64,
) -> ControlResponse {
    let (receiver, scope) = match admit(slot, caller, member, issued_at_ms) {
        Ok(admitted) => admitted,
        Err(refusal) => return refused(refusal),
    };
    match receiver
        .status(&scope, member, delivery_key, with_input)
        .await
    {
        Ok(answer) => ControlResponse::InjectStatus {
            status: answer.status,
            input: answer.input,
            sig_b64: None,
        },
        Err(refusal) => refused(refusal),
    }
}

fn push_receipt(out: &mut String, receipt: &NativeReceipt) {
    push_signed_field(out, &receipt.delivery_id);
    push_signed_field(out, &receipt.address);
    push_signed_field(out, &receipt.submission_digest);
    push_signed_field(out, &receipt.committed_at_ms.to_string());
}

/// The signed facts of a receipt answer, length-prefixed field by field.
pub(crate) fn receipt_facts(receipt: &NativeReceipt) -> String {
    let mut out = String::new();
    push_receipt(&mut out, receipt);
    out
}

/// The signed facts of a status answer. Every field is length-prefixed; the
/// input state, arbitrary JSON, enters as the SHA-256 of its serialization,
/// which round-trips stably because both sides serialize a
/// `serde_json::Value`.
pub(crate) fn status_facts(status: &NativeStatus, input: Option<&Value>) -> String {
    let mut out = String::new();
    match status {
        NativeStatus::NotCommitted => push_signed_field(&mut out, "not_committed"),
        NativeStatus::Pending { receipt, admitted } => {
            push_signed_field(&mut out, "pending");
            push_receipt(&mut out, receipt);
            match admitted {
                Some(admitted) => {
                    push_signed_field(&mut out, "admitted");
                    push_signed_field(&mut out, &admitted.session_id);
                    push_signed_field(&mut out, &admitted.input_id);
                }
                None => push_signed_field(&mut out, "not_admitted"),
            }
        }
        NativeStatus::Applied {
            receipt,
            session_id,
            input_id,
        } => {
            push_signed_field(&mut out, "applied");
            push_receipt(&mut out, receipt);
            push_signed_field(&mut out, session_id);
            push_signed_field(&mut out, input_id);
        }
        NativeStatus::Stranded { receipt } => {
            push_signed_field(&mut out, "stranded");
            push_receipt(&mut out, receipt);
        }
        NativeStatus::Refused { receipt, reason } => {
            push_signed_field(&mut out, "refused");
            push_receipt(&mut out, receipt);
            push_signed_field(&mut out, reason);
        }
    }
    match input {
        None => push_signed_field(&mut out, "no_input"),
        Some(value) => push_signed_field(
            &mut out,
            &format!(
                "input:{}",
                sha256_hex(&serde_json::to_vec(value).unwrap_or_default())
            ),
        ),
    }
    out
}

/// The signed facts of a refusal: its code, whether it carries a typed cause
/// and which, and its message, each length-prefixed.
pub(crate) fn refusal_facts(code: &str, cause: Option<&str>, message: &str) -> String {
    let mut out = String::new();
    push_signed_field(&mut out, code);
    match cause {
        Some(cause) => {
            push_signed_field(&mut out, "cause");
            push_signed_field(&mut out, cause);
        }
        None => push_signed_field(&mut out, "no_cause"),
    }
    push_signed_field(&mut out, message);
    out
}

/// Why a keyed inject call produced no signed answer. None of these is evidence
/// about the delivery: it stays owed and the caller asks again later.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum KeyedInjectCallError {
    /// This gateway cannot make an authenticated keyed inject call to the peer:
    /// the contact pins no gateway key, opted out of signed control
    /// responses or is not a socket transport, or this gateway holds no
    /// signing key.
    NotAuthenticatable(String),
    /// The request does not fit one control frame.
    Oversize { bytes: usize },
    /// The request could not be sent, or its answer was lost or late.
    Transport(String),
    /// The answer did not verify against the pinned key.
    Unauthenticated(String),
    /// The peer answered an unsigned error: an authorization refusal, an
    /// undecodable request from a gateway that predates these verbs, and so
    /// on.
    Rejected { code: String, message: String },
    /// The peer answered with a shape that does not belong to the verb.
    Unexpected(String),
}

impl std::fmt::Display for KeyedInjectCallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotAuthenticatable(detail) => {
                write!(
                    f,
                    "no authenticated keyed inject call is possible: {detail}"
                )
            }
            Self::Oversize { bytes } => {
                write!(
                    f,
                    "the request is {bytes} bytes, more than one control frame"
                )
            }
            Self::Transport(detail) => write!(f, "the keyed inject call failed: {detail}"),
            Self::Unauthenticated(detail) => {
                write!(f, "the answer is not authenticated: {detail}")
            }
            Self::Rejected { code, message } => {
                write!(f, "the peer rejected the call ({code}): {message}")
            }
            Self::Unexpected(detail) => write!(f, "unexpected answer: {detail}"),
        }
    }
}

impl std::error::Error for KeyedInjectCallError {}

/// A keyed inject call's verified outcome: the serving gateway's signed answer or
/// its signed refusal.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum KeyedInjectCallOutcome<T> {
    Answered(T),
    /// A signed refusal: its code, its message, and the wire spelling of its
    /// typed cause when it carries one.
    Refused {
        code: String,
        message: String,
        cause: Option<String>,
    },
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn freshness_admits_exactly_the_window_and_nothing_beyond() {
        let now = 1_000_000;
        let window = 300_000;
        assert_eq!(check_freshness(now, now, window), Ok(()));
        assert_eq!(
            check_freshness(now - window, now, window),
            Ok(()),
            "exactly one window old is fresh"
        );
        assert_eq!(
            check_freshness(now + window, now, window),
            Ok(()),
            "exactly one window ahead is fresh"
        );
        for issued in [now - window - 1, now + window + 1, 0, u64::MAX] {
            let refusal = check_freshness(issued, now, window).unwrap_err();
            assert_eq!(
                refusal.code,
                KeyedInjectRefusalCode::StaleRequest,
                "{issued}"
            );
        }
    }

    #[test]
    fn a_caller_scope_is_one_grant_label() {
        let first = KeyedInjectCallerScope::for_grant_label("first-gateway");
        let again = KeyedInjectCallerScope::for_grant_label("first-gateway");
        let second = KeyedInjectCallerScope::for_grant_label("second-gateway");
        assert_eq!(first.native_key("batch-1"), again.native_key("batch-1"));
        assert_ne!(first.native_key("batch-1"), second.native_key("batch-1"));
        assert_ne!(first.native_key("batch-1"), first.native_key("batch-2"));
        let key = first.native_key("batch-1");
        assert!(key.starts_with("keyed-inject:"), "{key}");
        assert!(key.ends_with(":batch-1"), "{key}");
        assert!(!key.contains("first-gateway"), "{key}");
    }

    #[test]
    fn refusal_codes_round_trip() {
        for code in KeyedInjectRefusalCode::ALL {
            assert_eq!(KeyedInjectRefusalCode::parse(code.as_str()), Some(code));
        }
        assert_eq!(KeyedInjectRefusalCode::parse("decode"), None);
        for cause in NotServedCause::ALL {
            assert_eq!(NotServedCause::parse(cause.as_str()), Some(cause));
            assert_eq!(
                KeyedInjectRefusalCause::parse(KeyedInjectRefusalCode::NotServed, cause.as_str()),
                Some(KeyedInjectRefusalCause::NotServed(cause))
            );
            assert_eq!(
                KeyedInjectRefusalCause::parse(KeyedInjectRefusalCode::Unavailable, cause.as_str()),
                None,
                "a cause belongs to its code"
            );
        }
        assert_eq!(NotServedCause::parse("built_without_feature"), None);
        for cause in StaleRequestCause::ALL {
            assert_eq!(StaleRequestCause::parse(cause.as_str()), Some(cause));
            assert_eq!(StaleRequestCause::read(Some(cause.as_str())), cause);
            assert_eq!(
                KeyedInjectRefusalCause::parse(
                    KeyedInjectRefusalCode::StaleRequest,
                    cause.as_str()
                ),
                Some(KeyedInjectRefusalCause::Stale(cause))
            );
            assert_eq!(
                KeyedInjectRefusalCause::parse(KeyedInjectRefusalCode::NotServed, cause.as_str()),
                None,
                "a cause belongs to its code"
            );
        }
        assert_eq!(StaleRequestCause::read(None), StaleRequestCause::Unknown);
        assert_eq!(
            StaleRequestCause::read(Some("clock_drift")),
            StaleRequestCause::Unknown,
            "a cause this build does not know is unknown, never another one"
        );
        assert_eq!(StaleRequestCause::parse("unknown"), None);
    }

    /// Every stale refusal says why, in its own typed field: a request
    /// outside the window, one issued before the serving process started,
    /// and a delivery past its deadline each carry their cause.
    #[tokio::test]
    async fn every_stale_refusal_carries_its_typed_cause() {
        let outside = check_freshness(1, 1_000_000, 300_000).unwrap_err();
        assert_eq!(outside.code, KeyedInjectRefusalCode::StaleRequest);
        assert_eq!(
            outside.cause,
            Some(KeyedInjectRefusalCause::Stale(
                StaleRequestCause::OutsideWindow
            ))
        );
        let late = deadline_passed(1_000, 1_001);
        assert_eq!(late.code, KeyedInjectRefusalCode::StaleRequest);
        assert_eq!(
            late.cause,
            Some(KeyedInjectRefusalCause::Stale(
                StaleRequestCause::DeadlinePassed
            ))
        );
        let receiver = Arc::new(CountingReceiver {
            started_at_ms: 1_000_001,
            ..CountingReceiver::default()
        });
        let slot = pending_keyed_inject_slot();
        seal_keyed_inject_slot(
            &slot,
            KeyedInjectServing::Serving(Arc::clone(&receiver) as Arc<dyn KeyedInjectReceiver>),
        )
        .unwrap();
        for request in [keyed_request("n1"), status_request("n2")] {
            match answer_keyed_inject_request(&slot, request, Some(&granted())).await {
                ControlResponse::InjectRefused { code, cause, .. } => {
                    assert_eq!(code, KeyedInjectRefusalCode::StaleRequest.as_str());
                    assert_eq!(
                        cause.as_deref(),
                        Some(StaleRequestCause::IssuedBeforeStart.as_str())
                    );
                }
                other => panic!("expected stale_request, got {other:?}"),
            }
        }
        assert_eq!(receiver.calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    /// A delivery reaches its receiver with its deadline: the request's issue
    /// time plus the receiver's window.
    #[tokio::test]
    async fn a_delivery_reaches_its_receiver_with_its_deadline() {
        let receiver = Arc::new(CountingReceiver::default());
        let slot = pending_keyed_inject_slot();
        seal_keyed_inject_slot(
            &slot,
            KeyedInjectServing::Serving(Arc::clone(&receiver) as Arc<dyn KeyedInjectReceiver>),
        )
        .unwrap();
        assert!(matches!(
            answer_keyed_inject_request(&slot, keyed_request("n1"), Some(&granted())).await,
            ControlResponse::InjectReceipt { .. }
        ));
        assert_eq!(
            receiver
                .deadline_ms
                .load(std::sync::atomic::Ordering::SeqCst),
            1_000_000 + 300_000
        );
    }

    fn keyed_request(nonce: &str) -> ControlRequest {
        ControlRequest::InjectKeyed {
            remote_member: "agent:specialist".to_string(),
            delivery_key: "batch-1".to_string(),
            content_digest: "digest".to_string(),
            summary: "1 announcement".to_string(),
            body: "a body".to_string(),
            issued_at_ms: 1_000_000,
            nonce: nonce.to_string(),
            caller: None,
        }
    }

    fn status_request(nonce: &str) -> ControlRequest {
        ControlRequest::InjectStatus {
            remote_member: "agent:specialist".to_string(),
            delivery_key: "batch-1".to_string(),
            with_input: false,
            issued_at_ms: 1_000_000,
            nonce: nonce.to_string(),
            caller: None,
        }
    }

    fn granted() -> AuthorizedCaller {
        AuthorizedCaller {
            label: "first-gateway".to_string(),
            pubkey: [7; 32],
        }
    }

    /// Before the gateway's composition seals the plane, both verbs answer
    /// `keyed_inject_unavailable`: the gateway is starting, and nothing about
    /// the member is implied.
    #[tokio::test]
    async fn a_pending_plane_answers_unavailable_until_it_is_sealed() {
        let slot = pending_keyed_inject_slot();
        for request in [keyed_request("n1"), status_request("n2")] {
            match answer_keyed_inject_request(&slot, request, Some(&granted())).await {
                ControlResponse::InjectRefused { code, cause, .. } => {
                    assert_eq!(code, KeyedInjectRefusalCode::Unavailable.as_str());
                    assert_eq!(cause, None);
                }
                other => panic!("expected keyed_inject_unavailable, got {other:?}"),
            }
        }
        let receiver = Arc::new(CountingReceiver::default());
        seal_keyed_inject_slot(
            &slot,
            KeyedInjectServing::Serving(Arc::clone(&receiver) as Arc<dyn KeyedInjectReceiver>),
        )
        .unwrap();
        assert!(matches!(
            answer_keyed_inject_request(&slot, keyed_request("n3"), Some(&granted())).await,
            ControlResponse::InjectReceipt { .. }
        ));
        assert_eq!(receiver.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    /// A gateway that does not serve keyed injects answers `not_served` with
    /// its typed cause in the refusal's own field, for both verbs.
    #[tokio::test]
    async fn a_plane_not_served_answers_not_served_with_its_typed_cause() {
        for cause in NotServedCause::ALL {
            let slot = pending_keyed_inject_slot();
            seal_keyed_inject_slot(&slot, KeyedInjectServing::NotServed(cause)).unwrap();
            for request in [keyed_request("n1"), status_request("n2")] {
                match answer_keyed_inject_request(&slot, request, Some(&granted())).await {
                    ControlResponse::InjectRefused {
                        code,
                        cause: answered,
                        ..
                    } => {
                        assert_eq!(code, KeyedInjectRefusalCode::NotServed.as_str());
                        assert_eq!(answered.as_deref(), Some(cause.as_str()));
                    }
                    other => panic!("expected not_served, got {other:?}"),
                }
            }
        }
    }

    /// The plane is sealed exactly once, and never into pending.
    #[test]
    fn the_plane_is_sealed_exactly_once() {
        let slot = pending_keyed_inject_slot();
        assert_eq!(
            seal_keyed_inject_slot(&slot, KeyedInjectServing::Pending),
            Err(KeyedInjectSealError::SealedPending)
        );
        assert_eq!(
            seal_keyed_inject_slot(
                &slot,
                KeyedInjectServing::NotServed(NotServedCause::RuntimeDisabled)
            ),
            Ok(())
        );
        assert_eq!(
            seal_keyed_inject_slot(
                &slot,
                KeyedInjectServing::Serving(
                    Arc::new(CountingReceiver::default()) as Arc<dyn KeyedInjectReceiver>
                )
            ),
            Err(KeyedInjectSealError::AlreadySealed)
        );
        assert_eq!(
            seal_keyed_inject_slot(&slot, KeyedInjectServing::Pending),
            Err(KeyedInjectSealError::SealedPending)
        );
        assert!(matches!(
            *slot.read().unwrap(),
            KeyedInjectServing::NotServed(NotServedCause::RuntimeDisabled)
        ));
    }

    /// A refusal's cause is signed with its code and message: two refusals
    /// that differ only in their cause never share a signature.
    #[test]
    fn a_refusals_cause_is_part_of_its_signed_facts() {
        let facts = |cause: Option<&str>| refusal_facts("not_served", cause, "not served");
        assert_ne!(
            facts(Some("runtime_disabled")),
            facts(Some("not_identity_first"))
        );
        assert_ne!(facts(Some("runtime_disabled")), facts(None));
        assert_ne!(
            facts(Some("")),
            facts(None),
            "an empty cause is still a cause"
        );
        assert_ne!(facts(Some("no_cause")), facts(None));
        assert_ne!(
            refusal_facts("not_served", Some("x"), "y"),
            refusal_facts("not_served", None, "xy"),
            "the fields are length-prefixed"
        );
    }

    /// A receiver that serves anything and counts what reaches it.
    #[derive(Default)]
    struct CountingReceiver {
        calls: std::sync::atomic::AtomicUsize,
        started_at_ms: u64,
        /// The deadline the last delivery reached it with.
        deadline_ms: std::sync::atomic::AtomicU64,
    }

    impl KeyedInjectReceiver for CountingReceiver {
        fn window_ms(&self) -> u64 {
            300_000
        }

        fn now_ms(&self) -> u64 {
            1_000_000
        }

        fn started_at_ms(&self) -> u64 {
            self.started_at_ms
        }

        fn deliver<'a>(
            &'a self,
            _scope: &'a KeyedInjectCallerScope,
            _delivery: KeyedInject<'a>,
            deadline_ms: u64,
        ) -> KeyedInjectFuture<'a, Result<NativeReceipt, KeyedInjectRefusal>> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.deadline_ms
                .store(deadline_ms, std::sync::atomic::Ordering::SeqCst);
            Box::pin(async { Ok(receipt()) })
        }

        fn status<'a>(
            &'a self,
            _scope: &'a KeyedInjectCallerScope,
            _member: &'a str,
            _delivery_key: &'a str,
            _with_input: bool,
        ) -> KeyedInjectFuture<'a, Result<KeyedInjectStatusAnswer, KeyedInjectRefusal>> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Box::pin(async {
                Ok(KeyedInjectStatusAnswer {
                    status: NativeStatus::NotCommitted,
                    input: None,
                })
            })
        }
    }

    /// The serving side refuses a member in any form but its public durable
    /// identity before its receiver sees the request, for a delivery and a
    /// status read alike.
    #[tokio::test]
    async fn a_member_not_in_its_public_form_never_reaches_the_receiver() {
        let receiver = Arc::new(CountingReceiver::default());
        let slot: KeyedInjectSlot = Arc::new(RwLock::new(KeyedInjectServing::Serving(Arc::clone(
            &receiver,
        )
            as Arc<dyn KeyedInjectReceiver>)));
        let caller = AuthorizedCaller {
            label: "first-gateway".to_string(),
            pubkey: [7; 32],
        };
        for member in ["mk--agent_cspecialist", "rt:agent:specialist:0", ""] {
            let deliver = ControlRequest::InjectKeyed {
                remote_member: member.to_string(),
                delivery_key: "batch-1".to_string(),
                content_digest: "digest".to_string(),
                summary: "1 announcement".to_string(),
                body: "a body".to_string(),
                issued_at_ms: 1_000_000,
                nonce: "n1".to_string(),
                caller: None,
            };
            let status = ControlRequest::InjectStatus {
                remote_member: member.to_string(),
                delivery_key: "batch-1".to_string(),
                with_input: false,
                issued_at_ms: 1_000_000,
                nonce: "n2".to_string(),
                caller: None,
            };
            for request in [deliver, status] {
                match answer_keyed_inject_request(&slot, request, Some(&caller)).await {
                    ControlResponse::InjectRefused { code, .. } => assert_eq!(
                        code,
                        KeyedInjectRefusalCode::InvalidRequest.as_str(),
                        "{member:?}"
                    ),
                    other => panic!("expected a refusal for {member:?}, got {other:?}"),
                }
            }
        }
        assert_eq!(
            receiver.calls.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "nothing reached the receiver"
        );
    }

    #[test]
    fn members_must_be_public_durable_identities() {
        assert!(check_member_form("agent:specialist").is_ok());
        // The roster encoding of a public identity, and a generated runtime
        // alias, name an identity only after decoding: both are refused.
        for bad in ["", "mk--agent_cspecialist", "rt:agent:specialist:0"] {
            let refusal = check_member_form(bad).unwrap_err();
            assert_eq!(
                refusal.code,
                KeyedInjectRefusalCode::InvalidRequest,
                "{bad:?}"
            );
        }
    }

    fn receipt() -> NativeReceipt {
        NativeReceipt {
            delivery_id: "d".into(),
            address: "a".into(),
            submission_digest: "s".into(),
            committed_at_ms: 7,
        }
    }

    #[test]
    fn status_facts_distinguish_every_field() {
        let applied = NativeStatus::Applied {
            receipt: receipt(),
            session_id: "s1".into(),
            input_id: "i1".into(),
        };
        let other_input = NativeStatus::Applied {
            receipt: receipt(),
            session_id: "s1".into(),
            input_id: "i2".into(),
        };
        let pending = NativeStatus::Pending {
            receipt: receipt(),
            admitted: None,
        };
        let facts = status_facts(&applied, None);
        assert_ne!(facts, status_facts(&other_input, None));
        assert_ne!(facts, status_facts(&pending, None));
        assert_ne!(
            facts,
            status_facts(&applied, Some(&serde_json::json!({ "phase": "consumed" })))
        );
        assert_ne!(
            status_facts(&applied, Some(&serde_json::json!({ "phase": "consumed" }))),
            status_facts(&applied, Some(&serde_json::json!({ "phase": "queued" })))
        );
        let same = NativeStatus::Applied {
            receipt: receipt(),
            session_id: "s1".into(),
            input_id: "i1".into(),
        };
        assert_eq!(
            facts,
            status_facts(&same, None),
            "equal answers sign identically"
        );
    }

    #[test]
    fn native_status_keeps_its_wire_shape() {
        let applied = NativeStatus::Applied {
            receipt: receipt(),
            session_id: "s1".into(),
            input_id: "i1".into(),
        };
        let encoded = serde_json::to_value(&applied).unwrap();
        assert_eq!(encoded["status"], "applied");
        assert_eq!(
            serde_json::from_value::<NativeStatus>(encoded).unwrap(),
            applied
        );
        let pending: NativeStatus = serde_json::from_value(serde_json::json!({
            "status": "pending",
            "receipt": { "delivery_id": "d", "address": "a", "submission_digest": "s",
                         "committed_at_ms": 7 }
        }))
        .unwrap();
        assert_eq!(
            pending,
            NativeStatus::Pending {
                receipt: receipt(),
                admitted: None
            }
        );
    }
}
