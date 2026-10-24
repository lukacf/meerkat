//! A local `tools.mob` member's own mob operator tools (`spawn_member` and
//! the rest, composed by the mob runtime for its session) admit against the
//! session's final sealed authority: the context the agent factory settles
//! after a host hook has edited the build, not a default the runtime composed
//! before the hook ran.
//!
//! Each row takes the member tools the runtime handed to the session service
//! (a fresh spawn's, or a restored member's from the resume pass), builds the
//! member through the real `meerkat::AgentFactory` with the host's edit
//! applied to the member's build config, and dispatches through those tools.

use super::*;
use meerkat_core::service::MobToolAuthorityContext;

const MEMBER: &str = "w-1";

/// The external tools the mob runtime composed for a member's session, as
/// the session service received them.
async fn recorded_member_tools(
    service: &MockSessionService,
    session_id: &SessionId,
) -> Arc<dyn AgentToolDispatcher> {
    service
        .external_tools_by_session
        .read()
        .await
        .get(session_id)
        .cloned()
        .expect("the member's session records its external tools")
}

async fn member_session(handle: &MobHandle, member: &str) -> SessionId {
    handle
        .get_member(&AgentIdentity::from(member))
        .await
        .expect("roster read")
        .expect("roster entry")
        .bridge_session_id()
        .cloned()
        .expect("session-backed member")
}

/// A mob whose worker profile declares `tools.mob`, with one worker spawned.
async fn mob_with_spawned_worker() -> (MobHandle, Arc<MockSessionService>, SessionId) {
    let (handle, service) = create_test_mob(sample_definition_with_mob_tools()).await;
    handle
        .spawn(
            ProfileName::from("worker"),
            AgentIdentity::from(MEMBER),
            None,
        )
        .await
        .expect("spawn the worker");
    let session_id = member_session(&handle, MEMBER).await;
    (handle, service, session_id)
}

/// The same mob, stopped and resumed: the resume pass restores the worker's
/// missing session and composes its tools again.
async fn mob_with_restored_worker() -> (MobHandle, Arc<MockSessionService>, SessionId) {
    let service = Arc::new(MockSessionService::new());
    let _ = service.enable_runtime_adapter();
    let storage = MobStorage::in_memory();
    let events = storage.events.clone();
    let runtime_metadata = storage.runtime_metadata.clone();
    let handle = MobBuilder::new(sample_definition_with_mob_tools(), storage)
        .with_session_service(service.clone())
        .create()
        .await
        .expect("create mob");
    handle
        .spawn(
            ProfileName::from("worker"),
            AgentIdentity::from(MEMBER),
            None,
        )
        .await
        .expect("spawn the worker");
    handle.stop().await.expect("stop");
    let old_session = member_session(&handle, MEMBER).await;
    MobSessionService::discard_live_session(service.as_ref(), &old_session)
        .await
        .expect("discard live session");
    crash_stop_and_release_routes(handle).await;
    let resumed = MobBuilder::for_resume(MobStorage::with_events_and_runtime_metadata(
        events,
        runtime_metadata,
    ))
    .with_session_service(service.clone())
    .resume()
    .await
    .expect("reconstruct the mob");
    resumed.resume().await.expect("resume restores the worker");
    let session_id = member_session(&resumed, MEMBER).await;
    assert_eq!(session_id, old_session, "the worker keeps its session");
    (resumed, service, session_id)
}

/// A worker's build config as the mob runtime produces it for a member with
/// no persisted authority, carrying the given member tools.
async fn worker_build_config(
    handle: &MobHandle,
    member: &str,
    tools: Arc<dyn AgentToolDispatcher>,
) -> meerkat::AgentBuildConfig {
    let definition = handle.definition();
    let profile_name = ProfileName::from("worker");
    let profile = definition
        .profiles
        .get(&profile_name)
        .and_then(|profile| profile.as_inline())
        .expect("inline worker profile");
    let mut config = crate::build::build_agent_config(crate::build::BuildAgentConfigParams {
        mob_id: &definition.id,
        profile_name: &profile_name,
        agent_identity: &AgentIdentity::from(member),
        profile,
        definition,
        realm_profile_store: None,
        external_tools: Some(tools),
        compaction_curator_override: None,
        context: None,
        labels: None,
        additional_instructions: None,
        shell_env: None,
        mob_tool_authority_context: None,
        mob_incarnation: Some(handle.live_incarnation()),
        tool_access_policy: None,
        inherited_tool_filter: None,
        system_prompt_override: None,
    })
    .await
    .expect("worker build config");
    config.llm_client_override = Some(Arc::new(meerkat_client::TestClient::default()));
    config
}

