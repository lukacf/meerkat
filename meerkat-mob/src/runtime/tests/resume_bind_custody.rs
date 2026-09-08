//! Controller custody over a real receiver's process-owned direct bind.

use super::*;
use crate::runtime::bridge_protocol::{BridgeCommand, BridgeReply, BridgeSupervisorPayload};
use meerkat_runtime::{InMemoryRuntimeStore, MeerkatMachine};

struct RealDirectBindReceiver {
    runtime: Arc<meerkat_comms::CommsRuntime>,
    machine: Arc<MeerkatMachine>,
    session_id: SessionId,
}

impl RealDirectBindReceiver {
    async fn spawn(
        name: &str,
        keypair: meerkat_comms::Keypair,
        store: Arc<InMemoryRuntimeStore>,
        session_id: SessionId,
    ) -> Self {
        let machine = Arc::new(MeerkatMachine::persistent_without_blobs(store));
        machine
            .register_session(session_id.clone())
            .await
            .expect("receiver registration");
        let runtime = Arc::new(
            meerkat_comms::CommsRuntime::inproc_only_with_keypair_and_silent_intents(
                name,
                None,
                keypair,
                Arc::new(HashSet::new()),
            )
            .expect("receiver route"),
        );
        let bindings = machine
            .prepare_bindings(session_id.clone())
            .await
            .expect("bindings");
        bindings
            .install_peer_comms_on(runtime.as_ref())
            .expect("peer ingress");
        runtime.install_peer_request_response_authority(
            meerkat_comms::PeerRequestResponseAuthority::new(
                Arc::clone(bindings.peer_interaction()),
                Arc::clone(bindings.interaction_stream()),
            ),
        );
        assert!(
            machine
                .maybe_spawn_comms_drain(&session_id, true, Some(runtime.clone()))
                .await
                .expect("receiver-owned drain admission")
        );
        Self {
            runtime,
            machine,
            session_id,
        }
    }

    fn binding(&self) -> crate::RuntimeBinding {
        crate::RuntimeBinding::External {
            peer_id: self.runtime.public_key().to_peer_id().to_string(),
            address: self.runtime.advertised_address(),
            bootstrap_token: Some(self.runtime.bridge_bootstrap_token().to_string().into()),
            pubkey: *self.runtime.public_key().as_bytes(),
        }
    }

    async fn crash(self) {
        self.machine
            .abort_comms_drain(&self.session_id)
            .await
            .expect("stop receiver drain");
    }
}

struct ReleaseHighWater(Arc<tokio::sync::Notify>);

