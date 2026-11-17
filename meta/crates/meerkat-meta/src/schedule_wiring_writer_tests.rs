//! Who writes a schedule: what a create and an update answer for the writer
//! and the member the schedule's target names.
//!
//! A launch with four members (`anna`, `boris` and `carl` hold the schedule
//! tools and take outside sends; `vault` takes none), a mob `anna` created
//! with one member (`helper`, which holds the schedule tools), and the
//! agents' schedule tools as a gateway composes them.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use meerkat_core::AgentToolDispatcher;
use meerkat_core::types::SessionId;
use meerkat_mob::ids::AgentIdentity;
use serde_json::{Value, json};

use super::*;

/// The denial code of a schedule whose writer cannot message the member its
/// target names.
const NOT_A_PEER: &str = "schedule_target_not_a_peer";

struct Launch {
    _temp_dir: tempfile::TempDir,
    runtime: crate::UnifiedRuntime,
    mob_id: String,
    handle: MobHandle,
    child: MobHandle,
    service: ScheduleService,
    mob_state: Arc<MobMcpState>,
    tools: Arc<dyn AgentToolDispatcher>,
}

async fn launch() -> Launch {
    static NEXT_MOB: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let state = temp_dir.path().join("state");
    std::fs::create_dir_all(&state).expect("state dir");
    let session_store: Arc<dyn meerkat::SessionStore> = Arc::new(
        meerkat_store::SqliteSessionStore::open(state.join("sessions.db")).expect("session store"),
    );
    let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> = Arc::new(
        meerkat_runtime::store::SqliteRuntimeStore::new(state.join("runtime.sqlite"))
            .expect("runtime store"),
    );
    let blob_store: Arc<dyn meerkat_core::BlobStore> =
        Arc::new(meerkat_store::MemoryBlobStore::new());
    let factory = meerkat::AgentFactory::new(&state).comms(true);
    let mut builder = FactoryAgentBuilder::new(factory, Config::default());
    builder.default_session_store = Some(Arc::new(meerkat_store::StoreAdapter::new(
        session_store.clone(),
    )));
    builder.default_blob_store = Some(blob_store.clone());
    let jobs: Arc<dyn meerkat::DetachedJobStore> = Arc::new(meerkat::MemoryDetachedJobStore::new());
    builder.default_detached_job_store = Some(Arc::clone(&jobs));
    let delivery =
        crate::mob_handle_runtime::MobRuntimeDelivery::new(Arc::clone(&runtime_store), jobs);
    let attached =
        attach_schedule_tools_with_identity_targets(&builder, &state).expect("schedule tools");
    let mob_tools_slot = Arc::clone(&builder.default_mob_tools);
    let adapter = Arc::new(
        MeerkatMachine::persistent(Arc::clone(&runtime_store), Arc::clone(&blob_store))
            .expect("the fixture's runtime machine"),
    );
    let sessions = Arc::new(PersistentSessionService::new(
        builder,
        16,
        session_store,
        runtime_store,
        blob_store,
    ));
    let mob_id = format!(
        "schedule-writer-{}",
        NEXT_MOB.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    );
    let definition = meerkat_mob::MobDefinition::from_toml(&format!(
        r#"
[mob]
id = "{mob_id}"

[profiles.desk]
model = "gpt-5.5"
external_addressable = true

[profiles.desk.tools]
comms = true
schedule = true

[profiles.inner]
model = "gpt-5.5"
external_addressable = false

[profiles.inner.tools]
comms = true
schedule = true
"#
    ))
    .expect("definition");
    let mob_spec = crate::mob_handle_runtime::MobBootstrapSpec::new(
        definition,
        meerkat_mob::MobStorage::in_memory(),
        sessions,
    )
    .with_session_runtime_adapter(adapter)
    .expect("the fixture's session runtime owner")
    .with_runtime_delivery(delivery)
    .with_agent_mob_tools(mob_tools_slot)
    .expect("the fixture's agent mob tools")
    .with_options(crate::mob_handle_runtime::MobBootstrapOptions {
        allow_ephemeral_sessions: true,
        notify_orchestrator_on_resume: true,
        default_llm_client: Some(Arc::new(meerkat_client::TestClient::for_provider(
            meerkat_core::Provider::OpenAI,
        ))),
    });
    let runtime = crate::UnifiedRuntime::bootstrap(
        mob_spec,
        crate::MetaConfig {
            modules: vec![],
            discovery: crate::DiscoverySpec {
                namespace: mob_id.clone(),
                modules: vec![],
            },
            pre_spawn: vec![],
        },
        Duration::from_secs(2),
    )
    .await
    .expect("bootstrap");
    let handle = runtime.mob_handle();
    for (profile, member) in [
        ("desk", "anna"),
        ("desk", "boris"),
        ("desk", "carl"),
        ("inner", "vault"),
    ] {
        handle
            .ensure_member(meerkat_mob::SpawnMemberSpec::new(
                meerkat_mob::ProfileName::from(profile),
                AgentIdentity::from(member),
            ))
            .await
            .unwrap_or_else(|error| panic!("{member} is seated: {error}"));
    }
    // A member private to a person, as the host declares one.
    handle
        .ensure_member(
            meerkat_mob::SpawnMemberSpec::new(
                meerkat_mob::ProfileName::from("desk"),
                AgentIdentity::from("diary"),
            )
            .with_labels(BTreeMap::from([(
                crate::access::PRIVATE_TO_LABEL.to_string(),
                "console-caller:v1:somebody".to_string(),
            )])),
        )
        .await
        .expect("the private member is seated");
    let mob_state = runtime
        .mob_runtime()
        .agent_mob_mcp_state()
        .expect("the launch installs the agent mob tools");
    attached
        .mob_target_registry
        .adopt_mob_runtime(runtime.mob_runtime());
    let anna = handle
        .resolve_bridge_session_id_observation(&AgentIdentity::from("anna"))
        .await
        .expect("anna has a session");
    let child_mob_id = format!("{mob_id}-child");
    let child_definition: meerkat_mob::MobDefinition = serde_json::from_value(json!({
        "id": child_mob_id,
        "profiles": { "worker": {
            "model": "gpt-5.5",
            "runtime_mode": "turn_driven",
            "tools": { "comms": true, "schedule": true }
        } }
    }))
    .expect("child definition");
    mob_state
        .mob_create_definition_with_owner_bridge_session(child_definition, anna, true, false)
        .await
        .expect("anna's mob is created");
    let child_id = meerkat_mob::MobId::from(child_mob_id.as_str());
    mob_state
        .mob_spawn(
            &child_id,
            meerkat_mob::ProfileName::from("worker"),
            AgentIdentity::from("helper"),
            Some(meerkat_mob::MobRuntimeMode::TurnDriven),
            None,
            None,
        )
        .await
        .expect("the helper is seated");
    let child = mob_state
        .handle_for(&child_id)
        .await
        .expect("the handle of anna's mob");
    let tools = agent_schedule_tool_chain(
        attached.service.clone(),
        attached.mob_target_registry.clone(),
    );
    Launch {
        _temp_dir: temp_dir,
        runtime,
        mob_id,
        handle,
        child,
        service: attached.service,
        mob_state,
        tools,
    }
}

