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

#[cfg(not(target_arch = "wasm32"))]
fn persistent_factory_metadata_fixture(
    path: &std::path::Path,
    head_canonical: bool,
) -> (
    meerkat_session::PersistentSessionService<meerkat::FactoryAgentBuilder>,
    Arc<dyn meerkat_runtime::RuntimeStore>,
    HeadCanonicalQueueGateClient,
) {
    let sessions: Arc<dyn SessionStore> =
        Arc::new(meerkat_store::SqliteSessionStore::open(path).expect("open session projection"));
    let runtime: Arc<dyn meerkat_runtime::RuntimeStore> = Arc::new(
        if head_canonical {
            meerkat_runtime::SqliteRuntimeStore::new_head_canonical(path)
        } else {
            meerkat_runtime::SqliteRuntimeStore::new_whole_blob(path)
        }
        .expect("open native runtime authority"),
    );
    let root = path.parent().expect("fixture database parent");
    for name in ["store", "user", "runtime", "project", "context"] {
        std::fs::create_dir_all(root.join(name)).expect("isolated factory root");
    }
    let factory = meerkat::AgentFactory::new(root.join("store"))
        .user_config_root(root.join("user"))
        .runtime_root(root.join("runtime"))
        .project_root(root.join("project"))
        .context_root(root.join("context"))
        .builtins(false)
        .shell(false)
        .comms(false);
    let mut builder = meerkat::FactoryAgentBuilder::new(factory, meerkat::Config::default());
    builder.default_session_store =
        Some(Arc::new(meerkat_store::StoreAdapter::new(sessions.clone())));
    let client = HeadCanonicalQueueGateClient::new();
    client.release();
    builder.default_llm_client = Some(Arc::new(client.clone()));
    (
        meerkat_session::PersistentSessionService::new(
            builder,
            4,
            sessions,
            runtime.clone(),
            Arc::new(meerkat_store::MemoryBlobStore::new()),
        ),
        runtime,
        client,
    )
}

#[cfg(not(target_arch = "wasm32"))]
#[tokio::test]
async fn retained_metadata_retirement_then_sqlite_reopen_preserves_exact_source() {
    for head_canonical in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("retained.sqlite3");
        let binding = meerkat_core::MobMemberBinding {
            mob_id: "retained-reopen".into(),
            role: "worker".into(),
            member: "same-logical-member".into(),
        };
        let (source_id, successor_id) = {
            let (service, runtime, client) =
                persistent_factory_metadata_fixture(&path, head_canonical);
            let machine = meerkat_runtime::MeerkatMachine::persistent(
                runtime,
                Arc::new(meerkat_store::MemoryBlobStore::new()),
            );
            let mut created = Vec::new();
            for policy in [Some(meerkat_core::ops::ToolAccessPolicy::ReadOnly), None] {
                let result = service
                    .create_session(CreateSessionRequest {
                        injected_context: Vec::new(),
                        model: "gpt-5.5".into(),
                        prompt: ContentInput::Text("no provider turn needed".into()),
                        system_prompt: meerkat_core::SystemPromptOverride::Inherit,
                        max_tokens: None,
                        event_tx: None,
                        initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
                        deferred_prompt_policy:
                            meerkat_core::service::DeferredPromptPolicy::Discard,
                        build: Some(meerkat_core::service::SessionBuildOptions {
                            mob_member_binding: Some(binding.clone()),
                            tool_access_policy: policy,
                            ..Default::default()
                        }),
                        labels: None,
                    })
                    .await
                    .expect("create source or later same-binding session");
                created.push(result.session_id);
            }
            service
                .archive_with_machine_protocol(
                    &created[0],
                    meerkat_session::MachineSessionArchiveProtocol::from_machine(&machine),
                )
                .await
                .expect("retire source through native archive authority");
            assert!(
                service
                    .load_persisted_session_metadata(&created[0])
                    .await
                    .unwrap()
                    .is_none()
            );
            assert_eq!(
                client.request_count(),
                0,
                "deferred fixture makes no LLM calls"
            );
            MobSessionService::cancel_all_checkpointers(&service).await;
            (created.remove(0), created.remove(0))
        };

        // The original service, machine and store handles have left scope.
        let (reopened, _, _) = persistent_factory_metadata_fixture(&path, head_canonical);
        let retained = reopened
            .load_retained_session_metadata(&source_id)
            .await
            .unwrap()
            .expect("retired exact source survives reopening SQLite");
        assert_eq!(retained.session_id, source_id);
        assert_eq!(retained.mob_member_binding(), Some(&binding));
        assert_eq!(
            retained
                .session_metadata
                .unwrap()
                .tooling
                .tool_access_policy,
            Some(meerkat_core::ops::ToolAccessPolicy::ReadOnly),
            "a later unrestricted session must not replace the retired source policy"
        );
        assert!(
            reopened
                .load_persisted_session_metadata(&source_id)
                .await
                .unwrap()
                .is_none()
        );
        let successor = reopened
            .load_retained_session_metadata(&successor_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(successor.session_id, successor_id);
        assert_eq!(
            successor
                .session_metadata
                .unwrap()
                .tooling
                .tool_access_policy,
            None
        );
        assert!(
            reopened
                .load_retained_session_metadata(&SessionId::new())
                .await
                .unwrap()
                .is_none()
        );
    }
}

