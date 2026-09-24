//! Controlling-side live-channel bridge proxy (multi-host mobs §16 /
//! DEC-P6B-C6, ADJ-P6B-11..15).
//!
//! Structural sibling of `member_history_proxy`: authority read →
//! `supervisor_spec_for_recipient` → typed `BridgeCommand` build →
//! `trust_recipient` → `send_bridge_command` → typed decode. Sends ride the
//! relaxed READ-guarded gate, so live control verbs never serialize behind
//! member long-polls (§16.7). Every caller dispatches OFF the actor loop
//! (ADJ-P4-12 / the phase-3 deadlock class).
//!
//! Token discipline (DEC-P6B-C8): the decoded [`LiveOpenResult`] is treated
//! as an OPAQUE value on every controlling path — no field access beyond the
//! typed decode, no `Debug` formatting into tracing or error reasons, no
//! retention after the reply channel. The controlling host holds no channel
//! map (DL2) and never mints, validates, or refreshes tokens (§16.9).

use std::sync::Arc;
use std::time::Duration;

use meerkat_core::comms::TrustedPeerDescriptor;

use super::MobSupervisorBridge;
use super::bridge_protocol::{
    BridgeCommand, BridgeLiveChannelPayload, BridgeLiveControlOutcome, BridgeLiveControlPayload,
    BridgeLiveControlVerb, BridgeLiveControlledResponse, BridgeLiveOpenPayload,
    BridgeLiveOpenedResponse, BridgeLiveStatusPayload, BridgeProtocolVersion, LiveCloseStatus,
    LiveOpenResult, LiveOpenTransport, RealtimeTurningMode, WireLiveAdapterStatus,
};
use crate::MobError;

/// ADJ-P6B-3: bridge budget for a proxied live OPEN. The owning host's open
/// pipeline does per-open credential resolution plus a provider realtime WS
/// connect, so the open rides the bridge's verified-ack budget (30s) rather
/// than the point-read budget. NESTING: the member-side detached open
/// carries its OWN ceiling (`MEMBER_LIVE_OPEN_CEILING` = 25s, pipeline
/// lane), strictly inside this deadline, so the member fails closed (abort +
/// `close_live_channel_after_open_failure`) BEFORE the controller's deadline
/// on the slow-provider path; the residual overlap window resolves via the
/// DEC-P6B-C9 probe→close primitives.
pub(crate) const LIVE_OPEN_BRIDGE_TIMEOUT: Duration = Duration::from_secs(30);

/// ADJ-P6B-3: close/status/control are point reads / small local mutations
/// on the owning host — the `HISTORY_BRIDGE_TIMEOUT` budget class (15s).
pub(crate) const LIVE_CHANNEL_BRIDGE_TIMEOUT: Duration = Duration::from_secs(15);

/// Domain carrier for one member live-channel status point read (local or
/// remote — ONE shape, DEC-P6B-C1; the `MemberHistoryPageDomain`
/// shape-with-provenance pattern). `channel_id` is the wire's `String`
/// vocabulary by design: `LiveOpenResult.channel_id` owns it as `String`,
/// and a third mob-side channel-id newtype would be dual vocabulary, not
/// newtype discipline.
#[derive(Debug, Clone)]
pub struct MemberLiveStatusDomain {
    pub channel_id: String,
    pub status: WireLiveAdapterStatus,
}

/// Proxied `OpenMemberLiveChannel`: the owning host runs its local open
/// pipeline unchanged and the returned [`LiveOpenResult`] — the owning
/// host's absolute advertised WS URL + single-use token — crosses back
/// VERBATIM (DEC-P6B-C7: no parse, no rewrite, no loopback substitution).
pub(crate) async fn open_remote_member_live_channel(
    bridge: &Arc<MobSupervisorBridge>,
    peer: &TrustedPeerDescriptor,
    expected_member: super::bridge_protocol::BridgeMemberIncarnation,
    turning_mode: Option<RealtimeTurningMode>,
    transport: Option<LiveOpenTransport>,
) -> Result<LiveOpenResult, MobError> {
    let authority = bridge.authority().await;
    let sup_spec = bridge.supervisor_spec_for_recipient(peer).await?;
    let command = BridgeCommand::OpenMemberLiveChannel(BridgeLiveOpenPayload {
        supervisor: sup_spec.into(),
        epoch: authority.epoch,
        protocol_version: BridgeProtocolVersion::V4,
        expected_member,
        turning_mode,
        transport,
    });
    let _ = bridge.trust_recipient(peer).await?;
    let value = bridge
        .send_bridge_command(peer, &command, LIVE_OPEN_BRIDGE_TIMEOUT)
        .await?;
    let opened: BridgeLiveOpenedResponse =
        super::bridge_protocol::decode_bridge_payload(&command, value, "open member live channel")?;
    Ok(opened.open)
}

