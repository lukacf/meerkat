//! Placement-switched member history reads (multi-host mobs §19.L2 /
//! DEC-P6E-20/21).
//!
//! ONE page shape for local and remote members by construction: the local
//! branch projects its `SessionHistoryPage` through
//! `WireMemberHistoryPageBody::try_from_history_page` (the same projection the
//! member host's `ReadMemberHistory` arm serves), and the remote branch
//! decodes the bridge page verbatim. Both fork sites and the
//! `MobCommand::MemberHistory` verb route through these helpers; the
//! remote sends run OFF the actor loop (ADJ-P4-12).

use std::sync::Arc;
use std::time::Duration;

use meerkat_contracts::wire::{WireMemberHistoryPageBody, WireProjectionProvenance};
use meerkat_core::comms::TrustedPeerDescriptor;

use super::MobSupervisorBridge;
use super::bridge_protocol::{
    BridgeCommand, BridgeMemberHistoryPage, BridgeMemberIncarnation,
    BridgeMemberVisibleToolsResponse, BridgeProtocolVersion, BridgeReadHistoryPayload,
    BridgeReadVisibleToolsPayload, BridgeRejectionCause,
};
use crate::MobError;
use crate::machines::mob_machine as mob_dsl;
use crate::tokio;

/// DEC-P6E-24: the `ReadMemberHistory` bridge budget per page.
pub(crate) const HISTORY_BRIDGE_TIMEOUT: Duration = Duration::from_secs(15);

/// The bound on reading a placed fork source's visible tools (V7
/// `ReadMemberVisibleTools`). A fork does not wait on a slow member host:
/// past it the child's fork notice says the source's tools were not
/// compared (`placed_source`).
pub(crate) const VISIBLE_TOOLS_BRIDGE_TIMEOUT: Duration = Duration::from_secs(5);

/// Domain carrier for one member-history page (local or remote — one
/// shape). `page` IS the shared wire body: pagination facts live inside it
/// (one owner); `generation` is the placement-side transcript-domain fact.
///
/// `placement`/`provenance` (phase 7, ADJ-P7-2): set by the SERVING branch
/// itself — the remote proxy stamps the owning host id +
/// [`WireProjectionProvenance::HostClaimed`] (the page crossed the bridge,
/// attested only by that host); the controlling host's local branch stamps
/// `None` + [`WireProjectionProvenance::ControllingHostVerified`]. Surface
/// handlers never re-derive placement.
#[derive(Debug, Clone)]
pub struct MemberHistoryPageDomain {
    pub generation: u64,
    pub page: WireMemberHistoryPageBody,
    /// Owning host id when the page was served over the bridge; `None` =
    /// served from the controlling host's own store.
    pub placement: Option<mob_dsl::HostId>,
    /// Who attests the page (§7/§20).
    pub provenance: WireProjectionProvenance,
}

async fn read_remote_page(
    bridge: &Arc<MobSupervisorBridge>,
    peer: &TrustedPeerDescriptor,
    placement: mob_dsl::HostId,
    expected_member: &super::bridge_protocol::BridgeMemberIncarnation,
    from_index: Option<u64>,
    limit: Option<u32>,
) -> Result<MemberHistoryPageDomain, MobError> {
    let authority = bridge.authority().await;
    let sup_spec = bridge.supervisor_spec_for_recipient(peer).await?;
    let command = BridgeCommand::ReadMemberHistory(BridgeReadHistoryPayload {
        supervisor: sup_spec.into(),
        epoch: authority.epoch,
        protocol_version: BridgeProtocolVersion::V4,
        expected_member: expected_member.clone(),
        from_index,
        limit,
    });
    let _ = bridge.trust_recipient(peer).await?;
    let value = bridge
        .send_bridge_command(peer, &command, HISTORY_BRIDGE_TIMEOUT)
        .await?;
    let page: BridgeMemberHistoryPage =
        super::bridge_protocol::decode_bridge_payload(&command, value, "read member history")?;
    Ok(MemberHistoryPageDomain {
        generation: page.generation,
        page: page.page,
        // The remote branch IS the placement switch's remote arm
        // (ADJ-P7-2): `placement` is the machine's `member_placement` fact
        // captured by the actor before dispatch — the page crossed the
        // bridge, so the owning host claims it.
        placement: Some(placement),
        provenance: WireProjectionProvenance::HostClaimed,
    })
}

