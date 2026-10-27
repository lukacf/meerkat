//! How a member's deliveries wait, and the one kind of fact about the member
//! that ends its subscriptions.
//!
//! A delivery that cannot go on now is never ended for it. Its batch waits,
//! and the owner keeps one [`SuspendMarker`] per member that says why and on
//! what evidence: a projection for operators, rebuilt from the next attempt
//! after a restart, and never read to decide a transition.
//!
//! A subscription ends only on a final fact. About the member that is its
//! departure, a [`MemberDeparture`], which is built only from the departure
//! facts its constructors name.

use std::fmt;

use serde::{Deserialize, Serialize};

use super::types::MobId;
use crate::runtime::keyed_inject::{NotServedCause, StaleRequestCause};

/// How strong the evidence behind an outcome is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum EvidenceClass {
    /// A signed answer whose signature verified with the key pinned for the
    /// peer.
    Verified,
    /// An unsigned answer, one that failed verification, or one that was
    /// lost or garbled. Never evidence of anything about the member.
    Unverified,
    /// This gateway's own fact.
    Local,
}

impl EvidenceClass {
    /// Stable wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Verified => "verified",
            Self::Unverified => "unverified",
            Self::Local => "local",
        }
    }
}

/// Why this gateway's runtime does not serve a member it has registered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum NotServedHereCause {
    /// The member is placed on a host.
    PlacedOnHost,
    /// The member is bound to an external runtime.
    ExternalBinding,
}

impl NotServedHereCause {
    /// Stable wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PlacedOnHost => "placed_on_host",
            Self::ExternalBinding => "external_binding",
        }
    }
}

/// Why a member's deliveries wait. Each names an availability state that can
/// change; none ends anything.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum SuspendCause {
    /// The peer's keyed inject plane is not composed yet: it is starting.
    PeerComposing,
    /// The peer does not serve keyed injects, for its typed cause. An absent
    /// cause, or one this build does not know, reads as `unknown`.
    PeerNotServing { cause: NotServedCause },
    /// The peer refused the request as issued outside its window. Both
    /// values are this gateway's own: the issue time it stamped and the
    /// window it is configured with.
    ClockSkew { issued_at_ms: u64, window_ms: u64 },
    /// The member is not registered where it is served: a signed
    /// `unknown_member`, or a miss in this gateway's own registry.
    MemberUnknown,
    /// The member is registered but not served where the delivery went:
    /// placed on a host or bound to an external runtime. A peer does not say
    /// which.
    MemberNotServedHere { cause: Option<NotServedHereCause> },
    /// The member's runtime admits work only through a native work
    /// authorization host, which refuses host continuations. Configuration
    /// can change it.
    Governed,
    /// The member has no live incarnation now; a respawn ends this.
    MemberNotLive,
    /// The peer could not read or write its native state now. Nothing is
    /// implied: the batch's status is read next.
    PeerNativeUnavailable,
    /// A signed refusal with a code this build does not know. Its meaning is
    /// unknown, so it is never read as "nothing committed" and ends nothing.
    PeerRefusedUnknown { code: String },
    /// This gateway cannot call the peer authenticated: its contact or key
    /// is missing or unusable. The peer was never asked.
    LocalNotAuthenticatable,
    /// An answer arrived but its signature did not verify (key drift or
    /// tampering). The outcome is unknown.
    AnswerFailedVerification,
    /// No signed answer: the peer was unreachable, or answered unsigned.
    /// `code` is the unsigned code, kept as text for operators and never
    /// branched on.
    PeerUnanswered { code: Option<String> },
    /// This gateway's own native owner could not be read or written now.
    NativeUnavailable,
    /// The peer refused the request for its timing, signed, and committed
    /// nothing: it was issued before the peer's serving process started, or
    /// the peer could not commit it before its deadline, or the cause is one
    /// this build does not know. The batch is owed again. A refusal of a
    /// request outside the peer's window reads as [`Self::ClockSkew`].
    StaleRequest { cause: StaleRequestCause },
    /// The peer refused a status read, signed, with a code this build
    /// knows but that settles nothing for a read (`conflict`, for one); the
    /// outcome stays unknown.
    PeerRefused { code: String },
}

