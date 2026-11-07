//! Operator instructions on placed members (supervisor bridge protocol 7).
//!
//! A placed member's session lives on its member host, so its mob records
//! the member's snapshot there over the supervisor bridge:
//! `RecordMemberInstructionTarget`, host-addressed and fenced by the
//! member's exact incarnation, which the host serves through its own session
//! service without waking the member. A retiring member's mob releases the
//! target with `ReleaseMemberInstructionTarget` once no member holds the
//! session. A member host that does not serve the commands (one before
//! protocol 7, or a protocol 7 host from before operator instructions)
//! refuses them typed, and the mob blocks the member
//! `member_host_unsupported`.
//!
//! Assertions are on public carriers: the mob's status read, the member
//! host's session service (the session's recorded target), the real host
//! daemon's replies to raw commands, and the commands a scripted host
//! received.

#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::time::Duration;

use meerkat_core::types::SessionId;
use meerkat_core::{
    InstructionEffect, InstructionKey, InstructionNamespace, InstructionRecordDisposition,
    InstructionRevision, InstructionSlot, InstructionTargetOwner,
};
use meerkat_mob::runtime::bridge_protocol::{
    BridgeCommand, BridgeInstructionTarget, BridgeInstructionTargetRecordedResponse,
    BridgeInstructionTargetReleasedResponse, BridgeInstructionTargetSlot, BridgeProtocolVersion,
    BridgeRecordInstructionTargetPayload, BridgeRejectionCause,
    BridgeReleaseInstructionTargetPayload, BridgeReply, HOST_FEATURE_ADOPTED_FIRST_TURN,
    HOST_FEATURE_PROMPT_CONTEXT_FORK,
};
use meerkat_mob::{
    AgentIdentity, MemberInstructionBlockReason, MemberInstructionDeliveryState,
    MemberInstructionEffectiveRead, MemberInstructionStatus, MobHandle,
    OperatorInstructionStatusQuery, OperatorInstructionTarget, OperatorInstructionWrite,
};
use support::{
    HostDaemonFixture, HostFixtureOptions, PeerCommsEndpoint, REAL_COMMS_TEST_LOCK,
    ScriptedHostPeer, ScriptedV7Mode, bind_then_materialize, create_controlling_mob,
    member_incarnation_from_ack, raw_release_member_command, spawn_host_daemon_fixture,
    spawn_peer_comms_endpoint, spawn_scripted_host_peer, supervisor_spec_for_endpoint,
};

const REPLY_TIMEOUT: Duration = Duration::from_secs(30);
const SETTLE: Duration = Duration::from_mins(1);
const FORMAL: &str = "Answer formally.";
const BRIEF: &str = "Keep every answer brief.";
const RECORD_COMMAND: &str = "record_member_instruction_target";

fn slot(key: &str) -> InstructionSlot {
    InstructionSlot {
        namespace: InstructionNamespace::new("app.example").expect("namespace"),
        key: InstructionKey::new(key).expect("key"),
    }
}

fn revision(value: u64) -> InstructionRevision {
    InstructionRevision::new(value).expect("revision")
}

/// A write of `key` at revision `at` for every member of the mob.
fn write(key: &str, at: u64, body: &str) -> OperatorInstructionWrite {
    OperatorInstructionWrite {
        slot: slot(key),
        revision: revision(at),
        body: body.to_string(),
        content_sha256: None,
        target: OperatorInstructionTarget::Mob,
    }
}

async fn member_build_fixture(name: &str) -> HostDaemonFixture {
    spawn_host_daemon_fixture(HostFixtureOptions::named(name).with_member_build())
        .await
        .expect("spawn member-build host fixture")
}

async fn member_status(handle: &MobHandle, identity: &AgentIdentity) -> MemberInstructionStatus {
    handle
        .operator_instruction_status(OperatorInstructionStatusQuery::default())
        .await
        .expect("the status read answers")
        .members
        .into_iter()
        .find(|member| &member.agent_identity == identity)
        .unwrap_or_else(|| panic!("the status page lists member {identity}"))
}

