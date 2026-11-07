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
//! `member_host_unsupported`. A member placed while its mob has
//! instructions is born with its snapshot on a member host that records one
//! at birth (the `instruction_target_birth` feature): the materialize carries
//! it, the member's first turn has it, and the delivery after it finds it
//! recorded. Toward a host without the feature the spawn is unchanged.
//!
//! Assertions are on public carriers: the mob's status read, the member
//! host's session service (the session's recorded target), the real host
//! daemon's replies to raw commands, and the commands a scripted host
//! received.

#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod governed_peer_support;
mod support;

use std::sync::Arc;
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
    RecordedInstructionRecord, ScriptedHostPeer, ScriptedV7Mode, bind_then_materialize,
    create_controlling_mob, member_incarnation_from_ack, placed_spawn_spec,
    raw_release_member_command, scripted_member_client_completing_recorded,
    spawn_host_daemon_fixture, spawn_peer_comms_endpoint, spawn_scripted_host_peer,
    supervisor_spec_for_endpoint, wait_until,
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

/// A placed member on a governed member host (its runtime has a native work
/// authorization host): an operator-instruction record needs prepared
/// authorization there, which nothing mints for one yet. The host refuses
/// the record typed (`prepared_authorization_required`) before anything is
/// written, and the mob blocks the member `member_host_unsupported` until
/// the next write instead of retrying; the session on the host holds none of
/// the write.
#[tokio::test(flavor = "multi_thread")]
async fn a_governed_member_host_refuses_a_placed_record() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let governed = governed_peer_support::GovernedMember::new();
    let mut options = HostFixtureOptions::named("oi-governed-host").with_member_build();
    options.member_llm_client = Some(governed.client());
    options.governed_member = Some(governed.clone());
    let fixture = spawn_host_daemon_fixture(options)
        .await
        .expect("spawn the governed member host");
    let controlling = create_controlling_mob("oi-governed-host").await;
    let report = controlling.bind_fixture(&fixture).await;
    controlling
        .spawn_placed("worker", "b2", &report.host_id)
        .await
        .expect("the placed spawn commits on the governed host");
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
        .expect("the ungoverned controller commits the write");
    let status = settle_member(
        &controlling.handle,
        &member,
        "the governed host's refusal blocks the member member_host_unsupported",
        blocked_member_host_unsupported,
    )
    .await;
    assert_eq!(
        status.delivery.recorded_generation, None,
        "the mob recorded nothing for the member"
    );
    let held = fixture
        .member_service
        .as_ref()
        .expect("the member-build fixture composes a member session service")
        .read_member_instructions(std::slice::from_ref(&session_id))
        .await
        .into_iter()
        .next()
        .expect("one read per session")
        .expect("the host session's operator-instruction state reads")
        .target;
    assert!(
        held.as_ref()
            .is_none_or(|target| target.target_generation() != written.generation),
        "the governed host recorded nothing of the write: {held:?}"
    );

    fixture.shutdown().await;
}

