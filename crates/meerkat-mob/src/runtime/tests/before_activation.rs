use super::*;

fn definition_for_hook_test(name: &str) -> MobDefinition {
    let mut definition = sample_definition();
    definition.id = MobId::from(format!("before-activation-{name}-{}", Uuid::new_v4()));
    definition
}

#[tokio::test]
async fn before_activation_fresh_binding_precedes_member_creation() {
    let service = Arc::new(MockSessionService::new());
    let _ = service.enable_runtime_adapter();
    let called = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&called);
    let observed_service = Arc::clone(&service);
    let hook: MobBeforeActivation = Arc::new(move |handle| {
        let observed = Arc::clone(&observed);
        let service = Arc::clone(&observed_service);
        Box::pin(async move {
            assert!(handle.member_creation_journal_cursor().await.unwrap() > 0);
            assert_eq!(service.session_counter.load(Ordering::SeqCst), 0);
            observed.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
    });
    let handle = MobBuilder::new(definition_for_hook_test("fresh"), MobStorage::in_memory())
        .with_session_service(service)
        .before_activation(hook)
        .create()
        .await
        .expect("create after binding");
    assert_eq!(called.load(Ordering::SeqCst), 1);
    handle
        .spawn(
            ProfileName::from("worker"),
            AgentIdentity::from("worker"),
            None,
        )
        .await
        .expect("spawn after binding");
    assert_eq!(called.load(Ordering::SeqCst), 1);
    handle.shutdown().await.expect("shutdown test mob");
}

#[cfg(not(target_arch = "wasm32"))]
#[tokio::test]
async fn before_activation_fresh_refusal_aborts_startup_and_allows_retry() {
    let service = Arc::new(MockSessionService::new());
    let _ = service.enable_runtime_adapter();
    let directory = tempfile::tempdir().expect("durable refusal fixture");
    let path = directory.path().join("mob.sqlite3");
    let storage = MobStorage::persistent(&path).expect("durable mob store");
    let hook: MobBeforeActivation =
        Arc::new(|_| Box::pin(async { Err(MobError::Internal("host binding refused".into())) }));
    let result = MobBuilder::new(definition_for_hook_test("refusal"), storage.clone())
        .with_session_service(service.clone())
        .before_activation(hook)
        .create()
        .await;
    assert!(
        matches!(result, Err(MobError::Internal(message)) if message == "host binding refused")
    );
    assert_eq!(service.session_counter.load(Ordering::SeqCst), 0);
    drop(storage);
    let recovered = MobBuilder::for_resume(
        MobStorage::persistent(&path).expect("reopen refused mob from disk"),
    )
    .with_session_service(service)
    .before_activation(Arc::new(|_| Box::pin(async { Ok(()) })))
    .resume()
    .await
    .expect("failed bootstrap releases its supervisor binding");
    recovered.shutdown().await.expect("shutdown recovered mob");
}

#[tokio::test]
async fn before_activation_no_hook_resume_preserves_public_member_state() {
    for initially_stopped in [false, true] {
        let service = Arc::new(MockSessionService::new());
        let _ = service.enable_runtime_adapter();
        let storage = MobStorage::in_memory();
        let handle = MobBuilder::new(definition_for_hook_test("no-hook"), storage.clone())
            .with_session_service(service.clone())
            .create()
            .await
            .expect("create without hook");
        let member = AgentIdentity::from("worker");
        handle
            .spawn(ProfileName::from("worker"), member.clone(), None)
            .await
            .expect("spawn without hook");
        let before = handle.get_member(&member).await.unwrap().unwrap();
        let session = before.bridge_session_id().unwrap().clone();
        let creation = handle.member_creation_for_session(&session).await.unwrap();
        if initially_stopped {
            handle.stop().await.expect("stop before cold resume");
        }
        let phase = handle.status().await.unwrap();
        crash_stop_and_release_routes(handle).await;
        let resumed = MobBuilder::for_resume(storage)
            .with_session_service(service)
            .resume()
            .await
            .expect("cold resume without hook");
        let after = resumed.get_member(&member).await.unwrap().unwrap();
        assert_eq!(after.bridge_session_id(), Some(&session));
        assert_eq!(after.agent_identity, before.agent_identity);
        assert_eq!(after.role, before.role);
        assert_eq!(after.generation, before.generation);
        assert_eq!(resumed.status().await.unwrap(), phase);
        assert_eq!(
            resumed.member_creation_for_session(&session).await.unwrap(),
            creation
        );
        resumed.shutdown().await.expect("shutdown no-hook fixture");
    }
}

#[tokio::test]
async fn before_activation_resume_refusal_precedes_member_materialization() {
    let service = Arc::new(MockSessionService::new());
    let _ = service.enable_runtime_adapter();
    let storage = MobStorage::in_memory();
    let handle = MobBuilder::new(definition_for_hook_test("resume"), storage.clone())
        .with_session_service(service.clone())
        .create()
        .await
        .expect("create predecessor");
    handle
        .spawn(
            ProfileName::from("worker"),
            AgentIdentity::from("worker"),
            None,
        )
        .await
        .expect("spawn predecessor member");
    let member = handle
        .get_member(&AgentIdentity::from("worker"))
        .await
        .expect("read predecessor member")
        .expect("predecessor exists");
    let session = member.bridge_session_id().expect("session binding").clone();
    let creation = handle
        .member_creation_for_session(&session)
        .await
        .expect("read predecessor history")
        .expect("creation exists");
    crash_stop_and_release_routes(handle).await;
    let before = service.create_requests.read().await.len();
    let observed_service = Arc::clone(&service);
    let called = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&called);
    let hook: MobBeforeActivation = Arc::new(move |handle| {
        let service = Arc::clone(&observed_service);
        let observed = Arc::clone(&observed);
        let session = session.clone();
        let creation = creation.clone();
        Box::pin(async move {
            assert_eq!(service.create_requests.read().await.len(), before);
            assert_eq!(
                handle
                    .get_member(&AgentIdentity::from("worker"))
                    .await
                    .expect("restored binding is readable before materialization")
                    .expect("restored member exists")
                    .bridge_session_id(),
                Some(&session)
            );
            assert_eq!(
                handle.member_creation_for_session(&session).await.unwrap(),
                Some(creation)
            );
            observed.fetch_add(1, Ordering::SeqCst);
            Err(MobError::Internal("restore binding refused".into()))
        })
    });
    let result = MobBuilder::for_resume(storage)
        .with_session_service(service.clone())
        .before_activation(hook)
        .resume()
        .await;
    assert!(
        matches!(result, Err(MobError::Internal(message)) if message == "restore binding refused")
    );
    assert_eq!(called.load(Ordering::SeqCst), 1);
    assert_eq!(service.create_requests.read().await.len(), before);
}