/// Proxied `CloseMemberLiveChannel` — close-what-you-name (required id,
/// DEC-P6B-C9's CAS property). An already-clear channel is a typed
/// `LiveChannelNotFound` rejection member-side, which reconciling callers
/// may treat as "nothing left to clear".
pub(crate) async fn close_remote_member_live_channel(
    bridge: &Arc<MobSupervisorBridge>,
    peer: &TrustedPeerDescriptor,
    expected_member: super::bridge_protocol::BridgeMemberIncarnation,
    channel_id: String,
) -> Result<LiveCloseStatus, MobError> {
    let authority = bridge.authority().await;
    let sup_spec = bridge.supervisor_spec_for_recipient(peer).await?;
    let command = BridgeCommand::CloseMemberLiveChannel(BridgeLiveChannelPayload {
        supervisor: sup_spec.into(),
        epoch: authority.epoch,
        protocol_version: BridgeProtocolVersion::V4,
        expected_member,
        channel_id,
    });
    let _ = bridge.trust_recipient(peer).await?;
    let value = bridge
        .send_bridge_command(peer, &command, LIVE_CHANNEL_BRIDGE_TIMEOUT)
        .await?;
    super::bridge_protocol::decode_bridge_payload(&command, value, "close member live channel")
}

/// Proxied `MemberLiveChannelStatus` point read. `channel_id: None` =
/// "the member's active channel" (ADJ-P6B-2) — the reply-loss DISCOVERY
/// primitive; no active channel is a typed `LiveChannelNotFound`.
pub(crate) async fn remote_member_live_status(
    bridge: &Arc<MobSupervisorBridge>,
    peer: &TrustedPeerDescriptor,
    expected_member: super::bridge_protocol::BridgeMemberIncarnation,
    channel_id: Option<String>,
) -> Result<MemberLiveStatusDomain, MobError> {
    let authority = bridge.authority().await;
    let sup_spec = bridge.supervisor_spec_for_recipient(peer).await?;
    let command = BridgeCommand::MemberLiveChannelStatus(BridgeLiveStatusPayload {
        supervisor: sup_spec.into(),
        epoch: authority.epoch,
        protocol_version: BridgeProtocolVersion::V4,
        expected_member,
        channel_id,
    });
    let _ = bridge.trust_recipient(peer).await?;
    let value = bridge
        .send_bridge_command(peer, &command, LIVE_CHANNEL_BRIDGE_TIMEOUT)
        .await?;
    super::bridge_protocol::decode_bridge_payload(&command, value, "member live channel status")
}

/// Proxied `ControlMemberLiveChannel` (DL10's turn-level verb vocabulary —
/// commit_input / interrupt / truncate / refresh; frame-level input rides
/// the direct WS, never the bridge).
pub(crate) async fn control_remote_member_live_channel(
    bridge: &Arc<MobSupervisorBridge>,
    peer: &TrustedPeerDescriptor,
    expected_member: super::bridge_protocol::BridgeMemberIncarnation,
    channel_id: String,
    verb: BridgeLiveControlVerb,
) -> Result<BridgeLiveControlOutcome, MobError> {
    let authority = bridge.authority().await;
    let sup_spec = bridge.supervisor_spec_for_recipient(peer).await?;
    let command = BridgeCommand::ControlMemberLiveChannel(BridgeLiveControlPayload {
        supervisor: sup_spec.into(),
        epoch: authority.epoch,
        protocol_version: BridgeProtocolVersion::V4,
        expected_member,
        channel_id,
        verb,
    });
    let _ = bridge.trust_recipient(peer).await?;
    let value = bridge
        .send_bridge_command(peer, &command, LIVE_CHANNEL_BRIDGE_TIMEOUT)
        .await?;
    let controlled: BridgeLiveControlledResponse = super::bridge_protocol::decode_bridge_payload(
        &command,
        value,
        "control member live channel",
    )?;
    Ok(controlled.outcome)
}