/// A host hook's edit: keep only the worker profile spawnable.
fn narrow_to_worker(config: &mut meerkat::AgentBuildConfig, mob_id: &str) {
    let authority = config
        .mob_tool_authority_context
        .as_ref()
        .expect("a tools.mob member gets a default authority");
    let narrowed = meerkat_runtime::mob_operator_authority::restrict_spawn_profiles_in_mob(
        authority,
        mob_id,
        ["worker"],
    )
    .expect("narrow the default spawn scope");
    config.mob_tool_authority_context = Some(narrowed);
}

/// Captures the effective-authority handle the factory gives its mob tools.
struct CaptureEffectiveAuthority {
    captured: Arc<std::sync::Mutex<Option<Arc<std::sync::RwLock<MobToolAuthorityContext>>>>>,
}

#[async_trait::async_trait]
impl meerkat_core::service::MobToolsFactory for CaptureEffectiveAuthority {
    async fn build_mob_tools(
        &self,
        args: meerkat_core::service::MobToolsBuildArgs,
    ) -> Result<Arc<dyn AgentToolDispatcher>, Box<dyn std::error::Error + Send + Sync>> {
        *self.captured.lock().expect("capture lock") = args.effective_authority.clone();
        Ok(Arc::new(
            meerkat_core::DynamicToolComposite::new(Vec::new()),
        ))
    }
}

/// Build the member through the real agent factory, with or without a
/// `MobToolsFactory`; the agent is returned so it outlives the dispatches.
async fn build_through_factory(
    config: meerkat::AgentBuildConfig,
    mob_tools: Option<Arc<CaptureEffectiveAuthority>>,
) -> (tempfile::TempDir, meerkat::DynAgent) {
    let temp = tempfile::tempdir().expect("factory dir");
    let mut factory = meerkat::AgentFactory::new(temp.path().join("sessions"))
        .builtins(false)
        .workgraph(false);
    if let Some(mob_tools) = mob_tools {
        factory = factory.mob(true).mob_tools_factory(mob_tools);
    }
    let agent = factory
        .build_agent(config, &meerkat_core::Config::default())
        .await
        .expect("build the member through the agent factory");
    (temp, agent)
}

async fn call(
    tools: &Arc<dyn AgentToolDispatcher>,
    name: &str,
    args: serde_json::Value,
) -> Result<ToolDispatchOutcome, ToolError> {
    let raw = RawValue::from_string(args.to_string()).expect("raw args");
    tools
        .dispatch(ToolCallView {
            id: "session-authority-call",
            name,
            args: &raw,
        })
        .await
}

async fn spawn(
    tools: &Arc<dyn AgentToolDispatcher>,
    profile: &str,
    member_id: &str,
) -> Result<ToolDispatchOutcome, ToolError> {
    call(
        tools,
        "spawn_member",
        serde_json::json!({ "profile": profile, "member_id": member_id }),
    )
    .await
}

fn tool_names(tools: &Arc<dyn AgentToolDispatcher>) -> Vec<String> {
    tools
        .tools()
        .iter()
        .map(|tool| tool.name.to_string())
        .collect()
}

fn assert_denied(result: Result<ToolDispatchOutcome, ToolError>, what: &str) {
    match result {
        Err(ToolError::AccessDenied { .. }) => {}
        Err(other) => panic!("{what}: expected access denied, got {other}"),
        Ok(outcome) => panic!(
            "{what}: expected access denied, got {}",
            outcome.result.text_content()
        ),
    }
}