/// A due time no row waits for.
fn far() -> chrono::DateTime<chrono::Utc> {
    chrono::Utc::now() + chrono::Duration::days(1)
}

/// A target that sends a note to member `member` of mob `mob`.
fn send_to(mob: &str, member: &str) -> Value {
    json!({
        "target_kind": "mob",
        "type": "member",
        "mob_id": mob,
        "member_id": member,
        "action": { "type": "send", "content": "A scheduled note." },
    })
}

/// The arguments of a create of one send to member `member` of mob `mob`.
fn one_send(mob: &str, member: &str) -> Value {
    json!({
        "name": "a note",
        "trigger": { "type": "once", "due_at_utc": far().to_rfc3339() },
        "target": send_to(mob, member),
    })
}

/// Whether `answer` is the refusal of a schedule whose writer cannot message
/// the member its target names.
fn refused_for_reach(answer: &Result<String, meerkat_core::ToolError>) -> bool {
    matches!(
        answer,
        Err(meerkat_core::ToolError::PolicyDenied { denial }) if denial.code == NOT_A_PEER
    )
}

/// The id of the schedule a create answered with.
fn created(answer: &Result<String, meerkat_core::ToolError>) -> String {
    let text = answer
        .as_ref()
        .unwrap_or_else(|error| panic!("the schedule is created: {error:?}"));
    let answered: Value = serde_json::from_str(text).expect("the create answers JSON");
    answered["schedule_id"]
        .as_str()
        .unwrap_or_else(|| panic!("the create names its schedule: {text}"))
        .to_string()
}

