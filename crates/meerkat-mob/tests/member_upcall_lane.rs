//! Multi-host mobs phase 3 — the member→controlling UPCALL lane over real
//! loopback comms (design-upcalls §1 rows T-15..T-18, T-21; merged with
//! design-fold-ins T-L10 dedup/admission legs and T-F4 presence-fact pins).
//!
//! Harness: scripted host + real controlling mob; the roster is seeded by a
//! REAL placed spawn whose scripted materialize ack carries a TEST-HELD
//! member identity (`script_member_identity`) — the probe endpoint then IS
//! the member: it signs upcalls with the roster-recorded key and receives
//! replies at the machine-recorded endpoint (ADJ-16 staging).
//!
//! Member-SIDE forwarding-dispatcher internals (op mapping, timeout ladder,
//! outcome envelope — U T-1..T-5) are in-crate mod-test rows (B2 lane); the
//! genuine tools.mob mount/absence pins ride `remote_spawn_ladder.rs`.

#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::time::Duration;

use meerkat_mob::runtime::bridge_protocol::{
    BridgeRejectionCause, BridgeReply, MemberOperatorOp, MemberOperatorOutcome,
    MemberOperatorSpawnSpec,
};
use meerkat_mob::runtime::{SpawnCustomizationContext, SpawnMemberCustomizer, SpawnMemberSpec};
use support::{
    MemberOperatorRequesterResidency, REAL_COMMS_TEST_LOCK, create_controlling_mob_with_customizer,
    create_controlling_mob_with_customizer_and_definition, create_controlling_mob_with_definition,
    member_identity_of, placed_spawn_spec, raw_member_operator_command_at,
    spawn_peer_comms_endpoint, spawn_scripted_host_peer, spawn_scripted_member_turn_responder,
};

const REPLY_TIMEOUT: Duration = Duration::from_secs(30);

struct UpcallHarness {
    scripted: support::ScriptedHostPeer,
    controlling: support::ControllingMob,
    host_id: String,
    /// The probe endpoint that IS member b2 (roster-recorded key + endpoint).
    member: std::sync::Arc<support::PeerCommsEndpoint>,
    supervisor: meerkat_core::comms::TrustedPeerDescriptor,
    requester_generation: u64,
    requester_fence_token: u64,
    requester_host_binding_generation: u64,
    requester_member_session_id: String,
}

impl UpcallHarness {
    fn command_as(
        &self,
        agent_identity: &str,
        request_id: &str,
        op: MemberOperatorOp,
    ) -> meerkat_mob::runtime::bridge_protocol::BridgeCommand {
        raw_member_operator_command_at(
            MemberOperatorRequesterResidency {
                agent_identity,
                generation: self.requester_generation,
                fence_token: self.requester_fence_token,
                host_id: &self.host_id,
                host_binding_generation: self.requester_host_binding_generation,
                member_session_id: &self.requester_member_session_id,
            },
            request_id,
            op,
        )
    }

    fn member_command(
        &self,
        request_id: &str,
        op: MemberOperatorOp,
    ) -> meerkat_mob::runtime::bridge_protocol::BridgeCommand {
        self.command_as("b2", request_id, op)
    }
}

async fn upcall_harness(label: &str) -> UpcallHarness {
    upcall_harness_composed(label, None, |spec| spec).await
}

/// [`upcall_harness`] with an optional spawn customizer on the controlling
/// mob and `shape` applied to b2's placed spawn request.
async fn upcall_harness_composed(
    label: &str,
    customizer: Option<std::sync::Arc<dyn SpawnMemberCustomizer>>,
    shape: impl FnOnce(SpawnMemberSpec) -> SpawnMemberSpec,
) -> UpcallHarness {
    let controlling = create_controlling_mob_with_customizer(label, customizer).await;
    upcall_harness_on(label, controlling, shape).await
}

/// [`upcall_harness_composed`] over a controlling mob the row built itself.
async fn upcall_harness_on(
    label: &str,
    controlling: support::ControllingMob,
    shape: impl FnOnce(SpawnMemberSpec) -> SpawnMemberSpec,
) -> UpcallHarness {
    let scripted = spawn_scripted_host_peer(&format!("{label}-scripted")).await;
    let report = controlling.bind_scripted(&scripted).await;

    // The probe endpoint is the member identity the scripted ack advertises.
    let member = std::sync::Arc::new(
        spawn_peer_comms_endpoint(&format!("{label}-member-b2"), true, None).await,
    );
    scripted.script_member_identity("b2", member_identity_of(&member));
    controlling
        .handle
        .spawn_spec(shape(placed_spawn_spec("worker", "b2", &report.host_id)))
        .await
        .expect("placed spawn seeds the roster with the probe-held identity");

    let supervisor = controlling
        .handle
        .routable_supervisor_peer()
        .await
        .expect("routable supervisor bridge peer");
    member.trust(supervisor.clone()).await;
    let binding = scripted
        .received_materialize_payloads()
        .into_iter()
        .rev()
        .find(|payload| payload.spec.agent_identity == "b2")
        .expect("b2 materialization carries its requester binding tuple");
    let requester_member_session_id = controlling
        .handle
        .member_status(&meerkat_mob::AgentIdentity::from("b2"))
        .await
        .expect("b2 status")
        .current_session_id
        .expect("placed b2 has a member session")
        .to_string();

    UpcallHarness {
        scripted,
        controlling,
        host_id: report.host_id,
        member,
        supervisor,
        requester_generation: binding.generation,
        requester_fence_token: binding.fence_token,
        requester_host_binding_generation: binding.binding_generation,
        requester_member_session_id,
    }
}

fn spawn_op(member_id: &str) -> MemberOperatorOp {
    MemberOperatorOp::SpawnMember(Box::new(MemberOperatorSpawnSpec {
        profile: "worker".to_string(),
        member_id: member_id.to_string(),
        initial_message: None,
        runtime_mode: None,
        launch_mode: None,
        auto_wire_parent: None,
        placement: None,
        requested_tool_access_policy_present: false,
        resolved_tool_access_policy: None,
    }))
}

fn expect_operator_reply(
    reply: &BridgeReply,
) -> &meerkat_mob::runtime::bridge_protocol::MemberOperatorReply {
    match reply {
        BridgeReply::MemberOperatorReply(operator) => operator,
        other => panic!("expected MemberOperatorReply, got {other:?}"),
    }
}

// ===========================================================================
// T-15 — machine-seeded ListMembers round trip: Admitted, served idle-inbox,
// Completed with the member list
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn admitted_upcall_list_members_completes_over_the_wire() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let harness = upcall_harness("upcall-t15").await;

    let reply = harness
        .member
        .send_bridge_command_raw(
            &harness.supervisor,
            &harness.member_command("req-list-1", MemberOperatorOp::ListMembers),
            REPLY_TIMEOUT,
        )
        .await
        .expect("upcall served while no bridge request is in flight (idle-inbox serving)");
    let operator = expect_operator_reply(&reply);
    assert_eq!(operator.request_id, "req-list-1");
    let MemberOperatorOutcome::Completed { result } = &operator.outcome else {
        panic!("expected Completed, got {:?}", operator.outcome);
    };
    assert!(
        result.as_str().contains("b2"),
        "the member list names the requester, got {}",
        result.as_str()
    );

    harness.scripted.shutdown();
}