/// One remote page (the `member_history` verb's remote branch). Pagination
/// stays caller-driven via `from_index`/`limit` — identical to the local
/// branch's one-page semantics. `placement` is the machine's recorded
/// owning host for the member (the actor's placement switch supplies it —
/// never re-derived from transport material).
pub(crate) async fn read_remote_member_history_page(
    bridge: &Arc<MobSupervisorBridge>,
    peer: &TrustedPeerDescriptor,
    placement: mob_dsl::HostId,
    expected_member: super::bridge_protocol::BridgeMemberIncarnation,
    from_index: Option<u64>,
    limit: Option<u32>,
) -> Result<MemberHistoryPageDomain, MobError> {
    read_remote_page(bridge, peer, placement, &expected_member, from_index, limit).await
}

fn checked_page_end(page: &WireMemberHistoryPageBody) -> Result<u64, MobError> {
    let served = u64::try_from(page.messages.len()).map_err(|_| {
        MobError::Internal("member history page row count does not fit u64".to_string())
    })?;
    let end = page.from_index.checked_add(served).ok_or_else(|| {
        MobError::Internal("member history page index overflowed u64".to_string())
    })?;
    if end > page.message_count {
        return Err(MobError::Internal(format!(
            "member history page ends at {end} beyond its message_count {}",
            page.message_count
        )));
    }
    match (page.complete, page.next_index) {
        (true, None) => {}
        (true, Some(next)) => {
            return Err(MobError::Internal(format!(
                "complete member history page unexpectedly carries next_index {next}"
            )));
        }
        (false, Some(next)) if next == end => {}
        (false, Some(next)) => {
            return Err(MobError::Internal(format!(
                "member history page next_index {next} does not equal served end {end}"
            )));
        }
        (false, None) => {
            return Err(MobError::Internal(
                "incomplete member history page carries no next_index".to_string(),
            ));
        }
    }
    Ok(end)
}

/// Finish the immutable prefix described by the first page's
/// `message_count`. The transcript may append while pages are crossing the
/// bridge; pinning the initial end prevents `FullHistory` from chasing a hot
/// session forever and keeps `LastMessages(n)` from silently widening past
/// the requested tail. A generation change or transcript shrink is a typed
/// retryable inconsistency.
async fn finish_remote_history_snapshot(
    bridge: &Arc<MobSupervisorBridge>,
    peer: &TrustedPeerDescriptor,
    placement: mob_dsl::HostId,
    expected_member: &super::bridge_protocol::BridgeMemberIncarnation,
    mut aggregate: MemberHistoryPageDomain,
) -> Result<MemberHistoryPageDomain, MobError> {
    let generation = aggregate.generation;
    let snapshot_end = aggregate.page.message_count;
    loop {
        let end = checked_page_end(&aggregate.page)?;
        if end == snapshot_end {
            aggregate.page.message_count = snapshot_end;
            aggregate.page.next_index = None;
            aggregate.page.complete = true;
            return Ok(aggregate);
        }
        if aggregate.page.messages.is_empty() {
            return Err(MobError::Internal(format!(
                "member history page made no progress at index {} before snapshot end {snapshot_end}",
                aggregate.page.from_index
            )));
        }
        let next_index = aggregate.page.next_index.ok_or_else(|| {
            MobError::Internal(format!(
                "member history snapshot ended at {end} before its declared message_count {snapshot_end}"
            ))
        })?;
        let remaining = snapshot_end - end;
        let limit = u32::try_from(remaining).unwrap_or(u32::MAX).max(1);
        let page = read_remote_page(
            bridge,
            peer,
            placement.clone(),
            expected_member,
            Some(next_index),
            Some(limit),
        )
        .await?;
        if page.generation != generation {
            return Err(MobError::Internal(format!(
                "member transcript generation changed mid-read ({generation} -> {}); retry the read",
                page.generation
            )));
        }
        if page.page.from_index != next_index {
            return Err(MobError::Internal(format!(
                "member history continuation requested index {next_index} but host served {}",
                page.page.from_index
            )));
        }
        if page.page.message_count < snapshot_end {
            return Err(MobError::Internal(format!(
                "member transcript shrank mid-read ({snapshot_end} -> {}); retry the read",
                page.page.message_count
            )));
        }
        let page_end = checked_page_end(&page.page)?;
        if page_end > snapshot_end {
            return Err(MobError::Internal(format!(
                "member history continuation crossed pinned snapshot end {snapshot_end} (served through {page_end})"
            )));
        }
        if page.page.messages.is_empty() && page_end < snapshot_end {
            return Err(MobError::Internal(format!(
                "member history continuation made no progress at index {next_index} before snapshot end {snapshot_end}"
            )));
        }
        aggregate.page.messages.extend(page.page.messages);
        aggregate.page.next_index = page.page.next_index;
        aggregate.page.complete = page.page.complete;
    }
}