fn assert_spawned(result: Result<ToolDispatchOutcome, ToolError>, member_id: &str) {
    let outcome = result.unwrap_or_else(|error| panic!("spawn {member_id}: {error}"));
    let payload: serde_json::Value =
        serde_json::from_str(&outcome.result.text_content()).expect("spawn payload");
    assert_eq!(payload["agent_identity"], member_id, "{payload}");
}

/// With no hook, the session's final context is the runtime's default, and
/// the member keeps exactly what it had: every profile spawnable, the
/// manage-scoped verbs denied, the same tools listed.
async fn assert_no_hook_equality(
    handle: MobHandle,
    service: Arc<MockSessionService>,
    session_id: SessionId,
) {
    let tools = recorded_member_tools(&service, &session_id).await;
    let listed_before = tool_names(&tools);
    let config = worker_build_config(&handle, MEMBER, Arc::clone(&tools)).await;
    let (_dir, _agent) = build_through_factory(config, None).await;

    assert_eq!(
        tool_names(&tools),
        listed_before,
        "the same tools stay listed"
    );
    assert!(listed_before.iter().any(|name| name == "spawn_member"));
    assert_spawned(spawn(&tools, "lead", "lead-2").await, "lead-2");
    assert_spawned(spawn(&tools, "worker", "w-2").await, "w-2");
    assert_denied(
        call(
            &tools,
            "retire_member",
            serde_json::json!({ "member_id": "lead-2" }),
        )
        .await,
        "retiring a member without manage scope",
    );
    assert_denied(
        call(
            &tools,
            "force_cancel_member",
            serde_json::json!({ "member_id": "lead-2" }),
        )
        .await,
        "force-cancelling a member without manage scope",
    );
}

#[tokio::test]
async fn a_fresh_member_without_a_hook_keeps_its_default_runtime_mob_tools() {
    let (handle, service, session_id) = mob_with_spawned_worker().await;
    assert_no_hook_equality(handle, service, session_id).await;
}

#[tokio::test]
async fn a_restored_member_without_a_hook_keeps_its_default_runtime_mob_tools() {
    let (handle, service, session_id) = mob_with_restored_worker().await;
    assert_no_hook_equality(handle, service, session_id).await;
}

/// The hook narrows the member's spawn scope to the worker profile: the
/// runtime's own `spawn_member` follows it, through a factory with a
/// `MobToolsFactory`, and its tools read the factory's effective-authority
/// handle, so an in-session authority update reaches them too.
async fn assert_narrowed_spawn_follows_the_hook(
    handle: MobHandle,
    service: Arc<MockSessionService>,
    session_id: SessionId,
) {
    let tools = recorded_member_tools(&service, &session_id).await;
    let mut config = worker_build_config(&handle, MEMBER, Arc::clone(&tools)).await;
    let default_authority = config
        .mob_tool_authority_context
        .clone()
        .expect("default authority");
    narrow_to_worker(&mut config, handle.definition().id.as_str());
    let captured = Arc::new(std::sync::Mutex::new(None));
    let mob_tools = Arc::new(CaptureEffectiveAuthority {
        captured: Arc::clone(&captured),
    });
    let (_dir, _agent) = build_through_factory(config, Some(mob_tools)).await;

    assert_denied(
        spawn(&tools, "lead", "lead-2").await,
        "spawning a profile the hook removed",
    );
    assert_denied(
        call(
            &tools,
            "spawn_many_members",
            serde_json::json!({ "specs": [{ "profile": "lead", "member_id": "lead-3" }] }),
        )
        .await,
        "spawning many of a profile the hook removed",
    );
    assert_spawned(spawn(&tools, "worker", "w-2").await, "w-2");

    let effective = captured
        .lock()
        .expect("capture lock")
        .clone()
        .expect("the factory gives its mob tools the effective-authority handle");
    *effective.write().expect("effective authority lock") = default_authority;
    assert_spawned(spawn(&tools, "lead", "lead-2").await, "lead-2");
}

#[tokio::test]
async fn a_fresh_member_runtime_spawn_follows_the_hook_narrowed_authority() {
    let (handle, service, session_id) = mob_with_spawned_worker().await;
    assert_narrowed_spawn_follows_the_hook(handle, service, session_id).await;
}