// ===========================================================================
// T-16 — admission failure matrix over the wire (ADJ-14 mapping), each
// rejected BEFORE any dispatcher execution
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn upcall_admission_rejects_map_typed_before_any_execution() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let harness = upcall_harness("upcall-t16").await;

    // UnknownIdentity: roster-trusted sender claiming a non-roster identity.
    let reply = harness
        .member
        .send_bridge_command_raw(
            &harness.supervisor,
            &harness.command_as("ghost", "req-ghost-1", spawn_op("never-1")),
            REPLY_TIMEOUT,
        )
        .await
        .expect("typed rejection reply");
    let operator = expect_operator_reply(&reply);
    match &operator.outcome {
        MemberOperatorOutcome::Rejected { cause, reason } => {
            assert_eq!(
                *cause,
                BridgeRejectionCause::SenderMismatch,
                "ADJ-14 mapping"
            );
            assert!(
                reason.contains("UnknownIdentity"),
                "the reason carries the machine kind, got {reason}"
            );
        }
        other => panic!("expected Rejected, got {other:?}"),
    }

    // SenderKeyMismatch: this sender claims ANOTHER committed member whose
    // recorded key differs. Seed b3 with a different scripted identity.
    let other_member =
        std::sync::Arc::new(spawn_peer_comms_endpoint("upcall-t16-member-b3", true, None).await);
    harness
        .scripted
        .script_member_identity("b3", member_identity_of(&other_member));
    harness
        .scripted
        .bind_member_endpoint("b3", std::sync::Arc::clone(&other_member));
    harness
        .controlling
        .spawn_placed("worker", "b3", &harness.host_id)
        .await
        .expect("b3 commits");
    let reply = harness
        .member
        .send_bridge_command_raw(
            &harness.supervisor,
            &harness.command_as("b3", "req-b3-1", spawn_op("never-2")),
            REPLY_TIMEOUT,
        )
        .await
        .expect("typed rejection reply");
    let operator = expect_operator_reply(&reply);
    match &operator.outcome {
        MemberOperatorOutcome::Rejected { cause, reason } => {
            assert_eq!(*cause, BridgeRejectionCause::SenderMismatch);
            assert!(
                reason.contains("SenderKeyMismatch"),
                "the reason names the machine kind, got {reason}"
            );
        }
        other => panic!("expected Rejected, got {other:?}"),
    }

    // Nothing executed: no spawn side effects reached the roster.
    let members = harness.controlling.handle.list_members().await;
    assert!(
        !members.iter().any(|entry| {
            entry.agent_identity == "never-1" || entry.agent_identity == "never-2"
        }),
        "rejected upcalls must never dispatch (execution counter zero)"
    );

    // HostRevoked → NotBound: after RevokeHost even a duplicate of a
    // previously-recorded request re-admits FIRST (dedup after admission).
    let requester_responder =
        spawn_scripted_member_turn_responder(std::sync::Arc::clone(&harness.member));
    let other_responder = spawn_scripted_member_turn_responder(other_member);
    harness
        .controlling
        .handle
        .revoke_host(&harness.host_id)
        .await
        .expect("revoke host");
    // The requester becomes the reply-draining caller again below.
    requester_responder.shutdown_and_join().await;
    other_responder.shutdown_and_join().await;
    let reply = harness
        .member
        .send_bridge_command_raw(
            &harness.supervisor,
            &harness.member_command("req-list-2", MemberOperatorOp::ListMembers),
            REPLY_TIMEOUT,
        )
        .await
        .expect("typed rejection reply");
    let operator = expect_operator_reply(&reply);
    match &operator.outcome {
        MemberOperatorOutcome::Rejected { cause, reason } => {
            assert_eq!(
                *cause,
                BridgeRejectionCause::NotBound,
                "HostRevoked → NotBound"
            );
            assert!(reason.contains("HostRevoked"), "got {reason}");
        }
        other => panic!("expected Rejected, got {other:?}"),
    }

    harness.scripted.shutdown();
}

// ===========================================================================
// Stale-incarnation held envelope: Resume reuses the same session + member
// key, but the signed generation/fence tuple binds the request to generation 1
// and MobMachine rejects it after generation 2 commits.
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn held_envelope_is_fenced_after_same_session_resume_generation_bump() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let harness = upcall_harness("upcall-stale-incarnation").await;
    let identity = meerkat_mob::AgentIdentity::from("b2");

    let initial_payload = harness
        .scripted
        .received_materialize_payloads()
        .into_iter()
        .find(|payload| payload.spec.agent_identity == "b2")
        .expect("initial b2 materialization payload");
    let old_session = harness
        .controlling
        .handle
        .member_status(&identity)
        .await
        .expect("initial b2 status")
        .current_session_id
        .expect("placed member has a machine session binding");
    let old_session_id = old_session.to_string();
    let held = raw_member_operator_command_at(
        MemberOperatorRequesterResidency {
            agent_identity: "b2",
            generation: initial_payload.generation,
            fence_token: initial_payload.fence_token,
            host_id: &harness.host_id,
            host_binding_generation: initial_payload.binding_generation,
            member_session_id: &old_session_id,
        },
        "req-held-generation-1",
        spawn_op("never-from-stale-envelope"),
    );

    let lifecycle_responder =
        spawn_scripted_member_turn_responder(std::sync::Arc::clone(&harness.member));
    harness
        .controlling
        .handle
        .retire(identity.clone())
        .await
        .expect("retire first incarnation");
    // Resume reuses this endpoint as the member-originated caller.
    lifecycle_responder.shutdown_and_join().await;
    let mut replacement = support::placed_spawn_spec("worker", "b2", &harness.host_id);
    replacement.launch_mode = meerkat_mob::launch::MemberLaunchMode::Resume {
        bridge_session_id: old_session.clone(),
        resume_from_role: None,
    };
    harness
        .controlling
        .handle
        .spawn_spec(replacement)
        .await
        .expect("resume replacement at a higher binding tuple");

    let replacement_payload = harness
        .scripted
        .received_materialize_payloads()
        .into_iter()
        .rev()
        .find(|payload| payload.spec.agent_identity == "b2")
        .expect("replacement b2 materialization payload");
    assert!(
        (
            replacement_payload.generation,
            replacement_payload.fence_token
        ) > (initial_payload.generation, initial_payload.fence_token),
        "replacement tuple must supersede the held envelope"
    );
    assert!(
        matches!(
            &replacement_payload.launch,
            meerkat_mob::runtime::bridge_protocol::MaterializeLaunchMode::Resume { session_id, .. }
                if session_id == &old_session.to_string()
        ),
        "the generation bump resumes the same session"
    );
    assert_eq!(
        harness
            .controlling
            .handle
            .member_status(&identity)
            .await
            .expect("replacement b2 status")
            .current_session_id,
        Some(old_session),
        "the replacement committed the same resumed session"
    );

    // The exact same endpoint/key signs the delayed generation-1 request.
    // Peer-key-only admission would execute it under generation 2; exact
    // signed-tuple admission must reject before dispatcher execution.
    let stale_reply = harness
        .member
        .send_bridge_command_raw(&harness.supervisor, &held, REPLY_TIMEOUT)
        .await
        .expect("stale envelope receives a typed rejection");
    let stale_operator = expect_operator_reply(&stale_reply);
    match &stale_operator.outcome {
        MemberOperatorOutcome::Rejected { cause, reason } => {
            assert_eq!(*cause, BridgeRejectionCause::StaleFence);
            assert!(
                reason.contains("StaleGeneration") || reason.contains("StaleFence"),
                "reason names the exact machine rejection, got {reason}"
            );
        }
        other => panic!("held stale envelope must reject, got {other:?}"),
    }
    assert!(
        !harness
            .controlling
            .handle
            .list_members()
            .await
            .iter()
            .any(|member| member.agent_identity == "never-from-stale-envelope"),
        "stale envelope may not reach the operator effect"
    );

    // Same endpoint/key remains valid for the replacement's current tuple,
    // proving the rejection was incarnation fencing rather than signer loss.
    let current_reply = harness
        .member
        .send_bridge_command_raw(
            &harness.supervisor,
            &raw_member_operator_command_at(
                MemberOperatorRequesterResidency {
                    agent_identity: "b2",
                    generation: replacement_payload.generation,
                    fence_token: replacement_payload.fence_token,
                    host_id: &harness.host_id,
                    host_binding_generation: replacement_payload.binding_generation,
                    member_session_id: &old_session_id,
                },
                "req-current-generation-2",
                MemberOperatorOp::ListMembers,
            ),
            REPLY_TIMEOUT,
        )
        .await
        .expect("current tuple is admitted for the same member key");
    assert!(matches!(
        expect_operator_reply(&current_reply).outcome,
        MemberOperatorOutcome::Completed { .. }
    ));

    harness.scripted.shutdown();
}

// ===========================================================================
// T-17 / T-L10 — duplicate-delivery replay: identical reply bytes, ONE
// execution (exactly one spawned member)
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn duplicate_request_id_replays_verbatim_with_one_execution() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let harness = upcall_harness("upcall-t17").await;

    let command = harness.member_command("req-spawn-1", spawn_op("b21"));
    let first = harness
        .member
        .send_bridge_command_raw(&harness.supervisor, &command, REPLY_TIMEOUT)
        .await
        .expect("first upcall spawn");
    let first_operator = expect_operator_reply(&first).clone();
    assert!(
        matches!(
            first_operator.outcome,
            MemberOperatorOutcome::Completed { .. }
        ),
        "spawn upcall completes, got {:?}",
        first_operator.outcome
    );

    let second = harness
        .member
        .send_bridge_command_raw(&harness.supervisor, &command, REPLY_TIMEOUT)
        .await
        .expect("duplicate upcall");
    let second_operator = expect_operator_reply(&second).clone();
    assert_eq!(
        serde_json::to_string(&first_operator).expect("first serializes"),
        serde_json::to_string(&second_operator).expect("second serializes"),
        "the recorded reply replays byte-identical"
    );

    // Exactly ONE execution: one b21 in the roster.
    assert_eq!(
        harness
            .controlling
            .handle
            .list_members()
            .await
            .iter()
            .filter(|entry| entry.agent_identity == "b21")
            .count(),
        1,
        "dedup must not double-spawn"
    );

    harness.scripted.shutdown();
}

// ===========================================================================
// T-18 — interleaving: an upcall whose EXECUTION itself sends on the bridge
// (spawn → materialize on the member's own host) does not deadlock the
// responder reply path (PeerResponse bypasses the request lock)
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn upcall_spawn_holding_the_bridge_lock_does_not_deadlock() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let harness = upcall_harness("upcall-t18").await;

    // Default placement = b2's host ⇒ execution performs a MaterializeMember
    // round trip on the single-flight bridge while the upcall reply is
    // pending — the reply must still arrive.
    let reply = tokio::time::timeout(
        REPLY_TIMEOUT,
        harness.member.send_bridge_command_raw(
            &harness.supervisor,
            &harness.member_command("req-spawn-nested", spawn_op("b22")),
            REPLY_TIMEOUT,
        ),
    )
    .await
    .expect("no deadlock between the responder and the in-flight bridge request")
    .expect("upcall reply");
    let operator = expect_operator_reply(&reply);
    assert!(
        matches!(operator.outcome, MemberOperatorOutcome::Completed { .. }),
        "nested-bridge upcall completes, got {:?}",
        operator.outcome
    );
    assert!(
        harness
            .controlling
            .handle
            .list_members()
            .await
            .iter()
            .any(|entry| entry.agent_identity == "b22"),
        "the upcall-spawned member committed"
    );

    harness.scripted.shutdown();
}

