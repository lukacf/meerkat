use super::*;

#[cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]
async fn placed_rollback_fixture(
    mut definition: MobDefinition,
) -> (
    MobHandle,
    Arc<RealCommsSessionService>,
    Arc<dyn crate::store::MobEventStore>,
) {
    let service = Arc::new(RealCommsSessionService::new());
    let adapter = service.runtime_adapter.clone();
    let mob_service: Arc<dyn MobSessionService> = service.clone();
    let acceptor = crate::ControllingAcceptorConfig::for_session_service(
        "127.0.0.1:0".parse().expect("loopback acceptor"),
        None,
        mob_service.clone(),
    );
    definition.backend.external = Some(crate::definition::ExternalBackendConfig {
        address_base: "tcp://127.0.0.1".into(),
        supervisor_bridge: None,
    });
    let storage = MobStorage::in_memory();
    let events = storage.events.clone();
    let handle = MobBuilder::new(definition, storage)
        .with_session_service(mob_service)
        .with_runtime_adapter(adapter)
        .with_controlling_acceptor(acceptor)
        .with_owner_bridge_session_create_authority(SessionId::new(), false, false)
        .with_spawn_base_prompt_source(Arc::new(crate::StaticSpawnBasePromptSource(
            "Deterministic placed rollback fixture".into(),
        )))
        .create()
        .await
        .expect("create placement-capable real-comms mob");
    (handle, service, events)
}

#[cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]
fn run_on_production_worker_stack(future: impl std::future::Future<Output = ()>) {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_stack_size(2 * 1024 * 1024)
        .enable_all()
        .build()
        .expect("production-sized rollback worker runtime")
        .block_on(future);
}

#[cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]
#[test]
fn spawn_rollback_placed_activation_failure_preserves_host_binding_until_release() {
    run_on_production_worker_stack(placed_activation_failure_scenario());
}

#[cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]
async fn placed_activation_failure_scenario() {
    let _guard = REAL_COMMS_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let host = placement_support::spawn_scripted_host_peer("placed-rollback-failed-host").await;
    let remote = Arc::new(
        placement_support::spawn_peer_comms_endpoint("placed-rollback-failed-member", true, None)
            .await,
    );
    host.script_member_identity(
        "failed-placed",
        placement_support::member_identity_of(&remote),
    );
    host.bind_member_endpoint("failed-placed", remote.clone());
    let responder = placement_support::spawn_scripted_member_turn_responder(remote.clone());
    responder.reject_next_deliver_decode();
    let (handle, _, events) = placed_rollback_fixture(sample_definition()).await;
    let report = handle
        .bind_host(placement_support::descriptor_to_bind_request(
            &host.descriptor,
        ))
        .await
        .expect("bind real host transport");
    let mut spec = placement_support::placed_spawn_spec("worker", "failed-placed", &report.host_id);
    spec.runtime_mode = Some(crate::MobRuntimeMode::TurnDriven);
    spec.initial_message = Some("reject this placed activation's initial admission".into());
    let failed = tokio::time::timeout(Duration::from_secs(15), handle.spawn_spec(spec))
        .await
        .expect("placed activation rollback settles")
        .expect_err("initial admission must fail");
    assert!(
        !failed.to_string().contains("rollback remains owned"),
        "the original activation error must not hide a stuck retirement journal: {failed}"
    );
    let recorded = events.replay_all().await.expect("retirement carriers");
    let (releasing, session) = recorded
        .iter()
        .find_map(|event| match &event.kind {
            MobEventKind::MemberRetirementStarted {
                agent_identity,
                releasing,
                session_id,
                ..
            } if agent_identity.as_str() == "failed-placed" => {
                Some((releasing.clone(), session_id.clone()))
            }
            _ => None,
        })
        .expect("generated placed retirement journal persisted before host release");
    assert!(
        releasing.is_none(),
        "host binding must not be journaled as locally released"
    );
    assert!(
        session.is_some(),
        "the exact host session remains correlated"
    );
    assert_eq!(
        host.release_count(),
        1,
        "exact host release actually completed"
    );
    assert!(recorded.iter().any(|event| matches!(
        &event.kind,
        MobEventKind::MemberRetired { agent_identity, .. }
            if agent_identity.as_str() == "failed-placed"
    )));
    assert!(
        handle
            .get_member(&AgentIdentity::from("failed-placed"))
            .await
            .expect("query rolled-back placed member")
            .is_none()
    );
    responder.shutdown();
    host.shutdown();
}

