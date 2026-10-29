//! Console side chats (`meta/fork_member`): a durable fork of an agent's
//! conversation into a new member the human continues in, unwired, kept until
//! it is closed, and listed under its source from Meerkat's durable roster.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use meerkat::{AgentFactory, Config};
use meerkat_client::TestClient;
use meerkat_core::types::{ContentInput, HandlingMode};
use meerkat_mob::{MobDefinition, MobStorage, SpawnMemberSpec};
use serde_json::{Value, json};

use super::*;
use crate::console_aggregator::AllowAllConsoleVisibilityPolicy;
use crate::{MobBootstrapOptions, MobBootstrapSpec};

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

const WORKER_PROFILES: &str = r#"
[profiles.worker]
model = "gpt-5.5"
runtime_mode = "turn_driven"
external_addressable = true

[profiles.worker.tools]
comms = true

[profiles.lead]
model = "gpt-5.5"
runtime_mode = "turn_driven"
external_addressable = true

[profiles.lead.tools]
comms = true
"#;

/// The composition of a console runtime whose sessions persist (SQLite
/// session and runtime stores), which a durable fork needs. `mob_header`
/// holds the `[mob]` table lines after `id`, and `tail` any `[wiring]`
/// tables.
pub(super) fn persistent_console_spec(
    mob_id: &str,
    mob_header: &str,
    tail: &str,
) -> Result<(tempfile::TempDir, MobBootstrapSpec), Box<dyn std::error::Error + Send + Sync>> {
    let temp_dir = tempfile::tempdir()?;
    let state = temp_dir.path().join("state");
    std::fs::create_dir_all(&state)?;
    let session_store: Arc<dyn meerkat::SessionStore> = Arc::new(
        meerkat_store::SqliteSessionStore::open(state.join("sessions.db"))?,
    );
    let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> = Arc::new(
        meerkat_runtime::store::SqliteRuntimeStore::new(state.join("runtime.sqlite"))?,
    );
    let blob_store: Arc<dyn meerkat_core::BlobStore> =
        Arc::new(meerkat_store::MemoryBlobStore::new());
    let factory = AgentFactory::new(&state).comms(true);
    let mut builder = meerkat::FactoryAgentBuilder::new(factory, Config::default());
    builder.default_session_store = Some(Arc::new(meerkat_store::StoreAdapter::new(
        session_store.clone(),
    )));
    builder.default_blob_store = Some(blob_store.clone());
    let machine = Arc::new(meerkat_runtime::MeerkatMachine::persistent(
        Arc::clone(&runtime_store),
        Arc::clone(&blob_store),
    )?);
    let session_service = Arc::new(meerkat_session::PersistentSessionService::new(
        builder,
        16,
        session_store,
        runtime_store,
        blob_store,
    ));
    let definition = MobDefinition::from_toml(&format!(
        "[mob]\nid = \"{mob_id}\"\n{mob_header}\n{WORKER_PROFILES}\n{tail}"
    ))?;
    let spec = MobBootstrapSpec::new(definition, MobStorage::in_memory(), session_service)
        .with_session_runtime_adapter(machine)?
        .with_options(MobBootstrapOptions {
            allow_ephemeral_sessions: true,
            notify_orchestrator_on_resume: true,
            default_llm_client: Some(Arc::new(TestClient::default())),
        });
    Ok((temp_dir, spec))
}

/// A console runtime over [`persistent_console_spec`].
async fn persistent_console_runtime(
    mob_id: &str,
    mob_header: &str,
    tail: &str,
) -> Result<(tempfile::TempDir, MobRuntime), Box<dyn std::error::Error + Send + Sync>> {
    let (temp_dir, spec) = persistent_console_spec(mob_id, mob_header, tail)?;
    let runtime = MobRuntime::bootstrap(spec).await?;
    Ok((temp_dir, runtime))
}

/// A console runtime as a gateway composes one over a state directory
/// (`MobBootstrapSpec::persistent`): its members can be seated dormant and
/// released, and its sessions persist.
async fn persistent_gateway_runtime(
    mob_id: &str,
    mob_header: &str,
    tail: &str,
) -> Result<(tempfile::TempDir, MobRuntime), Box<dyn std::error::Error + Send + Sync>> {
    let temp_dir = tempfile::tempdir()?;
    let runtime = persistent_gateway_runtime_in(temp_dir.path(), mob_id, mob_header, tail).await?;
    Ok((temp_dir, runtime))
}

/// [`persistent_gateway_runtime`] over the state directory `state`, which a
/// later runtime composed over the same directory restores.
async fn persistent_gateway_runtime_in(
    state: &std::path::Path,
    mob_id: &str,
    mob_header: &str,
    tail: &str,
) -> Result<MobRuntime, Box<dyn std::error::Error + Send + Sync>> {
    let (storage, provenance) = crate::mob_composition_manifest::persistent_mob_storage(
        state.join(crate::storage_layout::EVENT_LOG_DB_FILE_NAME),
    )?;
    let definition = MobDefinition::from_toml(&format!(
        "[mob]\nid = \"{mob_id}\"\n{mob_header}\n{WORKER_PROFILES}\n{tail}"
    ))?;
    let mut spec =
        MobBootstrapSpec::persistent(definition, storage, provenance, state.to_path_buf(), 4)
            .await
            .map_err(|error| error.to_string())?;
    spec.options.default_llm_client = Some(Arc::new(TestClient::default()));
    Ok(Box::pin(MobRuntime::bootstrap(spec)).await?)
}

fn request(method: &str, params: Value) -> JsonRpcRequest {
    JsonRpcRequest {
        jsonrpc: JSONRPC_VERSION.to_string(),
        id: Some(json!(1)),
        method: method.to_string(),
        params,
    }
}

async fn call(runtime: &MobRuntime, method: &str, params: Value) -> Value {
    Box::pin(handle_console_runtime_rpc(
        runtime,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        request(method, params),
        true,
    ))
    .await
}

/// Spawn `alias` as a member of `role` with `labels` and run one committed
/// turn on it, so its session holds a transcript to fork.
pub(super) async fn seeded_member(
    runtime: &MobRuntime,
    role: &str,
    alias: &str,
    labels: &[(&str, &str)],
) -> TestResult {
    let identity = AgentIdentity::from(alias);
    let labels = labels
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect::<BTreeMap<_, _>>();
    Box::pin(
        runtime
            .handle()
            .spawn_spec(SpawnMemberSpec::new(role, identity.clone()).with_labels(labels)),
    )
    .await?;
    let member = runtime.handle().member(&identity).await?;
    let turn = member
        .start_turn(
            ContentInput::Text("The code word is heron.".to_string()),
            HandlingMode::Queue,
            meerkat_mob::MemberTurnOptions::new(),
            None,
        )
        .await?;
    turn.wait().await?;
    Ok(())
}

