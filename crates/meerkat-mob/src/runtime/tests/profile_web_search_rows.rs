//! A profile's `tools.web_search` at a member's builds: its first build,
//! with and without a per-spawn override, and its rebuild at a revival.

use super::*;

const MEMBER: &str = "desk-north";

/// `sample_definition` whose `worker` profile states `web_search`, written
/// as a stored definition states it.
fn definition_stating(label: &str, web_search: bool) -> MobDefinition {
    let mut stored = serde_json::to_value(with_unique_mob_id(sample_definition(), label))
        .expect("the definition serializes");
    stored["profiles"]["worker"]["tools"]["web_search"] = serde_json::json!(web_search);
    serde_json::from_value(stored).expect("a definition stating web_search is read")
}

/// The web search switch of the build that creates a `worker` member whose
/// spawn names `spawn_override`.
async fn first_build_switch(
    definition: MobDefinition,
    spawn_override: ToolCategoryOverride,
) -> ToolCategoryOverride {
    let (handle, service) = create_test_mob(definition).await;
    let mut spec = SpawnMemberSpec::new("worker", AgentIdentity::from(MEMBER));
    spec.runtime_mode = Some(crate::MobRuntimeMode::TurnDriven);
    spec.tool_category_overrides.web_search = spawn_override;
    handle.spawn_spec(spec).await.expect("the member spawns");
    service
        .recorded_create_requests()
        .await
        .iter()
        .find(|record| {
            record
                .mob_member_binding
                .as_ref()
                .is_some_and(|binding| binding.member == MEMBER)
        })
        .expect("the member's create request")
        .override_web_search
}

/// A spawn that names no web search override builds its member with the
/// value its profile states.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_spawn_with_no_override_builds_with_the_profiles_web_search() {
    assert_eq!(
        first_build_switch(
            definition_stating("profile-search-on", true),
            ToolCategoryOverride::Inherit
        )
        .await,
        ToolCategoryOverride::Enable,
        "the profile's `web_search = true`"
    );
    assert_eq!(
        first_build_switch(
            definition_stating("profile-search-off", false),
            ToolCategoryOverride::Inherit
        )
        .await,
        ToolCategoryOverride::Disable,
        "the profile's `web_search = false`"
    );
}

/// A spawn's own web search override wins over the value its profile
/// states, in both directions.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_spawn_override_wins_over_the_profiles_web_search() {
    assert_eq!(
        first_build_switch(
            definition_stating("spawn-search-off", true),
            ToolCategoryOverride::Disable
        )
        .await,
        ToolCategoryOverride::Disable,
        "the spawn's disable over the profile's `web_search = true`"
    );
    assert_eq!(
        first_build_switch(
            definition_stating("spawn-search-on", false),
            ToolCategoryOverride::Enable
        )
        .await,
        ToolCategoryOverride::Enable,
        "the spawn's enable over the profile's `web_search = false`"
    );
}

/// Spawn [`MEMBER`] on `worker` of `definition` with `spawn_override`, then
/// restart the mob from the same storage, stopped first when `stop_first`
/// (so an explicit resume rebuilds the member). `edit`, when given, rewrites
/// the stored definition while the mob is down. Returns the member's first
/// build and its rebuild.
async fn first_build_and_rebuild(
    definition: MobDefinition,
    spawn_override: ToolCategoryOverride,
    edit: Option<&dyn Fn(&mut serde_json::Value)>,
    stop_first: bool,
) -> (CreateSessionRecord, CreateSessionRecord) {
    let member = AgentIdentity::from(MEMBER);
    let service = Arc::new(MockSessionService::new());
    let _ = service.enable_runtime_adapter();
    let storage = MobStorage::in_memory();
    let restart_storage = MobStorage::with_events_and_runtime_metadata(
        storage.events.clone(),
        storage.runtime_metadata.clone(),
    );
    let handle = MobBuilder::new(definition, storage)
        .with_session_service(service.clone())
        .create()
        .await
        .expect("create the mob");
    let mut spec = SpawnMemberSpec::new("worker", member.clone());
    spec.runtime_mode = Some(crate::MobRuntimeMode::TurnDriven);
    spec.tool_category_overrides.web_search = spawn_override;
    handle.spawn_spec(spec).await.expect("the member spawns");
    let first = last_member_build(&service, &member).await;
    let session = handle
        .resolve_bridge_session_id(&member)
        .await
        .expect("the member's session");
    let edited = edit.map(|edit| {
        let mut stored =
            serde_json::to_value(handle.definition()).expect("the definition serializes");
        edit(&mut stored);
        serde_json::from_value::<MobDefinition>(stored).expect("the edited definition is read")
    });
    if stop_first {
        handle.stop().await.expect("stop");
    }
    MobSessionService::discard_live_session(service.as_ref(), &session)
        .await
        .expect("discard the live session");
    crash_stop_and_release_routes(handle).await;
    if let Some(edited) = edited {
        restart_storage
            .update_definition(1, edited)
            .await
            .expect("the author rewrites the stored definition");
    }
    let resumed = MobBuilder::for_resume(restart_storage)
        .with_session_service(service.clone())
        .with_member_restore_policy(MemberRestorePolicy::Eager)
        .resume()
        .await
        .expect("resume the mob");
    if stop_first {
        resumed.resume().await.expect("explicit resume");
    }
    let rebuilt = last_member_build(&service, &member).await;
    assert!(
        rebuilt.resume_session_id.is_some(),
        "the member is rebuilt on its session (explicit resume: {stop_first})"
    );
    crash_stop_and_release_routes(resumed).await;
    (first, rebuilt)
}