/// A snapshot whose record command would not fit the comms frame cannot
/// reach a placed member: the mob measures the record before it sends it,
/// sends nothing, and blocks the member `member_host_unsupported` until the
/// next write instead of retrying a send that can never succeed.
#[tokio::test(flavor = "multi_thread")]
async fn an_oversized_placed_record_is_not_sent_and_blocks_the_member() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let scripted = spawn_scripted_host_peer("oi-oversized-host").await;
    let controlling = create_controlling_mob("oi-oversized-host").await;
    let report = controlling.bind_scripted(&scripted).await;

    // Four bodies at the per-body limit fill the mob's active budget; their
    // record, with its envelope, is larger than the comms frame. They are
    // written before the member is placed, so the first snapshot it is owed
    // is the oversized one: no smaller snapshot's record is sent first.
    let body = "x".repeat(256 * 1024);
    for key in ["one", "two", "three", "four"] {
        controlling
            .handle
            .set_operator_instruction(write(key, 1, &body))
            .await
            .expect("the write commits within the active budget");
    }
    controlling
        .spawn_placed("worker", "b2", &report.host_id)
        .await
        .expect("the placed spawn commits");
    let member = AgentIdentity::from("b2");
    let status = settle_member(
        &controlling.handle,
        &member,
        "the oversized record blocks the member member_host_unsupported",
        blocked_member_host_unsupported,
    )
    .await;
    assert_eq!(
        record_commands(&scripted),
        0,
        "no record command was sent: {:?}",
        scripted.received_command_kinds()
    );
    assert_eq!(
        status.delivery.recorded_generation, None,
        "the mob recorded nothing for the member"
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

// ===========================================================================
// A placed member born with its snapshot (protocol 7)
// ===========================================================================

/// The `HostFeatures` feature of a member host that records a placed
/// member's snapshot in the session its materialize mints.
const BIRTH_FEATURE: &str = "instruction_target_birth";

/// How many `HostFeatures` probes a scripted host received.
fn feature_probes(scripted: &ScriptedHostPeer) -> usize {
    scripted
        .received_command_kinds()
        .iter()
        .filter(|kind| kind.as_str() == "host_features")
        .count()
}

/// Every materialize a scripted host received, as wire JSON, in order.
fn materialize_wires(scripted: &ScriptedHostPeer) -> Vec<serde_json::Value> {
    scripted
        .received_materialize_payloads()
        .iter()
        .map(|payload| serde_json::to_value(payload).expect("serialize a received materialize"))
        .collect()
}

/// A member placed on a member host of this release while its mob has
/// operator instructions is born with its snapshot: the host records the
/// snapshot on the session its materialize mints (the session's create
/// carries it), so the member's first turn, its spawn's kickoff, already has
/// the instruction. The mob's delivery of that generation then finds it
/// recorded: the one record the member's session receives answers
/// `already_recorded` and writes nothing.
#[tokio::test(flavor = "multi_thread")]
async fn a_placed_member_is_born_with_its_snapshot_and_its_first_turn_has_it() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let requests = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut options = HostFixtureOptions::named("oi-placed-birth-host").with_member_build();
    options.member_llm_client = Some(scripted_member_client_completing_recorded(
        "done",
        Arc::clone(&requests),
    ));
    options.record_member_creates = true;
    let fixture = spawn_host_daemon_fixture(options)
        .await
        .expect("spawn member-build host fixture");
    let controlling = create_controlling_mob("oi-placed-birth").await;
    let report = controlling.bind_fixture(&fixture).await;
    let written = controlling
        .handle
        .set_operator_instruction(write("tone", 1, FORMAL))
        .await
        .expect("the write commits");

    let mut spec = placed_spawn_spec("worker", "b2", &report.host_id);
    spec.runtime_mode = Some(meerkat_mob::MobRuntimeMode::AutonomousHost);
    spec.initial_message = Some(meerkat_core::types::ContentInput::from("birth-first-input"));
    controlling
        .handle
        .spawn_spec(spec)
        .await
        .expect("the placed spawn commits");
    let member = AgentIdentity::from("b2");

    let host_service = fixture
        .failing_create
        .as_ref()
        .expect("the fixture records member creates");
    let creates = host_service.recorded_creates();
    let create = creates
        .iter()
        .find(|create| {
            create
                .mob_member_binding
                .as_ref()
                .is_some_and(|binding| binding.member == "b2")
        })
        .unwrap_or_else(|| panic!("the host's create of b2: {creates:#?}"));
    let born = create.born_instruction_target.as_ref().unwrap_or_else(|| {
        panic!("the session the materialize mints is born with b2's snapshot: {create:#?}")
    });
    assert_eq!(
        born.owner(),
        &InstructionTargetOwner::Mob {
            mob_id: controlling.mob_id.to_string(),
            agent_identity: "b2".to_string(),
        }
    );
    assert_eq!(born.target_generation(), written.generation);

    wait_until("b2's first turn reaches its model", || async {
        !requests
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty()
    })
    .await;
    let first = {
        let requests = requests
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        serde_json::to_string(&requests[0]).expect("serialize the first request's messages")
    };
    assert!(
        first.contains(FORMAL),
        "b2's first turn has the instruction: {first}"
    );

    settle_member(
        &controlling.handle,
        &member,
        "b2's delivery reads current",
        |status| {
            status.delivery.state == MemberInstructionDeliveryState::Current
                && status.delivery.recorded_generation == Some(written.generation)
        },
    )
    .await;
    let session_id = fixture
        .host_binding_record(controlling.mob_id.as_ref())
        .await
        .materialized
        .get("b2")
        .map(|row| SessionId::parse(&row.session_id).expect("a session id"))
        .expect("b2 is materialized on its host");
    let records = host_service
        .recorded_instruction_records()
        .into_iter()
        .filter(|record| record.session_id == session_id)
        .collect::<Vec<_>>();
    assert_eq!(
        records,
        vec![RecordedInstructionRecord {
            session_id: session_id.clone(),
            generation: written.generation,
            disposition: Some(InstructionRecordDisposition::AlreadyRecorded),
        }],
        "the mob's delivery found the snapshot recorded and wrote nothing"
    );

    fixture.shutdown().await;
}