/// `meta/fork_member` for `source`, retried while the runtime still settles
/// the source's last turn (a typed busy refusal), as the console waits for an
/// idle agent.
pub(super) async fn fork_when_idle(runtime: &MobRuntime, source: &str, side_chat: &str) -> Value {
    let params = json!({ "source_member_id": source, "agent_identity": side_chat });
    for _ in 0..50 {
        let answer = call(runtime, "meta/fork_member", params.clone()).await;
        let busy = answer["error"]["data"]["kind"] == json!("fork_source_unavailable")
            && answer["error"]["data"]["cause"] == json!("running");
        if !busy {
            return answer;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    call(runtime, "meta/fork_member", params).await
}

async fn bridge_session(runtime: &MobRuntime, alias: &str) -> Option<String> {
    runtime
        .handle()
        .get_member(&AgentIdentity::from(alias))
        .await
        .ok()
        .flatten()
        .and_then(|entry| entry.bridge_session_id().map(ToString::to_string))
}

/// The sidebar row of `member` among an experience's sidebar `rows`.
fn sidebar_row(rows: &[Value], member: &str) -> Option<Value> {
    rows.iter()
        .find(|row| row["member_id"] == json!(member))
        .cloned()
}

/// The fork is a spawn of the side chat and a read of its source: the
/// console gates it on `agent.spawn` for the new member (the source's
/// `agent.view` is judged in the handler), refuses it on a read-only console
/// and on a candidate launch.
#[test]
fn fork_member_is_gated_as_a_spawn_of_the_side_chat() {
    assert_eq!(
        console_rpc_access_requirements(
            "meta/fork_member",
            &json!({ "source_member_id": "alpha", "agent_identity": "alpha-side" }),
        ),
        Some(vec![(ACTION_AGENT_SPAWN, Some("alpha-side".to_string()))])
    );
    assert!(is_console_mutating_rpc_method("meta/fork_member"));
    assert!(crate::rpc::CANDIDATE_LAUNCH_REFUSED_METHODS.contains(&"meta/fork_member"));
}

/// A side chat starts from its source's committed transcript, leaves the
/// source's session untouched, is seated without a peer edge, is projected
/// under its source from Meerkat's roster, and closes through the ordinary
/// member retire.
#[tokio::test]
async fn a_side_chat_forks_the_source_unwired_and_closes_on_retire() -> TestResult {
    let (_temp_dir, runtime) = persistent_console_runtime("console-side-chat-fork", "", "").await?;
    // The source's own console flags: a side chat must not take them.
    seeded_member(
        &runtime,
        "worker",
        "alpha",
        &[
            ("team", "review"),
            ("singleton", "true"),
            ("console_watched", "true"),
        ],
    )
    .await?;
    let source_session = bridge_session(&runtime, "alpha").await;
    assert!(source_session.is_some(), "the source has a session");

    let capabilities = call(&runtime, "meta/capabilities", json!({})).await;
    assert!(
        capabilities["result"]["methods"]
            .as_array()
            .is_some_and(|methods| methods.contains(&json!("meta/fork_member"))),
        "a console over persistent sessions offers side chats: {capabilities:#?}"
    );

    let forked = fork_when_idle(&runtime, "alpha", "alpha-side").await;
    assert!(forked["error"].is_null(), "the fork succeeds: {forked:#?}");
    let result = &forked["result"];
    assert_eq!(result["agent_identity"], json!("alpha-side"));
    assert_eq!(result["side_chat_of"], json!("alpha"));
    assert_eq!(result["source_member_id"], json!("alpha"));
    assert_eq!(result["retention"], json!("until_closed"));
    assert_eq!(result["display_name"], json!("alpha (side chat)"));
    assert_eq!(
        result["source_session_id"].as_str(),
        source_session.as_deref(),
        "the side chat names the source session it was forked from"
    );
    assert_ne!(
        result["session_id"].as_str(),
        source_session.as_deref(),
        "the side chat runs on its own session"
    );

    assert_eq!(
        bridge_session(&runtime, "alpha").await,
        source_session,
        "the source keeps its session"
    );
    let side = runtime
        .handle()
        .get_member(&AgentIdentity::from("alpha-side"))
        .await?
        .expect("the side chat is seated");
    assert!(side.wired_to.is_empty(), "a side chat has no peer edge");
    assert_eq!(
        side.definition_wiring,
        meerkat_mob::DefinitionWiring::Exempt,
        "the side chat is seated exempt from the definition's wiring rules"
    );
    assert!(side.spawned_by.is_none());
    assert_eq!(
        side.labels.get(crate::side_chat::SIDE_CHAT_OF_LABEL),
        Some(&"alpha".to_string())
    );
    assert_eq!(
        (
            side.labels.get("team").map(String::as_str),
            side.labels.get("singleton").map(String::as_str),
            side.labels.get("console_watched").map(String::as_str),
        ),
        (Some("review"), Some("false"), Some("false")),
        "the side chat keeps the source's application labels, not its console flags"
    );
    let source = runtime
        .handle()
        .get_member(&AgentIdentity::from("alpha"))
        .await?
        .expect("the source stays seated");
    assert!(source.wired_to.is_empty(), "the source gained no peer");

    let source_session_id = source_session.clone().expect("the source has a session");
    assert_eq!(
        crate::side_chat::verified_side_chats(&runtime.handle()).await,
        BTreeMap::from([(
            "alpha-side".to_string(),
            crate::side_chat::SideChatLineage {
                source_identity: "alpha".to_string(),
                source_session_id: source_session_id.clone(),
            },
        )]),
        "the roster's fork facts name the side chat's source and the session it was forked from"
    );
    let (snapshot, _) = Box::pin(build_live_snapshot(
        &runtime,
        &[],
        None,
        &AllowAllConsoleVisibilityPolicy,
        &ConsoleSnapshotReadModel::default(),
        None,
    ))
    .await;
    let side_row = snapshot
        .members
        .iter()
        .find(|member| member.agent_identity == "alpha-side")
        .expect("the side chat is a console member");
    assert_eq!(
        (
            side_row
                .labels
                .get(crate::side_chat::SIDE_CHAT_OF_LABEL)
                .map(String::as_str),
            side_row
                .labels
                .get(crate::side_chat::SIDE_CHAT_OF_SESSION_LABEL)
                .map(String::as_str),
        ),
        (Some("alpha"), Some(source_session_id.as_str())),
        "the console labels the side chat with the source the roster's fork facts name"
    );

    let retired = call(
        &runtime,
        "meta/retire_member",
        json!({ "member_id": "alpha-side" }),
    )
    .await;
    assert_eq!(retired["result"]["accepted"], json!(true), "{retired:#?}");
    assert_eq!(
        bridge_session(&runtime, "alpha").await,
        source_session,
        "closing the side chat leaves the source as it was"
    );
    Ok(())
}

/// A side chat's audience is its source's: the access rules follow a side
/// chat to its source for view, send and retire. A grant that names the
/// source's identity covers the side chat, on the console experience and on
/// every route's access priming alike, and a grant on another identity does
/// not.
#[tokio::test]
async fn a_grant_on_the_source_covers_its_side_chat_and_no_other_grant_does() -> TestResult {
    let (_temp_dir, runtime) =
        persistent_console_runtime("console-side-chat-access", "", "").await?;
    seeded_member(&runtime, "worker", "alpha", &[("team", "review")]).await?;
    seeded_member(&runtime, "worker", "gamma", &[]).await?;
    let forked = fork_when_idle(&runtime, "alpha", "alpha-side").await;
    assert!(forked["error"].is_null(), "the fork succeeds: {forked:#?}");

    let actions = [ACTION_AGENT_VIEW, ACTION_AGENT_SEND, ACTION_AGENT_RETIRE];
    let grant = |id: &str, subject: &str, agent: &str| AccessRule {
        id: id.to_string(),
        subjects: vec![subject.to_string()],
        actions: Vec::from(actions.map(String::from)),
        agents: vec![agent.to_string()],
        ..AccessRule::default()
    };
    let controller = AccessController::new(crate::access::AccessControlConfig {
        enabled: true,
        admins: vec!["root@example.test".to_string()],
        rules: vec![
            grant("source-grant", "source-reader@example.test", "alpha"),
            grant("other-grant", "other-reader@example.test", "gamma"),
        ],
        ..crate::access::AccessControlConfig::default()
    })?;
    let publication = meerkat_authorization::publication::LocalAuthorizationPublication::new();
    controller
        .join_authorization_publication(publication.clone())
        .map_err(|_| "a fresh controller joins its first publication")?;

    let (snapshot, _) = Box::pin(build_live_snapshot(
        &runtime,
        &[],
        None,
        &AllowAllConsoleVisibilityPolicy,
        &ConsoleSnapshotReadModel::default(),
        None,
    ))
    .await;
    // The console experience's sidebar rows for `subject`.
    let sidebar = |subject: &str| -> Vec<Value> {
        let decisions = RuntimeDecisionState::local_console(
            crate::ConsolePolicy {
                require_app_auth: false,
                ..Default::default()
            },
            None,
        );
        let response = crate::runtime::handle_console_rest_json_route_with_snapshot_and_access(
            &decisions,
            &ConsoleRestJsonRequest {
                method: "GET".to_string(),
                path: "/console/experience".to_string(),
                auth: Some(crate::ConsoleAccessRequest {
                    provider: crate::AuthProvider::GoogleOAuth,
                    email: subject.to_string(),
                }),
            },
            Some(&snapshot),
            Some(&controller),
        );
        assert_eq!(response.status, 200, "experience: {:?}", response.body);
        response.body["agent_sidebar"]["live_snapshot"]["agents"]
            .as_array()
            .cloned()
            .unwrap_or_default()
    };

    let source_reader_rows = sidebar("source-reader@example.test");
    let side_chat = sidebar_row(&source_reader_rows, "alpha-side")
        .expect("a grant on the source shows its side chat");
    assert_eq!(side_chat["side_chat_of"], json!("alpha"));
    assert_eq!(
        (
            &side_chat["affordances"]["can_send_message"],
            &side_chat["affordances"]["can_retire"],
        ),
        (&json!(true), &json!(true)),
        "a grant on the source lets its reader send to and close the side chat"
    );
    assert!(sidebar_row(&source_reader_rows, "gamma").is_none());

    let other_reader_rows = sidebar("other-reader@example.test");
    assert!(
        sidebar_row(&other_reader_rows, "alpha-side").is_none(),
        "a grant on another identity does not show the side chat"
    );
    assert!(sidebar_row(&other_reader_rows, "gamma").is_some());

    // Every route records the side chat's value as the experience does, so
    // alternating them publishes no access change.
    prime_access_cache_from_runtime(&runtime, &controller).await;
    let quiet = publication
        .observe(|| ())
        .map_err(|_| "a quiet publication")?
        .1;
    sidebar("source-reader@example.test");
    prime_access_cache_from_runtime(&runtime, &controller).await;
    assert!(
        quiet.check_current().is_ok(),
        "the console experience and the roster priming record one value"
    );
    let source_reader = controller.view_for_subject(Some("source-reader@example.test"));
    let other_reader = controller.view_for_subject(Some("other-reader@example.test"));
    for action in actions {
        assert!(
            source_reader.allows_agent(action, "alpha-side"),
            "a grant naming the source covers its side chat: {action}"
        );
        assert!(
            !other_reader.allows_agent(action, "alpha-side"),
            "a grant on another identity does not cover the side chat: {action}"
        );
    }
    Ok(())
}

/// The peers `alias` is wired to, as Meerkat's roster holds them, by public
/// alias.
async fn wired_to(
    runtime: &MobRuntime,
    alias: &str,
) -> Result<std::collections::BTreeSet<String>, Box<dyn std::error::Error + Send + Sync>> {
    let entry = runtime
        .handle()
        .get_member(&AgentIdentity::from(alias))
        .await?
        .ok_or_else(|| format!("{alias} is seated"))?;
    Ok(entry
        .wired_to
        .iter()
        .map(|peer| crate::member_comms_id::runtime_alias_str(peer.as_str()).into_owned())
        .collect())
}

fn peers(aliases: &[&str]) -> std::collections::BTreeSet<String> {
    aliases.iter().map(|alias| (*alias).to_string()).collect()
}

/// The wiring rules of the mob definition never touch a side chat, whatever
/// its source's role: a side chat of a role `auto_wire_orchestrator` wires to
/// the orchestrator is seated with no peer edge, and its source keeps the
/// edge the rule gave it.
#[tokio::test]
async fn a_side_chat_of_a_wired_role_is_seated_unwired() -> TestResult {
    let (_temp_dir, runtime) = persistent_console_runtime(
        "console-side-chat-wired",
        "orchestrator = \"lead\"",
        "[wiring]\nauto_wire_orchestrator = true\n",
    )
    .await?;
    seeded_member(&runtime, "lead", "lead", &[]).await?;
    seeded_member(&runtime, "worker", "beta", &[]).await?;
    assert_eq!(
        wired_to(&runtime, "beta").await?,
        peers(&["lead"]),
        "the definition wires the worker to the orchestrator"
    );

    let forked = fork_when_idle(&runtime, "beta", "beta-side").await;
    assert!(
        forked["error"].is_null(),
        "a side chat of a wired role forks: {forked:#?}"
    );
    assert!(
        wired_to(&runtime, "beta-side").await?.is_empty(),
        "the side chat is seated with no peer edge"
    );
    assert_eq!(
        wired_to(&runtime, "beta").await?,
        peers(&["lead"]),
        "the source keeps its edge"
    );
    assert_eq!(
        wired_to(&runtime, "lead").await?,
        peers(&["beta"]),
        "the orchestrator gained no edge to the side chat"
    );
    Ok(())
}

/// A side chat of the orchestrator is no target of the definition's wiring
/// rules either: a worker spawned after it, and a worker seated dormant after
/// it, are wired to the orchestrator alone.
#[tokio::test]
async fn a_member_seated_after_an_orchestrator_side_chat_is_not_wired_to_it() -> TestResult {
    let (_temp_dir, runtime) = persistent_gateway_runtime(
        "console-side-chat-orchestrator",
        "orchestrator = \"lead\"",
        "[wiring]\nauto_wire_orchestrator = true\n",
    )
    .await?;
    seeded_member(&runtime, "lead", "lead", &[]).await?;
    let forked = fork_when_idle(&runtime, "lead", "lead-side").await;
    assert!(
        forked["error"].is_null(),
        "the orchestrator forks a side chat: {forked:#?}"
    );

    Box::pin(
        runtime
            .handle()
            .spawn_spec(SpawnMemberSpec::new("worker", AgentIdentity::from("gamma"))),
    )
    .await?;
    let mut seat = SpawnMemberSpec::new("worker", AgentIdentity::from("delta"));
    seat.runtime_mode = Some(meerkat_mob::MobRuntimeMode::TurnDriven);
    let outcomes = Box::pin(runtime.handle().seat_dormant_members(vec![seat])).await?;
    assert!(
        matches!(
            outcomes.as_slice(),
            [meerkat_mob::DormantSeatOutcome::Seated { .. }]
        ),
        "the worker is seated dormant: {outcomes:?}"
    );

    assert_eq!(
        wired_to(&runtime, "gamma").await?,
        peers(&["lead"]),
        "a worker spawned after the side chat is wired to the orchestrator alone"
    );
    assert_eq!(
        wired_to(&runtime, "delta").await?,
        peers(&["lead"]),
        "a worker seated after the side chat is wired to the orchestrator alone"
    );
    assert!(
        wired_to(&runtime, "lead-side").await?.is_empty(),
        "no member is wired to the side chat"
    );
    Ok(())
}

/// A source seated and never woken has no conversation: its session was
/// never born. The fork is refused as a source without a session, typed,
/// before anything is forked, and the source is not woken for it.
#[tokio::test]
async fn an_unborn_source_is_refused_typed_and_nothing_is_forked() -> TestResult {
    let (_temp_dir, runtime) =
        persistent_gateway_runtime("console-side-chat-unborn", "", "").await?;
    let source = AgentIdentity::from("alpha");
    let mut seat = SpawnMemberSpec::new("worker", source.clone());
    seat.runtime_mode = Some(meerkat_mob::MobRuntimeMode::TurnDriven);
    let outcomes = Box::pin(runtime.handle().seat_dormant_members(vec![seat])).await?;
    let unborn = match outcomes.as_slice() {
        [meerkat_mob::DormantSeatOutcome::Seated { session_id, .. }] => session_id.clone(),
        other => panic!("the fresh spec is seated: {other:?}"),
    };
    assert!(
        runtime.handle().session_unborn(&unborn),
        "the seat built nothing"
    );

    let refused = call(
        &runtime,
        "meta/fork_member",
        json!({ "source_member_id": "alpha", "agent_identity": "alpha-side" }),
    )
    .await;
    assert_eq!(
        refused["error"]["code"],
        json!(crate::side_chat::SIDE_CHAT_FORK_REFUSED_CODE),
        "{refused:#?}"
    );
    assert_eq!(
        refused["error"]["data"],
        json!({
            "kind": "fork_source_unavailable",
            "source_member_id": "alpha",
            "cause": "no_session",
        })
    );
    assert!(
        runtime
            .handle()
            .get_member(&AgentIdentity::from("alpha-side"))
            .await?
            .is_none(),
        "nothing was seated"
    );
    assert!(
        runtime.handle().session_unborn(&unborn),
        "the refusal did not wake the source"
    );
    assert!(runtime.handle().member_unmaterialized(&source));
    Ok(())
}

/// Release `alias` to dormant, retried while the runtime still settles its
/// last turn.
async fn release_when_idle(runtime: &MobRuntime, alias: &str) -> TestResult {
    let member = AgentIdentity::from(alias);
    for _ in 0..50 {
        match runtime.handle().release_member(&member).await {
            Err(meerkat_mob::MobError::LifecycleOperationPending { .. }) => {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
            result => return Ok(result?),
        }
    }
    Ok(runtime.handle().release_member(&member).await?)
}

/// A dormant source (released after its turn, its session stored) forks from
/// its stored session: the fork reads the transcript without waking it.
#[tokio::test]
async fn a_dormant_source_forks_without_waking() -> TestResult {
    let (_temp_dir, runtime) =
        persistent_gateway_runtime("console-side-chat-dormant", "", "").await?;
    seeded_member(&runtime, "worker", "alpha", &[]).await?;
    release_when_idle(&runtime, "alpha").await?;
    let source = AgentIdentity::from("alpha");
    assert!(
        runtime.handle().member_unmaterialized(&source),
        "the released source is dormant"
    );

    let forked = fork_when_idle(&runtime, "alpha", "alpha-side").await;
    assert!(forked["error"].is_null(), "the fork succeeds: {forked:#?}");
    assert!(
        runtime.handle().member_unmaterialized(&source),
        "the fork read the stored session without waking the source"
    );
    assert!(
        wired_to(&runtime, "alpha-side").await?.is_empty(),
        "the side chat is seated unwired"
    );
    Ok(())
}

/// A side chat's frames are decided along its source, end to end: the
/// console owner captures the side chat's lineage from the live roster
/// through a read's member cut (the side chat, then the source incarnation it
/// was forked from, with its attributes), and the captured lineage decides
/// each frame. A grant on the source alone shows the side chat's frames, a
/// deny on the source hides them under a grant on every agent, and a grant on
/// the side chat's own name shows none.
#[tokio::test]
async fn a_side_chat_frame_is_decided_along_its_source() -> TestResult {
    use crate::console_aggregator::{
        ConsoleFrameSource, ConsoleFrameSourceKind, ConsoleFrameStatus, ConsoleLogStore as _,
        MetaConsoleAggregator, NewConsoleFrame,
    };

    let (_temp_dir, runtime) =
        persistent_console_runtime("console-side-chat-frames", "", "").await?;
    seeded_member(&runtime, "worker", "alpha", &[]).await?;
    let forked = fork_when_idle(&runtime, "alpha", "alpha-side").await;
    assert!(forked["error"].is_null(), "the fork succeeds: {forked:#?}");
    let side_session = forked["result"]["session_id"]
        .as_str()
        .expect("the side chat's session")
        .to_string();
    let source_session = bridge_session(&runtime, "alpha")
        .await
        .expect("the source's session");

    let aggregator = MetaConsoleAggregator::in_memory();
    aggregator.register_runtime_handles_with_policy(
        "side-chat-frames",
        "",
        runtime.clone(),
        None,
        ConsoleEventStore::new(),
        Arc::new(AllowAllConsoleVisibilityPolicy),
    );
    let provenance = aggregator
        .capture_member_provenance_for_test("side-chat-frames", "alpha-side", &side_session)
        .await
        .expect("the owner captures the live side chat");
    assert_eq!(provenance.lineage.len(), 2, "{provenance:#?}");
    assert_eq!(provenance.lineage[1].identity, "alpha");
    assert!(
        provenance.lineage[1].labels.is_some(),
        "the source is captured from the incarnation the side chat was forked from: \
         {provenance:#?}"
    );
    assert_eq!(
        provenance.lineage[0]
            .labels
            .as_ref()
            .and_then(|labels| labels.get(crate::side_chat::SIDE_CHAT_OF_SESSION_LABEL)),
        Some(&source_session)
    );
    let frame = aggregator
        .store()
        .append_if_absent(NewConsoleFrame {
            id: None,
            dedupe_key: "side-chat-frame".to_string(),
            timestamp_ms: 2_000,
            runtime_key: "side-chat-frames".to_string(),
            identity: "alpha-side".to_string(),
            conversation_id: Some("alpha-side".to_string()),
            session_id: Some(side_session.clone()),
            kind: "text_delta".to_string(),
            status: ConsoleFrameStatus::Completed,
            payload: json!({ "delta": "heron" }),
            source: ConsoleFrameSource {
                member_provenance: Some(provenance),
                kind: ConsoleFrameSourceKind::ConsoleEvent,
                source_cursor: None,
            },
            source_event_id: Some("side-chat-frame".to_string()),
            interaction_id: None,
            turn_id: None,
            run_id: None,
            parent_frame_id: None,
            caused_by_frame_id: None,
        })
        .await
        .expect("the frame is stored")
        .frame;

    let controller = AccessController::new(crate::access::AccessControlConfig {
        enabled: true,
        admins: vec!["root@example.test".to_string()],
        rules: vec![
            AccessRule {
                id: "the-source".to_string(),
                subjects: vec!["source-reader@example.test".to_string()],
                actions: vec![ACTION_AGENT_VIEW.to_string()],
                agents: vec!["alpha".to_string()],
                ..AccessRule::default()
            },
            AccessRule {
                id: "every-agent".to_string(),
                subjects: vec!["broad-reader@example.test".to_string()],
                actions: vec![ACTION_AGENT_VIEW.to_string()],
                agents: vec!["*".to_string()],
                ..AccessRule::default()
            },
            AccessRule {
                id: "not-the-source".to_string(),
                effect: crate::access::AccessEffect::Deny,
                subjects: vec!["broad-reader@example.test".to_string()],
                actions: vec!["agent.*".to_string()],
                agents: vec!["alpha".to_string()],
                ..AccessRule::default()
            },
            AccessRule {
                id: "the-side-chat-name".to_string(),
                subjects: vec!["name-reader@example.test".to_string()],
                actions: vec![ACTION_AGENT_VIEW.to_string()],
                agents: vec!["alpha-side".to_string()],
                ..AccessRule::default()
            },
        ],
        ..crate::access::AccessControlConfig::default()
    })?;
    assert!(
        crate::console_aggregator::timeline_frame_visible_to(
            &controller.view_for_subject(Some("source-reader@example.test")),
            &frame,
        ),
        "a grant on the source shows the side chat's frame"
    );
    assert!(
        !crate::console_aggregator::timeline_frame_visible_to(
            &controller.view_for_subject(Some("name-reader@example.test")),
            &frame,
        ),
        "a grant on the side chat's own name shows none of its frames"
    );
    assert!(
        !crate::console_aggregator::timeline_frame_visible_to(
            &controller.view_for_subject(Some("broad-reader@example.test")),
            &frame,
        ),
        "a deny on the source hides the side chat's frame under a broad grant"
    );
    Ok(())
}

/// A side chat's fork facts never change for its incarnation, so the console
/// reads them from Meerkat's roster once and keeps them: one identity read
/// that records two side chats reads them once, a later read and a side
/// chat's stream events read them no more. The aggregator runs no session
/// history backfills, so nothing else reads while the reads are counted (its
/// registration's identity refresh and the identity read share one lock).
#[tokio::test]
async fn one_console_read_reads_the_fork_facts_once() -> TestResult {
    use crate::console_aggregator::{ConsoleAggregatorOptions, MetaConsoleAggregator};

    let (_temp_dir, runtime) =
        persistent_console_runtime("console-side-chat-reads", "", "").await?;
    seeded_member(&runtime, "worker", "alpha", &[]).await?;
    for side_chat in ["alpha-side", "alpha-draft"] {
        let forked = fork_when_idle(&runtime, "alpha", side_chat).await;
        assert!(forked["error"].is_null(), "the fork succeeds: {forked:#?}");
    }
    let reads = || runtime.side_chat_lineages().reads();
    let before = reads();

    let aggregator = MetaConsoleAggregator::in_memory_with_options(ConsoleAggregatorOptions {
        session_history_backfill_enabled: false,
        ..ConsoleAggregatorOptions::default()
    });
    aggregator.register_runtime_handles_with_policy(
        "side-chat-reads",
        "",
        runtime.clone(),
        None,
        ConsoleEventStore::new(),
        Arc::new(AllowAllConsoleVisibilityPolicy),
    );
    let identities = aggregator
        .list_identities_fresh()
        .await
        .expect("an identity read");
    for side_chat in ["alpha-side", "alpha-draft"] {
        let record = identities
            .iter()
            .find(|record| record.identity == side_chat)
            .unwrap_or_else(|| panic!("{side_chat} is listed: {identities:#?}"));
        assert_eq!(
            record
                .labels
                .get(crate::side_chat::SIDE_CHAT_OF_LABEL)
                .map(String::as_str),
            Some("alpha"),
            "{side_chat} is recorded under its source"
        );
    }
    assert_eq!(
        reads() - before,
        1,
        "one identity read reads the fork facts once for every side chat it records"
    );

    aggregator
        .list_identities_fresh()
        .await
        .expect("a later identity read");
    let side = runtime
        .handle()
        .get_member(&AgentIdentity::from("alpha-side"))
        .await?
        .expect("the side chat is seated");
    for _ in 0..3 {
        let projection = sse_member_authorization_projection(
            &runtime.handle(),
            Some(&runtime),
            &AllowAllConsoleVisibilityPolicy,
            "alpha-side",
            &side.agent_runtime_id,
            side.fence_token,
        )
        .await
        .expect("the side chat's events are authorized");
        assert_eq!(
            projection
                .attributes
                .labels
                .get(crate::side_chat::SIDE_CHAT_OF_LABEL)
                .map(String::as_str),
            Some("alpha"),
            "an event of the side chat is decided along its source"
        );
    }
    assert_eq!(
        reads() - before,
        1,
        "a later read and the side chat's stream events read no fork facts"
    );
    Ok(())
}

/// The fork takes the source's committed transcript end only: a prefix is
/// refused, not ignored, and the request names both members.
#[tokio::test]
async fn a_fork_request_names_both_members_and_takes_no_prefix() -> TestResult {
    let (_temp_dir, runtime) =
        persistent_console_runtime("console-side-chat-params", "", "").await?;
    let missing = call(
        &runtime,
        "meta/fork_member",
        json!({ "source_member_id": "alpha" }),
    )
    .await;
    assert_eq!(missing["error"]["code"], json!(-32602), "{missing:#?}");
    let prefix = call(
        &runtime,
        "meta/fork_member",
        json!({
            "source_member_id": "alpha",
            "agent_identity": "alpha-side",
            "message_count": 2,
        }),
    )
    .await;
    assert_eq!(prefix["error"]["code"], json!(-32602), "{prefix:#?}");
    let unknown = call(
        &runtime,
        "meta/fork_member",
        json!({ "source_member_id": "nobody", "agent_identity": "nobody-side" }),
    )
    .await;
    assert_eq!(unknown["error"]["code"], json!(-32602), "{unknown:#?}");
    Ok(())
}

/// An access rule `id` that gives `subject` `actions` on `agent`.
fn grant(id: &str, subject: &str, actions: &[&str], agent: &str) -> AccessRule {
    AccessRule {
        id: id.to_string(),
        subjects: vec![subject.to_string()],
        actions: actions.iter().map(|action| (*action).to_string()).collect(),
        agents: vec![agent.to_string()],
        ..AccessRule::default()
    }
}

/// An access rule `id` that denies `subject` every agent action on `agent`.
fn deny(id: &str, subject: &str, agent: &str) -> AccessRule {
    AccessRule {
        id: id.to_string(),
        effect: crate::access::AccessEffect::Deny,
        subjects: vec![subject.to_string()],
        actions: vec!["agent.*".to_string()],
        agents: vec![agent.to_string()],
        ..AccessRule::default()
    }
}

/// An enforced access policy over `rules`.
fn enforced(
    rules: Vec<AccessRule>,
) -> Result<AccessController, Box<dyn std::error::Error + Send + Sync>> {
    Ok(AccessController::new(crate::access::AccessControlConfig {
        enabled: true,
        admins: vec!["root@example.test".to_string()],
        rules,
        ..crate::access::AccessControlConfig::default()
    })?)
}

/// `meta/fork_member` as `subject`, a principal of `controller`, calls it on
/// the console route, retried while the runtime still settles the source's
/// last turn.
async fn fork_as(
    runtime: &MobRuntime,
    controller: &AccessController,
    subject: &str,
    source: &str,
    side_chat: &str,
) -> Value {
    let view = controller.view_for_subject(Some(subject));
    let params = json!({ "source_member_id": source, "agent_identity": side_chat });
    let mut answer = Value::Null;
    for _ in 0..50 {
        answer = Box::pin(handle_console_runtime_rpc_with_visibility(
            runtime,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            &AllowAllConsoleVisibilityPolicy,
            request("meta/fork_member", params.clone()),
            true,
            false,
            Some(subject),
            Some(controller),
            Some(&view),
            None,
            None,
            None,
            None,
            None,
            &crate::http_console::ConsoleSendGovernance::default(),
            &crate::topology_control::TopologyApplyScope::Anonymous,
        ))
        .await;
        let busy = answer["error"]["data"]["kind"] == json!("fork_source_unavailable")
            && answer["error"]["data"]["cause"] == json!("running");
        if !busy {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    answer
}

/// Forking a side chat needs `agent.view` and `agent.send` on its source and
/// `agent.spawn` on its name, and every decision on the side chat is its
/// source's: a caller who may only read the source, or only send to it,
/// cannot fork it, nor can one who drives the source without spawn on the
/// side chat's name, and grants on the side chat's own name do not let a
/// caller drive or close it.
#[tokio::test]
async fn forking_needs_view_and_send_on_the_source_and_the_side_chat_is_decided_as_it() -> TestResult
{
    let (_temp_dir, runtime) = persistent_console_runtime("console-side-chat-gate", "", "").await?;
    seeded_member(&runtime, "worker", "alpha", &[]).await?;
    let controller = enforced(vec![
        grant(
            "reader-reads-the-source",
            "reader@example.test",
            &[ACTION_AGENT_VIEW],
            "alpha",
        ),
        grant(
            "reader-holds-the-name",
            "reader@example.test",
            &[
                ACTION_AGENT_SPAWN,
                ACTION_AGENT_VIEW,
                ACTION_AGENT_SEND,
                ACTION_AGENT_RETIRE,
            ],
            "alpha-side",
        ),
        grant(
            "driver-drives-the-source",
            "driver@example.test",
            &[ACTION_AGENT_VIEW, ACTION_AGENT_SEND],
            "alpha",
        ),
        grant(
            "driver-spawns-the-name",
            "driver@example.test",
            &[ACTION_AGENT_SPAWN],
            "alpha-side",
        ),
        grant(
            "sender-sends-to-the-source",
            "sender@example.test",
            &[ACTION_AGENT_SEND],
            "alpha",
        ),
        grant(
            "sender-spawns-the-name",
            "sender@example.test",
            &[ACTION_AGENT_SPAWN],
            "alpha-side",
        ),
        grant(
            "nameless-drives-the-source",
            "nameless@example.test",
            &[ACTION_AGENT_VIEW, ACTION_AGENT_SEND],
            "alpha",
        ),
    ])?;

    for (subject, action, why) in [
        (
            "sender@example.test",
            ACTION_AGENT_VIEW,
            "send alone on the source does not fork it",
        ),
        (
            "nameless@example.test",
            ACTION_AGENT_SPAWN,
            "driving the source does not fork it without spawn on the side chat's name",
        ),
    ] {
        let refused = fork_as(&runtime, &controller, subject, "alpha", "alpha-side").await;
        assert_eq!(
            refused["error"]["code"],
            json!(ACCESS_DENIED_RPC_CODE),
            "{subject}: {refused:#?}"
        );
        assert_eq!(refused["error"]["data"]["action"], json!(action), "{why}");
    }

    let refused = fork_as(
        &runtime,
        &controller,
        "reader@example.test",
        "alpha",
        "alpha-side",
    )
    .await;
    assert_eq!(
        refused["error"]["code"],
        json!(ACCESS_DENIED_RPC_CODE),
        "{refused:#?}"
    );
    assert_eq!(
        refused["error"]["data"]["action"],
        json!(ACTION_AGENT_SEND),
        "view alone on the source does not fork it"
    );
    assert!(
        runtime
            .handle()
            .get_member(&AgentIdentity::from("alpha-side"))
            .await?
            .is_none(),
        "nothing was forked"
    );

    let forked = fork_as(
        &runtime,
        &controller,
        "driver@example.test",
        "alpha",
        "alpha-side",
    )
    .await;
    assert!(
        forked["error"].is_null(),
        "view and send on the source and spawn on the name fork it: {forked:#?}"
    );

    prime_access_cache_from_runtime(&runtime, &controller).await;
    let reader = controller.view_for_subject(Some("reader@example.test"));
    let driver = controller.view_for_subject(Some("driver@example.test"));
    assert!(
        reader.allows_agent(ACTION_AGENT_VIEW, "alpha-side"),
        "the reader reads the side chat, as it reads the source"
    );
    for action in [ACTION_AGENT_SEND, ACTION_AGENT_RETIRE] {
        assert!(
            !reader.allows_agent(action, "alpha-side"),
            "grants on the side chat's own name do not widen it: {action}"
        );
    }
    assert!(
        driver.allows_agent(ACTION_AGENT_SEND, "alpha-side"),
        "the driver drives the side chat, as it drives the source"
    );
    assert!(
        !driver.allows_agent(ACTION_AGENT_RETIRE, "alpha-side"),
        "the source grants no retire, so neither does the side chat"
    );
    Ok(())
}

/// The attributes the console decides `alias`'s stream events with: the
/// exact authority of its current roster incarnation.
async fn event_attributes(
    runtime: &MobRuntime,
    alias: &str,
) -> Result<AgentResourceAttributes, Box<dyn std::error::Error + Send + Sync>> {
    let entry = runtime
        .handle()
        .get_member(&AgentIdentity::from(alias))
        .await?
        .ok_or_else(|| format!("{alias} is seated"))?;
    let projection = sse_member_authorization_projection(
        &runtime.handle(),
        Some(runtime),
        &AllowAllConsoleVisibilityPolicy,
        alias,
        &entry.agent_runtime_id,
        entry.fence_token,
    )
    .await
    .ok_or_else(|| format!("{alias}'s events are attributed"))?;
    Ok(projection.attributes)
}

/// Respawn `alias`, retried while the runtime still settles its last turn.
async fn respawn_when_idle(runtime: &MobRuntime, alias: &str) -> TestResult {
    for _ in 0..50 {
        match runtime
            .handle()
            .respawn(AgentIdentity::from(alias), None)
            .await
        {
            Ok(_) => return Ok(()),
            Err(meerkat_mob::MobRespawnError::Mob(
                meerkat_mob::MobError::LifecycleOperationPending { .. },
            )) => tokio::time::sleep(std::time::Duration::from_millis(100)).await,
            Err(error) => return Err(error.to_string().into()),
        }
    }
    runtime
        .handle()
        .respawn(AgentIdentity::from(alias), None)
        .await
        .map(|_| ())
        .map_err(|error| error.to_string().into())
}

/// A side chat is decided along its source on every surface, from the
/// roster's fork facts only: the console's member rows, every route's access
/// priming and its stream events. A member that carries side chat labels no
/// fork made (`forger`) has no lineage on any of them, so a grant on the
/// source does not reach it and a deny on the source does not hide it. A
/// deny on the source still hides the side chat once the source is
/// respawned, and once it is retired.
#[tokio::test]
async fn a_side_chat_follows_its_source_on_every_surface_and_a_forged_label_on_none() -> TestResult
{
    let (_temp_dir, runtime) =
        persistent_console_runtime("console-side-chat-surfaces", "", "").await?;
    seeded_member(&runtime, "worker", "alpha", &[]).await?;
    Box::pin(runtime.handle().spawn_spec(
        SpawnMemberSpec::new("worker", AgentIdentity::from("forger")).with_labels(BTreeMap::from(
            [
                (
                    crate::side_chat::SIDE_CHAT_OF_LABEL.to_string(),
                    "alpha".to_string(),
                ),
                (
                    crate::side_chat::SIDE_CHAT_OF_SESSION_LABEL.to_string(),
                    "forged-session".to_string(),
                ),
            ],
        )),
    ))
    .await?;
    let forked = fork_when_idle(&runtime, "alpha", "alpha-side").await;
    assert!(forked["error"].is_null(), "the fork succeeds: {forked:#?}");

    let controller = enforced(vec![
        grant(
            "the-source",
            "source-reader@example.test",
            &[ACTION_AGENT_VIEW],
            "alpha",
        ),
        grant(
            "every-agent",
            "broad-reader@example.test",
            &[ACTION_AGENT_VIEW],
            "*",
        ),
        deny("not-the-source", "broad-reader@example.test", "alpha"),
        grant(
            "the-side-chat-name",
            "name-reader@example.test",
            &[ACTION_AGENT_VIEW],
            "alpha-side",
        ),
    ])?;
    prime_access_cache_from_runtime(&runtime, &controller).await;
    let source_reader = controller.view_for_subject(Some("source-reader@example.test"));
    let broad_reader = controller.view_for_subject(Some("broad-reader@example.test"));
    let name_reader = controller.view_for_subject(Some("name-reader@example.test"));

    // The console's member rows.
    let (snapshot, _) = Box::pin(build_live_snapshot(
        &runtime,
        &[],
        None,
        &AllowAllConsoleVisibilityPolicy,
        &ConsoleSnapshotReadModel::default(),
        None,
    ))
    .await;
    let row_labels = |member: &str| {
        snapshot
            .members
            .iter()
            .find(|row| row.agent_identity == member)
            .map(|row| row.labels.clone())
            .unwrap_or_else(|| panic!("{member} is a console member"))
    };
    assert_eq!(
        row_labels("alpha-side")
            .get(crate::side_chat::SIDE_CHAT_OF_LABEL)
            .map(String::as_str),
        Some("alpha")
    );
    assert_eq!(
        crate::console_spawn::lineage_parent(&row_labels("forger")),
        None,
        "a forged side chat label is no lineage on the member rows"
    );

    // Every route's access priming.
    assert!(source_reader.can_view_agent("alpha-side"));
    assert!(!broad_reader.can_view_agent("alpha-side"));
    assert!(
        !name_reader.can_view_agent("alpha-side"),
        "a grant on the side chat's own name grants nothing"
    );
    assert!(
        !source_reader.can_view_agent("forger"),
        "a forged label grants nothing"
    );
    assert!(
        broad_reader.can_view_agent("forger"),
        "a forged label hides nothing"
    );

    // Stream events, by the exact authority of the incarnation that emits
    // them.
    let side_events = event_attributes(&runtime, "alpha-side").await?;
    assert!(
        source_reader
            .decide_agent_with_attributes(ACTION_AGENT_VIEW, &side_events)
            .is_allow()
    );
    assert!(
        !broad_reader
            .decide_agent_with_attributes(ACTION_AGENT_VIEW, &side_events)
            .is_allow()
    );
    assert!(
        !name_reader
            .decide_agent_with_attributes(ACTION_AGENT_VIEW, &side_events)
            .is_allow()
    );
    let forger_events = event_attributes(&runtime, "forger").await?;
    assert_eq!(
        crate::console_spawn::lineage_parent(&forger_events.labels),
        None,
        "a forged side chat label is no lineage on stream events"
    );
    assert!(
        !source_reader
            .decide_agent_with_attributes(ACTION_AGENT_VIEW, &forger_events)
            .is_allow()
    );
    assert!(
        broad_reader
            .decide_agent_with_attributes(ACTION_AGENT_VIEW, &forger_events)
            .is_allow()
    );

    // A respawned source: the side chat follows the identity.
    respawn_when_idle(&runtime, "alpha").await?;
    prime_access_cache_from_runtime(&runtime, &controller).await;
    assert!(
        !broad_reader.can_view_agent("alpha-side"),
        "a deny on the source hides the side chat after the source is respawned"
    );
    assert!(source_reader.can_view_agent("alpha-side"));

    // A retired source: the side chat keeps naming it.
    runtime
        .handle()
        .retire(AgentIdentity::from("alpha"))
        .await?;
    prime_access_cache_from_runtime(&runtime, &controller).await;
    assert!(
        !broad_reader.can_view_agent("alpha-side"),
        "a deny on the source hides the side chat after the source is retired"
    );
    assert!(
        source_reader.can_view_agent("alpha-side"),
        "a grant on the source still reaches the side chat after the source is retired"
    );
    Ok(())
}

/// A side chat is decided along its source after a restart: the restored
/// side chat's lineage is read again from the roster's durable fork record.
#[tokio::test]
async fn a_side_chat_follows_its_source_after_a_restart() -> TestResult {
    let state = tempfile::tempdir()?;
    let runtime =
        persistent_gateway_runtime_in(state.path(), "console-side-chat-restart", "", "").await?;
    seeded_member(&runtime, "worker", "alpha", &[]).await?;
    let forked = fork_when_idle(&runtime, "alpha", "alpha-side").await;
    assert!(forked["error"].is_null(), "the fork succeeds: {forked:#?}");
    runtime.handle().shutdown().await?;
    let _ = runtime
        .stand_down_child_mobs(std::time::Instant::now() + std::time::Duration::from_secs(90))
        .await;
    drop(runtime);

    let runtime =
        persistent_gateway_runtime_in(state.path(), "console-side-chat-restart", "", "").await?;
    assert_eq!(
        crate::side_chat::verified_side_chats(&runtime.handle())
            .await
            .get("alpha-side")
            .map(|lineage| lineage.source_identity.clone()),
        Some("alpha".to_string()),
        "the restored roster's fork record names the side chat's source"
    );
    let controller = enforced(vec![
        grant(
            "the-source",
            "source-reader@example.test",
            &[ACTION_AGENT_VIEW],
            "alpha",
        ),
        grant(
            "every-agent",
            "broad-reader@example.test",
            &[ACTION_AGENT_VIEW],
            "*",
        ),
        deny("not-the-source", "broad-reader@example.test", "alpha"),
    ])?;
    prime_access_cache_from_runtime(&runtime, &controller).await;
    assert!(
        controller
            .view_for_subject(Some("source-reader@example.test"))
            .can_view_agent("alpha-side"),
        "a grant on the source reaches its restored side chat"
    );
    assert!(
        !controller
            .view_for_subject(Some("broad-reader@example.test"))
            .can_view_agent("alpha-side"),
        "a deny on the source hides its restored side chat"
    );
    Ok(())
}

/// A Console over several runtimes names a side chat's source in the side
/// chat's runtime namespace, as it names the side chat itself: in the access
/// attributes it records, on its member rows and in its identity list calls.
/// A rule on the prefixed source reaches its side chat, and one on the bare
/// name does not reach it through its lineage.
#[tokio::test]
async fn a_namespaced_console_names_a_side_chat_source_in_its_namespace() -> TestResult {
    use crate::console_aggregator::MetaConsoleAggregator;

    let attributes = namespace_console_attributes(
        AgentResourceAttributes {
            identity: "alpha-side".to_string(),
            agent_id: Some("alpha-side".to_string()),
            role: Some("worker".to_string()),
            labels: BTreeMap::from([
                (
                    crate::side_chat::SIDE_CHAT_OF_LABEL.to_string(),
                    "alpha".to_string(),
                ),
                (
                    crate::side_chat::SIDE_CHAT_OF_SESSION_LABEL.to_string(),
                    "alpha-session".to_string(),
                ),
            ]),
        },
        "b",
    );
    assert_eq!(attributes.identity, "b/alpha-side");
    assert_eq!(
        attributes
            .labels
            .get(crate::side_chat::SIDE_CHAT_OF_LABEL)
            .map(String::as_str),
        Some("b/alpha")
    );
    assert_eq!(
        attributes
            .labels
            .get(crate::side_chat::SIDE_CHAT_OF_SESSION_LABEL)
            .map(String::as_str),
        Some("alpha-session"),
        "a session id is not an identity"
    );

    let (_temp_dir, runtime) =
        persistent_console_runtime("console-side-chat-namespaced", "", "").await?;
    seeded_member(&runtime, "worker", "alpha", &[]).await?;
    Box::pin(
        runtime
            .handle()
            .spawn_spec(SpawnMemberSpec::new("worker", AgentIdentity::from("gamma"))),
    )
    .await?;
    let forked = fork_when_idle(&runtime, "alpha", "alpha-side").await;
    assert!(forked["error"].is_null(), "the fork succeeds: {forked:#?}");
    let aggregator = MetaConsoleAggregator::in_memory();
    aggregator.register_runtime_handles_with_policy(
        "runtime-b",
        "b",
        runtime.clone(),
        None,
        ConsoleEventStore::new(),
        Arc::new(AllowAllConsoleVisibilityPolicy),
    );
    let snapshot = build_aggregator_live_snapshot(&aggregator, &[]).await?;
    let side_row = snapshot
        .members
        .iter()
        .find(|member| member.agent_identity == "b/alpha-side")
        .ok_or("the side chat's aggregate row")?;
    assert_eq!(
        side_row
            .labels
            .get(crate::side_chat::SIDE_CHAT_OF_LABEL)
            .map(String::as_str),
        Some("b/alpha"),
        "the aggregate row names the source in the runtime's namespace"
    );

    let controller = enforced(vec![
        grant(
            "the-prefixed-source",
            "source-reader@example.test",
            &[ACTION_AGENT_VIEW],
            "b/alpha",
        ),
        grant(
            "the-bare-name",
            "bare-reader@example.test",
            &[ACTION_AGENT_VIEW],
            "alpha",
        ),
        grant(
            "every-agent",
            "broad-reader@example.test",
            &[ACTION_AGENT_VIEW],
            "*",
        ),
        deny("not-the-source", "broad-reader@example.test", "b/alpha"),
    ])?;
    let listed = |subject: &'static str| {
        let view = controller.view_for_subject(Some(subject));
        let aggregator = aggregator.clone();
        async move {
            let mut names = visible_identity_records(&aggregator, Some(&view))
                .await
                .expect("the identity list")
                .into_iter()
                .map(|record| record.identity)
                .collect::<Vec<_>>();
            names.sort();
            names
        }
    };
    assert_eq!(
        listed("source-reader@example.test").await,
        ["b/alpha", "b/alpha-side"],
        "a rule on the prefixed source lists its side chat"
    );
    assert_eq!(
        listed("bare-reader@example.test").await,
        ["b/alpha"],
        "a rule on the bare name reaches the source by its member id, not its side chat"
    );
    assert_eq!(
        listed("broad-reader@example.test").await,
        ["b/gamma"],
        "a deny on the prefixed source hides its side chat"
    );
    Ok(())
}

/// A side chat is closed and forked again, never respawned or reset: a
/// successor would be no fork of its agent, so it would be decided as itself
/// and wired by the definition. `meta/respawn`, `meta/respawn_member` and the
/// live fallback of `meta/reset` refuse it typed and leave it as it was, and
/// its sidebar row offers no respawn.
#[tokio::test]
async fn a_side_chat_is_never_respawned_or_reset() -> TestResult {
    let (_temp_dir, runtime) =
        persistent_console_runtime("console-side-chat-restart", "", "").await?;
    seeded_member(&runtime, "worker", "alpha", &[]).await?;
    let forked = fork_when_idle(&runtime, "alpha", "alpha-side").await;
    assert!(forked["error"].is_null(), "the fork succeeds: {forked:#?}");
    let session = bridge_session(&runtime, "alpha-side").await;
    assert!(session.is_some(), "the side chat has a session");

    for (method, params, operation) in [
        (
            "meta/respawn",
            json!({ "identity": "alpha-side" }),
            "respawn",
        ),
        (
            "meta/respawn_member",
            json!({ "member_id": "alpha-side" }),
            "respawn",
        ),
        ("meta/reset", json!({ "identity": "alpha-side" }), "reset"),
    ] {
        let refused = call(&runtime, method, params).await;
        assert_eq!(
            refused["error"]["code"],
            json!(crate::side_chat::SIDE_CHAT_RESTART_REFUSED_CODE),
            "{method}: {refused:#?}"
        );
        assert_eq!(
            refused["error"]["data"],
            json!({
                "kind": "side_chat_restart_refused",
                "operation": operation,
                "agent_identity": "alpha-side",
                "source_member_id": "alpha",
            }),
            "{method}"
        );
    }
    assert_eq!(
        bridge_session(&runtime, "alpha-side").await,
        session,
        "the side chat keeps its session"
    );
    assert!(
        crate::side_chat::verified_side_chats(&runtime.handle())
            .await
            .contains_key("alpha-side"),
        "the side chat is still a fork of its agent"
    );

    let (snapshot, _) = Box::pin(build_live_snapshot(
        &runtime,
        &[],
        None,
        &AllowAllConsoleVisibilityPolicy,
        &ConsoleSnapshotReadModel::default(),
        None,
    ))
    .await;
    let response = crate::runtime::handle_console_rest_json_route_with_snapshot_and_access(
        &RuntimeDecisionState::local_console(
            crate::ConsolePolicy {
                require_app_auth: false,
                ..Default::default()
            },
            None,
        ),
        &ConsoleRestJsonRequest {
            method: "GET".to_string(),
            path: "/console/experience".to_string(),
            auth: None,
        },
        Some(&snapshot),
        None,
    );
    assert_eq!(response.status, 200, "experience: {:?}", response.body);
    let rows = response.body["agent_sidebar"]["live_snapshot"]["agents"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        sidebar_row(&rows, "alpha-side").ok_or("the side chat's row")?["affordances"]["can_respawn"],
        json!(false),
        "a side chat's row offers no respawn"
    );
    assert_eq!(
        sidebar_row(&rows, "alpha").ok_or("the agent's row")?["affordances"]["can_respawn"],
        json!(true),
        "its agent's row still does"
    );
    Ok(())
}

/// An agent's `mob_respawn` tool never respawns a side chat either: Meta's
/// interception of the agent's mob tools refuses it as a typed policy denial
/// of kind `side_chat_restart_refused` before meerkat's tool runs, and the
/// side chat keeps its session and its fork. The same tool still reaches
/// meerkat's `mob_respawn` for the side chat's agent.
#[tokio::test]
async fn an_agent_mob_respawn_tool_never_respawns_a_side_chat() -> TestResult {
    use meerkat_core::AgentToolDispatcher as _;

    /// Stands in for meerkat's mob tools: records the member each call
    /// names and answers it done.
    #[derive(Default)]
    struct RecordingTools(std::sync::Mutex<Vec<String>>);
    impl RecordingTools {
        fn calls(&self) -> Vec<String> {
            self.0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        }
    }
    #[async_trait::async_trait]
    impl meerkat_core::AgentToolDispatcher for RecordingTools {
        fn tools(&self) -> Arc<[Arc<meerkat_core::types::ToolDef>]> {
            Arc::from(Vec::new())
        }
        async fn dispatch(
            &self,
            call: meerkat_core::types::ToolCallView<'_>,
        ) -> Result<meerkat_core::ToolDispatchOutcome, meerkat_core::ToolError> {
            let args: Value = serde_json::from_str(call.args.get()).map_err(|error| {
                meerkat_core::ToolError::invalid_arguments(call.name, error.to_string())
            })?;
            let member = args["agent_identity"].as_str().unwrap_or_default();
            self.0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(member.to_string());
            Ok(meerkat_core::ToolDispatchOutcome::sync_result(
                meerkat_core::types::ToolResult::new(call.id.to_string(), "{}".to_string(), false),
            ))
        }
    }

    const MOB: &str = "console-side-chat-agent-respawn";
    let (_temp_dir, runtime) = persistent_gateway_runtime(MOB, "", "").await?;
    seeded_member(&runtime, "worker", "alpha", &[]).await?;
    let forked = fork_when_idle(&runtime, "alpha", "alpha-side").await;
    assert!(forked["error"].is_null(), "the fork succeeds: {forked:#?}");
    let session = bridge_session(&runtime, "alpha-side").await;
    assert!(session.is_some(), "the side chat has a session");

    let tools = Arc::new(RecordingTools::default());
    let overrides = runtime
        .implicit_delegate_retirement_overrides()
        .ok_or("the runtime installs the agent mob tools")?;
    let dispatcher =
        crate::mob_handle_runtime::agent_mob_tool_wrapper_for_test(tools.clone(), overrides);
    let respawn_args = |member: &str| {
        serde_json::value::RawValue::from_string(
            json!({ "mob_id": MOB, "agent_identity": member }).to_string(),
        )
    };

    let side_chat_args = respawn_args("alpha-side")?;
    let refused = dispatcher
        .dispatch_with_context(
            meerkat_core::types::ToolCallView {
                id: "respawn-side-chat",
                name: "mob_respawn",
                args: &side_chat_args,
            },
            &meerkat_core::ToolDispatchContext::default(),
        )
        .await;
    match refused {
        Err(meerkat_core::ToolError::PolicyDenied { denial }) => {
            assert_eq!(denial.code, "side_chat_restart_refused");
            assert!(
                denial.message.contains("close it and fork alpha again"),
                "{}",
                denial.message
            );
        }
        other => panic!("the side chat's respawn is refused as a policy denial: {other:?}"),
    }
    assert!(
        tools.calls().is_empty(),
        "meerkat's mob_respawn never ran for the side chat"
    );
    assert_eq!(
        bridge_session(&runtime, "alpha-side").await,
        session,
        "the side chat keeps its session"
    );
    assert!(
        crate::side_chat::verified_side_chats(&runtime.handle())
            .await
            .contains_key("alpha-side"),
        "the side chat is still a fork of its agent"
    );

    let agent_args = respawn_args("alpha")?;
    let outcome = dispatcher
        .dispatch_with_context(
            meerkat_core::types::ToolCallView {
                id: "respawn-agent",
                name: "mob_respawn",
                args: &agent_args,
            },
            &meerkat_core::ToolDispatchContext::default(),
        )
        .await?;
    assert!(!outcome.result.is_error, "the agent is respawned");
    assert_eq!(
        tools.calls(),
        vec!["alpha".to_string()],
        "the agent's respawn reaches meerkat's mob_respawn"
    );
    Ok(())
}

/// `meta/reset_all` never resets a side chat, under a name the runtime
/// declares as a baseline member either: it reports the side chat as a
/// member it could not reset (`side_chat_restart_refused`) and leaves it as
/// it was, while it still acts on every other member.
#[tokio::test]
async fn reset_all_reports_a_side_chat_as_a_member_it_could_not_reset() -> TestResult {
    let (_temp_dir, runtime) =
        persistent_console_runtime("console-side-chat-reset-all", "", "").await?;
    seeded_member(&runtime, "worker", "alpha", &[]).await?;
    let forked = fork_when_idle(&runtime, "alpha", "alpha-side").await;
    assert!(forked["error"].is_null(), "the fork succeeds: {forked:#?}");
    let session = bridge_session(&runtime, "alpha-side").await;
    assert!(session.is_some(), "the side chat has a session");
    let baseline = SpawnMemberSpec::new("worker", AgentIdentity::from("alpha-side"));
    runtime.set_baseline_member_specs(vec![baseline]).await;

    let answer = call(&runtime, "meta/reset_all", json!({})).await;
    assert_eq!(answer["error"]["code"], json!(-32000), "{answer:#?}");
    let body = &answer["error"]["data"];
    let failed = body["failed"].as_array().cloned().unwrap_or_default();
    assert_eq!(failed.len(), 1, "only the side chat fails: {failed:#?}");
    assert_eq!(failed[0]["identity"], json!("alpha-side"));
    assert_eq!(failed[0]["kind"], json!("side_chat_restart_refused"));
    assert!(
        failed[0]["error"]
            .as_str()
            .is_some_and(|error| error.contains("close it and fork alpha again")),
        "{failed:#?}"
    );
    assert_eq!(
        body["retired_delegates"],
        json!([{ "identity": "alpha" }]),
        "the other member is still acted on: {body:#?}"
    );
    assert_eq!(
        bridge_session(&runtime, "alpha-side").await,
        session,
        "the side chat keeps its session"
    );
    assert!(
        crate::side_chat::verified_side_chats(&runtime.handle())
            .await
            .contains_key("alpha-side"),
        "the side chat is still a fork of its agent"
    );
    Ok(())
}

/// A side chat is never adopted as an identity
/// (`mob/adopt_member_identity_declaration`): an adopted identity is rebuilt
/// from its declaration, which no fork made. The adoption is refused typed
/// (`side_chat_adoption_refused`) and Core records no identity intent for
/// the side chat; its agent is no side chat and is not refused.
#[tokio::test]
async fn a_side_chat_is_never_adopted_as_an_identity() -> TestResult {
    const MOB: &str = "console-side-chat-adopt";
    let (_temp_dir, runtime) = persistent_console_runtime(MOB, "", "").await?;
    seeded_member(&runtime, "worker", "alpha", &[]).await?;
    let forked = fork_when_idle(&runtime, "alpha", "alpha-side").await;
    assert!(forked["error"].is_null(), "the fork succeeds: {forked:#?}");
    let session = bridge_session(&runtime, "alpha-side")
        .await
        .ok_or("the side chat has a session")?;

    let refused = call(
        &runtime,
        crate::rpc::mob_methods::ADOPT_MEMBER_IDENTITY_DECLARATION,
        json!({
            "mob_id": MOB,
            "agent_identity": "alpha-side",
            "request_id": "adopt-side-chat-1",
            "precondition": "expected_absent",
            "declaration_scope": "side-chat-adoption",
            "declaration_revision": 1,
            "session": {
                "session_id": session,
                "lineage_id": format!("session:{session}"),
                "lineage_generation": 0,
                "authority_policy": "require_existing"
            },
            "member": {
                "profile_name": "worker",
                "runtime_mode": "turn_driven",
                "execution": { "execution": "controlling_session" }
            },
            "owned_wiring": [],
            "convergence": { "kind": "drain", "max_wait_ms": 5000 }
        }),
    )
    .await;
    assert_eq!(
        refused["error"]["code"],
        json!(crate::side_chat::SIDE_CHAT_ADOPTION_REFUSED_CODE),
        "{refused:#?}"
    );
    assert_eq!(
        refused["error"]["data"],
        json!({
            "kind": "side_chat_adoption_refused",
            "agent_identity": "alpha-side",
            "source_member_id": "alpha",
        })
    );
    assert!(
        matches!(
            runtime
                .handle()
                .identity_intent(&AgentIdentity::from("alpha-side"))
                .await,
            Ok(meerkat_mob::identity::IdentityStoredObservation::Missing)
        ),
        "Core records no identity intent for the side chat"
    );
    assert!(
        crate::side_chat::verified_side_chats(&runtime.handle())
            .await
            .contains_key("alpha-side"),
        "the side chat is still a fork of its agent"
    );
    let agent = AgentIdentity::from("alpha");
    assert!(
        crate::side_chat::side_chat_adoption_refusal(&runtime.handle(), &agent)
            .await
            .is_none(),
        "its agent is no side chat"
    );
    Ok(())
}

/// The definition's wiring never reaches a member exempt from it, a side
/// chat or not: the definition-wiring reconcile and the topology's declared
/// edges leave every exempt member out, while a bound worker keeps the edge
/// the definition declares for it.
#[tokio::test]
async fn the_definition_reconcile_and_topology_leave_every_exempt_member_out() -> TestResult {
    let (_temp_dir, runtime) = persistent_console_runtime(
        "console-side-chat-reconcile",
        "orchestrator = \"lead\"",
        "[wiring]\nauto_wire_orchestrator = true\n",
    )
    .await?;
    seeded_member(&runtime, "lead", "lead", &[]).await?;
    Box::pin(
        runtime
            .handle()
            .spawn_spec(SpawnMemberSpec::new("worker", AgentIdentity::from("bound"))),
    )
    .await?;
    Box::pin(
        runtime.handle().spawn_spec(
            SpawnMemberSpec::new("worker", AgentIdentity::from("exempt"))
                .with_definition_wiring(meerkat_mob::DefinitionWiring::Exempt),
        ),
    )
    .await?;
    let forked = fork_when_idle(&runtime, "lead", "lead-side").await;
    assert!(forked["error"].is_null(), "the fork succeeds: {forked:#?}");
    let declared_edge =
        std::collections::BTreeSet::from([("bound".to_string(), "lead".to_string())]);

    let report = crate::unified_runtime::edge_reconcile::reconcile_definition_edges_with_topology(
        &runtime, None,
    )
    .await;
    let desired = report
        .desired_edges
        .iter()
        .map(|edge| {
            let (a, b) = edge.endpoints();
            (a.to_string(), b.to_string())
        })
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        desired, declared_edge,
        "the definition declares the bound worker's edge alone: {report:#?}"
    );
    assert_eq!(wired_to(&runtime, "bound").await?, peers(&["lead"]));
    assert!(
        wired_to(&runtime, "exempt").await?.is_empty(),
        "the reconcile wires no exempt member"
    );
    assert!(
        wired_to(&runtime, "lead-side").await?.is_empty(),
        "nor a side chat"
    );

    let discovery =
        crate::unified_runtime::edge_reconcile::DefinitionWiringEdgeDiscovery::from_definition(
            runtime.handle().definition(),
        )
        .ok_or("the definition declares wiring")?;
    let topology = crate::topology_control::TopologyRuntimeHandle::new(
        runtime.handle(),
        Some(Arc::new(discovery) as Arc<dyn crate::unified_runtime::edge_types::EdgeDiscovery>),
        Arc::new(tokio::sync::RwLock::new(std::collections::BTreeSet::new())),
        crate::topology_control::TopologyController::default(),
        None,
        false,
    );
    let topology_snapshot = topology.query().await?;
    let declared = topology_snapshot
        .edges
        .iter()
        .filter(|edge| edge.declared)
        .map(|edge| (edge.edge.a.identity.clone(), edge.edge.b.identity.clone()))
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        declared, declared_edge,
        "the topology declares no edge of an exempt member"
    );
    Ok(())
}

/// A respawn's archive-cleanup fallback seats the member again from its
/// roster entry before the respawn, exemption from the definition's wiring
/// rules included: an exempt member comes back exempt.
#[test]
fn a_respawn_fallback_seats_an_exempt_member_exempt_again() {
    let labels = BTreeMap::from([("team".to_string(), "review".to_string())]);
    let exempt = crate::mob_handle_runtime::archive_cleanup_respawn_spec(
        AgentIdentity::from("alpha-side"),
        &meerkat_mob::ProfileName::from("worker"),
        &labels,
        meerkat_mob::DefinitionWiring::Exempt,
    );
    assert_eq!(
        exempt.definition_wiring,
        meerkat_mob::DefinitionWiring::Exempt
    );
    assert_eq!(exempt.labels, Some(labels));
    assert_eq!(exempt.role_name, meerkat_mob::ProfileName::from("worker"));
    let bound = crate::mob_handle_runtime::archive_cleanup_respawn_spec(
        AgentIdentity::from("beta"),
        &meerkat_mob::ProfileName::from("worker"),
        &BTreeMap::new(),
        meerkat_mob::DefinitionWiring::Applies,
    );
    assert_eq!(
        bound.definition_wiring,
        meerkat_mob::DefinitionWiring::Applies
    );
    assert_eq!(bound.labels, None);
}

/// A Console with no local runtime decides a send to a side chat as its
/// source, also on the cold access cache of a restart: the send gate records
/// its aggregator's runtimes before it decides, so a grant on the side chat's
/// own name sends nothing and a deny on its source still applies.
#[tokio::test]
async fn a_console_without_a_local_runtime_decides_a_send_to_a_side_chat_as_its_source()
-> TestResult {
    use crate::console_aggregator::{ConsoleAggregatorOptions, MetaConsoleAggregator};

    let state = tempfile::tempdir()?;
    let runtime =
        persistent_gateway_runtime_in(state.path(), "console-side-chat-send", "", "").await?;
    seeded_member(&runtime, "worker", "alpha", &[]).await?;
    let forked = fork_when_idle(&runtime, "alpha", "alpha-side").await;
    assert!(forked["error"].is_null(), "the fork succeeds: {forked:#?}");
    runtime.handle().shutdown().await?;
    let _ = runtime
        .stand_down_child_mobs(std::time::Instant::now() + std::time::Duration::from_secs(90))
        .await;
    drop(runtime);

    let runtime =
        persistent_gateway_runtime_in(state.path(), "console-side-chat-send", "", "").await?;
    let aggregator = MetaConsoleAggregator::in_memory_with_options(ConsoleAggregatorOptions {
        session_history_backfill_enabled: false,
        ..ConsoleAggregatorOptions::default()
    });
    aggregator.register_runtime_handles_with_policy(
        "runtime-b",
        "b",
        runtime.clone(),
        None,
        ConsoleEventStore::new(),
        Arc::new(AllowAllConsoleVisibilityPolicy),
    );
    // A fresh controller: its attribute cache is cold, as after a restart.
    let controller = enforced(vec![
        grant(
            "the-side-chat-name",
            "name-sender@example.test",
            &[ACTION_AGENT_SEND],
            "b/alpha-side",
        ),
        grant(
            "every-agent",
            "broad-sender@example.test",
            &[ACTION_AGENT_SEND],
            "*",
        ),
        deny("not-the-source", "broad-sender@example.test", "b/alpha"),
        grant(
            "the-source",
            "source-sender@example.test",
            &[ACTION_AGENT_SEND],
            "b/alpha",
        ),
    ])?;
    let decisions = RuntimeDecisionState::local_console(
        crate::ConsolePolicy {
            require_app_auth: false,
            ..Default::default()
        },
        None,
    );
    assert!(
        matches!(
            send_gate_without_runtime(
                &decisions,
                &aggregator,
                &controller,
                "name-sender@example.test",
                "b/alpha-side",
            )
            .await,
            Err(ConsoleSendGateRefusal::AccessDenied)
        ),
        "a grant on the side chat's own name sends nothing"
    );
    assert!(
        matches!(
            send_gate_without_runtime(
                &decisions,
                &aggregator,
                &controller,
                "broad-sender@example.test",
                "b/alpha-side",
            )
            .await,
            Err(ConsoleSendGateRefusal::AccessDenied)
        ),
        "a deny on the source refuses a send to its side chat"
    );
    assert!(
        send_gate_without_runtime(
            &decisions,
            &aggregator,
            &controller,
            "source-sender@example.test",
            "b/alpha-side",
        )
        .await
        .is_ok(),
        "a grant on the source sends to its side chat"
    );
    Ok(())
}

/// A Console with no local runtime never decides a lineage-labelled member
/// by its bare name. Its access cache, primed from the aggregator's runtimes,
/// knows a side chat by its namespaced identity only; a send that names the
/// side chat by its bare member id, which the cache still cannot resolve and
/// whose aggregate record names the side chat's source, is refused, though a
/// rule grants that bare id. The namespaced identity is decided as its source.
#[tokio::test]
async fn a_send_to_a_side_chat_the_primed_cache_cannot_resolve_is_refused() -> TestResult {
    use crate::console_aggregator::{ConsoleAggregatorOptions, MetaConsoleAggregator};

    let (_temp_dir, runtime) =
        persistent_console_runtime("console-side-chat-unresolved", "", "").await?;
    seeded_member(&runtime, "worker", "alpha", &[]).await?;
    let forked = fork_when_idle(&runtime, "alpha", "alpha-side").await;
    assert!(forked["error"].is_null(), "the fork succeeds: {forked:#?}");
    let aggregator = MetaConsoleAggregator::in_memory_with_options(ConsoleAggregatorOptions {
        session_history_backfill_enabled: false,
        ..ConsoleAggregatorOptions::default()
    });
    aggregator.register_runtime_handles_with_policy(
        "runtime-b",
        "b",
        runtime.clone(),
        None,
        ConsoleEventStore::new(),
        Arc::new(AllowAllConsoleVisibilityPolicy),
    );
    let controller = enforced(vec![
        grant(
            "the-bare-member-id",
            "bare-sender@example.test",
            &[ACTION_AGENT_SEND],
            "alpha-side",
        ),
        grant(
            "the-source",
            "source-sender@example.test",
            &[ACTION_AGENT_SEND],
            "b/alpha",
        ),
    ])?;
    let decisions = RuntimeDecisionState::local_console(
        crate::ConsolePolicy {
            require_app_auth: false,
            ..Default::default()
        },
        None,
    );

    assert!(
        matches!(
            send_gate_without_runtime(
                &decisions,
                &aggregator,
                &controller,
                "bare-sender@example.test",
                "alpha-side",
            )
            .await,
            Err(ConsoleSendGateRefusal::AccessDenied)
        ),
        "a lineage-labelled member the primed cache cannot resolve is refused, not decided by \
         its bare name"
    );
    let bare_sender = controller.view_for_subject(Some("bare-sender@example.test"));
    assert!(
        !bare_sender.knows_agent("alpha-side"),
        "the primed cache knows the side chat by its namespaced identity only"
    );
    assert!(bare_sender.knows_agent("b/alpha-side"));
    assert!(
        send_gate_without_runtime(
            &decisions,
            &aggregator,
            &controller,
            "source-sender@example.test",
            "b/alpha-side",
        )
        .await
        .is_ok(),
        "the namespaced identity is decided as its source"
    );
    Ok(())
}

/// The `/console/send` gate of a Console with no local runtime, for
/// `subject` sending to `identity`.
async fn send_gate_without_runtime(
    decisions: &RuntimeDecisionState,
    aggregator: &crate::console_aggregator::MetaConsoleAggregator,
    controller: &AccessController,
    subject: &str,
    identity: &str,
) -> Result<(), ConsoleSendGateRefusal> {
    let view = controller.view_for_subject(Some(subject));
    console_send_gate(
        decisions,
        None,
        Some(aggregator),
        Some(controller),
        Some(&view),
        identity,
    )
    .await
}

/// A source whose lineage already fills the access rules' walk (the agent
/// and seven ancestors) is refused for a fork: its side chat's lineage would
/// go one link past the walk, and no one but an admin could open it. A source
/// one link shorter forks.
#[test]
fn a_source_whose_lineage_fills_the_walk_is_refused_for_a_fork() -> TestResult {
    let controller = enforced(vec![grant(
        "drives-every-agent",
        "driver@example.test",
        &[ACTION_AGENT_VIEW, ACTION_AGENT_SEND, ACTION_AGENT_SPAWN],
        "*",
    )])?;
    // `a0` was spawned by `a1`, and so on up to `a7`, which no agent spawned.
    for depth in 0..8 {
        let mut labels = BTreeMap::new();
        if depth < 7 {
            labels.insert("spawned_by".to_string(), format!("a{}", depth + 1));
        }
        controller.record_agent_attributes(AgentResourceAttributes {
            identity: format!("a{depth}"),
            agent_id: Some(format!("a{depth}")),
            role: Some("worker".to_string()),
            labels,
        });
    }
    let view = controller.view_for_subject(Some("driver@example.test"));
    assert!(
        view.allows_agent(ACTION_AGENT_SEND, "a0"),
        "the deepest agent is still decided"
    );
    let refused = side_chat_source_access_violation(Some(&view), "a0")
        .ok_or("a source whose lineage fills the walk is refused")?;
    assert_eq!(refused.code, ACCESS_DENIED_RPC_CODE);
    assert_eq!(
        refused.data,
        Some(json!({
            "kind": "access_denied",
            "action": ACTION_AGENT_SPAWN,
            "resource": "a0",
            "reason": "lineage_too_deep",
        }))
    );
    assert!(
        side_chat_source_access_violation(Some(&view), "a1").is_none(),
        "a side chat of a source with six ancestors fits the walk"
    );
    Ok(())
}

/// A console visibility policy that hides one member, by its member alias.
struct HideMember(&'static str);

impl crate::console_aggregator::ConsoleVisibilityPolicy for HideMember {
    fn member_visible(&self, member: &ConsoleMember) -> bool {
        member.agent_identity != self.0
    }

    fn identity_visible(&self, record: &ConsoleIdentityRecord) -> bool {
        record.runtime_member_id != self.0 && record.identity != self.0
    }
}

/// A console RPC call under `policy`, with no access rules.
async fn call_with_policy(
    runtime: &MobRuntime,
    policy: &dyn crate::console_aggregator::ConsoleVisibilityPolicy,
    method: &str,
    params: Value,
) -> Value {
    Box::pin(handle_console_runtime_rpc_with_visibility(
        runtime,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        policy,
        request(method, params),
        true,
        false,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        &crate::http_console::ConsoleSendGovernance::default(),
        &crate::topology_control::TopologyApplyScope::Anonymous,
    ))
    .await
}

/// A fork reads its source's conversation, so a source the console's
/// visibility policy hides is no source the console may fork:
/// `meta/fork_member` and `meta/fork_helper` answer it as an identity the
/// console does not show, before anything is read or seated, as `meta/reset`
/// and `meta/respawn` answer it.
#[tokio::test]
async fn a_fork_of_a_source_the_console_hides_is_refused() -> TestResult {
    let (_temp_dir, runtime) =
        persistent_console_runtime("console-side-chat-hidden", "", "").await?;
    seeded_member(&runtime, "worker", "alpha", &[]).await?;
    let hidden = HideMember("alpha");
    for (method, params, child) in [
        (
            "meta/fork_member",
            json!({ "source_member_id": "alpha", "agent_identity": "alpha-side" }),
            "alpha-side",
        ),
        (
            "meta/fork_helper",
            json!({
                "source_member_id": "alpha",
                "agent_identity": "alpha-helper",
                "task": "Summarize the conversation.",
                "result_label": "summary",
                "max_text_bytes": 1024,
            }),
            "alpha-helper",
        ),
    ] {
        let refused = call_with_policy(&runtime, &hidden, method, params).await;
        assert_eq!(
            refused["error"]["code"],
            json!(-32001),
            "{method}: {refused:#?}"
        );
        assert_eq!(
            refused["error"]["data"]["kind"],
            json!("identity_hidden_by_policy"),
            "{method}"
        );
        assert!(
            runtime
                .handle()
                .get_member(&AgentIdentity::from(child))
                .await?
                .is_none(),
            "{method} seated nothing"
        );
    }
    let forked = fork_when_idle(&runtime, "alpha", "alpha-side").await;
    assert!(
        forked["error"].is_null(),
        "a source the console shows forks: {forked:#?}"
    );
    Ok(())
}

/// A deny that matches a side chat itself refuses it whatever its source
/// grants: only an allow on the side chat's own link grants nothing.
#[tokio::test]
async fn a_deny_on_a_side_chat_itself_refuses_it() -> TestResult {
    let (_temp_dir, runtime) =
        persistent_console_runtime("console-side-chat-own-deny", "", "").await?;
    seeded_member(&runtime, "worker", "alpha", &[]).await?;
    let forked = fork_when_idle(&runtime, "alpha", "alpha-side").await;
    assert!(forked["error"].is_null(), "the fork succeeds: {forked:#?}");
    let controller = enforced(vec![
        grant(
            "the-source",
            "reader@example.test",
            &[ACTION_AGENT_VIEW, ACTION_AGENT_SEND],
            "alpha",
        ),
        deny("not-the-side-chat", "reader@example.test", "alpha-side"),
    ])?;
    prime_access_cache_from_runtime(&runtime, &controller).await;
    let reader = controller.view_for_subject(Some("reader@example.test"));
    assert!(reader.allows_agent(ACTION_AGENT_VIEW, "alpha"));
    for action in [ACTION_AGENT_VIEW, ACTION_AGENT_SEND] {
        assert!(
            !reader.allows_agent(action, "alpha-side"),
            "a deny on the side chat refuses it: {action}"
        );
    }
    let side_events = event_attributes(&runtime, "alpha-side").await?;
    assert!(
        !reader
            .decide_agent_with_attributes(ACTION_AGENT_VIEW, &side_events)
            .is_allow(),
        "a deny on the side chat hides its stream events"
    );
    Ok(())
}

/// A member list call reads no side chat fork facts of its own: the route's
/// access priming lists the roster and keeps each side chat's lineage, and
/// the call's rows take them from there. A grant on the source lists its side
/// chats.
#[tokio::test]
async fn a_member_list_call_reads_no_fork_facts_of_its_own() -> TestResult {
    let (_temp_dir, runtime) =
        persistent_console_runtime("console-side-chat-list-reads", "", "").await?;
    seeded_member(&runtime, "worker", "alpha", &[]).await?;
    for side_chat in ["alpha-side", "alpha-draft"] {
        let forked = fork_when_idle(&runtime, "alpha", side_chat).await;
        assert!(forked["error"].is_null(), "the fork succeeds: {forked:#?}");
    }
    let controller = enforced(vec![grant(
        "the-source",
        "reader@example.test",
        &[ACTION_AGENT_VIEW],
        "alpha",
    )])?;
    let view = controller.view_for_subject(Some("reader@example.test"));
    let reads = || runtime.side_chat_lineages().reads();
    let before = reads();
    let listed = Box::pin(handle_console_runtime_rpc_with_visibility(
        &runtime,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        &AllowAllConsoleVisibilityPolicy,
        request("meta/list_members", json!({})),
        true,
        false,
        Some("reader@example.test"),
        Some(&controller),
        Some(&view),
        None,
        None,
        None,
        None,
        None,
        &crate::http_console::ConsoleSendGovernance::default(),
        &crate::topology_control::TopologyApplyScope::Anonymous,
    ))
    .await;
    assert!(
        listed["error"].is_null(),
        "the list call answers: {listed:#?}"
    );
    let mut names = listed["result"]
        .as_array()
        .or_else(|| listed["result"]["members"].as_array())
        .ok_or("the list call's rows")?
        .iter()
        .filter_map(|row| {
            row.get("agent_identity")
                .or_else(|| row.get("member_id"))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect::<Vec<_>>();
    names.sort();
    assert_eq!(
        names,
        ["alpha", "alpha-draft", "alpha-side"],
        "a grant on the source lists its side chats"
    );
    assert_eq!(
        reads() - before,
        0,
        "the list call takes the side chats' lineages from its access priming"
    );
    Ok(())
}