#[cfg(not(target_arch = "wasm32"))]
struct RetainedPolicyBundle {
    dispatched: Arc<Mutex<Vec<String>>>,
}

#[cfg(not(target_arch = "wasm32"))]
#[async_trait]
impl AgentToolDispatcher for RetainedPolicyBundle {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        ["retained_read", "retained_edit"]
            .into_iter()
            .map(|name| {
                Arc::new(ToolDef {
                    name: name.into(),
                    description: "Native policy parity probe".into(),
                    input_schema: serde_json::json!({"type": "object"}),
                    provenance: None,
                })
            })
            .collect::<Vec<_>>()
            .into()
    }

    fn tool_mutation_class(&self, name: &str) -> meerkat_core::ToolMutationClass {
        match name {
            "retained_read" => meerkat_core::ToolMutationClass::ReadOnly,
            "retained_edit" => meerkat_core::ToolMutationClass::Mutating,
            _ => meerkat_core::ToolMutationClass::Unknown,
        }
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        if !matches!(call.name, "retained_read" | "retained_edit") {
            return Err(ToolError::not_found(call.name));
        }
        self.dispatched.lock().unwrap().push(call.name.into());
        Ok(ToolResult::new(call.id.into(), "probe admitted".into(), false).into())
    }
}

#[cfg(not(target_arch = "wasm32"))]
async fn assert_retained_profile_policy_matches_native_gate(read_only: bool) {
    use meerkat_core::{ToolExecutionPolicy, ToolMutationClass};

    for head_canonical in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("profile-policy.sqlite3");
        let (source_id, expected_policy) = {
            let (service, _, client) = persistent_factory_metadata_fixture(&path, head_canonical);
            let service = Arc::new(service);
            let dispatched = Arc::new(Mutex::new(Vec::new()));
            let bundle = Arc::new(RetainedPolicyBundle {
                dispatched: dispatched.clone(),
            });
            let mut definition = with_unique_mob_id(
                sample_definition_with_tool_bundle("retained-policy"),
                "retained-profile-policy",
            );
            let profile = definition
                .profiles
                .get_mut(&ProfileName::from("worker"))
                .and_then(ProfileBinding::as_inline_mut)
                .unwrap();
            profile.model = "gpt-5.5".into();
            profile.runtime_mode = crate::MobRuntimeMode::TurnDriven;
            profile.tools.read_only = read_only;
            if !read_only {
                // The name belongs to the registered profile bundle, so this
                // passes the same declared vocabulary validation as production.
                profile.tools.deny = vec!["retained_edit".into()];
            }
            let handle = MobBuilder::new(definition, MobStorage::in_memory())
                .with_session_service(service.clone())
                .register_tool_bundle("retained-policy", bundle)
                .create()
                .await
                .unwrap();
            let identity = AgentIdentity::from("profile-source");
            let source_id = handle
                .spawn(ProfileName::from("worker"), identity.clone(), None)
                .await
                .unwrap()
                .bridge_session_id()
                .unwrap()
                .clone();
            let metadata = service
                .load_retained_session_metadata(&source_id)
                .await
                .unwrap()
                .unwrap()
                .session_metadata
                .unwrap();
            let effective = metadata
                .tooling
                .tool_access_policy
                .expect("profile-only restriction is persisted in the effective field");
            let expected_policy = ToolExecutionPolicy::resolve(effective.clone()).unwrap();
            assert!(expected_policy.permits_call("retained_read", ToolMutationClass::ReadOnly));
            assert!(!expected_policy.permits_call("retained_edit", ToolMutationClass::Mutating));
            assert_eq!(
                metadata.tooling.spawn_tool_access_policy,
                Some(meerkat_core::ops::SpawnToolAccessPolicy::Unrestricted),
                "no launch restriction was requested"
            );
            let launch_only = metadata
                .tooling
                .spawn_tool_access_policy
                .unwrap()
                .into_launch()
                .map(ToolExecutionPolicy::resolve)
                .transpose()
                .unwrap()
                .unwrap_or_else(ToolExecutionPolicy::unrestricted);
            assert!(
                launch_only.permits_call("retained_edit", ToolMutationClass::Mutating),
                "negative control: the separate spawn field would over-grant edit"
            );
            for (name, class) in [
                ("retained_read", ToolMutationClass::ReadOnly),
                ("retained_edit", ToolMutationClass::Mutating),
            ] {
                let outcome = service
                    .dispatch_external_tool_call(
                        &source_id,
                        meerkat_core::ToolCall::new(
                            name.into(),
                            name.into(),
                            serde_json::json!({}),
                        ),
                    )
                    .await
                    .unwrap();
                assert_eq!(
                    !outcome.result.is_error,
                    expected_policy.permits_call(name, class),
                    "retained effective policy must match the actual native gate for {name}"
                );
                if outcome.result.is_error {
                    assert!(
                        outcome
                            .result
                            .text_content()
                            .contains("\"error\":\"access_denied\"")
                    );
                }
            }
            assert_eq!(*dispatched.lock().unwrap(), ["retained_read"]);
            handle.retire(identity).await.unwrap();
            assert!(
                service
                    .load_persisted_session_metadata(&source_id)
                    .await
                    .unwrap()
                    .is_none()
            );
            handle.shutdown().await.unwrap();
            assert_eq!(client.request_count(), 0);
            MobSessionService::cancel_all_checkpointers(service.as_ref()).await;
            (source_id, effective)
        };
        let (reopened, _, _) = persistent_factory_metadata_fixture(&path, head_canonical);
        let retained = reopened
            .load_retained_session_metadata(&source_id)
            .await
            .unwrap()
            .expect("retired profile policy survives cold SQLite reopen");
        assert_eq!(retained.session_id, source_id);
        assert_eq!(
            retained
                .session_metadata
                .unwrap()
                .tooling
                .tool_access_policy,
            Some(expected_policy)
        );
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[tokio::test]
async fn retained_metadata_profile_read_only_matches_native_gate_after_retirement() {
    assert_retained_profile_policy_matches_native_gate(true).await;
}

#[cfg(not(target_arch = "wasm32"))]
#[tokio::test]
async fn retained_metadata_profile_deny_matches_native_gate_after_retirement() {
    assert_retained_profile_policy_matches_native_gate(false).await;
}
