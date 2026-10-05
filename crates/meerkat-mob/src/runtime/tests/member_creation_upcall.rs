use super::placement_support as support;
use super::*;
use crate::MemberCreationProvenance;
use crate::runtime::bridge_protocol::{
    BridgeReply, MemberOperatorOp, MemberOperatorOutcome, MemberOperatorSpawnSpec,
};

fn upcall_spawn_spec(member_id: &str) -> MemberOperatorSpawnSpec {
    MemberOperatorSpawnSpec {
        profile: "worker".into(),
        member_id: member_id.into(),
        initial_message: None,
        runtime_mode: None,
        launch_mode: None,
        auto_wire_parent: None,
        placement: None,
        requested_tool_access_policy_present: false,
        resolved_tool_access_policy: None,
    }
}

async fn assert_remote_upcall_creation_is_unproven(batch: bool) {
    let label = if batch {
        "creation-upcall-batch"
    } else {
        "creation-upcall-single"
    };
    let host = support::spawn_scripted_host_peer(&format!("{label}-host")).await;
    let controlling = support::create_controlling_mob(label).await;
    let binding = controlling.bind_scripted(&host).await;
    let member = support::spawn_peer_comms_endpoint(&format!("{label}-source"), true, None).await;
    host.script_member_identity("source", support::member_identity_of(&member));
    controlling
        .spawn_placed("worker", "source", &binding.host_id)
        .await
        .expect("place the authenticated upcall source");

    let source = controlling
        .handle
        .get_member(&AgentIdentity::from("source"))
        .await
        .unwrap()
        .unwrap();
    let source_session = source.bridge_session_id().unwrap().clone();
    assert!(
        controlling
            .handle
            .capture_member_creation_source(&source_session)
            .await
            .is_err(),
        "a remote source has no controller-local persisted metadata proof"
    );
    let materialize = host
        .received_materialize_payloads()
        .into_iter()
        .find(|payload| payload.spec.agent_identity == "source")
        .expect("the real placed spawn supplies the exact requester fence");
    let supervisor = controlling
        .handle
        .routable_supervisor_peer()
        .await
        .expect("upcall reply route");
    member.trust(supervisor.clone()).await;
    let children = if batch {
        vec!["batch-one", "batch-two"]
    } else {
        vec!["single-child"]
    };
    let op = if batch {
        MemberOperatorOp::SpawnManyMembers {
            specs: children
                .iter()
                .map(|name| upcall_spawn_spec(name))
                .collect(),
        }
    } else {
        MemberOperatorOp::SpawnMember(Box::new(upcall_spawn_spec(children[0])))
    };
    let source_session_wire = source_session.to_string();
    let command = support::raw_member_operator_command_at(
        support::MemberOperatorRequesterResidency {
            agent_identity: "source",
            generation: materialize.generation,
            fence_token: materialize.fence_token,
            host_id: &binding.host_id,
            host_binding_generation: materialize.binding_generation,
            member_session_id: &source_session_wire,
        },
        label,
        op,
    );
    let reply = member
        .send_bridge_command_raw(&supervisor, &command, Duration::from_secs(30))
        .await
        .expect("serve the authenticated spawn upcall over loopback comms");
    let BridgeReply::MemberOperatorReply(reply) = reply else {
        panic!("expected an operator reply, got {reply:?}");
    };
    assert!(
        matches!(&reply.outcome, MemberOperatorOutcome::Completed { .. }),
        "the actual spawn path must complete before checking its facts: {:?}",
        reply.outcome
    );

    let events = controlling.storage_events.replay_all().await.unwrap();
    for child in children {
        let identity = AgentIdentity::from(child);
        let entry = controlling
            .handle
            .get_member(&identity)
            .await
            .unwrap()
            .expect("upcall must create every requested child");
        let child_session = entry.bridge_session_id().unwrap();
        assert!(
            controlling
                .handle
                .member_creation_for_session(child_session)
                .await
                .unwrap()
                .is_none(),
            "the placed event has no local session endpoint; history must not infer one"
        );
        let spawned = events
            .iter()
            .find_map(|event| match &event.kind {
                MobEventKind::MemberSpawned(spawned) if spawned.agent_identity == identity => {
                    Some(spawned)
                }
                _ => None,
            })
            .expect("actual spawn committed a MemberSpawned event");
        assert!(spawned.creation.creation_id.is_some());
        assert_eq!(
            spawned.creation.provenance,
            MemberCreationProvenance::Unproven,
            "a real remote upcall must never acquire host-root provenance"
        );
    }

    crash_stop_and_release_routes(controlling.handle.clone()).await;
    host.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn member_creation_upcall_spawn_one_remote_source_is_unproven() {
    let _guard = support::REAL_COMMS_TEST_LOCK.lock().await;
    assert_remote_upcall_creation_is_unproven(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn member_creation_upcall_spawn_many_remote_source_is_unproven() {
    let _guard = support::REAL_COMMS_TEST_LOCK.lock().await;
    assert_remote_upcall_creation_is_unproven(true).await;
}