/// Full-transcript accumulation for fork `FullHistory` (DEC-P6E-20):
/// follow `next_index` until `complete`. Every page is served under the
/// member host's per-page cap; a mid-accumulation generation bump is a
/// typed inconsistency (the transcript domain restarted under us).
pub(crate) async fn read_remote_member_full_history(
    bridge: &Arc<MobSupervisorBridge>,
    peer: &TrustedPeerDescriptor,
    placement: mob_dsl::HostId,
    expected_member: super::bridge_protocol::BridgeMemberIncarnation,
) -> Result<MemberHistoryPageDomain, MobError> {
    let first = read_remote_page(
        bridge,
        peer,
        placement.clone(),
        &expected_member,
        Some(0),
        None,
    )
    .await?;
    if first.page.from_index != 0 {
        return Err(MobError::Internal(format!(
            "full member history read requested index 0 but host served {}",
            first.page.from_index
        )));
    }
    finish_remote_history_snapshot(bridge, peer, placement, &expected_member, first).await
}

/// Tail read for fork `LastMessages(n)`: start with the tail-addressed
/// serving semantics (DEC-P6E-6 — `from_index: None, limit: Some(n)`) and
/// follow `next_index` when the host byte-bounds a large page. The initial
/// `message_count` pins the tail snapshot so concurrent appends cannot widen
/// the fork context.
pub(crate) async fn read_remote_member_history_tail(
    bridge: &Arc<MobSupervisorBridge>,
    peer: &TrustedPeerDescriptor,
    placement: mob_dsl::HostId,
    expected_member: super::bridge_protocol::BridgeMemberIncarnation,
    count: u32,
) -> Result<MemberHistoryPageDomain, MobError> {
    let first = if count == 0 {
        // Address beyond the transcript so the host returns generation and
        // message_count without serializing even one (potentially very large)
        // history row.
        read_remote_page(
            bridge,
            peer,
            placement.clone(),
            &expected_member,
            Some(u64::MAX),
            Some(1),
        )
        .await?
    } else {
        read_remote_page(
            bridge,
            peer,
            placement.clone(),
            &expected_member,
            None,
            Some(count),
        )
        .await?
    };
    if count == 0 {
        if first.page.from_index != first.page.message_count || !first.page.messages.is_empty() {
            return Err(MobError::Internal(
                "zero-count member history tail did not resolve to an empty end page".to_string(),
            ));
        }
        return Ok(first);
    }
    finish_remote_history_snapshot(bridge, peer, placement, &expected_member, first).await
}

/// A placed fork source's visible tool names, read from its member host for
/// the child's fork notice (V7 `ReadMemberVisibleTools`), within
/// [`VISIBLE_TOOLS_BRIDGE_TIMEOUT`].
pub(crate) async fn read_remote_member_visible_tools(
    bridge: &Arc<MobSupervisorBridge>,
    peer: &TrustedPeerDescriptor,
    expected_member: &BridgeMemberIncarnation,
) -> meerkat_core::fork_notice::ForkSeedSourceTools {
    let read = async {
        let authority = bridge.authority().await;
        let sup_spec = bridge.supervisor_spec_for_recipient(peer).await?;
        let command = BridgeCommand::ReadMemberVisibleTools(BridgeReadVisibleToolsPayload {
            supervisor: sup_spec.into(),
            epoch: authority.epoch,
            protocol_version: BridgeProtocolVersion::V7,
            expected_member: expected_member.clone(),
        });
        let _ = bridge.trust_recipient(peer).await?;
        let value = bridge
            .send_bridge_command(peer, &command, VISIBLE_TOOLS_BRIDGE_TIMEOUT)
            .await?;
        super::bridge_protocol::decode_bridge_payload::<BridgeMemberVisibleToolsResponse>(
            &command,
            value,
            "read member visible tools",
        )
    };
    let reply = tokio::time::timeout(VISIBLE_TOOLS_BRIDGE_TIMEOUT, read)
        .await
        .unwrap_or_else(|_| {
            Err(MobError::Internal(
                "member visible tools read did not answer in time".to_string(),
            ))
        });
    visible_tools_from_reply(reply)
}