/// Wait until the member's status satisfies `done`, within [`SETTLE`]; the
/// panic names the last status read.
async fn settle_member(
    handle: &MobHandle,
    identity: &AgentIdentity,
    what: &str,
    done: impl Fn(&MemberInstructionStatus) -> bool,
) -> MemberInstructionStatus {
    let deadline = tokio::time::Instant::now() + SETTLE;
    loop {
        let status = member_status(handle, identity).await;
        if done(&status) {
            return status;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "{what} never happened; last status: {status:?}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn blocked_member_host_unsupported(status: &MemberInstructionStatus) -> bool {
    status.delivery.state
        == MemberInstructionDeliveryState::Blocked {
            reason: MemberInstructionBlockReason::MemberHostUnsupported,
        }
}

/// How many record commands a scripted host received.
fn record_commands(scripted: &ScriptedHostPeer) -> usize {
    scripted
        .received_command_kinds()
        .iter()
        .filter(|kind| kind.as_str() == RECORD_COMMAND)
        .count()
}

/// The target a member session on the host holds, read from the host's own
/// session service.
async fn host_target(
    fixture: &HostDaemonFixture,
    session_id: &SessionId,
) -> meerkat_core::SessionInstructionState {
    fixture
        .member_service
        .as_ref()
        .expect("the member-build fixture composes a member session service")
        .read_member_instructions(std::slice::from_ref(session_id))
        .await
        .into_iter()
        .next()
        .expect("one read per session")
        .expect("the host session's operator-instruction state reads")
        .target
        .expect("the host session holds a target")
}

fn record_command(
    probe: &PeerCommsEndpoint,
    member: meerkat_mob::runtime::bridge_protocol::BridgeMemberIncarnation,
    generation: u64,
    protocol_version: BridgeProtocolVersion,
) -> BridgeCommand {
    let supervisor = supervisor_spec_for_endpoint(probe, &member.mob_id);
    BridgeCommand::RecordMemberInstructionTarget(BridgeRecordInstructionTargetPayload {
        supervisor,
        epoch: 1,
        binding_generation: 1,
        protocol_version,
        member,
        target: BridgeInstructionTarget {
            generation,
            slots: vec![BridgeInstructionTargetSlot {
                slot: slot("tone"),
                revision: revision(1),
                body: Some(FORMAL.to_string()),
            }],
        },
    })
}

fn release_command(
    probe: &PeerCommsEndpoint,
    mob_id: &str,
    member_session_id: &str,
    protocol_version: BridgeProtocolVersion,
) -> BridgeCommand {
    BridgeCommand::ReleaseMemberInstructionTarget(BridgeReleaseInstructionTargetPayload {
        supervisor: supervisor_spec_for_endpoint(probe, mob_id),
        epoch: 1,
        binding_generation: 1,
        protocol_version,
        mob_id: mob_id.to_string(),
        agent_identity: "b2".to_string(),
        member_session_id: member_session_id.to_string(),
    })
}

fn assert_rejected(reply: &BridgeReply, expected: &BridgeRejectionCause) {
    match reply {
        BridgeReply::Rejected { cause, .. } if cause == expected => {}
        other => panic!("expected Rejected({expected:?}), got {other:?}"),
    }
}

// ===========================================================================
// The mob: a placed member on a host of this release, and on older hosts
// ===========================================================================

/// A placed member on a member host of this release: a write's snapshot is
/// recorded in the member's session on its host and the member's delivery
/// reads current; a revoke reaches the session the same way. The status
/// reads the member's delivery facts and says its session is on its member
/// host. The member then retires: its mob's release of the host target does
/// not hold the retirement up.
#[tokio::test(flavor = "multi_thread")]
async fn placed_member_records_target_over_the_bridge() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let fixture = member_build_fixture("oi-placed-record-host").await;
    let controlling = create_controlling_mob("oi-placed-record").await;
    let report = controlling.bind_fixture(&fixture).await;
    controlling
        .spawn_placed("worker", "b2", &report.host_id)
        .await
        .expect("the placed spawn commits");
    let member = AgentIdentity::from("b2");
    let session_id = fixture
        .host_binding_record(controlling.mob_id.as_ref())
        .await
        .materialized
        .get("b2")
        .map(|row| SessionId::parse(&row.session_id).expect("a session id"))
        .expect("b2 is materialized on its host");

    let written = controlling
        .handle
        .set_operator_instruction(write("tone", 1, FORMAL))
        .await
        .expect("the write commits");
    assert_eq!(written.matched_members, 1);
    let status = settle_member(
        &controlling.handle,
        &member,
        "the placed member's delivery reads current",
        |status| {
            status.delivery.state == MemberInstructionDeliveryState::Current
                && status.delivery.recorded_generation == Some(written.generation)
        },
    )
    .await;
    assert!(
        matches!(
            &status.effective_read,
            MemberInstructionEffectiveRead::Unreadable { reason } if reason.contains("member host")
        ),
        "the status says the session is on the member host: {:?}",
        status.effective_read
    );
    let target = host_target(&fixture, &session_id).await;
    assert_eq!(
        target.owner(),
        &InstructionTargetOwner::Mob {
            mob_id: controlling.mob_id.to_string(),
            agent_identity: "b2".to_string(),
        }
    );
    assert_eq!(target.target_generation(), written.generation);
    let tone = target.slot(&slot("tone")).expect("the slot is recorded");
    assert_eq!(
        (tone.revision, tone.effect),
        (revision(1), InstructionEffect::Active)
    );
    assert_eq!(tone.pending_body.as_deref(), Some(FORMAL));

    let revoked = controlling
        .handle
        .revoke_operator_instruction(slot("tone"), revision(2))
        .await
        .expect("the revoke commits");
    settle_member(
        &controlling.handle,
        &member,
        "the revoke reaches the placed member's session",
        |status| {
            status.delivery.state == MemberInstructionDeliveryState::Current
                && status.delivery.recorded_generation == Some(revoked.generation)
        },
    )
    .await;
    let target = host_target(&fixture, &session_id).await;
    assert_eq!(target.target_generation(), revoked.generation);
    assert_eq!(
        target.slot(&slot("tone")).map(|tone| tone.effect),
        Some(InstructionEffect::Absent)
    );

    tokio::time::timeout(SETTLE, controlling.handle.retire(member.clone()))
        .await
        .expect("the retirement settles")
        .expect("the placed member retires");
    assert!(
        !controlling
            .handle
            .list_members()
            .await
            .iter()
            .any(|entry| entry.agent_identity == member),
        "the retired member left the roster"
    );

    fixture.shutdown().await;
}

/// A placed member on a member host that does not serve the record command:
/// a host before protocol 7 refuses it before reading it
/// (`unsupported_protocol_version`), and a protocol 7 host from before
/// operator instructions refuses a command it does not know (`unsupported`).
/// Each time the command reaches the host and is refused before any effect:
/// the member is blocked `member_host_unsupported` until the next write,
/// nothing is recorded on either side (the mob holds no recorded generation
/// for the member, and the host refused the command), and nothing else
/// changes (the member stays seated, and is not released).
#[tokio::test(flavor = "multi_thread")]
async fn older_member_host_blocks_member_host_unsupported() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let scripted = spawn_scripted_host_peer("oi-older-host").await;
    let controlling = create_controlling_mob("oi-older-host").await;
    let report = controlling.bind_scripted(&scripted).await;
    controlling
        .spawn_placed("worker", "b2", &report.host_id)
        .await
        .expect("the placed spawn commits");
    let member = AgentIdentity::from("b2");

    // A host before protocol 7.
    controlling
        .handle
        .set_operator_instruction(write("tone", 1, FORMAL))
        .await
        .expect("the write commits");
    let status = settle_member(
        &controlling.handle,
        &member,
        "the member is blocked member_host_unsupported",
        blocked_member_host_unsupported,
    )
    .await;
    assert_eq!(
        record_commands(&scripted),
        1,
        "the record command reached the host before protocol 7: {:?}",
        scripted.received_command_kinds()
    );
    assert_eq!(
        scripted.rejection_causes(RECORD_COMMAND),
        vec![BridgeRejectionCause::UnsupportedProtocolVersion],
        "the host before protocol 7 refused it before reading it"
    );
    assert_eq!(
        status.delivery.recorded_generation, None,
        "the mob recorded nothing for the member"
    );

    // A protocol 7 host from before operator instructions.
    scripted.serve_v7(ScriptedV7Mode::Features(vec![
        HOST_FEATURE_PROMPT_CONTEXT_FORK.to_string(),
        HOST_FEATURE_ADOPTED_FIRST_TURN.to_string(),
    ]));
    controlling
        .handle
        .set_operator_instruction(write("tone", 2, BRIEF))
        .await
        .expect("the write commits");
    let status = settle_member(
        &controlling.handle,
        &member,
        "the member is blocked again after the new write",
        |status| blocked_member_host_unsupported(status) && record_commands(&scripted) == 2,
    )
    .await;
    assert_eq!(
        scripted.rejection_causes(RECORD_COMMAND),
        vec![
            BridgeRejectionCause::UnsupportedProtocolVersion,
            BridgeRejectionCause::Unsupported,
        ],
        "the protocol 7 host refused a command it does not know"
    );
    assert_eq!(
        status.delivery.recorded_generation, None,
        "the mob recorded nothing for the member"
    );

    let kinds = scripted.received_command_kinds();
    assert!(
        !kinds.iter().any(|kind| kind == "release_member"),
        "a blocked member is not released: {kinds:?}"
    );
    assert!(
        controlling
            .handle
            .list_members()
            .await
            .iter()
            .any(|entry| entry.agent_identity == member),
        "the blocked member stays seated"
    );

    scripted.shutdown();
}