// T-C13 — controlling-side typed decode rows: opened → `LiveOpenResult`
// VERBATIM (token bytes included); `Rejected{cause}` →
// `MobError::BridgeCommandRejected` with the cause preserved; a reply-kind
// mismatch is a typed protocol fault, never a coerced success.
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::super::bridge_protocol::{
        BridgeMemberIncarnation, BridgePeerSpec, BridgeProtocolVersion, BridgeRejectionCause,
        BridgeReply, WireLiveTransportBootstrap, decode_bridge_payload,
    };
    use super::*;
    use meerkat_contracts::wire::{WireLiveChannelCapabilities, WireLiveContinuityMode};

    fn sample_peer_spec() -> BridgePeerSpec {
        BridgePeerSpec {
            name: "mob-a/__mob_supervisor__".to_string(),
            peer_id: "peer-lead".to_string(),
            address: "inproc://mob/supervisor/lead".to_string(),
            pubkey: [7u8; 32],
        }
    }

    fn open_command() -> BridgeCommand {
        BridgeCommand::OpenMemberLiveChannel(BridgeLiveOpenPayload {
            supervisor: sample_peer_spec(),
            epoch: 1,
            protocol_version: BridgeProtocolVersion::V4,
            turning_mode: None,
            transport: Some(LiveOpenTransport::Websocket),
            expected_member: BridgeMemberIncarnation::default(),
        })
    }

    fn status_command(channel_id: Option<String>) -> BridgeCommand {
        BridgeCommand::MemberLiveChannelStatus(BridgeLiveStatusPayload {
            supervisor: sample_peer_spec(),
            epoch: 1,
            protocol_version: BridgeProtocolVersion::V4,
            channel_id,
            expected_member: BridgeMemberIncarnation::default(),
        })
    }

    fn sample_open_result() -> LiveOpenResult {
        LiveOpenResult {
            channel_id: "chan-open-1".to_string(),
            transport: WireLiveTransportBootstrap::Websocket {
                url: "ws://live.advertised.test:19777/live/ws?token=tok-bytes&channel=chan-open-1"
                    .to_string(),
                token: "tok-bytes".to_string(),
            },
            capabilities: WireLiveChannelCapabilities {
                audio_in: true,
                audio_out: true,
                text_in: true,
                text_out: true,
                image_in: false,
                video_in: false,
                transcript_supported: true,
                barge_in_supported: true,
                provider_native_resume: false,
            },
            continuity: WireLiveContinuityMode::Fresh,
        }
    }

    #[test]
    fn opened_reply_decodes_live_open_result_verbatim() {
        let expected = sample_open_result();
        let reply = BridgeReply::MemberLiveChannelOpened(BridgeLiveOpenedResponse {
            open: expected.clone(),
        });
        let value = serde_json::to_value(&reply).expect("serialize opened reply");
        let decoded: BridgeLiveOpenedResponse =
            decode_bridge_payload(&open_command(), value, "open member live channel")
                .expect("opened payload decodes");
        // VERBATIM: every field — token bytes included — survives the
        // round trip untouched (gotcha #14 / DEC-P6B-C7).
        assert_eq!(decoded.open, expected);
    }

    #[test]
    fn rejected_reply_surfaces_typed_live_cause() {
        let reply = BridgeReply::Rejected {
            cause: BridgeRejectionCause::LiveChannelAlreadyBound,
            reason: "session already has an active live channel".to_string(),
        };
        let value = serde_json::to_value(&reply).expect("serialize rejection");
        let error = decode_bridge_payload::<BridgeLiveOpenedResponse>(
            &open_command(),
            value,
            "open member live channel",
        )
        .expect_err("rejection must surface as a typed error");
        match error {
            MobError::BridgeCommandRejected { cause, reason } => {
                assert_eq!(cause, BridgeRejectionCause::LiveChannelAlreadyBound);
                assert!(reason.contains("active live channel"));
            }
            other => panic!("expected BridgeCommandRejected, got {other:?}"),
        }
    }

    #[test]
    fn status_report_extracts_into_domain_carrier() {
        let reply = BridgeReply::MemberLiveChannelStatusReport {
            channel_id: "chan-disc-1".to_string(),
            status: WireLiveAdapterStatus::Ready,
        };
        let value = serde_json::to_value(&reply).expect("serialize status report");
        // Channel-less command (ADJ-P6B-2 discovery): the reply ECHOES the
        // member-resolved id.
        let domain: MemberLiveStatusDomain =
            decode_bridge_payload(&status_command(None), value, "member live channel status")
                .expect("status report decodes");
        assert_eq!(domain.channel_id, "chan-disc-1");
        assert_eq!(domain.status, WireLiveAdapterStatus::Ready);
    }

    #[test]
    fn reply_kind_mismatch_is_a_typed_protocol_fault() {
        // A closed reply for an OPEN command must never coerce into a
        // synthetic open success.
        let reply = BridgeReply::MemberLiveChannelClosed {
            status: LiveCloseStatus::Closed,
        };
        let value = serde_json::to_value(&reply).expect("serialize closed reply");
        let error = decode_bridge_payload::<BridgeLiveOpenedResponse>(
            &open_command(),
            value,
            "open member live channel",
        )
        .expect_err("kind mismatch must surface as an error");
        assert!(
            error
                .to_string()
                .contains("unexpected open member live channel bridge reply"),
            "mismatch is reported as a typed protocol error: {error}"
        );
    }

    #[test]
    fn adjudicated_bridge_budgets_hold() {
        // ADJ-P6B-3: 30s controlling ceiling nests the member's 25s
        // detached-open ceiling; point reads ride the history budget.
        assert_eq!(LIVE_OPEN_BRIDGE_TIMEOUT, Duration::from_secs(30));
        assert_eq!(LIVE_CHANNEL_BRIDGE_TIMEOUT, Duration::from_secs(15));
        assert!(LIVE_CHANNEL_BRIDGE_TIMEOUT < LIVE_OPEN_BRIDGE_TIMEOUT);
    }
}
