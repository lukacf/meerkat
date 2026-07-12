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
use support::{
    MemberOperatorRequesterResidency, REAL_COMMS_TEST_LOCK, create_controlling_mob,
    member_identity_of, raw_member_operator_command_at, spawn_peer_comms_endpoint,
    spawn_scripted_host_peer,
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
    let scripted = spawn_scripted_host_peer(&format!("{label}-scripted")).await;
    let controlling = create_controlling_mob(label).await;
    let report = controlling.bind_scripted(&scripted).await;

    // The probe endpoint is the member identity the scripted ack advertises.
    let member = std::sync::Arc::new(
        spawn_peer_comms_endpoint(&format!("{label}-member-b2"), true, None).await,
    );
    scripted.script_member_identity("b2", member_identity_of(&member));
    controlling
        .spawn_placed("worker", "b2", &report.host_id)
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
    let other_member = spawn_peer_comms_endpoint("upcall-t16-member-b3", true, None).await;
    harness
        .scripted
        .script_member_identity("b3", member_identity_of(&other_member));
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
    harness
        .controlling
        .handle
        .revoke_host(&harness.host_id)
        .await
        .expect("revoke host");
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

    harness
        .controlling
        .handle
        .retire(identity.clone())
        .await
        .expect("retire first incarnation");
    let mut replacement = support::placed_spawn_spec("worker", "b2", &harness.host_id);
    replacement.launch_mode = meerkat_mob::launch::MemberLaunchMode::Resume {
        bridge_session_id: old_session.clone(),
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
            meerkat_mob::runtime::bridge_protocol::MaterializeLaunchMode::Resume { session_id }
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
    let controlling = harness.controlling.restart().await;
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

// T-LS4 lives in-crate (meerkat-mob/src/runtime/tools.rs unit lane): the
// upcall executor's dispatch handle binding (`CommandAuthorityKind::AgentLane`)
// is not observable from this integration harness — the executor handle never
// leaves the crate. The behavioral halves (grants don't gate upcalls; agent
// authority satisfies no scope check) are T-LS1..T-LS3 above.