// ===========================================================================
// The member host: the record and release commands
// ===========================================================================

/// The member host records a member's snapshot for the member's exact
/// incarnation: the snapshot lands in the member's session, and a repeat
/// answers `already_recorded`. A stale incarnation is refused `stale_fence`,
/// and a command stamped below protocol 7 is refused before it is read.
#[tokio::test(flavor = "multi_thread")]
async fn member_host_records_instruction_target_for_the_exact_incarnation() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let fixture = member_build_fixture("oi-host-record-host").await;
    let probe = spawn_peer_comms_endpoint("oi-host-record-supervisor", true, None).await;
    probe.trust(fixture.host_peer_descriptor()).await;
    let mob_id = format!("mob-oi-record-{}", uuid::Uuid::new_v4().simple());
    let ack = bind_then_materialize(&probe, &fixture, &mob_id, "b2").await;
    let member = member_incarnation_from_ack(&fixture, &mob_id, "b2", &ack, 1, 1, 1);
    let send = |command: BridgeCommand| {
        let probe = &probe;
        let host = fixture.host_peer_descriptor();
        async move {
            probe
                .send_bridge_command_raw(&host, &command, REPLY_TIMEOUT)
                .await
                .expect("the host replies")
        }
    };

    let reply = send(record_command(
        &probe,
        member.clone(),
        1,
        BridgeProtocolVersion::V7,
    ))
    .await;
    assert_eq!(
        reply,
        BridgeReply::MemberInstructionTargetRecorded(BridgeInstructionTargetRecordedResponse {
            disposition: InstructionRecordDisposition::Recorded,
            target_generation: 1,
        })
    );
    let session_id = SessionId::parse(&ack.session_id).expect("a session id");
    let target = host_target(&fixture, &session_id).await;
    assert_eq!(
        target.owner(),
        &InstructionTargetOwner::Mob {
            mob_id: mob_id.clone(),
            agent_identity: "b2".to_string(),
        }
    );
    assert_eq!(target.target_generation(), 1);
    assert_eq!(
        target.slot(&slot("tone")).map(|tone| tone.effect),
        Some(InstructionEffect::Active)
    );

    let reply = send(record_command(
        &probe,
        member.clone(),
        1,
        BridgeProtocolVersion::V7,
    ))
    .await;
    assert_eq!(
        reply,
        BridgeReply::MemberInstructionTargetRecorded(BridgeInstructionTargetRecordedResponse {
            disposition: InstructionRecordDisposition::AlreadyRecorded,
            target_generation: 1,
        })
    );

    let mut stale = member.clone();
    stale.generation += 1;
    let reply = send(record_command(&probe, stale, 2, BridgeProtocolVersion::V7)).await;
    assert_rejected(&reply, &BridgeRejectionCause::StaleFence);
    let mut moved = member.clone();
    moved.member_session_id = SessionId::new().to_string();
    let reply = send(record_command(&probe, moved, 2, BridgeProtocolVersion::V7)).await;
    assert_rejected(&reply, &BridgeRejectionCause::StaleFence);
    let reply = send(record_command(&probe, member, 2, BridgeProtocolVersion::V6)).await;
    assert_rejected(&reply, &BridgeRejectionCause::UnsupportedProtocolVersion);
    assert_eq!(
        host_target(&fixture, &session_id).await.target_generation(),
        1,
        "no refused record wrote the session"
    );

    fixture.shutdown().await;
}

