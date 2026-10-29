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
    let state = temp_dir.path().to_path_buf();
    let (storage, provenance) = crate::mob_composition_manifest::persistent_mob_storage(
        state.join(crate::storage_layout::EVENT_LOG_DB_FILE_NAME),
    )?;
    let definition = MobDefinition::from_toml(&format!(
        "[mob]\nid = \"{mob_id}\"\n{mob_header}\n{WORKER_PROFILES}\n{tail}"
    ))?;
    let mut spec = MobBootstrapSpec::persistent(definition, storage, provenance, state, 4)
        .await
        .map_err(|error| error.to_string())?;
    spec.options.default_llm_client = Some(Arc::new(TestClient::default()));
    let runtime = Box::pin(MobRuntime::bootstrap(spec)).await?;
    Ok((temp_dir, runtime))
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
/// each frame. A grant on the source alone shows the side chat's frames, and
/// a deny on the source hides them under a grant on every agent.
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
/// chat's stream events read them no more.
#[tokio::test]
async fn one_console_read_reads_the_fork_facts_once() -> TestResult {
    use crate::console_aggregator::MetaConsoleAggregator;

    let (_temp_dir, runtime) =
        persistent_console_runtime("console-side-chat-reads", "", "").await?;
    seeded_member(&runtime, "worker", "alpha", &[]).await?;
    for side_chat in ["alpha-side", "alpha-draft"] {
        let forked = fork_when_idle(&runtime, "alpha", side_chat).await;
        assert!(forked["error"].is_null(), "the fork succeeds: {forked:#?}");
    }
    let reads = || runtime.side_chat_lineages().reads();
    let before = reads();

    let aggregator = MetaConsoleAggregator::in_memory();
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