/// A member host that does not record a snapshot at birth, one before
/// protocol 7 or a protocol 7 host whose features do not list it, still
/// spawns a placed member as before: the controller asks the host's features
/// once for the spawn and sends the materialize without the snapshot, in its
/// V4 shape. The member receives its snapshot by delivery after the spawn,
/// as before (a Known limitation of mixed-version deployments).
#[tokio::test(flavor = "multi_thread")]
async fn an_older_member_host_spawns_a_placed_member_without_a_birth_snapshot() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let scripted = spawn_scripted_host_peer("oi-birth-older-host").await;
    let controlling = create_controlling_mob("oi-birth-older").await;
    let report = controlling.bind_scripted(&scripted).await;
    controlling
        .handle
        .set_operator_instruction(write("tone", 1, FORMAL))
        .await
        .expect("the write commits");

    controlling
        .spawn_placed("worker", "b2", &report.host_id)
        .await
        .expect("the placed spawn commits on a host before protocol 7");
    scripted.serve_v7(ScriptedV7Mode::Features(vec![
        HOST_FEATURE_PROMPT_CONTEXT_FORK.to_string(),
        HOST_FEATURE_ADOPTED_FIRST_TURN.to_string(),
    ]));
    controlling
        .spawn_placed("worker", "b3", &report.host_id)
        .await
        .expect("the placed spawn commits on a protocol 7 host without the feature");

    assert_eq!(
        feature_probes(&scripted),
        2,
        "each Fresh spawn of a mob with instructions asks the host once: {:?}",
        scripted.received_command_kinds()
    );
    let wires = materialize_wires(&scripted);
    assert_eq!(wires.len(), 2, "two materializes: {wires:#?}");
    for wire in &wires {
        assert!(
            wire.get("instruction_target").is_none(),
            "no snapshot toward a host that does not record one at birth: {wire}"
        );
        assert_eq!(wire["protocol_version"], 4, "the V4 shape: {wire}");
    }

    scripted.shutdown();
}

/// A placed spawn asks the member host's features only when its mob has
/// operator instructions: a mob without any asks nothing, even a host that
/// records a snapshot at birth, and sends the V4 materialize. Once the mob
/// has an instruction, the next placed spawn asks once and its V7
/// materialize carries the member's whole snapshot at the mob's generation.
#[tokio::test(flavor = "multi_thread")]
async fn a_placed_spawn_asks_its_host_once_and_only_when_its_mob_has_instructions() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let scripted = spawn_scripted_host_peer("oi-birth-probe-host").await;
    scripted.serve_v7(ScriptedV7Mode::Features(vec![BIRTH_FEATURE.to_string()]));
    let controlling = create_controlling_mob("oi-birth-probe").await;
    let report = controlling.bind_scripted(&scripted).await;

    controlling
        .spawn_placed("worker", "b2", &report.host_id)
        .await
        .expect("the placed spawn commits");
    assert_eq!(
        feature_probes(&scripted),
        0,
        "a mob without instructions asks nothing: {:?}",
        scripted.received_command_kinds()
    );
    let wires = materialize_wires(&scripted);
    let [plain] = wires.as_slice() else {
        panic!("one materialize: {wires:#?}");
    };
    assert!(plain.get("instruction_target").is_none(), "{plain}");
    assert_eq!(plain["protocol_version"], 4, "the V4 shape: {plain}");

    let written = controlling
        .handle
        .set_operator_instruction(write("tone", 1, FORMAL))
        .await
        .expect("the write commits");
    controlling
        .spawn_placed("worker", "b3", &report.host_id)
        .await
        .expect("the placed spawn commits");
    assert_eq!(
        feature_probes(&scripted),
        1,
        "the spawn of a mob with instructions asks once: {:?}",
        scripted.received_command_kinds()
    );
    let wires = materialize_wires(&scripted);
    let [_, carried] = wires.as_slice() else {
        panic!("two materializes: {wires:#?}");
    };
    assert_eq!(carried["protocol_version"], 7, "a V7 command: {carried}");
    assert_eq!(
        carried["instruction_target"],
        serde_json::json!({
            "generation": written.generation,
            "slots": [{
                "slot": { "namespace": "app.example", "key": "tone" },
                "revision": 1,
                "body": FORMAL,
            }],
        }),
        "the materialize carries b3's whole snapshot: {carried}"
    );

    scripted.shutdown();
}