impl Launch {
    /// The session of launch member `member`.
    async fn session(&self, member: &str) -> SessionId {
        self.handle
            .resolve_bridge_session_id_observation(&AgentIdentity::from(member))
            .await
            .unwrap_or_else(|| panic!("{member} has a session"))
    }

    /// The session of `helper`, the member of the mob `anna` created.
    async fn helper(&self) -> SessionId {
        self.child
            .resolve_bridge_session_id(&AgentIdentity::from("helper"))
            .await
            .expect("the helper has a session")
    }

    /// One schedule tool call under `context`: the text it answers, or its
    /// refusal.
    async fn schedule_call_under(
        &self,
        context: &meerkat_core::ToolDispatchContext,
        name: &str,
        args: Value,
    ) -> Result<String, meerkat_core::ToolError> {
        let args = serde_json::value::RawValue::from_string(args.to_string()).expect("tool args");
        let outcome = self
            .tools
            .dispatch_with_context(
                meerkat_core::types::ToolCallView {
                    id: "schedule-writer-row",
                    name,
                    args: &args,
                },
                context,
            )
            .await?;
        Ok(outcome.result.text_content())
    }

    /// One schedule tool call as the turn of the member holding `caller`
    /// dispatches it.
    async fn schedule_call(
        &self,
        caller: &SessionId,
        name: &str,
        args: Value,
    ) -> Result<String, meerkat_core::ToolError> {
        self.schedule_call_under(
            &meerkat_core::ToolDispatchContext::default()
                .with_runtime_identity(caller.clone(), None),
            name,
            args,
        )
        .await
    }

