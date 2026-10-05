use super::*;
use crate::MemberCreationProvenance;

#[tokio::test]
async fn member_creation_source_metadata_fault_remains_an_error() {
    let (handle, service) = create_test_mob(sample_definition()).await;
    let member = handle
        .spawn(
            ProfileName::from("worker"),
            AgentIdentity::from("fault-source"),
            None,
        )
        .await
        .unwrap();
    let session = member.bridge_session_id().unwrap();
    service.fail_persisted_session_metadata_reads_for(session.clone());
    assert!(matches!(
        handle.capture_member_creation_source(session).await,
        Err(crate::MemberCreationError::Session(_))
    ));
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
        MemberCreationProvenance::LegacyUnknown
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
        .member_creation_for_session(context.bridge_session_id().unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        context.creation.provenance,
        MemberCreationProvenance::LegacyUnknown
    );
    handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn member_creation_runtime_fork_spawn_and_respawn_keep_exact_authority() {
    let (handle, _) = create_test_mob(sample_definition()).await;
    let identity = AgentIdentity::from("creation-parent");
    let mut parent_spec = SpawnMemberSpec::new(ProfileName::from("worker"), identity.clone());
    parent_spec.tool_access_policy = Some(meerkat_core::ops::ToolAccessPolicy::ReadOnly);
    let parent = handle.spawn_spec(parent_spec).await.unwrap();
    let parent_session = parent.bridge_session_id().unwrap().clone();
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
    let child_session = child.bridge_session_id().unwrap().clone();
    let child_proof = handle
        .member_creation_for_session(&child_session)
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(&child_proof.creation.provenance, MemberCreationProvenance::Spawn { source }
        if source.session_id == parent_session && Some(source.creation_id) == parent_proof.creation.creation_id
        && source.tool_access_policy == Some(meerkat_core::ops::ToolAccessPolicy::ReadOnly))
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
        source_creation_id, source_tool_access_policy: Some(meerkat_core::ops::ToolAccessPolicy::ReadOnly),
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
    let (target, target_service) =
        create_test_mob(with_unique_mob_id(sample_definition(), "creation-target")).await;
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
    let child_session = child.bridge_session_id().unwrap().clone();
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