#[cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]
#[test]
fn spawn_rollback_placed_survivor_remove_retains_edge_until_ack_and_retry() {
    run_on_production_worker_stack(placed_survivor_remove_scenario());
}

#[cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]
async fn placed_survivor_remove_scenario() {
    let _guard = REAL_COMMS_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let host = placement_support::spawn_scripted_host_peer("placed-rollback-survivor-host").await;
    let remote = Arc::new(
        placement_support::spawn_peer_comms_endpoint("placed-rollback-survivor", true, None).await,
    );
    host.script_member_identity("a-placed", placement_support::member_identity_of(&remote));
    host.bind_member_endpoint("a-placed", remote.clone());
    let responder = placement_support::spawn_scripted_member_turn_responder(remote.clone());
    let (handle, service, events) =
        placed_rollback_fixture(sample_definition_with_role_wiring()).await;
    let report = handle
        .bind_host(placement_support::descriptor_to_bind_request(
            &host.descriptor,
        ))
        .await
        .expect("bind survivor host");
    let mut placed = placement_support::placed_spawn_spec("worker", "a-placed", &report.host_id);
    placed.runtime_mode = Some(crate::MobRuntimeMode::TurnDriven);
    handle.spawn_spec(placed).await.expect("placed survivor");
    let mut later_peer = SpawnMemberSpec::new("worker", "z-missing");
    later_peer.runtime_mode = Some(crate::MobRuntimeMode::TurnDriven);
    let later_peer = handle
        .spawn_spec(later_peer)
        .await
        .expect("later wiring target");
    let later_session = handle
        .resolve_bridge_session_id(&later_peer.agent_identity)
        .await
        .expect("later target session");
    let unavailable_runtime = service
        .sessions
        .write()
        .await
        .remove(&later_session)
        .expect("hold the later target's runtime outside the service lookup");

    // The first planned edge installs real host-side trust. Failure on the
    // later planned edge then exercises the same activation compensation
    // after physical placement wiring has succeeded.
    host.reject_remove_peer_trust_for(
        "a-placed",
        crate::runtime::bridge_protocol::BridgeRejectionCause::Unavailable,
        "held survivor refuses the first Remove",
    );
    let held = host.hold_next_remove_peer_trust();
    let spawning = {
        let handle = handle.clone();
        tokio::spawn(async move {
            let mut spec = SpawnMemberSpec::new("worker", "m-failed");
            spec.runtime_mode = Some(crate::MobRuntimeMode::TurnDriven);
            handle.spawn_spec(spec).await
        })
    };
    let observed_remove = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Some(payload) = host.received_remove_peer_trust_payloads().last().cloned() {
                break payload;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    if observed_remove.is_err() {
        held.add_permits(1);
        let result = tokio::time::timeout(Duration::from_secs(2), spawning).await;
        panic!("rollback did not request physical survivor removal; spawn result: {result:?}");
    }
    let removal = observed_remove.expect("rollback must request physical survivor removal");
    assert_eq!(removal.agent_identity, "a-placed");
    assert_eq!(removal.epoch, report.epoch);
    let peer_id = removal.peer.peer_id;
    let failed_identity = AgentIdentity::from("m-failed");
    let edge = host_dsl::WiringEdge::new(
        host_dsl::AgentIdentity::from("a-placed"),
        host_dsl::AgentIdentity::from("m-failed"),
    );
    let failed_session = service
        .session_comms_names
        .read()
        .await
        .iter()
        .find(|(_, name)| **name == test_comms_name("worker", "m-failed"))
        .map(|(session, _)| session.clone())
        .expect("failed activation's exact session");
    let edge_held = handle
        .machine_state_watch_rx
        .borrow()
        .wiring_edges
        .contains(&edge);
    let trust_held = meerkat_core::agent::CommsRuntime::peers(remote.runtime.as_ref())
        .await
        .iter()
        .any(|peer| peer.peer_id.to_string() == peer_id);
    let archived_while_held = service
        .archived_session_ids
        .read()
        .await
        .contains(&failed_session);
    let returned_while_held = spawning.is_finished();
    held.add_permits(1);
    let failure = tokio::time::timeout(Duration::from_secs(10), spawning)
        .await
        .expect("failed Remove replies with retained rollback")
        .expect("spawn task")
        .expect_err("failed survivor removal must surface");
    assert!(
        edge_held && trust_held,
        "edge and real trust must survive an unacknowledged Remove"
    );
    assert!(
        !archived_while_held,
        "disposal must stay behind remote cleanup"
    );
    assert!(
        !returned_while_held,
        "a held Remove cannot settle activation compensation"
    );
    assert!(failure.to_string().contains("held survivor refuses"));
    assert!(
        handle
            .machine_state_watch_rx
            .borrow()
            .wiring_edges
            .contains(&edge)
    );
    assert!(
        meerkat_core::agent::CommsRuntime::peers(remote.runtime.as_ref())
            .await
            .iter()
            .any(|peer| peer.peer_id.to_string() == peer_id)
    );
    assert!(
        !service
            .archived_session_ids
            .read()
            .await
            .contains(&failed_session)
    );
    assert!(service.sessions.read().await.contains_key(&failed_session));
    assert!(!events.replay_all().await.expect("pre-retry events").iter().any(|event| matches!(
        &event.kind,
        MobEventKind::MemberRetired { agent_identity, .. } if agent_identity == &failed_identity
    )));

    handle
        .retire(failed_identity.clone())
        .await
        .expect("retained exact rollback retries Remove");
    assert!(
        !handle
            .machine_state_watch_rx
            .borrow()
            .wiring_edges
            .contains(&edge)
    );
    assert!(
        !meerkat_core::agent::CommsRuntime::peers(remote.runtime.as_ref())
            .await
            .iter()
            .any(|peer| peer.peer_id.to_string() == peer_id)
    );
    assert!(
        service
            .archived_session_ids
            .read()
            .await
            .contains(&failed_session)
    );
    assert!(
        handle
            .get_member(&failed_identity)
            .await
            .expect("settled roster")
            .is_none()
    );
    assert_eq!(
        host.release_count(),
        0,
        "the surviving host member must not be released"
    );
    service
        .sessions
        .write()
        .await
        .insert(later_session, unavailable_runtime);
    responder.shutdown();
    host.shutdown();
}

#[tokio::test]
async fn retirement_batch_completes_two_wired_members_without_losing_peer_custody() {
    let (handle, service) = create_test_mob(sample_definition()).await;
    let a = AgentIdentity::from("batch-a");
    let b = AgentIdentity::from("batch-b");
    let first = handle
        .spawn(ProfileName::from("worker"), a.clone(), None)
        .await
        .expect("first member");
    let second = handle
        .spawn(ProfileName::from("worker"), b.clone(), None)
        .await
        .expect("second member");
    let first_session = first.bridge_session_id().expect("first session").clone();
    let second_session = second.bridge_session_id().expect("second session").clone();
    handle
        .wire_members_batch([(a.clone(), b.clone())])
        .await
        .expect("real reciprocal wiring");

    tokio::time::timeout(Duration::from_secs(10), handle.complete())
        .await
        .expect("wired batch must settle without waiting on its own ticket or retired peer")
        .expect("complete");

    assert_eq!(
        handle.status().await.expect("completed phase"),
        MobState::Completed
    );
    assert!(handle.get_member(&a).await.expect("first roster").is_none());
    assert!(
        handle
            .get_member(&b)
            .await
            .expect("second roster")
            .is_none()
    );
    assert!(
        service
            .archived_session_ids
            .read()
            .await
            .contains(&first_session)
    );
    assert!(
        service
            .archived_session_ids
            .read()
            .await
            .contains(&second_session)
    );
}

async fn failed_spawn_rollback_fixture() -> (MobHandle, Arc<MockSessionService>, SessionId) {
    let (handle, service) = create_test_mob(sample_definition_with_auto_wire()).await;
    handle
        .spawn(ProfileName::from("lead"), AgentIdentity::from("lead"), None)
        .await
        .expect("spawn wiring peer");
    service
        .set_comms_behavior(
            &test_comms_name("lead", "lead"),
            MockCommsBehavior {
                fail_send_peer_added: true,
                ..MockCommsBehavior::default()
            },
        )
        .await;
    service
        .set_archive_failure_for_comms_name(&test_comms_name("worker", "rollback"))
        .await;
    let error = handle
        .spawn(
            ProfileName::from("worker"),
            AgentIdentity::from("rollback"),
            None,
        )
        .await
        .expect_err("first rollback must retain its failed archive");
    assert!(error.to_string().contains("rollback"));
    let entry = handle
        .get_member(&AgentIdentity::from("rollback"))
        .await
        .expect("query retained owner")
        .expect("failed rollback retains its incarnation");
    let session = entry.bridge_session_id().expect("session-backed").clone();
    assert!(!service.archived_session_ids.read().await.contains(&session));
    (handle, service, session)
}

#[tokio::test]
async fn spawn_rollback_held_archive_keeps_actor_and_unrelated_turn_responsive() {
    let (handle, service, session) = failed_spawn_rollback_fixture().await;
    // A profile without auto-wire avoids adding a dependency on the failed
    // member. The existing lead is also outside the rollback's archive effect.
    let lead = handle
        .member(&AgentIdentity::from("lead"))
        .await
        .expect("unrelated live member");
    let gate = Arc::new(TestRuntimeControlBarrier::new());
    service
        .retirement_archive_gates
        .write()
        .await
        .insert(session.clone(), gate.clone());
    service.clear_archive_failure(&session).await;
    let retry_handle = handle.clone();
    let retry =
        tokio::spawn(async move { retry_handle.retire(AgentIdentity::from("rollback")).await });
    tokio::time::timeout(Duration::from_secs(5), async {
        while gate.boundary_calls.load(Ordering::Acquire) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("real archive callback is held");
    let phase = tokio::time::timeout(Duration::from_secs(1), handle.status()).await;
    let turn = tokio::time::timeout(
        Duration::from_secs(2),
        lead.internal_turn("independent turn during failed-spawn cleanup"),
    )
    .await;
    let remained_owned = !retry.is_finished();
    gate.release_all();
    let settled = tokio::time::timeout(Duration::from_secs(5), retry)
        .await
        .expect("retry settles after release")
        .expect("retry task");
    assert_eq!(
        phase
            .expect("QueryPhase remains responsive")
            .expect("phase"),
        MobState::Running
    );
    turn.expect("unrelated member turn remains responsive")
        .expect("turn admitted");
    assert!(remained_owned, "held callback is not physical settlement");
    settled.expect("same rollback owner completes");
    assert!(
        handle
            .get_member(&AgentIdentity::from("rollback"))
            .await
            .unwrap()
            .is_none()
    );
    assert!(service.archived_session_ids.read().await.contains(&session));
}

#[tokio::test]
async fn spawn_rollback_failed_archive_has_an_executable_wake_retry() {
    let (handle, service, session) = failed_spawn_rollback_fixture().await;
    let before = service.archive_call_count(&session).await;
    service.clear_archive_failure(&session).await;
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            handle
                .status()
                .await
                .expect("wake actor without issuing retirement");
            if handle
                .get_member(&AgentIdentity::from("rollback"))
                .await
                .expect("query retry owner")
                .is_none()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("retained rollback has a scheduled physical owner");
    assert!(service.archive_call_count(&session).await > before);
    assert!(service.archived_session_ids.read().await.contains(&session));
}

#[cfg(feature = "runtime-adapter")]
#[tokio::test]
async fn spawn_rollback_durable_resume_retry_preserves_document_and_successor() {
    let service = Arc::new(MockSessionService::new());
    let adapter = service.enable_runtime_adapter();
    let initial = service
        .create_session(CreateSessionRequest {
            injected_context: Vec::new(),
            model: "claude-sonnet-4-5".into(),
            prompt: "durable resume seed".into(),
            system_prompt: meerkat_core::SystemPromptOverride::Inherit,
            max_tokens: None,
            event_tx: None,
            build: Some(meerkat_core::service::SessionBuildOptions {
                comms_name: Some(test_comms_name("worker", "durable")),
                ..Default::default()
            }),
            initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
            deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
            labels: None,
        })
        .await
        .expect("seed durable session");
    let session = initial.session_id;
    let mut document = service
        .live_session_clone(&session)
        .await
        .expect("seed body");
    document.push(meerkat_core::Message::User(
        meerkat_core::types::UserMessage::text("preserve this durable history"),
    ));
    service.replace_live_session(document).await;
    MobSessionService::discard_live_session(service.as_ref(), &session)
        .await
        .expect("cold durable session");
    let handle = MobBuilder::new(sample_definition_with_auto_wire(), MobStorage::in_memory())
        .with_session_service(service.clone())
        .create()
        .await
        .expect("create mob");
    handle
        .spawn(ProfileName::from("lead"), AgentIdentity::from("lead"), None)
        .await
        .expect("lead");
    service
        .set_comms_behavior(
            &test_comms_name("lead", "lead"),
            MockCommsBehavior {
                fail_send_peer_added: true,
                ..MockCommsBehavior::default()
            },
        )
        .await;
    service
        .discard_actor_failures_remaining
        .store(1, Ordering::Release);
    let mut spec =
        SpawnMemberSpec::new("worker", "durable").with_resume_bridge_session_id(session.clone());
    spec.runtime_mode = Some(crate::MobRuntimeMode::TurnDriven);
    handle
        .spawn_spec(spec)
        .await
        .expect_err("activation fails and exact restore is retained");
    assert!(service.persisted_session_clone(&session).await.is_some());
    assert!(!service.archived_session_ids.read().await.contains(&session));
    service
        .discard_actor_failures_remaining
        .store(0, Ordering::Release);
    handle
        .retire(AgentIdentity::from("durable"))
        .await
        .expect("resume rollback retry");
    assert_eq!(service.archive_call_count(&session).await, 0);
    assert!(
        !service
            .has_live_session(&session)
            .await
            .expect("cold after rollback")
    );
    let preserved = service
        .persisted_session_clone(&session)
        .await
        .expect("durable body survives");
    assert!(preserved.messages().iter().any(|message| {
        message
            .as_indexable_text()
            .contains("preserve this durable history")
    }));
    service
        .set_comms_behavior(
            &test_comms_name("lead", "lead"),
            MockCommsBehavior::default(),
        )
        .await;
    let mut successor =
        SpawnMemberSpec::new("worker", "durable").with_resume_bridge_session_id(session.clone());
    successor.runtime_mode = Some(crate::MobRuntimeMode::TurnDriven);
    handle
        .spawn_spec(successor)
        .await
        .expect("publish successor");
    let witness = adapter
        .current_executor_attachment_witness(&session)
        .await
        .expect("successor attachment");
    tokio::time::sleep(Duration::from_secs(3)).await;
    handle.status().await.expect("wake old retry schedule");
    assert_eq!(
        adapter.current_executor_attachment_witness(&session).await,
        Some(witness)
    );
    assert!(
        service
            .has_live_session(&session)
            .await
            .expect("successor remains live")
    );
    assert_eq!(service.archive_call_count(&session).await, 0);
}