/// What a visible-tools read tells the fork notice: the names when the
/// member host read them; `source_not_live` when the source session is not
/// live there; `member_host_unsupported` when the host refused the read
/// before reading it (a host before protocol V7); `placed_source` for any
/// other failure, a timeout included.
pub(crate) fn visible_tools_from_reply(
    reply: Result<BridgeMemberVisibleToolsResponse, MobError>,
) -> meerkat_core::fork_notice::ForkSeedSourceTools {
    use meerkat_core::fork_notice::{ForkSeedSourceTools, ForkToolsNotComparedReason};
    match reply {
        Ok(BridgeMemberVisibleToolsResponse {
            names: Some(names), ..
        }) => ForkSeedSourceTools::Read {
            names: names
                .into_iter()
                .map(meerkat_core::types::ToolName::from)
                .collect(),
        },
        Ok(_) => ForkSeedSourceTools::NotRead {
            reason: ForkToolsNotComparedReason::SourceNotLive,
        },
        Err(MobError::BridgeCommandRejected {
            cause:
                BridgeRejectionCause::UnsupportedProtocolVersion | BridgeRejectionCause::Unsupported,
            ..
        }) => ForkSeedSourceTools::NotRead {
            reason: ForkToolsNotComparedReason::MemberHostUnsupported,
        },
        Err(_) => ForkSeedSourceTools::NotRead {
            reason: ForkToolsNotComparedReason::PlacedSource,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use meerkat_core::fork_notice::{ForkSeedSourceTools, ForkToolsNotComparedReason};

    fn rejected(cause: BridgeRejectionCause) -> Result<BridgeMemberVisibleToolsResponse, MobError> {
        Err(MobError::BridgeCommandRejected {
            cause,
            reason: "refused".to_string(),
        })
    }

    fn not_read(reason: ForkToolsNotComparedReason) -> ForkSeedSourceTools {
        ForkSeedSourceTools::NotRead { reason }
    }

    #[test]
    fn a_read_reply_compares_the_names() {
        let names = vec!["shell".to_string(), "view_image".to_string()];
        assert_eq!(
            visible_tools_from_reply(Ok(BridgeMemberVisibleToolsResponse { names: Some(names) })),
            ForkSeedSourceTools::Read {
                names: vec!["shell".into(), "view_image".into()],
            }
        );
    }

    #[test]
    fn a_source_not_live_on_its_host_is_not_compared_as_not_live() {
        assert_eq!(
            visible_tools_from_reply(Ok(BridgeMemberVisibleToolsResponse::default())),
            not_read(ForkToolsNotComparedReason::SourceNotLive)
        );
    }

    /// A member host before protocol V7 refuses the read before it
    /// reads it, typed.
    #[test]
    fn a_host_before_v7_is_not_compared_as_unsupported() {
        for cause in [
            BridgeRejectionCause::UnsupportedProtocolVersion,
            BridgeRejectionCause::Unsupported,
        ] {
            assert_eq!(
                visible_tools_from_reply(rejected(cause)),
                not_read(ForkToolsNotComparedReason::MemberHostUnsupported)
            );
        }
    }

    /// A held reply past the bound, or any other failure, is a placed
    /// source whose tools were not read.
    #[test]
    fn a_timeout_or_other_failure_is_not_compared_as_placed() {
        for reply in [
            Err(MobError::BridgeRequestTimedOut {
                request_envelope_id: "envelope".to_string(),
                timeout_ms: 5_000,
            }),
            rejected(BridgeRejectionCause::Unavailable),
            rejected(BridgeRejectionCause::StaleFence),
        ] {
            assert_eq!(
                visible_tools_from_reply(reply),
                not_read(ForkToolsNotComparedReason::PlacedSource)
            );
        }
    }
}
