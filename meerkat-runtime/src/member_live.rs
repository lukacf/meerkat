//! Member-host live seam (multi-host mobs §16, DEC-P6B-L2 / ADJ-P6B-1).
//!
//! The member runtime's comms drain serves four supervisor bridge commands
//! (`OpenMemberLiveChannel` / `CloseMemberLiveChannel` /
//! `MemberLiveChannelStatus` / `ControlMemberLiveChannel`) but the live
//! open/close/control pipeline lives ABOVE this crate, in the `meerkat`
//! facade (`LiveOrchestrator`). This module defines the injected host trait
//! the drain resolves — the `MemberObservationHost` precedent — speaking
//! contracts vocabulary only: no `meerkat-live` type crosses the seam, so
//! `meerkat-mob`'s local branch consumes the same trait while staying
//! live-feature-free (ADJ-P6B-1).
//!
//! Cause parity across placements: the ONE total
//! [`MemberLiveError::to_bridge_rejection`] conversion is shared by the
//! member-side drain arms and the controlling-side local branch, so a given
//! pipeline failure surfaces as the SAME `BridgeRejectionCause` regardless
//! of where the member session lives (pinned by T-C11 + T-L19).

use meerkat_contracts::wire::supervisor_bridge::{
    BridgeLiveControlOutcome, BridgeLiveControlVerb, BridgeRejectionCause,
};
use meerkat_contracts::{
    LiveCloseStatus, LiveOpenResult, LiveOpenTransport, RealtimeTurningMode, WireLiveAdapterStatus,
};
use meerkat_core::time_compat::Duration;
use meerkat_core::types::SessionId;

/// Mechanical custody for the machine-owned member-live lifecycle boundary.
///
/// The guard is minted by [`crate::MeerkatMachine`] and deliberately exposes
/// no mutation API.  Holding it means a session-scoped `live/open` and a
/// lifecycle owner cannot cross: opens retain the lease through their full
/// provider/transport materialization, while disposal retains it from the
/// absence proof through the durable retire/archive marker.
pub struct MemberLiveLifecycleLease {
    #[cfg(not(feature = "live"))]
    _uninhabited: std::convert::Infallible,
    #[cfg(feature = "live")]
    session_id: SessionId,
    #[cfg(feature = "live")]
    gate: std::sync::Arc<crate::tokio::sync::Mutex<()>>,
    #[cfg(feature = "live")]
    _guard: crate::tokio::sync::OwnedMutexGuard<()>,
}

#[cfg(feature = "live")]
impl MemberLiveLifecycleLease {
    pub(crate) fn new(
        session_id: SessionId,
        gate: std::sync::Arc<crate::tokio::sync::Mutex<()>>,
        guard: crate::tokio::sync::OwnedMutexGuard<()>,
    ) -> Self {
        Self {
            session_id,
            gate,
            _guard: guard,
        }
    }

    pub(crate) fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    pub(crate) fn matches_gate(
        &self,
        gate: &std::sync::Arc<crate::tokio::sync::Mutex<()>>,
    ) -> bool {
        std::sync::Arc::ptr_eq(&self.gate, gate)
    }
}

/// Member-side ceiling for one detached live open (ADJ-P6B-3).
///
/// The controlling host's `OpenMemberLiveChannel` round-trip runs under
/// `LIVE_OPEN_BRIDGE_TIMEOUT = 30s`. The member-side open is enforced
/// STRICTLY INSIDE that budget — 25s < 30s — so on the slow-provider path
/// the member fails closed (abort + `close_live_channel_after_open_failure`
/// eviction of any partial binding) and replies typed BEFORE the
/// controller's deadline fires. The ceiling deliberately bounds only the
/// OPEN — the fail-closed cleanup after an abort runs un-time-boxed,
/// because cutting cleanup off at a deadline could leak a half-installed
/// binding (the worse failure). Under a pathologically slow post-abort
/// cleanup the typed reply may therefore land after the controller's 30s;
/// that reply-loss — like the successful-open variant — resolves via the
/// caller-driven probe→close reconciliation primitives (DEC-P6B-C9):
/// `status(None)` discovers, `close(id)` clears, a fresh open succeeds.
pub const MEMBER_LIVE_OPEN_CEILING: Duration = Duration::from_secs(25);

/// Ceiling for each status/close step in runtime-owned member disposal.
/// Release and host-revoke retain their pre-terminal retry anchor when either
/// step times out; no session/archive or host receipt may publish first.
pub const MEMBER_LIVE_DISPOSAL_CEILING: Duration = Duration::from_secs(15);