impl SuspendCause {
    /// Whether two causes name the same wait: equal, except that a clock
    /// skew is the same wait whichever request it was read from.
    pub(crate) fn same_wait(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::ClockSkew { window_ms: a, .. }, Self::ClockSkew { window_ms: b, .. }) => a == b,
            _ => self == other,
        }
    }

    /// Stable wire spelling of the kind.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::PeerComposing => "peer_composing",
            Self::PeerNotServing { .. } => "peer_not_serving",
            Self::ClockSkew { .. } => "clock_skew",
            Self::MemberUnknown => "member_unknown",
            Self::MemberNotServedHere { .. } => "member_not_served_here",
            Self::Governed => "governed",
            Self::MemberNotLive => "member_not_live",
            Self::PeerNativeUnavailable => "peer_native_unavailable",
            Self::PeerRefusedUnknown { .. } => "peer_refused_unknown",
            Self::LocalNotAuthenticatable => "local_not_authenticatable",
            Self::AnswerFailedVerification => "answer_failed_verification",
            Self::PeerUnanswered { .. } => "peer_unanswered",
            Self::NativeUnavailable => "native_unavailable",
            Self::StaleRequest { .. } => "stale_request",
            Self::PeerRefused { .. } => "peer_refused",
        }
    }
}

impl fmt::Display for SuspendCause {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PeerNotServing { cause } => write!(f, "peer_not_serving ({cause})"),
            Self::ClockSkew {
                issued_at_ms,
                window_ms,
            } => write!(
                f,
                "clock_skew (issued at {issued_at_ms} ms, window {window_ms} ms)"
            ),
            Self::MemberNotServedHere { cause: Some(cause) } => {
                write!(f, "member_not_served_here ({})", cause.as_str())
            }
            Self::PeerRefusedUnknown { code } => write!(f, "peer_refused_unknown ({code})"),
            Self::PeerRefused { code } => write!(f, "peer_refused ({code})"),
            Self::PeerUnanswered { code: Some(code) } => write!(f, "peer_unanswered ({code})"),
            Self::StaleRequest { cause } => write!(f, "stale_request ({cause})"),
            other => f.write_str(other.kind()),
        }
    }
}

/// Why a member's deliveries wait now, and on what evidence: the latest
/// classified outcome of an attempt for the member.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct SuspendMarker {
    pub cause: SuspendCause,
    pub evidence: EvidenceClass,
    /// When this gateway first saw this cause on this evidence, in a row.
    pub since_ms: u64,
}

/// How a member's departure is known.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum DepartureEvidence {
    /// The member's peer gateway answered, signed, that the member departed.
    SignedPeerDeparture { peer: MobId },
}

/// That a member left for good: the one fact about a member that ends its
/// subscriptions. It is built only from the departure facts its
/// constructors name, never from a miss, a refusal that can change, or a
/// silence. It derives `Deserialize` only so the owner's own store can read
/// it back; one deserialized anywhere else is outside this contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemberDeparture {
    evidence: DepartureEvidence,
}

impl MemberDeparture {
    /// From the member's peer gateway's signed `member_departed` answer,
    /// verified with the key pinned for that peer.
    pub(crate) fn from_signed_departure(peer: MobId) -> Self {
        Self {
            evidence: DepartureEvidence::SignedPeerDeparture { peer },
        }
    }

    /// How the departure is known.
    pub fn evidence(&self) -> &DepartureEvidence {
        &self.evidence
    }
}

impl fmt::Display for MemberDeparture {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.evidence {
            DepartureEvidence::SignedPeerDeparture { peer } => {
                write!(f, "its gateway '{peer}' answered, signed, that it departed")
            }
        }
    }
}

/// Why a batch is blocked: native evidence or the request itself rules out
/// any later attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum BlockCause {
    /// The batch id is bound to other content.
    Conflict,
    /// The peer answered, signed, that the request is invalid. The request
    /// is deterministic, so a retry cannot help.
    PeerInvalidRequest,
    /// The request this gateway built is larger than one control frame.
    RequestOversize,
    /// The native owner refused the delivery as it is.
    NativeInvalid,
    /// A native status read contradicts what this batch holds.
    NativeInconsistent,
    /// The batch's native key is not a key the native owner accepts.
    NativeKeyInvalid,
}

impl BlockCause {
    /// Stable wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Conflict => "conflict",
            Self::PeerInvalidRequest => "peer_invalid_request",
            Self::RequestOversize => "request_oversize",
            Self::NativeInvalid => "native_invalid",
            Self::NativeInconsistent => "native_inconsistent",
            Self::NativeKeyInvalid => "native_key_invalid",
        }
    }

    /// The text a blocked batch keeps: the typed cause first, then what the
    /// evidence said.
    pub(crate) fn with_detail(self, detail: &str) -> String {
        if detail.is_empty() {
            self.as_str().to_string()
        } else {
            format!("{}: {detail}", self.as_str())
        }
    }
}