impl Drop for ReleaseHighWater {
    fn drop(&mut self) {
        self.0.notify_one();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn held_receiver_bind_survives_retry_read_failure_and_supervisor_ack() {
    let _serial = lock_real_comms_tests();
    let definition = with_unique_mob_id(
        sample_definition_with_external_backend(),
        "real-resume-bind-custody",
    );
    let identity = AgentIdentity::from("held-receiver");
    let peer_name = test_comms_name_for(&definition.id, "worker", identity.as_str());
    let metadata = Arc::new(FaultInjectedRuntimeMetadataStore::new());
    let mut storage = MobStorage::in_memory();
    storage.runtime_metadata = metadata.clone();
    let (handle, _service) = create_test_mob_with_real_comms_and_storage(definition, storage).await;
    let keypair = meerkat_comms::Keypair::generate();
    let store = Arc::new(InMemoryRuntimeStore::new());
    let session_id = SessionId::new();
    let receiver = RealDirectBindReceiver::spawn(
        &peer_name,
        keypair.clone(),
        Arc::clone(&store),
        session_id.clone(),
    )
    .await;
    let binding = receiver.binding();
    handle
        .spawn_with_binding(
            ProfileName::from("worker"),
            identity.clone(),
            None,
            binding.clone(),
        )
        .await
        .expect("initial direct bind");
    let before = handle
        .get_member(&identity)
        .await
        .expect("roster")
        .expect("member")
        .direct_member_fence
        .expect("initial fence");
    handle.stop().await.expect("stop controller");
    receiver.crash().await;

    // Same durable semantic high-water and signing identity, but no old
    // process-local direct fence survives the receiver restart.
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    store.block_next_direct_member_high_water_admission(Arc::clone(&entered), Arc::clone(&release));
    let release_guard = ReleaseHighWater(release);
    let receiver = RealDirectBindReceiver::spawn(&peer_name, keypair, store, session_id).await;
    assert_eq!(receiver.binding(), binding);
    let mut resume = tokio::spawn({
        let handle = handle.clone();
        async move { handle.resume().await }
    });
    tokio::time::timeout(Duration::from_secs(5), entered.notified())
        .await
        .expect("real receiver reached high-water admission");
    metadata
        .fail_next_load_supervisor
        .store(true, Ordering::Release);
    tokio::time::timeout(Duration::from_secs(8), async {
        while metadata.fail_next_load_supervisor.load(Ordering::Acquire) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("automatic retry reaches the failed controller metadata read");
    let early_observation = if resume.is_finished() {
        let result = (&mut resume).await.expect("resume observer");
        assert!(matches!(
            result,
            Err(MobError::LifecycleOperationProgressStalled { .. })
        ));
        Some(result)
    } else {
        None
    };
    assert!(
        handle
            .machine_state_watch_rx
            .borrow()
            .explicit_resume_topology_pending
    );
    let phase = tokio::time::timeout(Duration::from_secs(1), handle.status())
        .await
        .expect("actor query remains responsive")
        .expect("phase");
    assert_eq!(phase, MobState::Running);

    let peer = external_peer_descriptor_for_binding(&binding);
    let authority = handle.supervisor_bridge.authority().await;
    let supervisor = handle
        .supervisor_bridge
        .supervisor_spec()
        .await
        .expect("supervisor");
    let reply = handle
        .supervisor_bridge
        .send_bridge_command(
            &peer,
            &BridgeCommand::AuthorizeSupervisor(BridgeSupervisorPayload {
                supervisor: supervisor.into(),
                epoch: authority.epoch,
                protocol_version: authority.protocol_version,
            }),
            Duration::from_secs(2),
        )
        .await
        .expect("supervisor binding is independently acknowledged");
    assert!(matches!(
        serde_json::from_value::<BridgeReply>(reply).expect("typed ack"),
        BridgeReply::Ack(_)
    ));
    let mut retire = tokio::spawn({
        let handle = handle.clone();
        let identity = identity.clone();
        async move { handle.retire(identity).await }
    });
    assert!(
        tokio::time::timeout(Duration::from_millis(250), &mut retire)
            .await
            .is_err()
    );
    assert!(
        handle
            .machine_state_watch_rx
            .borrow()
            .explicit_resume_topology_pending
    );
    let stop = tokio::spawn({
        let handle = handle.clone();
        async move { handle.stop().await }
    });
    drop(release_guard);
    tokio::time::timeout(Duration::from_secs(10), stop)
        .await
        .expect("stop settles")
        .expect("stop task")
        .expect("stop");
    let resumed = match early_observation {
        Some(result) => result,
        None => tokio::time::timeout(Duration::from_secs(10), resume)
            .await
            .expect("resume observer settles")
            .expect("resume task"),
    };
    assert!(
        resumed.is_err(),
        "Stop superseded Resume after exact bind settlement"
    );
    tokio::time::timeout(Duration::from_secs(10), retire)
        .await
        .expect("retire settles")
        .expect("retire task")
        .expect("retire");
    let recovered = metadata
        .observed_direct_fences
        .lock()
        .expect("bound receipt history")
        .last()
        .cloned()
        .expect("a real bind fence was durably projected before retirement");
    assert_eq!(recovered.incarnation(), before.incarnation());
    assert_ne!(
        recovered.runtime_session_token,
        before.runtime_session_token
    );
    handle.shutdown().await.expect("controller shutdown");
    receiver.crash().await;
}