#[tokio::test]
async fn a_restored_member_runtime_spawn_follows_the_hook_narrowed_authority() {
    let (handle, service, session_id) = mob_with_restored_worker().await;
    assert_narrowed_spawn_follows_the_hook(handle, service, session_id).await;
}

/// A Core-only embedder: the factory has no `MobToolsFactory`, and the
/// member's runtime tools still admit against the final sealed context.
#[tokio::test]
async fn a_member_built_without_a_mob_tools_factory_follows_the_final_authority() {
    let (handle, service, session_id) = mob_with_spawned_worker().await;
    let tools = recorded_member_tools(&service, &session_id).await;
    let mut config = worker_build_config(&handle, MEMBER, Arc::clone(&tools)).await;
    narrow_to_worker(&mut config, handle.definition().id.as_str());
    let (_dir, _agent) = build_through_factory(config, None).await;

    assert_denied(
        spawn(&tools, "lead", "lead-2").await,
        "spawning a profile the hook removed",
    );
    assert_spawned(spawn(&tools, "worker", "w-2").await, "w-2");
}

/// The hook removes the member's mob authority: the runtime's mob operator
/// tools are no longer listed, and a call that reaches them is denied.
async fn assert_removed_authority_hides_and_denies(edit: fn(&mut meerkat::AgentBuildConfig)) {
    let (handle, service, session_id) = mob_with_spawned_worker().await;
    let tools = recorded_member_tools(&service, &session_id).await;
    let mut config = worker_build_config(&handle, MEMBER, Arc::clone(&tools)).await;
    edit(&mut config);
    let (_dir, _agent) = build_through_factory(config, None).await;

    let listed = tool_names(&tools);
    for name in ["spawn_member", "spawn_many_members", "list_members"] {
        assert!(
            !listed.iter().any(|listed| listed == name),
            "{name} must not be listed: {listed:?}"
        );
    }
    assert_denied(spawn(&tools, "worker", "w-2").await, "spawning");
    assert_denied(
        call(&tools, "list_members", serde_json::json!({})).await,
        "listing members",
    );
    assert_denied(
        call(
            &tools,
            "wire_members",
            serde_json::json!({ "member_id": MEMBER, "peer_member_id": MEMBER }),
        )
        .await,
        "wiring members",
    );
}

#[tokio::test]
async fn a_hook_that_clears_the_authority_hides_and_denies_runtime_mob_tools() {
    assert_removed_authority_hides_and_denies(|config| {
        config.mob_tool_authority_context = None;
    })
    .await;
}

#[tokio::test]
async fn a_hook_that_disables_mob_tools_hides_and_denies_runtime_mob_tools() {
    assert_removed_authority_hides_and_denies(|config| {
        let authority = config.mob_tool_authority_context.clone();
        config.apply_persisted_mob_operator_access(ToolCategoryOverride::Disable, authority);
    })
    .await;
}

/// The controlling side's upcall executor builds its dispatcher with the
/// context it mints for each request of a placed member: a session binding
/// never reaches it, so it keeps listing its tools and admitting against
/// that context.
#[tokio::test]
async fn a_fixed_context_operator_dispatcher_ignores_session_bindings() {
    let (handle, _service) = create_test_mob(sample_definition_with_mob_tools()).await;
    let mob_id = handle.definition().id.to_string();
    let dispatcher = super::super::tools::MobOperatorToolDispatcher::new(
        handle.clone(),
        true,
        generated_mob_operator_authority_with_spawn_profile(
            &mob_id,
            handle.live_incarnation(),
            "worker",
        ),
    );
    dispatcher.bind_session_mob_tool_authority(None);
    let tools: Arc<dyn AgentToolDispatcher> = Arc::new(dispatcher);

    assert!(tool_names(&tools).iter().any(|name| name == "spawn_member"));
    assert_spawned(spawn(&tools, "worker", "w-fixed").await, "w-fixed");
    assert_denied(
        spawn(&tools, "lead", "lead-fixed").await,
        "spawning a profile the fixed context does not hold",
    );
}