#[tokio::test]
async fn before_activation_retained_resume_handle_observes_live_lifecycle() {
    for initially_stopped in [false, true] {
        let service = Arc::new(MockSessionService::new());
        let _ = service.enable_runtime_adapter();
        let storage = MobStorage::in_memory();
        let handle = MobBuilder::new(definition_for_hook_test("live-phase"), storage.clone())
            .with_session_service(service.clone())
            .create()
            .await
            .expect("create predecessor");
        if initially_stopped {
            handle.stop().await.expect("persist stopped phase");
        }
        crash_stop_and_release_routes(handle).await;
        let retained = Arc::new(RwLock::new(None));
        let observed = Arc::clone(&retained);
        let hook: MobBeforeActivation = Arc::new(move |handle| {
            let observed = Arc::clone(&observed);
            Box::pin(async move {
                *observed.write().await = Some(handle);
                Ok(())
            })
        });
        let handle = MobBuilder::for_resume(storage)
            .with_session_service(service)
            .before_activation(hook)
            .resume()
            .await
            .expect("cold resume");
        let preview = retained.read().await.clone().expect("host retained handle");
        assert_eq!(
            preview.status_observation_snapshot(),
            handle.status().await.unwrap()
        );
        if initially_stopped {
            handle.resume().await.expect("resume stopped mob");
        } else {
            handle.stop().await.expect("stop running mob");
        }
        assert_eq!(
            preview.status_observation_snapshot(),
            handle.status().await.unwrap()
        );
        handle.destroy().await.expect("destroy mob");
        assert_eq!(preview.status_observation_snapshot(), MobState::Destroyed);
    }
}
