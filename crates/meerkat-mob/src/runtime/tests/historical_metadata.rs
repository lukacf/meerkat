use super::*;

#[cfg(not(target_arch = "wasm32"))]
fn persistent_metadata_fixture() -> (
    meerkat_session::PersistentSessionService<PersistentMockBuilder>,
    Arc<dyn SessionStore>,
    Arc<dyn meerkat_runtime::RuntimeStore>,
) {
    let session_store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
    let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> =
        Arc::new(meerkat_runtime::InMemoryRuntimeStore::new());
    let service = meerkat_session::PersistentSessionService::new(
        PersistentMockBuilder,
        4,
        session_store.clone(),
        runtime_store.clone(),
        Arc::new(meerkat_store::MemoryBlobStore::new()),
    );
    (service, session_store, runtime_store)
}

#[cfg(not(target_arch = "wasm32"))]
async fn commit_metadata_session(store: &dyn meerkat_runtime::RuntimeStore, session: &Session) {
    store
        .commit_session_snapshot(
            &meerkat_runtime::LogicalRuntimeId::for_session(session.id()),
            meerkat_runtime::SerializedSessionSnapshot {
                session_snapshot: serde_json::to_vec(session).unwrap().into(),
            },
        )
        .await
        .expect("commit authoritative test session");
}

#[cfg(not(target_arch = "wasm32"))]
#[tokio::test]
async fn retained_metadata_active_session_matches_ordinary_metadata() {
    let (service, _, runtime_store) = persistent_metadata_fixture();
    let session = factory_policy_session(Session::new(), "active-model".into(), 1024);
    commit_metadata_session(runtime_store.as_ref(), &session).await;
    let ordinary = service
        .load_persisted_session_metadata(session.id())
        .await
        .unwrap()
        .unwrap();
    let retained = service
        .load_retained_session_metadata(session.id())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(retained.session_id, ordinary.session_id);
    assert_eq!(retained.lifecycle_terminal, ordinary.lifecycle_terminal);
    assert_eq!(
        serde_json::to_value(retained.session_metadata).unwrap(),
        serde_json::to_value(ordinary.session_metadata).unwrap()
    );
}

#[cfg(not(target_arch = "wasm32"))]
#[tokio::test]
async fn retained_metadata_reads_archived_exact_session_without_changing_visibility() {
    let (service, _, runtime_store) = persistent_metadata_fixture();
    let binding = meerkat_core::MobMemberBinding {
        mob_id: "historical-mob".into(),
        role: "worker".into(),
        member: "member".into(),
    };
    let policy =
        meerkat_core::ops::ToolAccessPolicy::AllowList(["retained_read"].into_iter().collect());
    let mut archived = factory_policy_session(Session::new(), "old-model".into(), 1024);
    let mut metadata = archived.session_metadata().unwrap();
    metadata.mob_member_binding = Some(binding.clone());
    metadata.tooling.tool_access_policy = Some(policy.clone());
    archived.set_session_metadata(metadata).unwrap();
    archived
        .set_lifecycle_terminal(meerkat_core::SessionLifecycleTerminal::Archived)
        .unwrap();
    commit_metadata_session(runtime_store.as_ref(), &archived).await;

    let mut successor = factory_policy_session(Session::new(), "new-model".into(), 2048);
    let mut metadata = successor.session_metadata().unwrap();
    metadata.mob_member_binding = Some(binding.clone());
    successor.set_session_metadata(metadata).unwrap();
    commit_metadata_session(runtime_store.as_ref(), &successor).await;

    let retained = service
        .load_retained_session_metadata(archived.id())
        .await
        .unwrap()
        .expect("archived metadata remains available as historical evidence");
    assert_eq!(&retained.session_id, archived.id());
    assert_eq!(retained.mob_member_binding(), Some(&binding));
    let metadata = retained.session_metadata.unwrap();
    assert_eq!(metadata.model, "old-model");
    assert_eq!(metadata.tooling.tool_access_policy, Some(policy));
    assert!(
        service
            .load_persisted_session_metadata(archived.id())
            .await
            .unwrap()
            .is_none(),
        "historical observation must not change ordinary metadata visibility"
    );
    assert!(
        service
            .load_persisted_session(archived.id())
            .await
            .unwrap()
            .is_none(),
        "historical observation must not make the archived document ordinarily readable"
    );
    let current = service
        .load_retained_session_metadata(successor.id())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&current.session_id, successor.id());
    assert_eq!(current.session_metadata.unwrap().model, "new-model");
}