/// A hook that narrows the member's spawn profiles to none keeps the
/// member's binding to its mob: the tools stay listed, the member still
/// lists and retires the members it spawned, and spawning is denied.
#[tokio::test]
async fn a_member_narrowed_to_no_spawn_profiles_keeps_its_own_children() {
    let (handle, service) = create_test_mob(sample_definition_with_mob_tools()).await;
    service.set_return_exact_run_result(true);
    let forker = AgentIdentity::from("owner-forker");
    spawn_bounded_fork_source(&handle, &forker).await;
    let forker_session = handle
        .resolve_bridge_session_id(&forker)
        .await
        .expect("forker session");
    let child = AgentIdentity::from("owned-fork-child");
    let (_fork, run) = handle
        .fork_member_then_run_detached(
            &forker,
            bounded_fork_child_spec(&child),
            None,
            "fork_child_result",
            256,
            meerkat_core::DurableForkSourceAdmission::CallerTurn,
            None,
            None,
        )
        .await
        .expect("caller-turn fork");
    run.outcome().await.expect("child outcome");

    let tools = recorded_member_tools(&service, &forker_session).await;
    let mut config = worker_build_config(&handle, forker.as_str(), Arc::clone(&tools)).await;
    let authority = config
        .mob_tool_authority_context
        .as_ref()
        .expect("a tools.mob member gets a default authority");
    let narrowed = meerkat_runtime::mob_operator_authority::restrict_spawn_profiles_in_mob(
        authority,
        handle.definition().id.as_str(),
        std::iter::empty::<&str>(),
    )
    .expect("narrow the default spawn scope to no profile");
    config.mob_tool_authority_context = Some(narrowed);
    let (_dir, _agent) = build_through_factory(config, None).await;

    let listed = tool_names(&tools);
    for name in ["spawn_member", "list_members", "retire_member"] {
        assert!(
            listed.iter().any(|listed| listed == name),
            "{name} stays listed while the member is bound to its mob: {listed:?}"
        );
    }
    let owner_bound = dispatched_from_session(Arc::clone(&tools), forker_session);
    let members = call(&owner_bound, "list_members", serde_json::json!({}))
        .await
        .expect("the member lists its own children");
    let payload: serde_json::Value =
        serde_json::from_str(&members.result.text_content()).expect("list payload");
    let names: Vec<_> = payload["members"]
        .as_array()
        .expect("members")
        .iter()
        .filter_map(|member| {
            member["member_id"]
                .as_str()
                .or(member["agent_identity"].as_str())
        })
        .collect();
    assert_eq!(names, vec!["owned-fork-child"], "the owner view is kept");
    assert_denied(
        spawn(&owner_bound, "worker", "w-2").await,
        "spawning with no spawn profile",
    );
    call(
        &owner_bound,
        "retire_member",
        serde_json::json!({ "member_id": "owned-fork-child" }),
    )
    .await
    .expect("the member retires its own child");
    assert!(
        handle
            .get_member(&child)
            .await
            .expect("roster read")
            .is_none()
    );
}

/// Member tools no agent factory bound (a session built without one, or
/// behind a wrapper that does not forward the binding) stay listed as
/// composed and deny every call: nothing is admitted against the default
/// the runtime composed for them.
#[tokio::test]
async fn member_tools_that_were_never_bound_deny_every_call() {
    let (handle, _service) = create_test_mob(sample_definition_with_mob_tools()).await;
    let profile = handle
        .definition()
        .profiles
        .get(&ProfileName::from("worker"))
        .and_then(|profile| profile.as_inline())
        .expect("inline worker profile");
    let tools = super::super::tools::compose_external_tools_for_profile(
        profile,
        &BTreeMap::new(),
        handle.clone(),
        None,
        None,
        None,
    )
    .expect("compose the member tools")
    .expect("a tools.mob member gets its operator tools");

    assert!(tool_names(&tools).iter().any(|name| name == "spawn_member"));
    assert_denied(
        spawn(&tools, "worker", "w-unbound").await,
        "spawning through tools no factory bound",
    );
    assert_denied(
        call(&tools, "list_members", serde_json::json!({})).await,
        "listing through tools no factory bound",
    );
    assert!(
        handle
            .get_member(&AgentIdentity::from("w-unbound"))
            .await
            .expect("roster read")
            .is_none()
    );
}