/// A spawn's disable over a profile that enables web search survives the
/// member's revival, on a restart restore and on an explicit resume: the
/// rebuild names the disable itself.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_spawns_disable_over_an_enabling_profile_survives_a_revival() {
    for stop_first in [false, true] {
        let (first, rebuilt) = first_build_and_rebuild(
            definition_stating("revive-spawn-off", true),
            ToolCategoryOverride::Disable,
            None,
            stop_first,
        )
        .await;
        assert_eq!(first.override_web_search, ToolCategoryOverride::Disable);
        assert_eq!(
            rebuilt.override_web_search,
            ToolCategoryOverride::Disable,
            "the revived member still has no web search (explicit resume: {stop_first})"
        );
    }
}

/// The reverse: a spawn's enable over a profile that disables web search
/// survives the member's revival.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_spawns_enable_over_a_disabling_profile_survives_a_revival() {
    for stop_first in [false, true] {
        let (first, rebuilt) = first_build_and_rebuild(
            definition_stating("revive-spawn-on", false),
            ToolCategoryOverride::Enable,
            None,
            stop_first,
        )
        .await;
        assert_eq!(first.override_web_search, ToolCategoryOverride::Enable);
        assert_eq!(
            rebuilt.override_web_search,
            ToolCategoryOverride::Enable,
            "the revived member still has web search (explicit resume: {stop_first})"
        );
    }
}

fn worker_tools(stored: &mut serde_json::Value) -> &mut serde_json::Map<String, serde_json::Value> {
    stored["profiles"]["worker"]["tools"]
        .as_object_mut()
        .expect("the worker's tools table")
}

/// A member built while its profile enabled web search, revived after the
/// key was removed from the profile: the rebuild names no choice, so the
/// member's session keeps the enable it recorded. Stating `false` is what
/// takes it away: that rebuild names the disable.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_removed_key_names_no_choice_at_a_revival_and_false_names_the_disable() {
    let (first, rebuilt) = first_build_and_rebuild(
        definition_stating("revive-key-removed", true),
        ToolCategoryOverride::Inherit,
        Some(&|stored| {
            worker_tools(stored).remove("web_search");
        }),
        false,
    )
    .await;
    assert_eq!(first.override_web_search, ToolCategoryOverride::Enable);
    assert_eq!(
        rebuilt.override_web_search,
        ToolCategoryOverride::Inherit,
        "with the key removed the rebuild names no choice: the session's record decides"
    );

    let (first, rebuilt) = first_build_and_rebuild(
        definition_stating("revive-key-false", true),
        ToolCategoryOverride::Inherit,
        Some(&|stored| {
            worker_tools(stored).insert("web_search".to_string(), serde_json::json!(false));
        }),
        false,
    )
    .await;
    assert_eq!(first.override_web_search, ToolCategoryOverride::Enable);
    assert_eq!(
        rebuilt.override_web_search,
        ToolCategoryOverride::Disable,
        "`web_search = false` takes web search from the revived member"
    );
}

/// A member of a profile that omits the key is built and rebuilt with no
/// web search choice, as before the key existed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_member_of_a_profile_without_the_key_is_built_and_rebuilt_with_no_choice() {
    let (first, rebuilt) = first_build_and_rebuild(
        with_unique_mob_id(sample_definition(), "revive-no-key"),
        ToolCategoryOverride::Inherit,
        None,
        false,
    )
    .await;
    assert_eq!(first.override_web_search, ToolCategoryOverride::Inherit);
    assert_eq!(rebuilt.override_web_search, ToolCategoryOverride::Inherit);
}