    /// The member's own message to member `target` of `mob`, sent by the
    /// comms runtime of the session `writer`: the send its `send_message`
    /// tool makes.
    async fn direct_send(
        &self,
        writer: &SessionId,
        mob: &MobHandle,
        target: &str,
    ) -> Result<(), String> {
        let endpoint = mob
            .member_peer_endpoint(&AgentIdentity::from(target))
            .await
            .unwrap_or_else(|error| panic!("the endpoint of {target} is read: {error}"))
            .unwrap_or_else(|| panic!("{target} has an endpoint"));
        let comms = self
            .mob_state
            .session_service()
            .comms_runtime(writer)
            .await
            .expect("the writer has a comms runtime");
        let command = meerkat_core::comms::CommsCommandRequest::PeerMessage {
            to: endpoint.descriptor.peer_id,
            body: "A direct note.".to_string(),
            blocks: None,
            content_taint: None,
            handling_mode: None,
        }
        .into_command(writer)
        .expect("a peer message command");
        comms
            .send(command)
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    /// The target the stored schedule `schedule` holds, as it is serialized.
    async fn stored_target(&self, schedule: &str) -> Value {
        let schedule = meerkat::ScheduleId::parse(schedule).expect("a schedule id");
        serde_json::to_value(
            self.service
                .get(&schedule)
                .await
                .expect("the schedule is read")
                .target,
        )
        .expect("a target serializes")
    }

    async fn wire(&self, one: &str, other: &str) {
        self.handle
            .wire(AgentIdentity::from(one), AgentIdentity::from(other))
            .await
            .unwrap_or_else(|error| panic!("{one} and {other} are wired: {error}"));
    }

    async fn stop(self) {
        self.runtime.shutdown().await;
    }
}

/// A member of a mob a launch member created holds the schedule tools. Its
/// own message to a launch member that takes no outside sends is refused,
/// and so is its schedule of a send to that member: nothing is stored.
#[tokio::test(flavor = "multi_thread")]
async fn a_child_mob_members_schedule_for_a_launch_member_it_cannot_message_is_refused() {
    let launch = launch().await;
    let helper = launch.helper().await;
    let sent = launch.direct_send(&helper, &launch.handle, "vault").await;
    assert!(
        sent.is_err(),
        "the helper's own message to the launch member is refused: {sent:?}"
    );
    let answer = launch
        .schedule_call(
            &helper,
            "meerkat_schedule_create",
            one_send(&launch.mob_id, "vault"),
        )
        .await;
    assert!(
        refused_for_reach(&answer),
        "the helper's schedule of a send to that member is refused as its message is: {answer:?}"
    );
    let stored = launch.service.list().await.expect("the schedules are read");
    assert!(stored.is_empty(), "nothing is stored: {stored:?}");
    launch.stop().await;
}

/// The schedule write answers as the writer's own message does: for the same
/// writer and the same member, a schedule of a send is accepted exactly where
/// the message is accepted and refused exactly where it is refused.
#[tokio::test(flavor = "multi_thread")]
async fn a_schedule_write_is_refused_exactly_where_the_writers_message_is() {
    let launch = launch().await;
    let anna = launch.session("anna").await;
    let helper = launch.helper().await;
    launch.wire("anna", "boris").await;
    // Writer, its session, the target's mob, the target, and what the row
    // knows the message answers (`None`: read from the message itself).
    let pairs = [
        ("anna", &anna, &launch.handle, "boris", Some(true)),
        ("anna", &anna, &launch.handle, "carl", Some(false)),
        ("anna", &anna, &launch.handle, "vault", Some(false)),
        ("helper", &helper, &launch.handle, "vault", Some(false)),
        ("helper", &helper, &launch.handle, "anna", None),
        ("anna", &anna, &launch.child, "helper", None),
    ];
    let mut answers = Vec::new();
    let mut differing = Vec::new();
    for (writer, session, mob, target, messages) in pairs {
        let sent = launch.direct_send(session, mob, target).await;
        let scheduled = launch
            .schedule_call(
                session,
                "meerkat_schedule_create",
                one_send(mob.mob_id().as_str(), target),
            )
            .await;
        if let Some(messages) = messages {
            assert_eq!(
                sent.is_ok(),
                messages,
                "{writer}'s own message to {target}: {sent:?}"
            );
        }
        assert!(
            scheduled.is_ok() || refused_for_reach(&scheduled),
            "{writer}'s schedule for {target} is accepted or refused for its reach: {scheduled:?}"
        );
        let pair = format!(
            "{writer} to {target}: message {}, schedule {}",
            if sent.is_ok() { "accepted" } else { "refused" },
            if scheduled.is_ok() {
                "accepted"
            } else {
                "refused"
            },
        );
        if scheduled.is_ok() != sent.is_ok() {
            differing.push(pair.clone());
        }
        answers.push(pair);
    }
    assert!(
        differing.is_empty(),
        "the schedule answers as the message does; differing: {differing:#?}; all: {answers:#?}"
    );
    launch.stop().await;
}

/// An update is judged as a create is, for the member who makes it and the
/// target the schedule has afterwards: a retarget to a member the writer
/// cannot message is refused and changes nothing, and so is an update that
/// names no target, by a member that cannot message the schedule's target.
#[tokio::test(flavor = "multi_thread")]
async fn a_schedule_update_is_refused_where_the_writers_message_is() {
    let launch = launch().await;
    let anna = launch.session("anna").await;
    let carl = launch.session("carl").await;
    launch.wire("anna", "boris").await;
    let schedule = created(
        &launch
            .schedule_call(
                &anna,
                "meerkat_schedule_create",
                one_send(&launch.mob_id, "boris"),
            )
            .await,
    );
    let written = launch.stored_target(&schedule).await;

    let retargeted = launch
        .schedule_call(
            &anna,
            "meerkat_schedule_update",
            json!({ "schedule_id": schedule, "target": send_to(&launch.mob_id, "carl") }),
        )
        .await;
    assert!(
        refused_for_reach(&retargeted),
        "a retarget to a member the writer cannot message is refused: {retargeted:?}"
    );
    let renamed_by_another = launch
        .schedule_call(
            &carl,
            "meerkat_schedule_update",
            json!({ "schedule_id": schedule, "name": "another note" }),
        )
        .await;
    assert!(
        refused_for_reach(&renamed_by_another),
        "an update by a member that cannot message the schedule's target is refused: \
         {renamed_by_another:?}"
    );
    assert_eq!(
        launch.stored_target(&schedule).await,
        written,
        "the refused updates change nothing"
    );

    let renamed = launch
        .schedule_call(
            &anna,
            "meerkat_schedule_update",
            json!({ "schedule_id": schedule, "name": "a second note" }),
        )
        .await;
    assert!(
        renamed.is_ok(),
        "the writer's update of a schedule aimed at a member it can message stands: {renamed:?}"
    );
    launch
        .handle
        .unwire(AgentIdentity::from("anna"), AgentIdentity::from("boris"))
        .await
        .expect("anna and boris are unwired");
    let renamed_unwired = launch
        .schedule_call(
            &anna,
            "meerkat_schedule_update",
            json!({ "schedule_id": schedule, "name": "a third note" }),
        )
        .await;
    assert!(
        refused_for_reach(&renamed_unwired),
        "once the writer cannot message the member, its update is refused: {renamed_unwired:?}"
    );
    launch.stop().await;
}

/// A writer reads nothing of who exists from the refusal: a member it cannot
/// message, a member of another mob it cannot message, a name no member
/// holds and a mob no host holds all answer the same refusal, word for word
/// but for the names the writer itself gave.
#[tokio::test(flavor = "multi_thread")]
async fn a_schedule_for_a_member_out_of_reach_answers_as_for_a_name_no_member_holds() {
    let launch = launch().await;
    let anna = launch.session("anna").await;
    let child_mob = launch.child.mob_id().to_string();
    let mut answers = Vec::new();
    for (mob, member) in [
        (launch.mob_id.as_str(), "carl"),
        (launch.mob_id.as_str(), "nobody"),
        (child_mob.as_str(), "helper"),
        (child_mob.as_str(), "nobody"),
        ("no-such-mob", "carl"),
    ] {
        let answer = launch
            .schedule_call(&anna, "meerkat_schedule_create", one_send(mob, member))
            .await;
        assert!(
            refused_for_reach(&answer),
            "a schedule for {member} of {mob} is refused for the writer's reach: {answer:?}"
        );
        answers.push(
            format!("{answer:?}")
                .replace(mob, "<mob>")
                .replace(member, "<member>"),
        );
    }
    answers.dedup();
    assert_eq!(
        answers.len(),
        1,
        "every target out of the writer's reach answers alike: {answers:#?}"
    );
    launch.stop().await;
}

/// A member private to a person is no exception: a schedule for it by a
/// member that is not private to that person answers the very refusal a
/// name no member holds answers, so the refusal does not say that the name
/// is somebody's private member.
#[tokio::test(flavor = "multi_thread")]
async fn a_schedule_for_a_private_member_answers_as_for_a_name_no_member_holds() {
    let launch = launch().await;
    let anna = launch.session("anna").await;
    let mut answers = Vec::new();
    for member in ["diary", "nobody"] {
        let answer = launch
            .schedule_call(
                &anna,
                "meerkat_schedule_create",
                one_send(&launch.mob_id, member),
            )
            .await;
        assert!(
            refused_for_reach(&answer),
            "a schedule for {member} is refused for the writer's reach: {answer:?}"
        );
        answers.push(format!("{answer:?}").replace(member, "<member>"));
    }
    assert_eq!(
        answers[0], answers[1],
        "a private member answers as a name no member holds does"
    );
    launch.stop().await;
}

/// A schedule that would have the host act in a mob (spawn a helper there,
/// run one of its flows) is a member's to write for its own mob. A member
/// of another mob that does not operate that mob is refused, with the
/// answer a mob no host holds gets. A helper in the member's own mob is in
/// its reach, and is then refused as a schedule that creates a helper for a
/// writer the dispatch does not state to hold the mob tools.
#[tokio::test(flavor = "multi_thread")]
async fn a_schedule_that_acts_in_a_mob_is_refused_for_a_member_that_does_not_operate_it() {
    let launch = launch().await;
    let anna = launch.session("anna").await;
    let helper = launch.helper().await;
    let spawn_in = |mob: &str| {
        json!({
            "name": "a helper",
            "trigger": { "type": "once", "due_at_utc": far().to_rfc3339() },
            "target": {
                "target_kind": "mob",
                "type": "spawn_helper",
                "mob_id": mob,
                "member_id": "scheduled-helper",
                "prompt": "Look at the queue.",
            },
        })
    };
    let flow_in = |mob: &str| {
        json!({
            "name": "a flow",
            "trigger": { "type": "once", "due_at_utc": far().to_rfc3339() },
            "target": {
                "target_kind": "mob",
                "type": "flow",
                "mob_id": mob,
                "flow_id": "daily",
                "params": {},
            },
        })
    };
    let mut answers = Vec::new();
    for (mob, args) in [
        (launch.mob_id.as_str(), spawn_in(&launch.mob_id)),
        (launch.mob_id.as_str(), flow_in(&launch.mob_id)),
        ("no-such-mob", spawn_in("no-such-mob")),
    ] {
        let answer = launch
            .schedule_call(&helper, "meerkat_schedule_create", args)
            .await;
        assert!(
            refused_for_reach(&answer),
            "the helper's schedule that acts in {mob} is refused for its reach: {answer:?}"
        );
        answers.push(format!("{answer:?}").replace(mob, "<mob>"));
    }
    answers.dedup();
    assert_eq!(
        answers.len(),
        1,
        "a mob the writer does not operate answers as a mob no host holds: {answers:#?}"
    );
    let own = launch
        .schedule_call(&anna, "meerkat_schedule_create", spawn_in(&launch.mob_id))
        .await;
    assert!(
        !refused_for_reach(&own),
        "a member's schedule that acts in its own mob is not refused for its reach: {own:?}"
    );
    assert!(
        refused_session_creation(&own),
        "a member not stated to hold the mob tools schedules no helper: {own:?}"
    );
    let own_flow = launch
        .schedule_call(&anna, "meerkat_schedule_create", flow_in(&launch.mob_id))
        .await;
    assert!(
        !refused_for_reach(&own_flow) && !refused_session_creation(&own_flow),
        "a member's flow in its own mob is refused neither way: {own_flow:?}"
    );
    launch.stop().await;
}

/// A member's schedule of a send to itself is accepted, wired to nobody,
/// also for a member that takes no outside sends.
#[tokio::test(flavor = "multi_thread")]
async fn a_members_schedule_for_itself_is_accepted() {
    let launch = launch().await;
    for member in ["anna", "vault"] {
        let session = launch.session(member).await;
        let answer = launch
            .schedule_call(
                &session,
                "meerkat_schedule_create",
                one_send(&launch.mob_id, member),
            )
            .await;
        assert!(
            answer.is_ok(),
            "{member}'s schedule for itself is accepted: {answer:?}"
        );
    }
    launch.stop().await;
}

/// A schedule no member's turn writes is not judged: the host's own create
/// on the schedule service, and a schedule tool call that belongs to no
/// member's session, are accepted for a member that takes no outside sends.
#[tokio::test(flavor = "multi_thread")]
async fn a_schedule_no_member_writes_is_not_judged() {
    let launch = launch().await;
    let hosts = launch
        .service
        .create(CreateScheduleRequest {
            name: Some("a host's note".to_owned()),
            description: None,
            trigger: TriggerSpec::Once { due_at_utc: far() },
            target: TargetBinding::Mob(Box::new(MobTargetBinding::Member {
                mob_id: launch.mob_id.clone(),
                member_id: "vault".to_string(),
                action: ScheduledMobAction::Send {
                    content: meerkat_core::ContentInput::Text("A scheduled note.".to_string()),
                    render_metadata: None,
                },
            })),
            misfire_policy: meerkat::MisfirePolicy::default(),
            overlap_policy: meerkat::OverlapPolicy::default(),
            missing_target_policy: meerkat::MissingTargetPolicy::default(),
            labels: BTreeMap::new(),
            planning_horizon_days: None,
            planning_horizon_occurrences: None,
        })
        .await;
    assert!(hosts.is_ok(), "the host's own schedule stands: {hosts:?}");
    let no_member = launch
        .schedule_call_under(
            &meerkat_core::ToolDispatchContext::default(),
            "meerkat_schedule_create",
            one_send(&launch.mob_id, "vault"),
        )
        .await;
    assert!(
        no_member.is_ok(),
        "a schedule tool call of no member's session is the host's: {no_member:?}"
    );
    launch.stop().await;
}

/// A session no member holds is no member of the launch: its judge says
/// so, whatever the target. Such a session's schedule into another session
/// no member holds, by id, is refused with the code of a schedule that
/// creates a session and stores nothing, and its schedule into its own
/// session is taken as before. A plain resumable prompt that names a
/// MEMBER's session is written on a gateway as that member's target, so it
/// is a schedule for a member, taken from such a writer as before; the
/// exact form, or an event, into a member's session is not written so, and
/// is refused.
#[tokio::test(flavor = "multi_thread")]
async fn a_session_no_member_holds_schedules_into_no_other_session() {
    let launch = launch().await;
    let anna = launch.session("anna").await;
    let nobodys = SessionId::new();
    let somebody_elses = SessionId::new();
    let into = |session: &SessionId| {
        json!({
            "name": "a note for a session",
            "trigger": { "type": "once", "due_at_utc": far().to_rfc3339() },
            "target": {
                "target_kind": "session",
                "type": "resumable_session",
                "session_id": session.to_string(),
                "action": { "type": "prompt", "prompt": "Run the report." },
            },
        })
    };
    let judge = launch
        .service
        .write_judge()
        .expect("the launch judges its agents' schedule writes");
    for (case, args) in [
        ("a member's session", into(&anna)),
        ("another session no member holds", into(&somebody_elses)),
        ("its own session", into(&nobodys)),
        ("a member", one_send(&launch.mob_id, "anna")),
    ] {
        let target: TargetBinding =
            serde_json::from_value(args["target"].clone()).expect("the target decodes");
        assert_eq!(
            judge.judge_schedule_write(&nobodys, &target).await,
            meerkat::ScheduleWriteVerdict::NotAMember,
            "a session no member holds is no member here: {case}"
        );
    }

    let refused = launch
        .schedule_call(&nobodys, "meerkat_schedule_create", into(&somebody_elses))
        .await;
    assert!(
        refused_session_creation(&refused),
        "its schedule into another session is refused: {refused:?}"
    );
    let mut exact = into(&anna);
    exact["target"]["type"] = json!("exact_session");
    let mut event = into(&anna);
    event["target"]["action"] =
        json!({ "type": "event", "event_type": "report.due", "payload": {} });
    for (case, args) in [("the exact form", exact), ("an event", event)] {
        let refused = launch
            .schedule_call(&nobodys, "meerkat_schedule_create", args)
            .await;
        assert!(
            refused_session_creation(&refused),
            "{case} into a member's session is not written for the member, and is refused: \
             {refused:?}"
        );
    }
    assert!(
        launch.service.list().await.unwrap().is_empty(),
        "nothing is stored"
    );
    let own = launch
        .schedule_call(&nobodys, "meerkat_schedule_create", into(&nobodys))
        .await;
    assert!(
        own.is_ok(),
        "its schedule into its own session is taken: {own:?}"
    );
    let members = launch
        .schedule_call(&nobodys, "meerkat_schedule_create", into(&anna))
        .await
        .expect("a schedule naming a member's session is written for the member");
    let members: Value = serde_json::from_str(&members).expect("the created schedule");
    assert_eq!(
        members["target"]["target_kind"], "mob",
        "a member's session is written as the member's target: {members}"
    );
    launch.stop().await;
}

/// The arguments of a create whose target is a session created on demand.
fn one_fresh_session() -> Value {
    json!({
        "name": "a fresh session",
        "trigger": { "type": "once", "due_at_utc": far().to_rfc3339() },
        "target": {
            "target_kind": "session",
            "type": "materialize_on_demand_session",
            "create": { "model": "gpt-5.4" },
            "action": { "type": "prompt", "prompt": "Run the report." },
        },
    })
}

/// Whether `answer` is the refusal of a schedule that would create a
/// session.
fn refused_session_creation(answer: &Result<String, meerkat_core::ToolError>) -> bool {
    matches!(
        answer,
        Err(meerkat_core::ToolError::PolicyDenied { denial })
            if denial.code == "schedule_session_creation_not_allowed"
    )
}

/// A member's schedule whose target is a session created on demand is
/// refused on the gateway, by a create and by an update of one the host
/// wrote, and nothing is written for it. The host's own call writes one as
/// before.
#[tokio::test(flavor = "multi_thread")]
async fn a_members_schedule_that_creates_a_session_is_refused() {
    let launch = launch().await;
    let anna = launch.session("anna").await;
    let helper = launch.helper().await;
    for (member, session) in [("anna", &anna), ("helper", &helper)] {
        let created = launch
            .schedule_call(session, "meerkat_schedule_create", one_fresh_session())
            .await;
        assert!(
            refused_session_creation(&created),
            "{member}'s schedule that creates a session is refused: {created:?}"
        );
    }
    assert!(
        launch.service.list().await.unwrap().is_empty(),
        "nothing is stored"
    );

    let hosts = launch
        .schedule_call_under(
            &meerkat_core::ToolDispatchContext::default(),
            "meerkat_schedule_create",
            one_fresh_session(),
        )
        .await
        .expect("the host's own call writes one");
    let hosts: Value = serde_json::from_str(&hosts).expect("the created schedule");
    let stored = launch.service.list().await.unwrap();
    assert_eq!(stored.len(), 1);
    let updated = launch
        .schedule_call(
            &anna,
            "meerkat_schedule_update",
            json!({ "schedule_id": hosts["schedule_id"], "name": "renamed" }),
        )
        .await;
    assert!(
        refused_session_creation(&updated),
        "a member's update of a schedule that creates a session is refused: {updated:?}"
    );
    assert_eq!(
        launch.service.list().await.unwrap(),
        stored,
        "the schedule is as it was"
    );
    launch.stop().await;
}