/// The binding reaches a member's tools through the composites that can
/// wrap them before the agent factory builds the session: a surface's
/// `ToolGateway` over the runtime's `DynamicToolComposite`.
#[tokio::test]
async fn the_binding_reaches_member_tools_through_a_gateway_and_a_composite() {
    let (handle, _service) = create_test_mob(sample_definition_with_mob_tools()).await;
    let mob_id = handle.definition().id.to_string();
    let profile = handle
        .definition()
        .profiles
        .get(&ProfileName::from("worker"))
        .and_then(|profile| profile.as_inline())
        .expect("inline worker profile");
    let member_tools = super::super::tools::compose_external_tools_for_profile(
        profile,
        &BTreeMap::new(),
        handle.clone(),
        None,
        None,
        None,
    )
    .expect("compose the member tools")
    .expect("a tools.mob member gets its operator tools");
    let composite: Arc<dyn AgentToolDispatcher> = Arc::new(
        meerkat_core::DynamicToolComposite::new(vec![Arc::clone(&member_tools)]),
    );
    let gateway: Arc<dyn AgentToolDispatcher> = Arc::new(
        meerkat_core::ToolGateway::new(composite, None).expect("gateway over the member tools"),
    );
    assert_denied(
        spawn(&gateway, "worker", "w-before-bind").await,
        "spawning before the binding",
    );

    gateway.bind_session_mob_tool_authority(Some(Arc::new(std::sync::RwLock::new(
        generated_mob_operator_authority_with_spawn_profile(
            &mob_id,
            handle.live_incarnation(),
            "worker",
        ),
    ))));
    assert!(
        tool_names(&gateway)
            .iter()
            .any(|name| name == "spawn_member")
    );
    assert_spawned(spawn(&gateway, "worker", "w-2").await, "w-2");
    assert_denied(
        spawn(&gateway, "lead", "lead-2").await,
        "spawning a profile the bound authority does not hold",
    );

    gateway.bind_session_mob_tool_authority(None);
    assert!(
        !tool_names(&gateway)
            .iter()
            .any(|name| name == "spawn_member")
    );
    let routed = spawn(&gateway, "worker", "w-3").await;
    assert!(
        matches!(routed, Err(ToolError::NotFound { .. })),
        "the gateway no longer routes a hidden tool: {:?}",
        routed.map(|outcome| outcome.result.text_content())
    );
    assert_denied(
        spawn(&member_tools, "worker", "w-3").await,
        "spawning after the authority was removed",
    );
}

/// The start of the warning a restore build logs when the member's settled
/// authority holds operator facts its persisted record did not.
const REGAINED_AUTHORITY_WARNING: &str = "restored mob member holds mob operator authority";

fn regained_authority_warnings(captured: &CapturedWarnings) -> Vec<CapturedFields> {
    captured
        .lock()
        .expect("captured warnings lock")
        .iter()
        .filter(|fields| {
            fields
                .get("message")
                .is_some_and(|message| message.contains(REGAINED_AUTHORITY_WARNING))
        })
        .cloned()
        .collect()
}

/// The member's session as a store hands it back: its recorded authority is
/// the unsealed record a restore build reads.
fn stored_session(session: &meerkat_core::Session) -> meerkat_core::Session {
    serde_json::from_value(serde_json::to_value(session).expect("session encodes"))
        .expect("session decodes")
}

/// What the warning reported across one restore of the worker.
struct RestoreWarnings {
    first_build: Vec<CapturedFields>,
    restore_build: Vec<CapturedFields>,
    session_id: SessionId,
    mob_id: String,
}