// ===========================================================================
// T-F4 — presence-fact split (no laundering): resolved policy present with
// requested_tool_access_policy_present == false is NOT the privileged arm
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn presence_fact_is_never_derived_from_the_resolved_policy() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let harness = upcall_harness("upcall-tf4").await;

    // Case (i): the laundering counterexample — the caller named NO policy,
    // the member host resolved Inherit to its parent's DenyList. The
    // controlling receiver must feed presence=false into the admission
    // (Inherit-resolution is not a privileged request) and still apply the
    // resolved policy to the child.
    let op = MemberOperatorOp::SpawnMember(Box::new(MemberOperatorSpawnSpec {
        profile: "worker".to_string(),
        member_id: "b21".to_string(),
        initial_message: None,
        runtime_mode: None,
        launch_mode: None,
        auto_wire_parent: None,
        placement: None,
        requested_tool_access_policy_present: false,
        resolved_tool_access_policy: Some(
            meerkat_mob::runtime::bridge_protocol::WireResolvedToolAccessPolicy::DenyList(vec![
                "shell_execute".to_string(),
            ]),
        ),
    }));
    let reply = harness
        .member
        .send_bridge_command_raw(
            &harness.supervisor,
            &harness.member_command("req-tf4-1", op),
            REPLY_TIMEOUT,
        )
        .await
        .expect("upcall reply");
    let operator = expect_operator_reply(&reply);
    assert!(
        matches!(operator.outcome, MemberOperatorOutcome::Completed { .. }),
        "presence=false + resolved=Some must NOT trip the privileged-args arm \
         (deriving presence from the resolved policy is exactly the laundering \
         this pins), got {:?}",
        operator.outcome
    );
    assert!(
        harness
            .controlling
            .handle
            .list_members()
            .await
            .iter()
            .any(|entry| entry.agent_identity == "b21"),
        "the contained child committed"
    );

    harness.scripted.shutdown();
}

// ===========================================================================
// T-21 — controlling restart / reply loss: the durable terminal survives and
// the same request_id replays byte-identically without re-executing
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn restart_and_reply_loss_replay_the_durable_terminal_without_reexecution() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let harness = upcall_harness("upcall-t21").await;

    // This member is materialized by the scripted host during the upcall.
    // Give it a real endpoint so graceful restart can prove the absence of a
    // live channel instead of dialing the scripted synthetic port 1.
    let spawned_member =
        std::sync::Arc::new(spawn_peer_comms_endpoint("upcall-t21-member-b21", true, None).await);
    harness
        .scripted
        .script_member_identity("b21", member_identity_of(&spawned_member));
    harness
        .scripted
        .bind_member_endpoint("b21", std::sync::Arc::clone(&spawned_member));

    let command = harness.member_command("req-restart-1", spawn_op("b21"));
    let first = harness
        .member
        .send_bridge_command_raw(&harness.supervisor, &command, REPLY_TIMEOUT)
        .await
        .expect("pre-restart upcall");
    assert!(
        matches!(
            expect_operator_reply(&first).outcome,
            MemberOperatorOutcome::Completed { .. }
        ),
        "pre-restart spawn completes, got {:?}",
        expect_operator_reply(&first).outcome
    );
    let first_operator = expect_operator_reply(&first).clone();

    // Treat the first reply as lost by the caller, then restart the
    // controlling runtime over the same metadata store.
    // Both placed endpoints must serve the ordinary structural
    // LiveTransportUnavailable reply while graceful shutdown reconciles live
    // channels. The requester cannot run this responder while it is itself
    // waiting for the upcall reply, so install it only after that reply lands.
    let requester_responder =
        spawn_scripted_member_turn_responder(std::sync::Arc::clone(&harness.member));
    let spawned_responder = spawn_scripted_member_turn_responder(spawned_member);
    let controlling = harness.controlling.restart().await;
    // The probe becomes the reply-draining caller again below. Join the
    // temporary responder first so the two consumers cannot race.
    requester_responder.shutdown_and_join().await;
    let supervisor = controlling
        .handle
        .routable_supervisor_peer()
        .await
        .expect("post-restart supervisor peer");
    harness.member.trust(supervisor.clone()).await;

    // Re-sending the SAME request id must replay the immutable terminal; it
    // must not attempt SpawnMember a second time.
    let second = harness
        .member
        .send_bridge_command_raw(&supervisor, &command, REPLY_TIMEOUT)
        .await
        .expect("post-restart upcall");
    let second_operator = expect_operator_reply(&second).clone();
    assert_eq!(
        serde_json::to_string(&first_operator).expect("first serializes"),
        serde_json::to_string(&second_operator).expect("second serializes"),
        "the durable terminal must replay byte-identically across restart"
    );
    assert_eq!(
        controlling
            .handle
            .list_members()
            .await
            .iter()
            .filter(|member| member.agent_identity == "b21")
            .count(),
        1,
        "restart replay must not duplicate the spawn effect"
    );

    spawned_responder.shutdown();
    harness.scripted.shutdown();
}

// ===========================================================================
// Phase 5 lane-separation pins (§15.2:506 / §15.9:606, ADJ-P5-1): the member
// upcall lane (agent authority) and the principal ControlScope lane are
// DISJOINT. Grants never gate upcalls; agent authority never satisfies scope
// checks — both directions, pinned.
// ===========================================================================

// T-LS1 — upcalls execute with ZERO principal grant rows for the member's
// PeerId: grants neither gate nor precondition the upcall lane.
#[tokio::test(flavor = "multi_thread")]
async fn upcall_executes_with_zero_principal_grants() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let harness = upcall_harness("upcall-ls1").await;

    // Explicit precondition: no grant record exists for anyone.
    let grants = harness
        .controlling
        .handle
        .grants(meerkat_mob::MobControlPrincipal::Owner)
        .await
        .expect("owner reads grants");
    assert!(grants.is_empty(), "precondition: zero principal grant rows");

    let reply = harness
        .member
        .send_bridge_command_raw(
            &harness.supervisor,
            &harness.member_command("req-ls1-1", MemberOperatorOp::ListMembers),
            REPLY_TIMEOUT,
        )
        .await
        .expect("upcall served with zero grants");
    let operator = expect_operator_reply(&reply);
    assert!(
        matches!(operator.outcome, MemberOperatorOutcome::Completed { .. }),
        "grants must not gate upcalls (agent lane), got {:?}",
        operator.outcome
    );

    harness.scripted.shutdown();
}

// T-LS2 — a Named principal whose id string equals the member's PeerId (which
// holds a live MobMemberOperatorAuthorityRecord) presents the EMPTY set at a
// console verb: agent-authority records are invisible to the principal lane.
#[tokio::test(flavor = "multi_thread")]
async fn agent_authority_never_satisfies_principal_checks() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let harness = upcall_harness("upcall-ls2").await;

    // Establish the member's live operator-authority record via an admitted
    // upcall.
    let reply = harness
        .member
        .send_bridge_command_raw(
            &harness.supervisor,
            &harness.member_command("req-ls2-1", MemberOperatorOp::ListMembers),
            REPLY_TIMEOUT,
        )
        .await
        .expect("upcall establishes the authority record");
    assert!(
        matches!(
            expect_operator_reply(&reply).outcome,
            MemberOperatorOutcome::Completed { .. }
        ),
        "arrange upcall completes"
    );

    // Console verb as a principal named by the member's PeerId string.
    let member_peer_id = support::member_identity_of(&harness.member).member_peer_id;
    let as_member_principal = harness.controlling.handle.clone().with_command_authority(
        meerkat_mob::CommandAuthority::principal(meerkat_mob::MobControlPrincipal::External(
            meerkat_core::auth::PrincipalId::new(member_peer_id)
                .expect("peer id parses as a principal id"),
        )),
    );
    match as_member_principal
        .retire(meerkat_mob::AgentIdentity::from("b2"))
        .await
    {
        Err(meerkat_mob::MobError::ScopeDenied(denial)) => {
            assert!(
                denial.presented.is_empty(),
                "the agent-authority record confers zero principal scopes, got {:?}",
                denial.presented
            );
        }
        other => panic!("expected typed ScopeDenied with empty presented, got {other:?}"),
    }

    harness.scripted.shutdown();
}