/// Typed failure vocabulary for member live serving (DEC-P6B-L3).
///
/// Mirrors the six landed V4 live rejection causes plus the ADJ-P4-7
/// generic pair (`Unavailable` / `Internal`). Every facade pipeline failure
/// maps onto exactly one variant — never a stringly bucket — and every
/// variant maps totally onto a wire cause via
/// [`Self::to_bridge_rejection`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MemberLiveError {
    /// The member's resolved model has no realtime capability (B19).
    #[error("model {model} (provider {provider}) does not support realtime")]
    ModelNotRealtime { model: String, provider: String },
    /// No live adapter is available for the member's provider (B18 / #302
    /// provider-known adapter absence).
    #[error("provider {provider} has no live adapter wired on this host")]
    AdapterUnavailable { provider: String },
    /// The host serves no live transport for this open (no factory, no
    /// configured websocket transport).
    #[error("member host has no live transport configured")]
    TransportUnavailable,
    /// The member session already has an active live channel.
    #[error("member session already has an active live channel")]
    ChannelAlreadyBound,
    /// The addressed live channel does not exist for this member session.
    #[error("live channel not found")]
    ChannelNotFound,
    /// The requested live transport is not supported by this host.
    #[error("live transport {requested} is not supported by this host")]
    TransportUnsupported { requested: String },
    /// The live substrate cannot serve this member right now (session not
    /// resident, host not ready). Transient — degrade typed, never quiet.
    #[error("member live substrate unavailable: {reason}")]
    Unavailable { reason: String },
    /// An invariant was violated while serving. Maps to `Internal`.
    #[error("member live internal fault: {reason}")]
    Internal { reason: String },
}

impl MemberLiveError {
    /// The ONE total `MemberLiveError → BridgeRejectionCause` conversion
    /// (ADJ-P6B-1). Both the member drain arms and the controlling-side
    /// local branch project through this impl — cause parity across
    /// placements is exactly this match. Exhaustive by construction: a new
    /// variant forces an arm here.
    #[must_use]
    pub fn to_bridge_rejection(&self) -> BridgeRejectionCause {
        match self {
            Self::ModelNotRealtime { model, provider } => BridgeRejectionCause::ModelNotRealtime {
                model: model.clone(),
                provider: provider.clone(),
            },
            Self::AdapterUnavailable { provider } => BridgeRejectionCause::LiveAdapterUnavailable {
                provider: provider.clone(),
            },
            Self::TransportUnavailable => BridgeRejectionCause::LiveTransportUnavailable,
            Self::ChannelAlreadyBound => BridgeRejectionCause::LiveChannelAlreadyBound,
            Self::ChannelNotFound => BridgeRejectionCause::LiveChannelNotFound,
            Self::TransportUnsupported { requested } => {
                BridgeRejectionCause::LiveTransportUnsupported {
                    requested: requested.clone(),
                }
            }
            Self::Unavailable { .. } => BridgeRejectionCause::Unavailable,
            Self::Internal { .. } => BridgeRejectionCause::Internal,
        }
    }
}

/// One member live-channel point read — the reply shape the drain emits
/// (`BridgeReply::MemberLiveChannelStatusReport { channel_id, status }`),
/// carried losslessly so the serving arm does zero semantic interpretation.
/// `channel_id` echoes the RESOLVED channel: for a `status(None)` probe it
/// is the member's active channel discovered via
/// `live_active_channel_by_session` (ADJ-P6B-2 — the reply-loss
/// reconciliation discovery primitive).
#[derive(Debug, Clone, PartialEq)]
pub struct MemberLiveStatus {
    /// The resolved channel the status describes.
    pub channel_id: String,
    /// Wire status projected from generated live-channel status authority.
    pub status: WireLiveAdapterStatus,
}

/// Machine-wide injected member live host (ADJ-P6B-1). Implemented by the
/// facade (`meerkat::surface::ServiceMemberLiveHost`) over the ONE extracted
/// live pipeline; installed via `MeerkatMachine::set_member_live_host` by
/// the composing surface (the mob host daemon, live-capable `rkat-rpc`).
/// Absent host ⇒ the live drain arms reply typed `LiveTransportUnavailable`
/// (the live-specific sibling of the observation host's `Unavailable`: the
/// landed cause names precisely "this host serves no live substrate").
///
/// Session-id-addressed and object-safe; vocabulary is contracts-owned so
/// `meerkat-mob`'s local branch consumes the identical trait without live
/// features.
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
pub trait MemberLiveHost: Send + Sync {
    /// Run the full S1-S12 open pipeline for the member session.
    /// `turning_mode: None` ⇒ `ProviderManaged` (the pipeline owns the
    /// default, DEC-P6B-L15). Returns the owning host's `LiveOpenResult`
    /// VERBATIM — absolute advertised-URL bootstrap + machine-minted token.
    async fn open(
        &self,
        session: &SessionId,
        turning_mode: Option<RealtimeTurningMode>,
        transport: Option<LiveOpenTransport>,
    ) -> Result<LiveOpenResult, MemberLiveError>;