/// Build the worker once, narrowed by the host's hook when `narrow_first`,
/// then restore it from its stored session, narrowed again when
/// `narrow_restore`.
async fn warnings_across_a_restore(narrow_first: bool, narrow_restore: bool) -> RestoreWarnings {
    let (handle, service, session_id) = mob_with_spawned_worker().await;
    let tools = recorded_member_tools(&service, &session_id).await;
    let mob_id = handle.definition().id.to_string();
    // A second registered Dispatch, alive as long as the capture, so a
    // callsite's interest never comes from another thread's default alone.
    let _anchor = tracing::Dispatch::new(tracing_subscriber::Registry::default());
    let (captured, _guard) = capture_warnings();

    let mut first = worker_build_config(&handle, MEMBER, Arc::clone(&tools)).await;
    if narrow_first {
        narrow_to_worker(&mut first, &mob_id);
    }
    let (_first_dir, first_agent) = build_through_factory(first, None).await;
    let first_build = regained_authority_warnings(&captured);
    let stored = stored_session(first_agent.session());
    let restored_session = stored.id().clone();
    // The first build's agent holds the session identity until it is gone,
    // as a process that stopped would have released it.
    drop(first_agent);

    let mut restore = worker_build_config(&handle, MEMBER, Arc::clone(&tools)).await;
    restore.resume_session = Some(stored);
    if narrow_restore {
        narrow_to_worker(&mut restore, &mob_id);
    }
    let (_restore_dir, _restore_agent) = build_through_factory(restore, None).await;
    let restore_build = regained_authority_warnings(&captured)
        .into_iter()
        .skip(first_build.len())
        .collect();
    RestoreWarnings {
        first_build,
        restore_build,
        session_id: restored_session,
        mob_id,
    }
}

/// A member narrowed by the host on an earlier build and restored without
/// the narrowing regains the `lead` profile: the restore build warns once,
/// naming only the mob, the member, the session and the regained facts.
#[tokio::test]
async fn a_restored_member_that_regains_operator_authority_warns_once() {
    let warnings = warnings_across_a_restore(true, false).await;
    assert!(
        warnings.first_build.is_empty(),
        "a first build never warns: {:?}",
        warnings.first_build
    );
    assert_eq!(
        warnings.restore_build.len(),
        1,
        "the restore build warns exactly once: {:?}",
        warnings.restore_build
    );
    let warning = &warnings.restore_build[0];
    let field = |name: &str| warning.get(name).map(String::as_str);
    assert_eq!(field("mob_id"), Some(warnings.mob_id.as_str()));
    assert_eq!(field("member"), Some(MEMBER));
    assert_eq!(
        field("session_id"),
        Some(warnings.session_id.to_string().as_str())
    );
    assert_eq!(
        field("regained_spawn_profiles"),
        Some(format!("{{{:?}: {{\"lead\"}}}}", warnings.mob_id).as_str())
    );
    assert_eq!(field("regained_manage_scope"), Some("{}"));
    assert_eq!(field("regained_create"), Some("false"));
    assert_eq!(field("regained_mutate"), Some("false"));
    let named = warning.keys().map(String::as_str).collect::<Vec<_>>();
    assert_eq!(
        named,
        vec![
            "member",
            "message",
            "mob_id",
            "regained_create",
            "regained_manage_scope",
            "regained_mutate",
            "regained_spawn_profiles",
            "session_id",
            "tracing.level",
        ],
        "the warning names nothing else"
    );
}

/// A restore the host narrows again holds nothing its record lacked.
#[tokio::test]
async fn a_restored_member_narrowed_again_does_not_warn() {
    let warnings = warnings_across_a_restore(true, true).await;
    assert!(
        warnings.first_build.is_empty() && warnings.restore_build.is_empty(),
        "first build {:?}, restore build {:?}",
        warnings.first_build,
        warnings.restore_build
    );
}

/// A first build has no persisted record to compare with, so it never warns
/// (the narrowed first builds of the two rows above do not either), and a
/// member never narrowed regains nothing when it is restored.
#[tokio::test]
async fn a_first_build_does_not_warn() {
    let warnings = warnings_across_a_restore(false, false).await;
    assert!(
        warnings.first_build.is_empty(),
        "a first build never warns: {:?}",
        warnings.first_build
    );
    assert!(
        warnings.restore_build.is_empty(),
        "a restore with the same authority does not warn: {:?}",
        warnings.restore_build
    );
}