/// A deny list that names `web_search` is declared again at the member's
/// revival, beside the profile's enable: the gate that refuses the tool is
/// recomputed at every build.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_deny_list_naming_web_search_is_declared_again_at_a_revival() {
    let mut stored =
        serde_json::to_value(definition_stating("revive-denied", true)).expect("serializes");
    worker_tools(&mut stored).insert("deny".to_string(), serde_json::json!(["web_search"]));
    let definition: MobDefinition = serde_json::from_value(stored).expect("is read");
    let (first, rebuilt) =
        first_build_and_rebuild(definition, ToolCategoryOverride::Inherit, None, false).await;
    for (build, when) in [(&first, "first build"), (&rebuilt, "revival")] {
        assert_eq!(
            build.override_web_search,
            ToolCategoryOverride::Enable,
            "{when}: the profile's enable"
        );
        assert!(
            build.declared_deny.contains("web_search"),
            "{when}: the deny list still names the tool: {:?}",
            build.declared_deny
        );
    }
}

/// A tool grant gate that covers `web_search` is named again at the member's
/// revival, beside the profile's enable: the member's first build and its
/// rebuild both carry the gate, under which the tool is mounted closed until
/// a grant opens it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_grant_gate_covering_web_search_is_named_again_at_a_revival() {
    let mut stored =
        serde_json::to_value(definition_stating("revive-gated", true)).expect("serializes");
    worker_tools(&mut stored).insert(
        "grant_gated".to_string(),
        serde_json::json!({ "families": ["web_search"] }),
    );
    let definition: MobDefinition = serde_json::from_value(stored).expect("is read");
    let (first, rebuilt) =
        first_build_and_rebuild(definition, ToolCategoryOverride::Inherit, None, false).await;
    for (build, when) in [(&first, "first build"), (&rebuilt, "revival")] {
        assert_eq!(
            build.override_web_search,
            ToolCategoryOverride::Enable,
            "{when}: the gated family is mounted"
        );
        assert!(
            build.grant_gated_families.contains("web_search"),
            "{when}: the build names the gate: {:?}",
            build.grant_gated_families
        );
    }
}

async fn sealed_intent(
    store: &Arc<InMemoryMobIdentityStore>,
    mob_id: &MobId,
    member: &AgentIdentity,
) -> crate::identity::IdentityIntentRecord {
    match store
        .observe_identity_intent(mob_id, member)
        .await
        .expect("read the identity intent")
    {
        crate::identity::IdentityStoredObservation::Valid(record) => record,
        other => panic!("expected a valid identity intent, got {other:?}"),
    }
}

/// The sealed profile and overlay of a present identity intent: the
/// profile's web search choice, and the overlay's web search override.
fn sealed_web_search(
    record: &crate::identity::IdentityIntentRecord,
) -> (Option<bool>, ToolCategoryOverride) {
    match &record.intent {
        crate::identity::IdentityIntent::Present { member, .. } => (
            member.material.profile.tools.web_search,
            member.material.overlay.tool_category_overrides.web_search,
        ),
        crate::identity::IdentityIntent::Absent { .. } => panic!("a present intent"),
    }
}