// T-LS3 — principal grants for the member's PeerId do not perturb upcall
// admission: the T-16 reject rows and one admitted row are byte-identical
// before the grant, after the grant, and after revoke.
#[tokio::test(flavor = "multi_thread")]
async fn principal_grants_do_not_perturb_upcall_admission() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let harness = upcall_harness("upcall-ls3").await;
    let member_peer_id = support::member_identity_of(&harness.member).member_peer_id;

    // One admitted row + one reject row (UnknownIdentity → SenderMismatch),
    // re-driven at each grant state. Distinct request ids per pass keep the
    // dedup map out of the comparison.
    async fn drive(harness: &UpcallHarness, pass: &str) -> (String, String) {
        let admitted = harness
            .member
            .send_bridge_command_raw(
                &harness.supervisor,
                &harness.member_command(
                    &format!("req-ls3-admit-{pass}"),
                    MemberOperatorOp::ListMembers,
                ),
                REPLY_TIMEOUT,
            )
            .await
            .expect("admitted upcall");
        let admitted_outcome = match &expect_operator_reply(&admitted).outcome {
            MemberOperatorOutcome::Completed { .. } => "completed".to_string(),
            other => format!("{other:?}"),
        };
        let rejected = harness
            .member
            .send_bridge_command_raw(
                &harness.supervisor,
                &harness.command_as(
                    "ghost",
                    &format!("req-ls3-ghost-{pass}"),
                    MemberOperatorOp::ListMembers,
                ),
                REPLY_TIMEOUT,
            )
            .await
            .expect("rejected upcall reply");
        let rejected_outcome = match &expect_operator_reply(&rejected).outcome {
            MemberOperatorOutcome::Rejected { cause, .. } => format!("{cause:?}"),
            other => format!("unexpected {other:?}"),
        };
        (admitted_outcome, rejected_outcome)
    }

    let before = drive(&harness, "before").await;

    // Grant the member's PeerId (as a principal) real scopes.
    harness
        .controlling
        .grant(
            &member_peer_id,
            &[
                meerkat_mob::machines::mob_machine::ControlScope::SendCommand,
                meerkat_mob::machines::mob_machine::ControlScope::Cancel,
            ],
            None,
        )
        .await;
    let with_grant = drive(&harness, "granted").await;

    harness
        .controlling
        .handle
        .revoke_scopes(
            meerkat_mob::MobControlPrincipal::Owner,
            meerkat_core::auth::PrincipalId::new(member_peer_id.clone()).expect("peer id parses"),
            None,
        )
        .await
        .expect("owner revokes the probe grant");
    let after_revoke = drive(&harness, "revoked").await;

    assert_eq!(
        before, with_grant,
        "granting must not perturb upcall admission"
    );
    assert_eq!(
        before, after_revoke,
        "revoking must not perturb upcall admission"
    );

    harness.scripted.shutdown();
}

// T-LS4 lives in-crate (crates/meerkat-mob/src/runtime/tools.rs unit lane): the
// upcall executor's dispatch handle binding (`CommandAuthorityKind::AgentLane`)
// is not observable from this integration harness — the executor handle never
// leaves the crate. The behavioral halves (grants don't gate upcalls; agent
// authority satisfies no scope check) are T-LS1..T-LS3 above.

// ===========================================================================
// Operator authority ceiling. The controlling host bounds the authority it
// records for a placed member. With no ceiling the member keeps manage scope:
// every profile, the privileged spawn arguments, list and retire. A ceiling
// without manage scope admits only its own profiles, never a privileged
// spawn argument, and refuses list and retire. The narrowing is recorded with
// the member, so it holds across a controlling restart and a respawn.
// ===========================================================================

/// Every use of the ceiling API in these rows goes through this module, so
/// the rows themselves compile against any revision of the crate.
mod ceiling_api {
    use meerkat_mob::runtime::SpawnMemberSpec;
    use meerkat_mob::{MemberOperatorAuthorityCeiling, MobError, MobRespawnError};

    pub type Ceiling = MemberOperatorAuthorityCeiling;

    /// A ceiling that lowers nothing.
    pub fn unrestricted() -> Ceiling {
        MemberOperatorAuthorityCeiling::unrestricted()
    }

    /// The ceiling the narrowed rows give b2: spawn only `quiet-worker`, with
    /// no manage scope.
    pub fn narrowed(spec: SpawnMemberSpec) -> SpawnMemberSpec {
        spec.with_operator_authority_ceiling(
            unrestricted()
                .without_manage()
                .with_spawn_profiles(["quiet-worker"]),
        )
    }

    /// A narrowed ceiling that keeps `worker`, a profile with mob tools, so
    /// the member can spawn a child that holds operator authority itself.
    pub fn narrowed_keeping_mob_tools(spec: SpawnMemberSpec) -> SpawnMemberSpec {
        spec.with_operator_authority_ceiling(
            unrestricted()
                .without_manage()
                .with_spawn_profiles(["worker"]),
        )
    }

    /// A ceiling that narrows the spawn profiles and keeps manage scope.
    pub fn profiles_with_manage(spec: SpawnMemberSpec) -> SpawnMemberSpec {
        spec.with_operator_authority_ceiling(unrestricted().with_spawn_profiles(["quiet-worker"]))
    }

    /// An explicit ceiling that keeps manage scope but drops create
    /// authority.
    pub fn manage_without_create(spec: SpawnMemberSpec) -> SpawnMemberSpec {
        spec.with_operator_authority_ceiling(unrestricted().without_create())
    }

    /// An explicit ceiling that keeps manage scope but drops profile-mutation
    /// authority.
    pub fn manage_without_mutate(spec: SpawnMemberSpec) -> SpawnMemberSpec {
        spec.with_operator_authority_ceiling(unrestricted().without_mutate())
    }

    /// `ceiling` without create authority.
    pub fn without_create(ceiling: Ceiling) -> Ceiling {
        ceiling.without_create()
    }

    pub fn ceiling_of(spec: &SpawnMemberSpec) -> Option<Ceiling> {
        spec.operator_authority_ceiling().cloned()
    }

    pub fn set_ceiling(spec: &mut SpawnMemberSpec, ceiling: Ceiling) {
        spec.set_operator_authority_ceiling(ceiling);
    }

    /// The typed refusal reason of a spawn refused for its ceiling.
    pub fn ceiling_refusal<T>(result: &Result<T, MobError>) -> Option<String> {
        match result {
            Err(MobError::OperatorAuthorityCeilingRefused { reason, .. }) => {
                Some(format!("{reason:?}"))
            }
            _ => None,
        }
    }

    pub fn respawn_refused_by_ceiling<T>(result: &Result<T, MobRespawnError>) -> bool {
        matches!(
            result,
            Err(MobRespawnError::Mob(
                MobError::OperatorAuthorityCeilingRefused { .. }
            ))
        )
    }
}

use ceiling_api::{
    manage_without_create, manage_without_mutate, narrowed, narrowed_keeping_mob_tools,
    profiles_with_manage,
};

/// Every profile of the fixture mob, in the recorded (sorted) order.
const ALL_PROFILES: [&str; 5] = [
    "builtins-worker",
    "lead",
    "quiet-worker",
    "worker",
    "workgrapher",
];

/// The operator authority facts the controlling host recorded for `member`
/// in its committed placed-spawn carrier, with the per-compile principal
/// token masked.
async fn recorded_facts(controlling: &support::ControllingMob, member: &str) -> serde_json::Value {
    let carrier = controlling
        .storage_metadata
        .load_placed_spawn(&controlling.mob_id, member)
        .await
        .expect("placed-spawn carrier read")
        .expect("placed-spawn carrier present");
    assert!(
        matches!(
            carrier.phase,
            meerkat_mob::store::PlacedSpawnCarrierPhase::Committed(_)
        ),
        "{member}'s carrier is committed"
    );
    let mut facts = serde_json::to_value(
        carrier
            .spec
            .overlay
            .mob_tool_authority_context
            .as_ref()
            .expect("a tools.mob member records operator facts"),
    )
    .expect("operator facts encode");
    facts["principal_token"] = serde_json::Value::from("<minted>");
    facts
}

/// The facts a placed member spawned without a ceiling records (the shape
/// recorded before ceilings existed).
fn default_facts(mob_id: &str) -> serde_json::Value {
    serde_json::json!({
        "principal_token": "<minted>",
        "can_create_mobs": true,
        "can_mutate_profiles": true,
        "can_run_adaptive_packs": false,
        "managed_mob_scope": [mob_id],
        "spawn_profile_scope": { mob_id: ALL_PROFILES },
    })
}

/// The facts b2 records under [`narrowed`].
fn narrowed_facts(mob_id: &str) -> serde_json::Value {
    serde_json::json!({
        "principal_token": "<minted>",
        "can_create_mobs": true,
        "can_mutate_profiles": true,
        "can_run_adaptive_packs": false,
        "spawn_profile_scope": { mob_id: ["quiet-worker"] },
    })
}

fn spawn_member_op(
    profile: &str,
    member_id: &str,
    shape: impl FnOnce(&mut MemberOperatorSpawnSpec),
) -> MemberOperatorOp {
    let mut spec = MemberOperatorSpawnSpec {
        profile: profile.to_string(),
        member_id: member_id.to_string(),
        initial_message: None,
        runtime_mode: None,
        launch_mode: None,
        auto_wire_parent: None,
        placement: None,
        requested_tool_access_policy_present: false,
        resolved_tool_access_policy: None,
    };
    shape(&mut spec);
    MemberOperatorOp::SpawnMember(Box::new(spec))
}

async fn upcall(harness: &UpcallHarness, request_id: &str, op: MemberOperatorOp) -> BridgeReply {
    harness
        .member
        .send_bridge_command_raw(
            &harness.supervisor,
            &harness.member_command(request_id, op),
            REPLY_TIMEOUT,
        )
        .await
        .expect("upcall reply")
}

