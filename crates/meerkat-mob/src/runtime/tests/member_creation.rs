use super::*;
use crate::MemberCreationProvenance;

#[tokio::test]
async fn member_creation_host_root_is_explicit_and_agent_lane_downgrades_it() {
    let (handle, _) = create_test_mob(sample_definition()).await;
    for (spec, agent_lane, expected) in [
        (
            SpawnMemberSpec::new("worker", "default-origin"),
            false,
            MemberCreationProvenance::Unproven,
        ),
        (
            SpawnMemberSpec::host_root("worker", "host-origin"),
            false,
            MemberCreationProvenance::Root,
        ),
        (
            SpawnMemberSpec::host_root("worker", "agent-origin"),
            true,
            MemberCreationProvenance::Unproven,
        ),
    ] {
        let caller = if agent_lane {
            handle
                .clone()
                .with_command_authority(crate::control_policy::CommandAuthority::agent_lane())
        } else {
            handle.clone()
        };
        let result = caller.spawn_spec(spec).await.unwrap();
        let session = handle
            .resolve_bridge_session_id(&result.agent_identity)
            .await
            .unwrap();
        assert_eq!(
            handle
                .member_creation_for_session(&session)
                .await
                .unwrap()
                .unwrap()
                .creation
                .provenance,
            expected,
        );
    }
    handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn member_creation_policy_auto_spawn_has_no_attested_host_origin() {
    let target = AgentIdentity::from("policy-origin");
    let (handle, _) = create_test_mob(sample_definition_with_static_spawn_policy(
        target.as_str(),
        "lead",
    ))
    .await;
    handle
        .submit_work(
            AgentRuntimeId::initial(target.clone()),
            FenceToken::new(0),
            WorkRef::new(),
            WorkSpec::new("policy origin".to_string(), WorkOrigin::External),
        )
        .await
        .unwrap();
    let session = handle.resolve_bridge_session_id(&target).await.unwrap();
    assert_eq!(
        handle
            .member_creation_for_session(&session)
            .await
            .unwrap()
            .unwrap()
            .creation
            .provenance,
        MemberCreationProvenance::Unproven,
    );
    handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn member_creation_operator_ingress_proves_owner_or_records_unproven() {
    for (bind_owner, fail_metadata) in [(false, false), (true, false), (true, true)] {
        let (handle, service) = create_test_mob(sample_definition_with_mob_tools()).await;
        let parent = AgentIdentity::from("operator-parent");
        handle
            .spawn(ProfileName::from("worker"), parent.clone(), None)
            .await
            .unwrap();
        let parent_session = handle.resolve_bridge_session_id(&parent).await.unwrap();
        let dispatcher: Arc<dyn AgentToolDispatcher> =
            Arc::new(super::super::tools::MobOperatorToolDispatcher::new(
                handle.clone(),
                true,
                generated_mob_operator_authority_with_scope(handle.mob_id().as_str()),
            ));
        let dispatcher = if bind_owner {
            match dispatcher
                .bind_ops_lifecycle(
                    Arc::new(meerkat_runtime::ops_lifecycle::RuntimeOpsLifecycleRegistry::new()),
                    parent_session.clone(),
                )
                .unwrap()
            {
                meerkat_core::agent::BindOutcome::Bound(bound)
                | meerkat_core::agent::BindOutcome::Skipped(bound) => bound,
            }
        } else {
            dispatcher
        };
        if fail_metadata {
            service.fail_persisted_session_metadata_reads_for(parent_session.clone());
        }
        for (tool, identity, args) in [
            (
                "spawn_member",
                "single",
                serde_json::json!({"profile":"worker", "member_id":"single"}),
            ),
            (
                "spawn_many_members",
                "batch",
                serde_json::json!({"specs":[{"profile":"worker", "member_id":"batch"}]}),
            ),
        ] {
            let args = serde_json::value::RawValue::from_string(args.to_string()).unwrap();
            dispatcher
                .dispatch(ToolCallView {
                    id: "creation-ingress",
                    name: tool,
                    args: &args,
                })
                .await
                .unwrap();
            let session = handle
                .resolve_bridge_session_id(&AgentIdentity::from(identity))
                .await
                .unwrap();
            let proof = handle
                .member_creation_for_session(&session)
                .await
                .unwrap()
                .unwrap();
            if bind_owner && !fail_metadata {
                assert!(
                    matches!(proof.creation.provenance, MemberCreationProvenance::Spawn { source }
                    if source.session_id == parent_session)
                );
            } else {
                assert_eq!(
                    proof.creation.provenance,
                    MemberCreationProvenance::Unproven
                );
            }
        }
        service.metadata_read_failures_for.lock().unwrap().clear();
        handle.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn member_creation_public_roster_snapshot_cannot_mutate_live_history() {
    let (handle, _) = create_test_mob(sample_definition()).await;
    let identity = AgentIdentity::from("snapshot-member");
    handle
        .spawn(ProfileName::from("worker"), identity.clone(), None)
        .await
        .unwrap();
    let session = handle.resolve_bridge_session_id(&identity).await.unwrap();
    let before = handle
        .member_creation_for_session(&session)
        .await
        .unwrap()
        .unwrap();
    let mut event = handle.events.replay_all().await.unwrap().into_iter()
        .find(|event| matches!(&event.kind, MobEventKind::MemberSpawned(spawned) if spawned.agent_identity == identity)).unwrap();
    if let MobEventKind::MemberSpawned(spawned) = &mut event.kind {
        spawned.creation.creation_id = Some(crate::MemberCreationId::new());
    }
    let mut public = handle.roster().await;
    public.apply(&event);
    assert_eq!(
        handle.member_creation_for_session(&session).await.unwrap(),
        Some(before)
    );
    handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn member_creation_source_metadata_fault_remains_an_error() {
    let (handle, service) = create_test_mob(sample_definition()).await;
    let member = spawn_settled_fork_source(&handle, &AgentIdentity::from("fault-source")).await;
    let session = member.bridge_session_id().unwrap();
    service.fail_persisted_session_metadata_reads_for(session.clone());
    assert!(matches!(
        handle.capture_member_creation_source(session).await,
        Err(crate::MemberCreationError::Session(_))
    ));
    let fork = handle
        .fork_member(
            &AgentIdentity::from("fault-source"),
            SpawnMemberSpec::new("worker", "fault-child"),
            None,
        )
        .await
        .expect("optional creation proof must not change ordinary fork admission");
    assert_eq!(
        handle
            .member_creation_for_session(&fork.session_id)
            .await
            .unwrap()
            .unwrap()
            .creation
            .provenance,
        MemberCreationProvenance::Unproven,
    );
    service.metadata_read_failures_for.lock().unwrap().clear();
    handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn member_creation_caller_turn_fork_retains_proof_after_child_completion() {
    let (handle, service) = create_test_mob(sample_definition_with_mob_tools()).await;
    service.set_return_exact_run_result(true);
    let source = AgentIdentity::from("creation-fork-off-parent");
    spawn_bounded_fork_source(&handle, &source).await;
    let session = handle.resolve_bridge_session_id(&source).await.unwrap();
    let parent = handle
        .member_creation_for_session(&session)
        .await
        .unwrap()
        .unwrap();
    let (fork, run) = handle
        .fork_member_then_run_detached(
            &source,
            bounded_fork_child_spec(&AgentIdentity::from("creation-fork-off-child")),
            None,
            "result",
            256,
            meerkat_core::DurableForkSourceAdmission::CallerTurn,
            None,
            None,
        )
        .await
        .unwrap();
    run.outcome().await.unwrap();
    let child = handle
        .member_creation_for_session(&fork.session_id)
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(child.creation.provenance, MemberCreationProvenance::Fork { source_creation_id, .. }
        if Some(source_creation_id) == parent.creation.creation_id)
    );
    assert_eq!(child.fork_source.unwrap().source_session_id, session);
    handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn member_creation_unproven_boundary_and_context_forks_are_not_roots() {
    let (handle, _) = create_test_mob(sample_definition()).await;
    let source = AgentIdentity::from("creation-unproven-source");
    handle
        .spawn(ProfileName::from("worker"), source.clone(), None)
        .await
        .unwrap();
    let outcome = handle
        .fork_member_at_turn_boundary(
            &source,
            SpawnMemberSpec::new(
                ProfileName::from("worker"),
                AgentIdentity::from("boundary-child"),
            ),
            None,
            Duration::from_secs(1),
        )
        .await
        .unwrap();
    let ForkMemberAtTurnBoundary::Forked(fork) = outcome else {
        panic!("idle source must fork")
    };
    let boundary = handle
        .member_creation_for_session(&fork.session_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        boundary.creation.provenance,
        MemberCreationProvenance::Unproven
    );
    assert!(boundary.fork_source.is_none());
    let context = handle
        .spawn_spec(
            SpawnMemberSpec::new(
                ProfileName::from("worker"),
                AgentIdentity::from("context-child"),
            )
            .with_launch_mode(crate::MemberLaunchMode::Fork {
                source_member_id: source,
                fork_context: crate::ForkContext::FullHistory,
            }),
        )
        .await
        .unwrap();
    let context = handle
        .member_creation_for_session(
            &handle
                .resolve_bridge_session_id(&context.agent_identity)
                .await
                .unwrap(),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        context.creation.provenance,
        MemberCreationProvenance::Unproven
    );
    handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn member_creation_runtime_fork_spawn_and_respawn_keep_exact_authority() {
    let (handle, _) = create_test_mob(sample_definition()).await;
    let identity = AgentIdentity::from("creation-parent");
    let mut parent_spec = SpawnMemberSpec::host_root(ProfileName::from("worker"), identity.clone());
    parent_spec.runtime_mode = Some(crate::MobRuntimeMode::TurnDriven);
    parent_spec.tool_access_policy = Some(meerkat_core::ops::ToolAccessPolicy::ReadOnly);
    let parent = handle.spawn_spec(parent_spec).await.unwrap();
    let parent_session = handle
        .resolve_bridge_session_id(&parent.agent_identity)
        .await
        .unwrap();
    wait_for_fork_source_settled(&handle, &parent_session).await;
    let parent_proof = handle
        .member_creation_for_session(&parent_session)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        parent_proof.creation.provenance,
        MemberCreationProvenance::Root
    );

    let witness = handle
        .capture_member_creation_source(&parent_session)
        .await
        .unwrap();
    let child_identity = AgentIdentity::from("creation-spawn");
    let child = handle
        .spawn_spec(
            SpawnMemberSpec::new(ProfileName::from("worker"), child_identity.clone())
                .with_creation_source(witness),
        )
        .await
        .unwrap();
    let child_session = handle
        .resolve_bridge_session_id(&child.agent_identity)
        .await
        .unwrap();
    let child_proof = handle
        .member_creation_for_session(&child_session)
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(&child_proof.creation.provenance, MemberCreationProvenance::Spawn { source }
                if source.session_id == parent_session && Some(source.creation_id) == parent_proof.creation.creation_id
        )
    );
    assert!(child_proof.fork_source.is_none());

    let fork_identity = AgentIdentity::from("creation-fork");
    let fork = handle
        .fork_member(
            &identity,
            SpawnMemberSpec::new(ProfileName::from("worker"), fork_identity.clone()),
            None,
        )
        .await
        .unwrap();
    let fork_proof = handle
        .member_creation_for_session(&fork.session_id)
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(fork_proof.creation.provenance, MemberCreationProvenance::Fork {
        source_creation_id,
    } if Some(source_creation_id) == parent_proof.creation.creation_id)
    );
    assert_eq!(
        fork_proof.fork_source.unwrap().source_session_id,
        parent_session
    );

    handle.respawn(fork_identity.clone(), None).await.unwrap();
    let replacement_session = handle
        .get_member(&fork_identity)
        .await
        .unwrap()
        .unwrap()
        .bridge_session_id()
        .unwrap()
        .clone();
    let replacement = handle
        .member_creation_for_session(&replacement_session)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        replacement.creation.creation_id,
        fork_proof.creation.creation_id
    );
    assert_eq!(replacement.birth_cursor, fork_proof.birth_cursor);
    assert!(
        matches!(replacement.creation.provenance, MemberCreationProvenance::Successor { predecessor_session_id, .. }
        if predecessor_session_id == fork.session_id)
    );
    assert!(
        replacement.fork_source.is_none(),
        "respawn keeps fresh-context build semantics"
    );
    assert!(
        handle
            .member_creation_for_session(&fork.session_id)
            .await
            .unwrap()
            .unwrap()
            .fork_source
            .is_some()
    );

    handle.retire(identity.clone()).await.unwrap();
    assert_eq!(
        handle
            .member_creation_for_session(&parent_session)
            .await
            .unwrap()
            .unwrap(),
        parent_proof
    );
    let reused = handle
        .spawn(ProfileName::from("worker"), identity, None)
        .await
        .unwrap();
    let reused_proof = handle
        .member_creation_for_session(reused.bridge_session_id().unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_ne!(
        reused_proof.creation.creation_id,
        parent_proof.creation.creation_id
    );
    assert!(matches!(
        handle.capture_member_creation_source(&parent_session).await,
        Err(crate::MemberCreationError::Unavailable(_))
    ));
    handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn member_creation_cross_mob_delegate_has_source_proof_without_comms() {
    let (source, _) =
        create_test_mob(with_unique_mob_id(sample_definition(), "creation-source")).await;
    let mut target_definition = with_unique_mob_id(sample_definition(), "creation-target");
    target_definition.profiles.insert(
        ProfileName::from("delegate"),
        target_definition.profiles[&ProfileName::from("worker")].clone(),
    );
    let (target, target_service) = create_test_mob(target_definition).await;
    target_service.set_return_exact_run_result(true);
    let parent = source
        .spawn(
            ProfileName::from("worker"),
            AgentIdentity::from("same-name"),
            None,
        )
        .await
        .unwrap();
    let parent_session = parent.bridge_session_id().unwrap().clone();
    let mut request = DelegationExecutionRequest::new(
        AgentIdentity::from("same-name"),
        "report",
        BoundedResultSpec::new("proof", 256).unwrap(),
    );
    request.member.creation_source = Some(
        source
            .capture_member_creation_source(&parent_session)
            .await
            .unwrap(),
    );
    let execution = DelegationExecutionService::new(target.clone())
        .execute(request)
        .await
        .unwrap();
    let child_session = execution.turn().result().session_id();
    let proof = target
        .member_creation_for_session(child_session)
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(proof.creation.provenance, MemberCreationProvenance::Spawn { source: parent }
        if parent.session_id == parent_session && parent.member_binding.mob_id == source.mob_id().as_str())
    );
    assert!(
        target
            .get_member(&AgentIdentity::from("same-name"))
            .await
            .unwrap()
            .is_none(),
        "delegate retired, proof retained"
    );
    source.shutdown().await.unwrap();
    target.shutdown().await.unwrap();
}

#[tokio::test]
async fn member_creation_cold_resume_projects_recovered_binding_immediately() {
    let service = Arc::new(MockSessionService::new());
    let _ = service.enable_runtime_adapter();
    let storage = MobStorage::in_memory();
    let definition = with_unique_mob_id(sample_definition(), "creation-recovery");
    let mob_id = definition.id.clone();
    let handle = MobBuilder::new(definition, storage.clone())
        .with_session_service(service.clone())
        .create()
        .await
        .unwrap();
    let identity = AgentIdentity::from("recovered-worker");
    let member = handle
        .spawn(ProfileName::from("worker"), identity.clone(), None)
        .await
        .unwrap();
    let original_session = member.bridge_session_id().unwrap().clone();
    let original = handle
        .member_creation_for_session(&original_session)
        .await
        .unwrap()
        .unwrap();
    crash_stop_and_release_routes(handle).await;
    MobSessionService::discard_live_session(service.as_ref(), &original_session)
        .await
        .unwrap();
    service.delete_persisted_session(&original_session).await;
    let replacement = service
        .create_session(CreateSessionRequest {
            injected_context: Vec::new(),
            model: "claude-sonnet-4-5".into(),
            prompt: "recovered head".into(),
            system_prompt: meerkat_core::SystemPromptOverride::Inherit,
            max_tokens: None,
            event_tx: None,
            build: Some(meerkat_core::service::SessionBuildOptions {
                comms_name: Some(format!("{mob_id}/worker/{identity}")),
                mob_member_binding: Some(original.member_binding.clone()),
                ..Default::default()
            }),
            initial_turn: meerkat_core::service::InitialTurnPolicy::RunImmediately,
            deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
            labels: None,
        })
        .await
        .unwrap();
    MobSessionService::discard_live_session(service.as_ref(), &replacement.session_id)
        .await
        .unwrap();
    let restored_service = Arc::new(PersistedListingSessionService::new(service));
    let retained = Arc::new(RwLock::new(None));
    let observed = Arc::clone(&retained);
    let hook: MobBeforeActivation = Arc::new(move |handle| {
        let observed = Arc::clone(&observed);
        Box::pin(async move {
            *observed.write().await = Some(handle);
            Ok(())
        })
    });
    let resumed = MobBuilder::for_resume(storage)
        .with_session_service(restored_service)
        .before_activation(hook)
        .notify_orchestrator_on_resume(false)
        .resume()
        .await
        .unwrap();
    let preview = retained.read().await.clone().unwrap();
    for (label, handle) in [
        ("final", resumed.read_handle()),
        ("retained preview", preview),
    ] {
        assert_eq!(
            handle
                .get_member(&identity)
                .await
                .unwrap()
                .unwrap()
                .bridge_session_id(),
            Some(&replacement.session_id),
            "{label} handle must observe the recovered exact session"
        );
        let recovered = handle
            .member_creation_for_session(&replacement.session_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            recovered.creation.creation_id,
            original.creation.creation_id
        );
        assert_eq!(recovered.birth_cursor, original.birth_cursor);
        assert!(matches!(
            recovered.creation.provenance,
            MemberCreationProvenance::Successor { predecessor_session_id, .. }
                if predecessor_session_id == original_session
        ));
    }
    resumed.shutdown().await.unwrap();
}

#[cfg(not(target_arch = "wasm32"))]
#[tokio::test]
async fn member_creation_sqlite_restart_retains_retired_parent_and_child_tokens() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("creation.sqlite");
    let service = Arc::new(MockSessionService::new());
    let _ = service.enable_runtime_adapter();
    let handle = MobBuilder::new(
        with_unique_mob_id(sample_definition(), "creation-restart"),
        MobStorage::persistent(&path).unwrap(),
    )
    .with_session_service(service.clone())
    .create()
    .await
    .unwrap();
    let parent_id = AgentIdentity::from("historical-parent");
    let parent = handle
        .spawn(ProfileName::from("worker"), parent_id.clone(), None)
        .await
        .unwrap();
    let parent_session = parent.bridge_session_id().unwrap().clone();
    let witness = handle
        .capture_member_creation_source(&parent_session)
        .await
        .unwrap();
    let child = handle
        .spawn_spec(
            SpawnMemberSpec::new(
                ProfileName::from("worker"),
                AgentIdentity::from("surviving-child"),
            )
            .with_creation_source(witness),
        )
        .await
        .unwrap();
    let child_session = handle
        .resolve_bridge_session_id(&child.agent_identity)
        .await
        .unwrap();
    let parent_before = handle
        .member_creation_for_session(&parent_session)
        .await
        .unwrap()
        .unwrap();
    let child_before = handle
        .member_creation_for_session(&child_session)
        .await
        .unwrap()
        .unwrap();
    handle.retire(parent_id).await.unwrap();
    crash_stop_and_release_routes(handle).await;
    let resumed = MobBuilder::for_resume(MobStorage::persistent(&path).unwrap())
        .with_session_service(service)
        .notify_orchestrator_on_resume(false)
        .resume()
        .await
        .unwrap();
    assert_eq!(
        resumed
            .member_creation_for_session(&parent_session)
            .await
            .unwrap()
            .unwrap(),
        parent_before
    );
    assert_eq!(
        resumed
            .member_creation_for_session(&child_session)
            .await
            .unwrap()
            .unwrap(),
        child_before
    );
    resumed.shutdown().await.unwrap();
}