/// An identity adopted from a profile that enables web search seals the
/// choice in its stored material, and a member tool declaration update, the
/// one update such an identity has, rewrites the overlay only: the sealed
/// profile keeps the choice, also after the key is removed from the
/// definition. A web search override through that update is what changes
/// what the member gets.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_adopted_identity_seals_the_profiles_web_search_and_a_tool_update_keeps_it() {
    let definition = definition_stating("adopted-sealed", true);
    let mob_id = definition.id.clone();
    let event_store = InMemoryMobEventStore::new();
    let identity_store = Arc::new(InMemoryMobIdentityStore::paired_with_event_store(
        &event_store,
        Arc::new(crate::store::SystemMobIdentityStoreClock),
    ));
    let session_store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
    let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> =
        Arc::new(meerkat_runtime::InMemoryRuntimeStore::new());
    let blob_store: Arc<dyn meerkat_core::BlobStore> =
        Arc::new(meerkat_store::MemoryBlobStore::new());
    let service = Arc::new(meerkat_session::PersistentSessionService::new(
        PersistentMockBuilder,
        16,
        session_store,
        runtime_store,
        blob_store,
    ));
    let storage = MobStorage {
        events: Arc::new(event_store),
        runs: Arc::new(InMemoryMobRunStore::new()),
        specs: Arc::new(InMemoryMobSpecStore::new()),
        definition_projection_composition:
            crate::storage::DefinitionProjectionComposition::Independent,
        runtime_metadata: Arc::new(InMemoryMobRuntimeMetadataStore::new()),
        identity: identity_store.clone(),
        identity_member: Some(identity_store.clone()),
        identity_status: Arc::new(InMemoryMobIdentityStatusStore::new()),
        identity_status_projection_order: Default::default(),
        realm_profiles: None,
        forked_participants: None,
    };
    let handle = MobBuilder::new(definition, storage.clone())
        .with_session_service(service)
        .with_spawn_base_prompt_source(Arc::new(crate::StaticSpawnBasePromptSource(
            "adoption base prompt".to_string(),
        )))
        .create()
        .await
        .expect("create the mob");
    let member = AgentIdentity::from(MEMBER);
    let mut spawn = SpawnMemberSpec::new("worker", member.clone());
    spawn.runtime_mode = Some(crate::MobRuntimeMode::TurnDriven);
    handle.spawn_spec(spawn).await.expect("the member spawns");
    let session_id = handle
        .resolve_bridge_session_id(&member)
        .await
        .expect("the member's session");

    handle
        .adopt_member_identity_declaration(crate::identity::AdoptMemberIdentityDeclaration {
            mob_id: mob_id.clone(),
            agent_identity: member.clone(),
            request_id: crate::identity::IdentityAdoptionId::new("adopt-desk")
                .expect("valid adoption id"),
            precondition: crate::identity::IdentityAdoptionPrecondition::ExpectedAbsent,
            declaration_scope: crate::identity::IdentityDeclarationScopeId::new("desk-policy")
                .expect("valid declaration scope"),
            declaration_revision: 1,
            session: crate::identity::DesiredSessionTarget {
                session_id: session_id.clone(),
                lineage_id: meerkat_core::SessionLineageId::for_session(&session_id),
                lineage_generation: meerkat_core::SessionGeneration::INITIAL,
                authority_policy: crate::identity::DesiredSessionAuthorityPolicy::RequireExisting,
            },
            member: crate::identity::IdentityProfileMemberDeclaration {
                profile_name: ProfileName::from("worker"),
                profile_override: None,
                model_override: None,
                external_addressable_override: None,
                context: None,
                labels: None,
                additional_instructions: None,
                system_prompt_override: None,
                tool_access_policy: None,
                auth_binding: None,
                budget_limits: None,
                runtime_mode: Some(meerkat_contracts::wire::WireMobRuntimeMode::TurnDriven),
                required_env_keys: Vec::new(),
                required_local_callback_tools: Vec::new(),
                execution: crate::identity::DesiredExecution::ControllingSession,
            },
            wiring_custody: crate::identity::IdentityWiringCustody::ExternalManaged,
            owned_wiring: BTreeSet::new(),
            convergence: crate::identity::IdentityConvergenceMode::Drain { max_wait_ms: 5_000 },
        })
        .await
        .expect("adopt the member");
    // The adoption converges before the next revision is applied.
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            if matches!(
                handle
                    .identity_convergence_status(&member)
                    .await
                    .expect("read the adoption's convergence"),
                crate::identity::IdentityStoredObservation::Valid(
                    crate::identity::IdentityConvergenceStatus {
                        intent_revision: Some(1),
                        active_intent_revision: Some(1),
                        decision: Some(crate::identity::IdentityReconcileDecision::Converged),
                        ..
                    }
                )
            ) {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the adoption converges");
    let adopted = sealed_intent(&identity_store, &mob_id, &member).await;
    assert_eq!(
        sealed_web_search(&adopted),
        (Some(true), ToolCategoryOverride::Inherit),
        "the adoption seals the profile's enable in the identity's material"
    );

    // The author removes the key from the stored definition; the identity's
    // one update, a member tool declaration, names a web search disable.
    let mut edited = serde_json::to_value(handle.definition()).expect("the definition serializes");
    worker_tools(&mut edited).remove("web_search");
    storage
        .update_definition(
            1,
            serde_json::from_value(edited).expect("the edited definition is read"),
        )
        .await
        .expect("the stored definition loses the key");
    let mut declaration = match &adopted.intent {
        crate::identity::IdentityIntent::Present { member, .. } => {
            member.material.member_tool_declaration()
        }
        crate::identity::IdentityIntent::Absent { .. } => panic!("a present intent"),
    };
    declaration.category_overrides.web_search = ToolCategoryOverride::Disable;
    handle
        .apply_member_tool_declaration(crate::identity::ApplyMemberToolDeclaration {
            mob_id: mob_id.clone(),
            agent_identity: member.clone(),
            request_id: crate::identity::MemberToolMutationId::new("search-off")
                .expect("valid mutation id"),
            expected_intent_revision: adopted.intent_revision,
            declaration,
            convergence: crate::identity::IdentityConvergenceMode::CancelActive,
        })
        .await
        .expect("apply the tool declaration");
    assert_eq!(
        sealed_web_search(&sealed_intent(&identity_store, &mob_id, &member).await),
        (Some(true), ToolCategoryOverride::Disable),
        "the update rewrites the overlay only: the sealed profile keeps the choice"
    );
    handle.shutdown().await.expect("shutdown");
}