    /// Close the named channel (close-what-you-name; an unknown channel is
    /// typed [`MemberLiveError::ChannelNotFound`], idempotent-safe for the
    /// caller-driven reconciliation loop).
    async fn close(
        &self,
        session: &SessionId,
        channel_id: &str,
    ) -> Result<LiveCloseStatus, MemberLiveError>;

    /// Read-only point read. `channel_id: None` ⇒ resolve the member's
    /// active channel via `live_active_channel_by_session`; no active
    /// channel ⇒ typed [`MemberLiveError::ChannelNotFound`] (ADJ-P6B-2).
    async fn status(
        &self,
        session: &SessionId,
        channel_id: Option<String>,
    ) -> Result<MemberLiveStatus, MemberLiveError>;

    /// Drive one turn-level control verb (CommitInput / Interrupt /
    /// Truncate / Refresh — DL10's closed verb set) against the named
    /// channel, pinned to the member session pre-effect (DEC-P6B-L6).
    async fn control(
        &self,
        session: &SessionId,
        channel_id: &str,
        verb: BridgeLiveControlVerb,
    ) -> Result<BridgeLiveControlOutcome, MemberLiveError>;
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    /// T-L2: variant completeness against the six landed live causes plus
    /// the generic pair — compile-exhaustive: a new `MemberLiveError`
    /// variant forces an arm in `to_bridge_rejection` AND a row here.
    #[test]
    fn every_variant_maps_to_its_landed_cause() {
        let rows: Vec<(MemberLiveError, BridgeRejectionCause)> = vec![
            (
                MemberLiveError::ModelNotRealtime {
                    model: "gpt-5.4".to_string(),
                    provider: "openai".to_string(),
                },
                BridgeRejectionCause::ModelNotRealtime {
                    model: "gpt-5.4".to_string(),
                    provider: "openai".to_string(),
                },
            ),
            (
                MemberLiveError::AdapterUnavailable {
                    provider: "anthropic".to_string(),
                },
                BridgeRejectionCause::LiveAdapterUnavailable {
                    provider: "anthropic".to_string(),
                },
            ),
            (
                MemberLiveError::TransportUnavailable,
                BridgeRejectionCause::LiveTransportUnavailable,
            ),
            (
                MemberLiveError::ChannelAlreadyBound,
                BridgeRejectionCause::LiveChannelAlreadyBound,
            ),
            (
                MemberLiveError::ChannelNotFound,
                BridgeRejectionCause::LiveChannelNotFound,
            ),
            (
                MemberLiveError::TransportUnsupported {
                    requested: "webrtc".to_string(),
                },
                BridgeRejectionCause::LiveTransportUnsupported {
                    requested: "webrtc".to_string(),
                },
            ),
            (
                MemberLiveError::Unavailable {
                    reason: "session not resident".to_string(),
                },
                BridgeRejectionCause::Unavailable,
            ),
            (
                MemberLiveError::Internal {
                    reason: "invariant".to_string(),
                },
                BridgeRejectionCause::Internal,
            ),
        ];
        for (error, expected) in rows {
            assert_eq!(
                error.to_bridge_rejection(),
                expected,
                "cause mapping drifted for {error:?}"
            );
        }
    }

    /// T-L2: display text is the reply `reason`; keep the human-facing
    /// strings stable and reason-bearing.
    #[test]
    fn display_carries_the_reason_material() {
        assert_eq!(
            MemberLiveError::ModelNotRealtime {
                model: "m".to_string(),
                provider: "p".to_string(),
            }
            .to_string(),
            "model m (provider p) does not support realtime"
        );
        assert_eq!(
            MemberLiveError::TransportUnavailable.to_string(),
            "member host has no live transport configured"
        );
        assert_eq!(
            MemberLiveError::Unavailable {
                reason: "why".to_string()
            }
            .to_string(),
            "member live substrate unavailable: why"
        );
    }

    /// ADJ-P6B-3: the member open ceiling nests strictly inside the
    /// controlling bridge deadline (30s).
    #[test]
    fn open_ceiling_nests_inside_bridge_open_timeout() {
        assert!(MEMBER_LIVE_OPEN_CEILING < Duration::from_secs(30));
    }
}