#[cfg(not(target_arch = "wasm32"))]
#[tokio::test]
async fn retained_metadata_absence_does_not_fall_back_to_session_projection() {
    let (service, session_store, _) = persistent_metadata_fixture();
    let session = factory_policy_session(Session::new(), "projection-only".into(), 1024);
    assert!(
        service
            .load_retained_session_metadata(session.id())
            .await
            .unwrap()
            .is_none()
    );
    session_store.save(&session).await.unwrap();
    assert!(
        service
            .load_retained_session_metadata(session.id())
            .await
            .unwrap()
            .is_none(),
        "a compatibility projection cannot replace absent runtime authority"
    );
}

#[cfg(not(target_arch = "wasm32"))]
#[tokio::test]
async fn retained_metadata_preserves_corrupt_metadata_faults() {
    let (service, _, runtime_store) = persistent_metadata_fixture();
    for archived in [false, true] {
        let mut session = Session::new();
        if archived {
            session
                .set_lifecycle_terminal(meerkat_core::SessionLifecycleTerminal::Archived)
                .unwrap();
        }
        let mut corrupt = serde_json::to_value(&session).unwrap();
        corrupt["metadata"][meerkat_core::session::SESSION_METADATA_KEY] =
            serde_json::json!("invalid-typed-session-metadata");
        runtime_store
            .commit_session_snapshot(
                &meerkat_runtime::LogicalRuntimeId::for_session(session.id()),
                meerkat_runtime::SerializedSessionSnapshot {
                    session_snapshot: serde_json::to_vec(&corrupt).unwrap().into(),
                },
            )
            .await
            .expect("commit valid envelope containing malformed typed metadata");
        let retained_error = service
            .load_retained_session_metadata(session.id())
            .await
            .expect_err("corrupt history remains a read fault");
        let ordinary_error = service
            .load_persisted_session_metadata(session.id())
            .await
            .expect_err("ordinary metadata also refuses the corrupt document");
        assert!(
            retained_error
                .to_string()
                .contains("durable metadata failed typed restore")
        );
        assert_eq!(retained_error.to_string(), ordinary_error.to_string());
    }
}

#[tokio::test]
async fn retained_metadata_default_is_unsupported_for_nonpersistent_service() {
    let service = meerkat_session::EphemeralSessionService::new(PersistentMockBuilder, 4);
    let created = SessionService::create_session(
        &service,
        CreateSessionRequest {
            injected_context: Vec::new(),
            model: "retained-ephemeral".into(),
            prompt: ContentInput::Text("metadata only".into()),
            system_prompt: meerkat_core::SystemPromptOverride::Inherit,
            max_tokens: None,
            event_tx: None,
            initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
            deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
            build: None,
            labels: None,
        },
    )
    .await
    .unwrap();
    assert!(
        service
            .load_persisted_session_metadata(&created.session_id)
            .await
            .unwrap()
            .is_some(),
        "fixture must have ordinary live metadata to detect an unsafe fallback"
    );
    for session_id in [created.session_id, SessionId::new()] {
        let error = service
            .load_retained_session_metadata(&session_id)
            .await
            .expect_err("a backend without retained metadata authority must report unsupported");
        assert!(matches!(error, SessionError::Unsupported(_)));
    }
}