/// Send `op` as b2's current incarnation: the tuple of its latest
/// materialization and its current member session.
async fn upcall_as_current_b2(
    harness: &UpcallHarness,
    request_id: &str,
    op: MemberOperatorOp,
) -> BridgeReply {
    upcall_as(harness, &harness.member, "b2", request_id, op).await
}

/// Send `op` from `endpoint` as `member`'s current incarnation.
async fn upcall_as(
    harness: &UpcallHarness,
    endpoint: &support::PeerCommsEndpoint,
    member: &str,
    request_id: &str,
    op: MemberOperatorOp,
) -> BridgeReply {
    let payload = harness
        .scripted
        .received_materialize_payloads()
        .into_iter()
        .rev()
        .find(|payload| payload.spec.agent_identity == member)
        .unwrap_or_else(|| panic!("{member} materialization payload"));
    let session = harness
        .controlling
        .handle
        .member_status(&meerkat_mob::AgentIdentity::from(member))
        .await
        .unwrap_or_else(|error| panic!("{member} status: {error}"))
        .current_session_id
        .unwrap_or_else(|| panic!("placed {member} has a member session"))
        .to_string();
    let command = raw_member_operator_command_at(
        MemberOperatorRequesterResidency {
            agent_identity: member,
            generation: payload.generation,
            fence_token: payload.fence_token,
            host_id: &harness.host_id,
            host_binding_generation: payload.binding_generation,
            member_session_id: &session,
        },
        request_id,
        op,
    );
    endpoint
        .send_bridge_command_raw(&harness.supervisor, &command, REPLY_TIMEOUT)
        .await
        .expect("upcall reply")
}

/// A probe endpoint the scripted host acknowledges as `member`, trusted by
/// the controlling host's supervisor, so a row can send upcalls as that
/// member and answer its lifecycle traffic.
async fn probe_member(
    harness: &UpcallHarness,
    label: &str,
    member: &str,
) -> std::sync::Arc<support::PeerCommsEndpoint> {
    let endpoint = std::sync::Arc::new(
        spawn_peer_comms_endpoint(&format!("{label}-member-{member}"), true, None).await,
    );
    harness
        .scripted
        .script_member_identity(member, member_identity_of(&endpoint));
    harness
        .scripted
        .bind_member_endpoint(member, std::sync::Arc::clone(&endpoint));
    endpoint.trust(harness.supervisor.clone()).await;
    endpoint
}

/// The tool outcome a completed upcall reply carries.
fn tool_outcome(reply: &BridgeReply) -> serde_json::Value {
    let operator = expect_operator_reply(reply);
    let MemberOperatorOutcome::Completed { result } = &operator.outcome else {
        panic!("expected a completed upcall, got {:?}", operator.outcome);
    };
    serde_json::from_str(result.as_str()).expect("the tool outcome is JSON")
}

fn assert_admitted(reply: &BridgeReply, what: &str) {
    let outcome = tool_outcome(reply);
    assert!(
        outcome["kind"] == "ok" && outcome["is_error"] == false,
        "{what} must be admitted, got {outcome}"
    );
}

fn assert_access_denied(reply: &BridgeReply, what: &str) {
    let outcome = tool_outcome(reply);
    assert!(
        outcome["kind"] == "tool_error" && outcome["class"] == "access_denied",
        "{what} must be refused with access_denied, got {outcome}"
    );
}

async fn has_member(controlling: &support::ControllingMob, member: &str) -> bool {
    controlling
        .handle
        .list_members()
        .await
        .iter()
        .any(|entry| entry.agent_identity == member)
}

#[tokio::test(flavor = "multi_thread")]
async fn narrowed_placed_member_spawns_only_its_ceiling_profiles() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let harness = upcall_harness_composed("ceiling-profiles", None, narrowed).await;

    let reply = upcall(
        &harness,
        "req-ceiling-profiles-in",
        spawn_member_op("quiet-worker", "c1", |_| {}),
    )
    .await;
    assert_admitted(&reply, "a spawn of a profile inside the ceiling");
    assert!(has_member(&harness.controlling, "c1").await);

    let reply = upcall(
        &harness,
        "req-ceiling-profiles-out",
        spawn_member_op("worker", "c2", |_| {}),
    )
    .await;
    assert_access_denied(&reply, "a spawn of a profile outside the ceiling");
    assert!(
        !has_member(&harness.controlling, "c2").await,
        "a refused spawn reaches no roster"
    );
    assert_eq!(
        recorded_facts(&harness.controlling, "b2").await,
        narrowed_facts(harness.controlling.mob_id.as_str()),
        "the controlling host records the narrowed facts"
    );

    harness.scripted.shutdown();
}

/// A placed member spawned without a ceiling keeps today's authority: every
/// profile, list and retire, and the same recorded facts as before ceilings
/// existed.
#[tokio::test(flavor = "multi_thread")]
async fn default_placed_member_keeps_every_profile_list_and_retire() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let harness = upcall_harness("ceiling-default").await;
    assert_eq!(
        recorded_facts(&harness.controlling, "b2").await,
        default_facts(harness.controlling.mob_id.as_str()),
        "a placed member without a ceiling records the default facts"
    );

    // c1 is retired below: give it a real endpoint that answers its
    // retirement, instead of the scripted host's synthetic address.
    let retired_member = std::sync::Arc::new(
        spawn_peer_comms_endpoint("ceiling-default-member-c1", true, None).await,
    );
    harness
        .scripted
        .script_member_identity("c1", member_identity_of(&retired_member));
    harness
        .scripted
        .bind_member_endpoint("c1", std::sync::Arc::clone(&retired_member));
    for (index, profile) in ["worker", "quiet-worker"].into_iter().enumerate() {
        let reply = upcall(
            &harness,
            &format!("req-ceiling-default-spawn-{index}"),
            spawn_member_op(profile, &format!("c{index}"), |_| {}),
        )
        .await;
        assert_admitted(&reply, &format!("a default member's spawn of {profile}"));
    }
    let reply = upcall(
        &harness,
        "req-ceiling-default-list",
        MemberOperatorOp::ListMembers,
    )
    .await;
    assert_admitted(&reply, "a default member's list_members");
    let retired_responder = spawn_scripted_member_turn_responder(retired_member);
    let reply = upcall(
        &harness,
        "req-ceiling-default-retire",
        MemberOperatorOp::RetireMember {
            member_id: "c1".to_string(),
        },
    )
    .await;
    retired_responder.shutdown_and_join().await;
    assert_admitted(&reply, "a default member's retire_member");
    assert!(
        !has_member(&harness.controlling, "c1").await,
        "the retired member left the roster"
    );

    harness.scripted.shutdown();
}

/// Narrowing the spawn profiles while keeping manage scope could not hold
/// (manage scope admits every profile), so the spawn is refused, typed, and
/// so is a ceiling on a spawn without a placement.
#[tokio::test(flavor = "multi_thread")]
async fn ceiling_that_cannot_hold_is_refused_typed() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let harness = upcall_harness("ceiling-refused").await;

    let result = harness
        .controlling
        .handle
        .spawn_spec(profiles_with_manage(placed_spawn_spec(
            "worker",
            "c9",
            &harness.host_id,
        )))
        .await;
    assert_eq!(
        ceiling_api::ceiling_refusal(&result).as_deref(),
        Some("SpawnProfilesWithManage"),
        "spawn profiles with manage scope must be refused typed, got {result:?}"
    );
    assert!(!has_member(&harness.controlling, "c9").await);

    let result = harness
        .controlling
        .handle
        .spawn_spec(narrowed(SpawnMemberSpec::new("quiet-worker", "c8")))
        .await;
    assert_eq!(
        ceiling_api::ceiling_refusal(&result).as_deref(),
        Some("UnplacedMember"),
        "a ceiling on a spawn without a placement must be refused typed, got {result:?}"
    );
    assert!(!has_member(&harness.controlling, "c8").await);

    harness.scripted.shutdown();
}

/// The three privileged spawn arguments manage scope admits.
#[derive(Clone, Copy, Debug)]
enum PrivilegedSpawnArg {
    RuntimeMode,
    LaunchMode,
    RequestedToolAccessPolicy,
}

impl PrivilegedSpawnArg {
    fn set(self, spec: &mut MemberOperatorSpawnSpec) {
        match self {
            Self::RuntimeMode => {
                spec.runtime_mode = Some(meerkat_contracts::wire::WireMobRuntimeMode::TurnDriven);
            }
            Self::LaunchMode => {
                spec.launch_mode = Some(meerkat_contracts::wire::WireMemberLaunchMode::Fresh);
            }
            Self::RequestedToolAccessPolicy => {
                spec.requested_tool_access_policy_present = true;
                spec.resolved_tool_access_policy = Some(
                    meerkat_mob::runtime::bridge_protocol::WireResolvedToolAccessPolicy::DenyList(
                        vec!["shell_execute".to_string()],
                    ),
                );
            }
        }
    }
}