/// The member host releases a mob's target of a member session only once no
/// member holds the session: a release while the member holds it is refused
/// `stale_fence` and leaves the target; once the member is released (the
/// host archived its session) the release finds nothing to release. A
/// command stamped below protocol 7 is refused before it is read.
#[tokio::test(flavor = "multi_thread")]
async fn member_host_releases_instruction_target_once_no_member_holds_the_session() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let fixture = member_build_fixture("oi-host-release-host").await;
    let probe = spawn_peer_comms_endpoint("oi-host-release-supervisor", true, None).await;
    probe.trust(fixture.host_peer_descriptor()).await;
    let mob_id = format!("mob-oi-release-{}", uuid::Uuid::new_v4().simple());
    let ack = bind_then_materialize(&probe, &fixture, &mob_id, "b2").await;
    let member = member_incarnation_from_ack(&fixture, &mob_id, "b2", &ack, 1, 1, 1);
    let send = |command: BridgeCommand| {
        let probe = &probe;
        let host = fixture.host_peer_descriptor();
        async move {
            probe
                .send_bridge_command_raw(&host, &command, REPLY_TIMEOUT)
                .await
                .expect("the host replies")
        }
    };
    let recorded = send(record_command(&probe, member, 1, BridgeProtocolVersion::V7)).await;
    assert!(
        matches!(recorded, BridgeReply::MemberInstructionTargetRecorded(_)),
        "{recorded:?}"
    );

    let reply = send(release_command(
        &probe,
        &mob_id,
        &ack.session_id,
        BridgeProtocolVersion::V7,
    ))
    .await;
    assert_rejected(&reply, &BridgeRejectionCause::StaleFence);
    let session_id = SessionId::parse(&ack.session_id).expect("a session id");
    assert_eq!(
        host_target(&fixture, &session_id).await.target_generation(),
        1,
        "a refused release leaves the member's target"
    );
    let reply = send(release_command(
        &probe,
        &mob_id,
        &ack.session_id,
        BridgeProtocolVersion::V6,
    ))
    .await;
    assert_rejected(&reply, &BridgeRejectionCause::UnsupportedProtocolVersion);

    let released = send(raw_release_member_command(&probe, &mob_id, 1, "b2", 1, 1)).await;
    assert!(
        matches!(released, BridgeReply::MemberReleased(_)),
        "{released:?}"
    );
    let reply = send(release_command(
        &probe,
        &mob_id,
        &ack.session_id,
        BridgeProtocolVersion::V7,
    ))
    .await;
    assert_eq!(
        reply,
        BridgeReply::MemberInstructionTargetReleased(BridgeInstructionTargetReleasedResponse {
            disposition: None,
        })
    );

    fixture.shutdown().await;
}