/// A spawn of a profile the member holds, carrying `arg`: refused under the
/// narrowed ceiling and admitted under the default. The same spawn without
/// the argument is admitted under both, so the argument alone decides.
async fn privileged_spawn_arg_row(label: &str, narrow: bool, arg: PrivilegedSpawnArg) {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let harness = if narrow {
        upcall_harness_composed(label, None, narrowed).await
    } else {
        upcall_harness(label).await
    };

    let reply = upcall(
        &harness,
        "req-privileged-control",
        spawn_member_op("quiet-worker", "c1", |_| {}),
    )
    .await;
    assert_admitted(&reply, "the same spawn without the privileged argument");

    let reply = upcall(
        &harness,
        "req-privileged-arg",
        spawn_member_op("quiet-worker", "c2", |spec| arg.set(spec)),
    )
    .await;
    if narrow {
        assert_access_denied(&reply, &format!("{arg:?} under the narrowed ceiling"));
        assert!(!has_member(&harness.controlling, "c2").await);
    } else {
        assert_admitted(&reply, &format!("{arg:?} under the default authority"));
        assert!(has_member(&harness.controlling, "c2").await);
    }

    harness.scripted.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn narrowed_placed_member_is_refused_a_runtime_mode() {
    privileged_spawn_arg_row("ceiling-arg-rm-n", true, PrivilegedSpawnArg::RuntimeMode).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn narrowed_placed_member_is_refused_a_launch_mode() {
    privileged_spawn_arg_row("ceiling-arg-lm-n", true, PrivilegedSpawnArg::LaunchMode).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn narrowed_placed_member_is_refused_a_requested_tool_access_policy() {
    privileged_spawn_arg_row(
        "ceiling-arg-tap-n",
        true,
        PrivilegedSpawnArg::RequestedToolAccessPolicy,
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn default_placed_member_is_admitted_a_runtime_mode() {
    privileged_spawn_arg_row("ceiling-arg-rm-d", false, PrivilegedSpawnArg::RuntimeMode).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn default_placed_member_is_admitted_a_launch_mode() {
    privileged_spawn_arg_row("ceiling-arg-lm-d", false, PrivilegedSpawnArg::LaunchMode).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn default_placed_member_is_admitted_a_requested_tool_access_policy() {
    privileged_spawn_arg_row(
        "ceiling-arg-tap-d",
        false,
        PrivilegedSpawnArg::RequestedToolAccessPolicy,
    )
    .await;
}

/// Without manage scope, list_members and retire_member of a member the
/// caller does not own are refused, typed.
#[tokio::test(flavor = "multi_thread")]
async fn narrowed_placed_member_is_refused_list_and_retire() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let harness = upcall_harness_composed("ceiling-manage", None, narrowed).await;
    harness
        .controlling
        .handle
        .spawn_spec(placed_spawn_spec("quiet-worker", "c0", &harness.host_id))
        .await
        .expect("the controlling host places c0");

    let reply = upcall(&harness, "req-ceiling-list", MemberOperatorOp::ListMembers).await;
    assert_access_denied(&reply, "list_members without manage scope");
    let reply = upcall(
        &harness,
        "req-ceiling-retire",
        MemberOperatorOp::RetireMember {
            member_id: "c0".to_string(),
        },
    )
    .await;
    assert_access_denied(
        &reply,
        "retire_member of a member it does not own, without manage scope",
    );
    // A flow run needs manage scope too, so a narrowed member cannot
    // provision members through a flow either.
    let reply = upcall(
        &harness,
        "req-ceiling-run-flow",
        MemberOperatorOp::MobRunFlow {
            flow_id: "any-flow".to_string(),
            params: None,
        },
    )
    .await;
    assert_access_denied(&reply, "mob_run_flow without manage scope");
    assert!(
        has_member(&harness.controlling, "c0").await,
        "the refused retire left c0 in place"
    );

    harness.scripted.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn narrowed_placed_member_ceiling_survives_a_controlling_restart() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let harness = upcall_harness_composed("ceiling-restart", None, narrowed).await;
    let refused = harness.member_command(
        "req-ceiling-restart-out",
        spawn_member_op("worker", "c2", |_| {}),
    );
    let admitted = harness.member_command(
        "req-ceiling-restart-in",
        spawn_member_op("quiet-worker", "c1", |_| {}),
    );

    // Graceful shutdown reconciles b2's live channel; the probe answers it
    // until the restarted controller is up, then becomes the caller again.
    let requester_responder =
        spawn_scripted_member_turn_responder(std::sync::Arc::clone(&harness.member));
    let controlling = harness.controlling.restart().await;
    requester_responder.shutdown_and_join().await;
    let supervisor = controlling
        .handle
        .routable_supervisor_peer()
        .await
        .expect("post-restart supervisor peer");
    harness.member.trust(supervisor.clone()).await;

    let reply = harness
        .member
        .send_bridge_command_raw(&supervisor, &refused, REPLY_TIMEOUT)
        .await
        .expect("post-restart upcall");
    assert_access_denied(
        &reply,
        "a spawn of a profile outside the ceiling after a controlling restart",
    );
    let reply = harness
        .member
        .send_bridge_command_raw(&supervisor, &admitted, REPLY_TIMEOUT)
        .await
        .expect("post-restart upcall");
    assert_admitted(
        &reply,
        "a spawn of a profile inside the ceiling after a controlling restart",
    );
    assert_eq!(
        recorded_facts(&controlling, "b2").await,
        narrowed_facts(controlling.mob_id.as_str()),
        "the restarted controller admits against the same recorded facts"
    );

    harness.scripted.shutdown();
}

/// Respawn b2, answering its retirement on the probe endpoint.
async fn respawn_b2(
    harness: &UpcallHarness,
    successor: Option<SpawnMemberSpec>,
) -> Result<meerkat_mob::MemberRespawnReceipt, meerkat_mob::MobRespawnError> {
    let responder = spawn_scripted_member_turn_responder(std::sync::Arc::clone(&harness.member));
    let result = match successor {
        Some(successor) => {
            harness
                .controlling
                .handle
                .respawn_with_successor_spec(successor)
                .await
        }
        None => {
            harness
                .controlling
                .handle
                .respawn(meerkat_mob::AgentIdentity::from("b2"), None)
                .await
        }
    };
    responder.shutdown_and_join().await;
    result
}

/// After a respawn, the replacement incarnation admits exactly what its
/// predecessor's recorded ceiling admits.
async fn assert_replacement_is_narrowed(harness: &UpcallHarness) {
    assert_eq!(
        recorded_facts(&harness.controlling, "b2").await,
        narrowed_facts(harness.controlling.mob_id.as_str()),
        "the replacement records its predecessor's narrowed facts"
    );
    let reply = upcall_as_current_b2(
        harness,
        "req-respawned-out",
        spawn_member_op("worker", "c2", |_| {}),
    )
    .await;
    assert_access_denied(
        &reply,
        "the replacement's spawn of a profile outside the ceiling",
    );
    let reply = upcall_as_current_b2(
        harness,
        "req-respawned-in",
        spawn_member_op("quiet-worker", "c1", |_| {}),
    )
    .await;
    assert_admitted(
        &reply,
        "the replacement's spawn of a profile inside the ceiling",
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn respawn_keeps_the_recorded_ceiling() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let harness = upcall_harness_composed("ceiling-respawn", None, narrowed).await;
    respawn_b2(&harness, None).await.expect("b2 respawns");
    assert_replacement_is_narrowed(&harness).await;
    harness.scripted.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn respawn_with_a_successor_spec_keeps_the_recorded_ceiling() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let harness = upcall_harness_composed("ceiling-successor", None, narrowed).await;
    respawn_b2(
        &harness,
        Some(placed_spawn_spec("worker", "b2", &harness.host_id)),
    )
    .await
    .expect("b2 respawns from a successor spec without a ceiling");
    assert_replacement_is_narrowed(&harness).await;
    harness.scripted.shutdown();
}

/// A respawn request that sets its own ceiling replaces the recorded one,
/// wider as well as narrower; one that cannot hold refuses the respawn before
/// anything is retired.
#[tokio::test(flavor = "multi_thread")]
async fn respawn_request_ceiling_replaces_the_recorded_one() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let harness = upcall_harness_composed("ceiling-replace", None, narrowed).await;
    let before = harness
        .controlling
        .storage_metadata
        .load_placed_spawn(&harness.controlling.mob_id, "b2")
        .await
        .expect("carrier read")
        .expect("carrier present");

    let result = respawn_b2(
        &harness,
        Some(profiles_with_manage(placed_spawn_spec(
            "worker",
            "b2",
            &harness.host_id,
        ))),
    )
    .await;
    assert!(
        ceiling_api::respawn_refused_by_ceiling(&result),
        "a successor ceiling that cannot hold refuses the respawn, got {result:?}"
    );
    let unchanged = harness
        .controlling
        .storage_metadata
        .load_placed_spawn(&harness.controlling.mob_id, "b2")
        .await
        .expect("carrier read")
        .expect("carrier present");
    assert_eq!(
        unchanged, before,
        "the refused respawn retired nothing: b2's incarnation and record are unchanged"
    );

    respawn_b2(
        &harness,
        Some(manage_without_create(placed_spawn_spec(
            "worker",
            "b2",
            &harness.host_id,
        ))),
    )
    .await
    .expect("b2 respawns with a ceiling of its own");
    let mut expected = default_facts(harness.controlling.mob_id.as_str());
    expected["can_create_mobs"] = serde_json::Value::from(false);
    assert_eq!(
        recorded_facts(&harness.controlling, "b2").await,
        expected,
        "the request's ceiling replaced the recorded one"
    );
    let reply =
        upcall_as_current_b2(&harness, "req-replaced-list", MemberOperatorOp::ListMembers).await;
    assert_admitted(
        &reply,
        "list_members under the replacing ceiling's manage scope",
    );

    harness.scripted.shutdown();
}

/// What a spawn customizer saw on each spawn source, and an optional ceiling
/// it sets on a respawn.
#[derive(Default)]
struct CeilingCustomizer {
    seen: std::sync::Mutex<
        Vec<(
            meerkat_mob::runtime::SpawnSource,
            Option<ceiling_api::Ceiling>,
        )>,
    >,
    on_respawn: Option<ceiling_api::Ceiling>,
}

impl SpawnMemberCustomizer for CeilingCustomizer {
    fn customize_spawn(
        &self,
        ctx: &SpawnCustomizationContext,
        spec: &mut SpawnMemberSpec,
    ) -> Result<(), meerkat_mob::MobError> {
        self.seen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push((ctx.spawn_source, ceiling_api::ceiling_of(spec)));
        if ctx.spawn_source == meerkat_mob::runtime::SpawnSource::Respawn
            && let Some(ceiling) = &self.on_respawn
        {
            ceiling_api::set_ceiling(spec, ceiling.clone());
        }
        Ok(())
    }
}

/// A respawn's customizer sees the recorded ceiling; leaving it keeps it.
#[tokio::test(flavor = "multi_thread")]
async fn respawn_customizer_sees_the_recorded_ceiling() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let customizer = std::sync::Arc::new(CeilingCustomizer::default());
    let harness = upcall_harness_composed(
        "ceiling-customizer",
        Some(customizer.clone() as std::sync::Arc<dyn SpawnMemberCustomizer>),
        narrowed,
    )
    .await;
    respawn_b2(&harness, None).await.expect("b2 respawns");

    let respawn_views = customizer
        .seen
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
        .filter(|(source, _)| *source == meerkat_mob::runtime::SpawnSource::Respawn)
        .map(|(_, ceiling)| ceiling.clone())
        .collect::<Vec<_>>();
    assert_eq!(
        respawn_views,
        vec![ceiling_api::ceiling_of(&narrowed(SpawnMemberSpec::new(
            "worker", "b2"
        )))],
        "the respawn's customizer sees the predecessor's recorded ceiling"
    );
    assert_replacement_is_narrowed(&harness).await;

    harness.scripted.shutdown();
}

/// A customizer that sets a ceiling on the respawn replaces the recorded one.
#[tokio::test(flavor = "multi_thread")]
async fn respawn_customizer_ceiling_replaces_the_recorded_one() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let customizer = std::sync::Arc::new(CeilingCustomizer {
        on_respawn: Some(ceiling_api::unrestricted()),
        ..CeilingCustomizer::default()
    });
    let harness = upcall_harness_composed(
        "ceiling-customizer-wide",
        Some(customizer.clone() as std::sync::Arc<dyn SpawnMemberCustomizer>),
        narrowed,
    )
    .await;
    assert_eq!(
        recorded_facts(&harness.controlling, "b2").await,
        narrowed_facts(harness.controlling.mob_id.as_str()),
        "b2 starts narrowed"
    );
    respawn_b2(&harness, None).await.expect("b2 respawns");
    assert_eq!(
        recorded_facts(&harness.controlling, "b2").await,
        default_facts(harness.controlling.mob_id.as_str()),
        "the customizer's explicit ceiling replaced the recorded one"
    );

    harness.scripted.shutdown();
}

/// A respawn of a placed member spawned without a ceiling records the
/// default facts, as before ceilings existed.
#[tokio::test(flavor = "multi_thread")]
async fn respawn_of_a_default_placed_member_keeps_the_default_authority() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let harness = upcall_harness("ceiling-respawn-default").await;
    respawn_b2(&harness, None).await.expect("b2 respawns");
    assert_eq!(
        recorded_facts(&harness.controlling, "b2").await,
        default_facts(harness.controlling.mob_id.as_str()),
        "a default member's replacement records the default facts"
    );
    let reply = upcall_as_current_b2(
        &harness,
        "req-respawned-default-list",
        MemberOperatorOp::ListMembers,
    )
    .await;
    assert_admitted(&reply, "the default replacement's list_members");

    harness.scripted.shutdown();
}

// ===========================================================================
// A member's own spawns inherit its recorded ceiling as an upper bound: a
// narrowed member whose ceiling keeps a profile with mob tools cannot hand a
// child more authority than it holds, and a customizer can narrow the bound
// but never widen it.
// ===========================================================================

/// The facts a child records under [`narrowed_keeping_mob_tools`]'s bound.
fn bounded_child_facts(mob_id: &str) -> serde_json::Value {
    serde_json::json!({
        "principal_token": "<minted>",
        "can_create_mobs": true,
        "can_mutate_profiles": true,
        "can_run_adaptive_packs": false,
        "spawn_profile_scope": { mob_id: ["worker"] },
    })
}

/// The child c1 of a member narrowed to `worker`, spawned through b2's
/// upcall and answering as its own probe endpoint, is held to b2's bound.
async fn assert_child_is_bounded(
    harness: &UpcallHarness,
    child: &support::PeerCommsEndpoint,
    label: &str,
) {
    let reply = upcall_as(
        harness,
        child,
        "c1",
        &format!("req-{label}-child-list"),
        MemberOperatorOp::ListMembers,
    )
    .await;
    assert_access_denied(
        &reply,
        "the child's list_members (it holds no manage scope)",
    );
    let reply = upcall_as(
        harness,
        child,
        "c1",
        &format!("req-{label}-child-out"),
        spawn_member_op("quiet-worker", "c2", |_| {}),
    )
    .await;
    assert_access_denied(&reply, "the child's spawn of a profile outside the bound");
    let reply = upcall_as(
        harness,
        child,
        "c1",
        &format!("req-{label}-child-arg"),
        spawn_member_op("worker", "c3", |spec| {
            PrivilegedSpawnArg::RuntimeMode.set(spec);
        }),
    )
    .await;
    assert_access_denied(&reply, "the child's spawn with a privileged argument");
    assert_eq!(
        recorded_facts(&harness.controlling, "c1").await,
        bounded_child_facts(harness.controlling.mob_id.as_str()),
        "the child records its requester's bound"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn narrowed_members_child_is_bounded_by_its_ceiling() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let harness = upcall_harness_composed("bound-child", None, narrowed_keeping_mob_tools).await;
    let child = probe_member(&harness, "bound-child", "c1").await;
    let reply = upcall(
        &harness,
        "req-bound-child-spawn",
        spawn_member_op("worker", "c1", |_| {}),
    )
    .await;
    assert_admitted(&reply, "the narrowed member's spawn of a profile it keeps");
    assert_child_is_bounded(&harness, &child, "bound-child").await;
    harness.scripted.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn narrowed_members_spawn_many_child_is_bounded_by_its_ceiling() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let harness = upcall_harness_composed("bound-many", None, narrowed_keeping_mob_tools).await;
    let child = probe_member(&harness, "bound-many", "c1").await;
    let MemberOperatorOp::SpawnMember(child_spec) = spawn_member_op("worker", "c1", |_| {}) else {
        unreachable!("spawn_member_op builds a SpawnMember op");
    };
    let reply = upcall(
        &harness,
        "req-bound-many-spawn",
        MemberOperatorOp::SpawnManyMembers {
            specs: vec![*child_spec],
        },
    )
    .await;
    let outcome = tool_outcome(&reply);
    assert!(
        outcome["kind"] == "ok" && outcome["is_error"] == false,
        "the narrowed member's spawn_many of a profile it keeps must be admitted, got {outcome}"
    );
    assert_child_is_bounded(&harness, &child, "bound-many").await;
    harness.scripted.shutdown();
}

/// Sets `ceiling` on the spawn request of `identity`, whatever its source.
struct IdentityCeilingCustomizer {
    identity: &'static str,
    ceiling: ceiling_api::Ceiling,
}

impl SpawnMemberCustomizer for IdentityCeilingCustomizer {
    fn customize_spawn(
        &self,
        _ctx: &SpawnCustomizationContext,
        spec: &mut SpawnMemberSpec,
    ) -> Result<(), meerkat_mob::MobError> {
        if spec.identity == self.identity {
            ceiling_api::set_ceiling(spec, self.ceiling.clone());
        }
        Ok(())
    }
}

/// A customizer ceiling wider than the requester's bound (manage, every
/// profile) and narrower in one dimension (no create) yields their
/// intersection: the bound's profiles without manage, and no create.
#[tokio::test(flavor = "multi_thread")]
async fn customizer_ceiling_cannot_widen_an_inherited_bound() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let customizer = std::sync::Arc::new(IdentityCeilingCustomizer {
        identity: "c1",
        ceiling: ceiling_api::without_create(ceiling_api::unrestricted()),
    });
    let harness = upcall_harness_composed(
        "bound-customizer",
        Some(customizer as std::sync::Arc<dyn SpawnMemberCustomizer>),
        narrowed_keeping_mob_tools,
    )
    .await;
    let reply = upcall(
        &harness,
        "req-bound-customizer-spawn",
        spawn_member_op("worker", "c1", |_| {}),
    )
    .await;
    assert_admitted(&reply, "the narrowed member's spawn of a profile it keeps");
    let mut expected = bounded_child_facts(harness.controlling.mob_id.as_str());
    expected["can_create_mobs"] = serde_json::Value::from(false);
    assert_eq!(
        recorded_facts(&harness.controlling, "c1").await,
        expected,
        "the child records the intersection of the bound and the customizer's ceiling"
    );
    harness.scripted.shutdown();
}

/// A member without a ceiling bounds its child by nothing more than the
/// default: the child records the default facts, as before.
#[tokio::test(flavor = "multi_thread")]
async fn default_members_child_keeps_the_default_authority() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let harness = upcall_harness("bound-default").await;
    let reply = upcall(
        &harness,
        "req-bound-default-spawn",
        spawn_member_op("worker", "c1", |_| {}),
    )
    .await;
    assert_admitted(&reply, "a default member's spawn");
    assert_eq!(
        recorded_facts(&harness.controlling, "c1").await,
        default_facts(harness.controlling.mob_id.as_str()),
        "a default member's child records the default facts"
    );
    harness.scripted.shutdown();
}

/// The bound is recorded with the child, so the child's respawn keeps it.
#[tokio::test(flavor = "multi_thread")]
async fn inherited_bound_survives_the_childs_respawn() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let harness = upcall_harness_composed("bound-respawn", None, narrowed_keeping_mob_tools).await;
    let child = probe_member(&harness, "bound-respawn", "c1").await;
    let reply = upcall(
        &harness,
        "req-bound-respawn-spawn",
        spawn_member_op("worker", "c1", |_| {}),
    )
    .await;
    assert_admitted(&reply, "the narrowed member's spawn of a profile it keeps");

    let responder = spawn_scripted_member_turn_responder(std::sync::Arc::clone(&child));
    harness
        .controlling
        .handle
        .respawn(meerkat_mob::AgentIdentity::from("c1"), None)
        .await
        .expect("c1 respawns");
    responder.shutdown_and_join().await;
    assert_child_is_bounded(&harness, &child, "bound-respawn").await;
    harness.scripted.shutdown();
}

// ===========================================================================
// A member-initiated flow run provisions members bounded like the member's own
// spawns: a placed provision records at most the requester's recorded
// authority, a provision that would be local is refused before the run starts,
// and a flow whose roles already have members runs with nothing provisioned.
// ===========================================================================

/// `flow-worker` is a profile with mob tools that no member holds. Flow
/// `provision` has a step on it, so running it provisions a member; flow
/// `existing` has a step on `worker`, which b2 already is.
fn with_provisioning_flows(definition: &mut meerkat_mob::MobDefinition) {
    let worker = definition
        .profiles
        .get(&meerkat_mob::ProfileName::from("worker"))
        .cloned()
        .expect("the fixture defines worker");
    definition
        .profiles
        .insert(meerkat_mob::ProfileName::from("flow-worker"), worker);
    definition.flows.insert(
        meerkat_mob::FlowId::from("provision"),
        support::single_step_flow("flow-worker", "provisioned by a flow"),
    );
    definition.flows.insert(
        meerkat_mob::FlowId::from("existing"),
        support::single_step_flow("worker", "run on an existing member"),
    );
}

const PROVISIONED: &str = "__flow_flow-worker_provision";

/// Places every member a flow run provisions on the scripted host.
#[derive(Default)]
struct PlaceFlowProvisions {
    host: std::sync::Mutex<Option<String>>,
}

impl SpawnMemberCustomizer for PlaceFlowProvisions {
    fn customize_spawn(
        &self,
        ctx: &SpawnCustomizationContext,
        spec: &mut SpawnMemberSpec,
    ) -> Result<(), meerkat_mob::MobError> {
        if ctx.spawn_source == meerkat_mob::runtime::SpawnSource::FlowProvisioning
            && let Some(host) = self
                .host
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        {
            spec.placement = Some(meerkat_mob::machines::mob_machine::HostId(host));
        }
        Ok(())
    }
}

async fn run_flow_upcall(harness: &UpcallHarness, request_id: &str, flow: &str) -> BridgeReply {
    upcall(
        harness,
        request_id,
        MemberOperatorOp::MobRunFlow {
            flow_id: flow.to_string(),
            params: None,
        },
    )
    .await
}

async fn flow_runs(harness: &UpcallHarness, flow: &str) -> usize {
    harness
        .controlling
        .storage_runs
        .list_runs(
            &harness.controlling.mob_id,
            Some(&meerkat_mob::FlowId::from(flow)),
        )
        .await
        .expect("flow runs read")
        .len()
}

/// b2 keeps manage scope (a flow run needs it) but its ceiling drops create or
/// mutate. The flow's provision would be a local member with its profile's
/// full authority, and no ceiling can bound a local member, so the run is
/// refused, typed, before it starts: no member, no run.
async fn bounded_flow_provision_is_refused_row(
    label: &str,
    shape: fn(SpawnMemberSpec) -> SpawnMemberSpec,
) {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let controlling = create_controlling_mob_with_definition(label, with_provisioning_flows).await;
    let harness = upcall_harness_on(label, controlling, shape).await;
    let reply = run_flow_upcall(&harness, &format!("req-{label}-run"), "provision").await;
    let outcome = tool_outcome(&reply);
    assert!(
        !has_member(&harness.controlling, PROVISIONED).await,
        "a member whose ceiling drops create or mutate must not provision, through a flow, a \
         member that holds them; the flow run replied {outcome}"
    );
    assert!(
        outcome["kind"] == "tool_error"
            && outcome["message"]
                .as_str()
                .is_some_and(|message| message.contains("operator authority ceiling")),
        "the flow run is refused for the provision's ceiling, got {outcome}"
    );
    assert_eq!(
        flow_runs(&harness, "provision").await,
        0,
        "the refusal comes before the run starts"
    );
    harness.scripted.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn flow_provision_for_a_member_without_create_is_refused_before_the_run() {
    bounded_flow_provision_is_refused_row("flow-create", manage_without_create).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn flow_provision_for_a_member_without_mutate_is_refused_before_the_run() {
    bounded_flow_provision_is_refused_row("flow-mutate", manage_without_mutate).await;
}

/// A flow provision is never placed: the placed lane refuses the reserved
/// flow identity, so a customizer that places one fails the run, ceiling or
/// not. Every provision a flow makes is therefore local.
#[tokio::test(flavor = "multi_thread")]
async fn flow_provision_is_never_placed() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let place = std::sync::Arc::new(PlaceFlowProvisions::default());
    let controlling = create_controlling_mob_with_customizer_and_definition(
        "flow-placed",
        Some(place.clone() as std::sync::Arc<dyn SpawnMemberCustomizer>),
        with_provisioning_flows,
    )
    .await;
    let harness = upcall_harness_on("flow-placed", controlling, |spec| spec).await;
    *place
        .host
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(harness.host_id.clone());
    let reply = run_flow_upcall(&harness, "req-flow-placed-run", "provision").await;
    let outcome = tool_outcome(&reply);
    assert!(
        outcome["kind"] == "tool_error"
            && outcome["message"]
                .as_str()
                .is_some_and(|message| message.contains("reserved system identifier namespace")),
        "a placed flow provision is refused for its reserved identity, got {outcome}"
    );
    assert!(!has_member(&harness.controlling, PROVISIONED).await);
    harness.scripted.shutdown();
}

/// A member without a ceiling runs a flow as before: the provision is local
/// and admitted.
#[tokio::test(flavor = "multi_thread")]
async fn default_members_flow_provisions_as_before() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let controlling =
        create_controlling_mob_with_definition("flow-default", with_provisioning_flows).await;
    let harness = upcall_harness_on("flow-default", controlling, |spec| spec).await;
    let reply = run_flow_upcall(&harness, "req-flow-default-run", "provision").await;
    assert_admitted(&reply, "a default member's flow run");
    assert!(
        has_member(&harness.controlling, PROVISIONED).await,
        "the flow provisioned its member"
    );
    harness.scripted.shutdown();
}

/// When every role of the flow already has a member, the narrowed member's
/// flow runs and provisions nothing.
#[tokio::test(flavor = "multi_thread")]
async fn narrowed_members_flow_with_existing_targets_runs_unprovisioned() {
    let _guard = REAL_COMMS_TEST_LOCK.lock().await;
    let controlling =
        create_controlling_mob_with_definition("flow-existing", with_provisioning_flows).await;
    let harness = upcall_harness_on("flow-existing", controlling, manage_without_create).await;
    let reply = run_flow_upcall(&harness, "req-flow-existing-run", "existing").await;
    assert_admitted(&reply, "the narrowed member's flow run on existing targets");
    assert!(
        !harness
            .controlling
            .handle
            .list_members()
            .await
            .iter()
            .any(|entry| entry.agent_identity.as_str().starts_with("__flow_")),
        "nothing was provisioned"
    );
    assert_eq!(flow_runs(&harness, "existing").await, 1, "the run started");
    harness.scripted.shutdown();
}
