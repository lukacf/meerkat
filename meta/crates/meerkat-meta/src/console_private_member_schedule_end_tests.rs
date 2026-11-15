//! What becomes of the schedules that target a member private to a person
//! once that member ends.
//!
//! The launch is composed the way `rpc_gateway` composes one (the gateway's
//! schedule tools attached to the builder, the runtime bootstrapped, the mob
//! targets adopted, the gateway schedule firing host spawned), and the
//! console is the one with sign-in. Only the model is scripted. A row runs
//! to its end and then fails with every expectation it found unmet.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use meerkat_client::types::LlmStream;
use meerkat_client::{LlmClient, LlmError, LlmEvent, LlmRequest};
use meerkat_core::Message;
use meerkat_core::service::{SessionHistoryQuery, SessionServiceHistoryExt as _};
use meerkat_core::types::{ContentInput, HandlingMode, SessionId};
use serde_json::json;

use super::console_private_side_chat_tests::{
    PRIVATE_TO, alice_claims, bearer, enforced_console, fork_on_console,
};
use super::console_token_lapse_tests::console_rpc_as;
use super::*;

/// A turn whose prompt holds this word makes the member's schedule; with
/// `every=<seconds>` the schedule's interval is that many seconds.
const MAKE: &str = "MAKE-THE-SCHEDULE-6R";
/// The schedule's prompt: the text of the member that made it.
const MARKER: &str = "PRIVATE-NOTE-6R-keep-this-between-us";
const SCHEDULE_CREATE: &str = "meerkat_schedule_create";
const SCHEDULE_LIST: &str = "meerkat_schedule_list";
const SCHEDULE_GET: &str = "meerkat_schedule_get";
/// The model of the profile whose members have the schedule and mob tools.
const PLANNER_MODEL: &str = "gpt-5.5";
/// The model of the profile of a later shared member.
const WORKER_MODEL: &str = "gpt-5.4";
/// The name the private side chat holds.
const SIDE_NAME: &str = "alpha-side";
/// A turn whose prompt holds this word and `mob=<id>` calls
/// `mob_spawn_member` for [`SIDE_NAME`].
const TAKE_BY_SPAWN: &str = "TAKE-THE-NAME-BY-SPAWN-6R";
/// A turn whose prompt holds this word calls the member tool `spawn_member`
/// for [`SIDE_NAME`].
const TAKE_BY_MEMBER_SPAWN: &str = "TAKE-THE-NAME-BY-MEMBER-SPAWN-6R";
/// A turn whose prompt holds this word calls `fork_off` with [`SIDE_NAME`]
/// as the child's name.
const TAKE_BY_FORK: &str = "TAKE-THE-NAME-BY-FORK-6R";
/// A turn whose prompt holds this word and `child=<name>` calls `fork_off`
/// with that name as the child's; with `make=yes` the child's task is to
/// make a schedule, and with `idle=yes` too the child is retired once it has
/// been idle a second.
const FORK_A_CHILD: &str = "FORK-A-CHILD-6R";
/// A turn whose prompt holds this word calls `meerkat_schedule_list`.
const LIST_SCHEDULES: &str = "LIST-THE-SCHEDULES-6R";
/// A turn whose prompt holds this word and `id=<schedule id>` calls
/// `meerkat_schedule_get` for that id.
const GET_SCHEDULE: &str = "GET-THE-SCHEDULE-6R";
/// A turn whose prompt holds this word, `tool=<schedule tool>` and
/// `id=<schedule id>` calls that tool for that id (an update renames it).
const CALL_SCHEDULE_TOOL: &str = "CALL-A-SCHEDULE-TOOL-6R";
/// A turn whose prompt holds this word and `child=<name>` calls the member
/// tool `spawn_member` for a member of that name.
const SPAWN_A_CHILD: &str = "SPAWN-A-CHILD-6R";
const MOB_SPAWN_MEMBER: &str = "mob_spawn_member";
const SPAWN_MEMBER: &str = "spawn_member";
const FORK_OFF: &str = "fork_off";
/// The call id of a scripted schedule read.
const READ_CALL: &str = "read-the-schedules";
/// The call id of a scripted seating under [`SIDE_NAME`].
const TAKE_CALL: &str = "take-the-name";

#[derive(Debug, Clone)]
struct SeenRequest {
    last_user_text: String,
    all_messages: String,
    tool_results: String,
}

#[derive(Default)]
struct Seen(Mutex<Vec<SeenRequest>>);

impl Seen {
    fn all(&self) -> Vec<SeenRequest> {
        self.0.lock().unwrap().clone()
    }
}

struct ScriptedClient {
    seen: Arc<Seen>,
}

fn last_user_text(request: &LlmRequest) -> String {
    request
        .messages
        .iter()
        .rev()
        .find_map(|message| match message {
            Message::User(user) => Some(user.text_content()),
            _ => None,
        })
        .unwrap_or_default()
}

fn results_this_turn(request: &LlmRequest) -> Vec<String> {
    request
        .messages
        .iter()
        .rev()
        .take_while(|message| !matches!(message, Message::User(_)))
        .filter_map(|message| match message {
            Message::ToolResults { results, .. } => Some(format!("{results:?}")),
            _ => None,
        })
        .collect()
}

/// The value of the word `<prefix><value>` in `text`, or nothing.
fn word(text: &str, prefix: &str) -> String {
    text.split_whitespace()
        .find_map(|word| word.strip_prefix(prefix))
        .unwrap_or_default()
        .to_string()
}

#[async_trait::async_trait]
impl LlmClient for ScriptedClient {
    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(&'a self, request: &'a LlmRequest) -> LlmStream<'a> {
        let last = last_user_text(request);
        let results = results_this_turn(request);
        self.seen.0.lock().unwrap().push(SeenRequest {
            last_user_text: last.clone(),
            all_messages: format!("{:?}", request.messages),
            tool_results: results.join(" | "),
        });
        let usage = LlmEvent::UsageUpdate {
            usage: meerkat_core::TurnUsage::host_declared(
                meerkat_core::Provider::OpenAI,
                &request.model,
                meerkat_core::Usage::default(),
            ),
        };
        let holds = |name: &str| request.tools.iter().any(|tool| tool.name == name);
        let call = |id: &str, name: &str, args: serde_json::Value| {
            vec![
                LlmEvent::ToolCallComplete {
                    id: id.to_string(),
                    name: name.to_string(),
                    args,
                    meta: None,
                },
                usage.clone(),
                LlmEvent::Done {
                    outcome: meerkat_client::LlmDoneOutcome::success(
                        meerkat_core::StopReason::ToolUse,
                    ),
                },
            ]
        };
        let first = results.is_empty();
        let events = if first && last.contains(LIST_SCHEDULES) && holds(SCHEDULE_LIST) {
            call(READ_CALL, SCHEDULE_LIST, json!({}))
        } else if first && last.contains(GET_SCHEDULE) && holds(SCHEDULE_GET) {
            call(
                READ_CALL,
                SCHEDULE_GET,
                json!({ "schedule_id": word(&last, "id=") }),
            )
        } else if first && last.contains(CALL_SCHEDULE_TOOL) {
            let tool = word(&last, "tool=");
            let args = if tool == "meerkat_schedule_update" {
                json!({ "schedule_id": word(&last, "id="), "name": "renamed" })
            } else {
                json!({ "schedule_id": word(&last, "id=") })
            };
            call(READ_CALL, &tool, args)
        } else if first && last.contains(SPAWN_A_CHILD) && holds(SPAWN_MEMBER) {
            call(
                "spawn-a-child",
                SPAWN_MEMBER,
                json!({ "profile": "planner", "member_id": word(&last, "child=") }),
            )
        } else if first && last.contains(TAKE_BY_SPAWN) && holds(MOB_SPAWN_MEMBER) {
            call(
                TAKE_CALL,
                MOB_SPAWN_MEMBER,
                json!({
                    "mob_id": word(&last, "mob="),
                    "profile": "worker",
                    "member_id": SIDE_NAME
                }),
            )
        } else if first && last.contains(TAKE_BY_MEMBER_SPAWN) && holds(SPAWN_MEMBER) {
            call(
                TAKE_CALL,
                SPAWN_MEMBER,
                json!({ "profile": "worker", "member_id": SIDE_NAME }),
            )
        } else if first && last.contains(TAKE_BY_FORK) && holds(FORK_OFF) {
            call(
                TAKE_CALL,
                FORK_OFF,
                json!({
                    "member_id": SIDE_NAME,
                    "task": "Reply with the single word ready."
                }),
            )
        } else if first && last.contains(FORK_A_CHILD) && holds(FORK_OFF) {
            let args = if last.contains("make=yes") && last.contains("idle=yes") {
                json!({
                    "member_id": word(&last, "child="),
                    "task": format!("{MAKE} every=6 remind me of what I told you"),
                    "idle_retire_secs": 1
                })
            } else if last.contains("make=yes") {
                json!({
                    "member_id": word(&last, "child="),
                    "task": format!("{MAKE} every=6 remind me of what I told you")
                })
            } else {
                json!({
                    "member_id": word(&last, "child="),
                    "task": "Reply with the single word ready."
                })
            };
            call("fork-a-child", FORK_OFF, args)
        } else if first && last.contains(MAKE) && holds(SCHEDULE_CREATE) {
            let every = word(&last, "every=").parse::<u64>().unwrap_or(2);
            call(
                "make-schedule",
                SCHEDULE_CREATE,
                json!({
                    "name": "a reminder",
                    "trigger": {
                        "type": "interval",
                        "start_at_utc": chrono::Utc::now().to_rfc3339(),
                        "every_seconds": every
                    },
                    "target": {
                        "target_kind": "session",
                        "type": "current_session",
                        "action": { "type": "prompt", "prompt": MARKER }
                    },
                    "missing_target_policy": "mark_misfired",
                    "planning_horizon_occurrences": 1
                }),
            )
        } else {
            vec![
                LlmEvent::TextDelta {
                    delta: "done".to_string(),
                    meta: None,
                },
                usage.clone(),
                LlmEvent::Done {
                    outcome: meerkat_client::LlmDoneOutcome::success(
                        meerkat_core::StopReason::EndTurn,
                    ),
                },
            ]
        };
        Box::pin(futures::stream::iter(events.into_iter().map(Ok)))
    }

    fn provider(&self) -> meerkat_core::Provider {
        meerkat_core::Provider::OpenAI
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }
}

type Sessions = Arc<meerkat_session::PersistentSessionService<meerkat::FactoryAgentBuilder>>;

/// A gateway-shaped launch: stores, the gateway's schedule tools attached
/// for the authoritative launch, the runtime, the adopted mob targets and
/// the gateway schedule firing host.
struct Launch {
    runtime: crate::UnifiedRuntime,
    attached: crate::schedule_wiring::AttachedScheduleTools,
    host: Option<meerkat::surface::ScheduleHostHandle>,
    watchdog: tokio::task::JoinHandle<()>,
    sessions: Sessions,
    seen: Arc<Seen>,
    mob_id: String,
    /// The expectations the row found unmet so far.
    unmet: Mutex<Vec<String>>,
    _stores: tempfile::TempDir,
    _schedule_state: tempfile::TempDir,
}

async fn boot(label: &str) -> Launch {
    use crate::mob_composition_manifest::CompositionAuthority;

    let authority = CompositionAuthority::Authoritative;
    let stores = tempfile::tempdir().expect("stores dir");
    let schedule_state = tempfile::tempdir().expect("schedule state dir");
    let session_store: Arc<dyn meerkat::SessionStore> = Arc::new(
        meerkat_store::SqliteSessionStore::open(stores.path().join("sessions.db"))
            .expect("session store"),
    );
    let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> = Arc::new(
        meerkat_runtime::store::SqliteRuntimeStore::new(stores.path().join("runtime.sqlite"))
            .expect("runtime store"),
    );
    let blob_store: Arc<dyn meerkat_core::BlobStore> =
        Arc::new(meerkat_store::MemoryBlobStore::new());
    let mut builder = meerkat::FactoryAgentBuilder::new(
        meerkat::AgentFactory::new(stores.path()).comms(true),
        meerkat::Config::default(),
    );
    builder.default_session_store = Some(Arc::new(meerkat_store::StoreAdapter::new(
        session_store.clone(),
    )));
    builder.default_blob_store = Some(blob_store.clone());
    let jobs: Arc<dyn meerkat::DetachedJobStore> = Arc::new(meerkat::MemoryDetachedJobStore::new());
    builder.default_detached_job_store = Some(Arc::clone(&jobs));
    let delivery =
        crate::mob_handle_runtime::MobRuntimeDelivery::new(Arc::clone(&runtime_store), jobs);
    let attached = crate::gateway_composition::attach_gateway_schedule_tools(
        &builder,
        schedule_state.path(),
        crate::gateway_composition::ScheduleFiringRole::for_composition_authority(authority),
    )
    .expect("schedule tools attach");
    let mob_tools_slot = Arc::clone(&builder.default_mob_tools);
    let machine = Arc::new(
        meerkat_runtime::MeerkatMachine::persistent(
            Arc::clone(&runtime_store),
            Arc::clone(&blob_store),
        )
        .expect("runtime machine"),
    );
    let sessions: Sessions = Arc::new(meerkat_session::PersistentSessionService::new(
        builder,
        16,
        session_store,
        runtime_store,
        blob_store,
    ));
    let mob_id = format!(
        "pme-{label}-{}",
        &uuid::Uuid::new_v4().simple().to_string()[..10]
    );
    let definition = meerkat_mob::MobDefinition::from_toml(&format!(
        r#"
[mob]
id = "{mob_id}"

[profiles.planner]
model = "{PLANNER_MODEL}"
runtime_mode = "turn_driven"
external_addressable = true

[profiles.planner.tools]
comms = true
schedule = true
mob = true

[profiles.reviewer]
model = "{PLANNER_MODEL}"
runtime_mode = "turn_driven"
external_addressable = true

[profiles.reviewer.tools]
comms = true
schedule = true
mob = true

[profiles.worker]
model = "{WORKER_MODEL}"
runtime_mode = "turn_driven"
external_addressable = true

[profiles.worker.tools]
comms = true
"#
    ))
    .expect("definition");
    let seen = Arc::new(Seen::default());
    let spec = crate::mob_handle_runtime::MobBootstrapSpec::new(
        definition,
        meerkat_mob::MobStorage::in_memory(),
        sessions.clone(),
    )
    .with_session_runtime_adapter(Arc::clone(&machine))
    .expect("the session runtime owner")
    .with_runtime_delivery(delivery)
    .with_agent_mob_tools(mob_tools_slot)
    .expect("install the agent mob tools")
    .with_composition_authority(authority)
    .with_options(crate::mob_handle_runtime::MobBootstrapOptions {
        allow_ephemeral_sessions: true,
        notify_orchestrator_on_resume: true,
        default_llm_client: Some(Arc::new(ScriptedClient {
            seen: Arc::clone(&seen),
        })),
    });
    // The idle sweep passes every second, so that a member whose own spawn
    // asked to be retired after a second of idleness is.
    let runtime = crate::UnifiedRuntime::bootstrap_with_options(
        spec,
        crate::MetaConfig {
            modules: vec![],
            discovery: crate::DiscoverySpec {
                namespace: format!("pme-{label}"),
                modules: vec![],
            },
            pre_spawn: vec![],
        },
        Vec::new(),
        Duration::from_secs(2),
        crate::runtime::RuntimeOptions {
            implicit_delegate_idle_sweep_interval_ms: 1_000,
            ..crate::runtime::RuntimeOptions::default()
        },
        Arc::new(crate::InMemoryMetadataStore::new()),
    )
    .await
    .expect("bootstrap");
    let mob_state = crate::gateway_composition::adopt_schedule_mob_targets(
        &runtime,
        &attached.service,
        &attached.mob_target_registry,
    )
    .await;
    let (host, watchdog) = crate::gateway_composition::spawn_gateway_schedule_host(
        &runtime,
        mob_state,
        crate::gateway_composition::GatewayScheduleHostInputs {
            schedule_service: attached.service.clone(),
            session_service: sessions.clone(),
            runtime_adapter: machine,
            schedule_store_path: schedule_state
                .path()
                .join(crate::schedule_wiring::SCHEDULE_STORE_FILE),
            firing_host_binding: attached.firing_host_binding.clone(),
            runnable_host: None,
            workgraph_service: None,
            owner_id: format!("pme-{label}-{}", uuid::Uuid::new_v4().simple()),
        },
    )
    .await;
    Launch {
        runtime,
        attached,
        host,
        watchdog,
        sessions,
        seen,
        mob_id,
        unmet: Mutex::new(Vec::new()),
        _stores: stores,
        _schedule_state: schedule_state,
    }
}

/// What the roster holds of one member.
struct Seat {
    private_to: Option<String>,
    session: Option<SessionId>,
}

/// The agent tool a shared agent seats a member under [`SIDE_NAME`] with.
#[derive(Debug, Clone, Copy)]
enum Door {
    MobSpawnMember,
    SpawnMember,
    ForkOff,
}

impl Launch {
    fn mob(&self) -> &MobRuntime {
        self.runtime.mob_runtime()
    }

    /// Note that the row expected `met` and did not find it.
    fn expect(&self, met: bool, what: impl Into<String>) {
        if !met {
            self.unmet.lock().unwrap().push(what.into());
        }
    }

    /// Stop the launch, then fail the row with what it found unmet.
    async fn finish(self) {
        let unmet = std::mem::take(&mut *self.unmet.lock().unwrap());
        if let Some(host) = self.host {
            host.shutdown().await;
        }
        self.watchdog.abort();
        let _ = self.watchdog.await;
        self.runtime.shutdown().await;
        assert!(unmet.is_empty(), "unmet: {unmet:#?}");
    }

    async fn entry(&self, name: &str) -> Option<Seat> {
        self.mob()
            .handle()
            .get_member(&AgentIdentity::from(name))
            .await
            .expect("roster read")
            .map(|entry| Seat {
                private_to: entry.labels.get(PRIVATE_TO).cloned(),
                session: entry.bridge_session_id().cloned(),
            })
    }

    /// Whether the roster stops holding `name` within `seconds`.
    async fn gone_within(&self, name: &str, seconds: u64) -> bool {
        for _ in 0..seconds * 10 {
            if self.entry(name).await.is_none() {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        false
    }

    /// A shared member of profile `profile` under `name`, seated by the
    /// host, after one turn of its own.
    async fn shared(&self, profile: &str, name: &str) {
        self.mob()
            .handle()
            .spawn_spec(meerkat_mob::SpawnMemberSpec::new(
                profile,
                AgentIdentity::from(name),
            ))
            .await
            .expect("spawn a shared member");
        self.turn(name, "The code word is heron.").await;
    }

    async fn turn(&self, name: &str, prompt: &str) {
        let member = self
            .mob()
            .handle()
            .member(&AgentIdentity::from(name))
            .await
            .expect("member handle");
        let turn = member
            .start_turn(
                ContentInput::Text(prompt.to_string()),
                HandlingMode::Queue,
                meerkat_mob::MemberTurnOptions::new(),
                None,
            )
            .await
            .expect("start the turn");
        turn.wait().await.expect("the turn ends");
    }

    /// The ids of the stored schedules that are not deleted.
    async fn schedule_ids(&self) -> Vec<String> {
        self.attached
            .service
            .list()
            .await
            .expect("list schedules")
            .into_iter()
            .map(|schedule| schedule.schedule_id.to_string())
            .collect()
    }

    /// `member`'s own turn makes its schedule, firing every `every`
    /// seconds: the schedule's id.
    async fn make_schedule(&self, member: &str, every: u64) -> String {
        let before = self.schedule_ids().await;
        self.turn(member, &format!("{MAKE} every={every} remind me"))
            .await;
        self.new_schedule(&before, 1).await
    }

    /// The one schedule stored since `before`, waited for `seconds`.
    async fn new_schedule(&self, before: &[String], seconds: u64) -> String {
        for _ in 0..seconds * 10 {
            let made: Vec<String> = self
                .schedule_ids()
                .await
                .into_iter()
                .filter(|id| !before.contains(id))
                .collect();
            if let [id] = made.as_slice() {
                return id.clone();
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!("no schedule was stored within {seconds} s");
    }

    /// The phase the store holds the schedule in now.
    async fn phase(&self, schedule_id: &str) -> Option<meerkat::SchedulePhase> {
        let id = meerkat::ScheduleId::parse(schedule_id).expect("schedule id");
        self.attached
            .service
            .get(&id)
            .await
            .ok()
            .map(|schedule| schedule.phase)
    }

    /// Whether the store holds the schedule as deleted within `seconds`.
    async fn deleted_within(&self, schedule_id: &str, seconds: u64) -> bool {
        for _ in 0..seconds * 10 {
            if self.phase(schedule_id).await == Some(meerkat::SchedulePhase::Deleted) {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        false
    }

    /// The schedule's occurrences as the store holds them now, in order:
    /// each one's ordinal, phase and failure.
    async fn occurrences(&self, schedule_id: &str) -> Vec<String> {
        let id = meerkat::ScheduleId::parse(schedule_id).expect("schedule id");
        let mut occurrences = self
            .attached
            .service
            .list_occurrences(&id)
            .await
            .expect("list the occurrences");
        occurrences.sort_by_key(|occurrence| occurrence.occurrence_ordinal.0);
        occurrences
            .into_iter()
            .map(|occurrence| {
                format!(
                    "#{} {:?} {:?} {:?}",
                    occurrence.occurrence_ordinal.0,
                    occurrence.phase,
                    occurrence.failure_class,
                    occurrence.failure_detail
                )
            })
            .collect()
    }

    /// Whether a model request made after the first `since` holds the
    /// schedule's prompt, waited for `seconds`.
    async fn fired_within(&self, since: usize, seconds: u64) -> bool {
        for _ in 0..seconds * 4 {
            if self.fired_since(since) {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        false
    }

    /// Whether a model request made after the first `since` holds the
    /// schedule's prompt as its last user text.
    fn fired_since(&self, since: usize) -> bool {
        self.seen
            .all()
            .into_iter()
            .skip(since)
            .any(|request| request.last_user_text.contains(MARKER))
    }

    /// Whether any model request made after the first `since` holds the
    /// schedule's prompt anywhere.
    fn marker_since(&self, since: usize) -> bool {
        self.seen
            .all()
            .into_iter()
            .skip(since)
            .any(|request| request.all_messages.contains(MARKER))
    }

    async fn history_holds_marker(&self, session: &SessionId) -> bool {
        let page = self
            .sessions
            .read_history(
                session,
                SessionHistoryQuery {
                    offset: 0,
                    limit: None,
                },
            )
            .await
            .expect("read the history");
        format!("{:?}", page.messages).contains(MARKER)
    }

    /// One schedule read by `agent`'s own turn: the tool's answer as its
    /// model saw it.
    async fn schedule_read(&self, agent: &str, prompt: &str) -> String {
        let before = self.seen.all().len();
        self.turn(agent, prompt).await;
        self.seen
            .all()
            .into_iter()
            .skip(before)
            .find(|request| request.tool_results.contains(READ_CALL))
            .map(|request| request.tool_results)
            .expect("the agent's schedule tool answered")
    }

    /// The shared agent `agent` seats a member under [`SIDE_NAME`] with its
    /// own tool call through `door`; the seat, when the name is held within
    /// ten seconds. A `fork_off` call answers only when its child's run
    /// ends, so the agent's turn runs off this task.
    async fn agent_takes_the_name(&self, agent: &str, door: Door) -> Option<Seat> {
        let word = match door {
            Door::MobSpawnMember => TAKE_BY_SPAWN,
            Door::SpawnMember => TAKE_BY_MEMBER_SPAWN,
            Door::ForkOff => TAKE_BY_FORK,
        };
        self.agent_seats(
            agent,
            &format!("{word} mob={} please", self.mob_id),
            SIDE_NAME,
        )
        .await
    }

    /// `agent`'s turn on `prompt`, off this task, and the seat of `name`
    /// when the roster holds it within ten seconds.
    async fn agent_seats(&self, agent: &str, prompt: &str, name: &str) -> Option<Seat> {
        let member = self
            .mob()
            .handle()
            .member(&AgentIdentity::from(agent))
            .await
            .expect("member handle");
        let prompt = prompt.to_string();
        let turn = tokio::spawn(async move {
            let turn = member
                .start_turn(
                    ContentInput::Text(prompt),
                    HandlingMode::Queue,
                    meerkat_mob::MemberTurnOptions::new(),
                    None,
                )
                .await
                .expect("start the turn");
            let _ = turn.wait().await;
        });
        for _ in 0..100 {
            if let Some(seat) = self.entry(name).await {
                return Some(seat);
            }
            if turn.is_finished() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        self.entry(name).await
    }
}

/// The console with sign-in over `launch`, and the first person's side chat
/// of the shared agent `alpha` under [`SIDE_NAME`], private to her.
async fn private_side_chat(launch: &Launch) -> (Router, String) {
    launch.shared("planner", "alpha").await;
    let app = console_json_router_with_state(enforced_console(launch.mob(), None));
    let owner = bearer(alice_claims());
    let forked = fork_on_console(&app, Some(&owner), "alpha", SIDE_NAME).await;
    assert!(forked["error"].is_null(), "{forked:#}");
    let side = launch
        .entry(SIDE_NAME)
        .await
        .expect("the side chat is seated");
    assert!(
        side.private_to.is_some(),
        "the side chat is private to its creator"
    );
    (app, owner)
}

/// The owner closes her side chat on the console.
async fn close(launch: &Launch, app: &Router, owner: &str, name: &str) {
    let closed = console_rpc_as(
        app,
        Some(owner),
        "meta/retire_member",
        json!({ "member_id": name }),
    )
    .await;
    assert!(closed["error"].is_null(), "{closed:#}");
    assert!(
        launch.gone_within(name, 10).await,
        "the roster no longer holds the closed side chat"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_private_side_chats_schedule_is_deleted_when_its_owner_closes_it() {
    let launch = boot("close").await;
    let (app, owner) = private_side_chat(&launch).await;
    let before = launch.seen.all().len();
    let schedule = launch.make_schedule(SIDE_NAME, 2).await;
    launch.expect(
        launch.fired_within(before, 15).await,
        "the schedule fires into the private side chat while it lives",
    );
    launch.expect(
        launch.phase(&schedule).await == Some(meerkat::SchedulePhase::Active),
        "the schedule is active while the private side chat lives",
    );

    close(&launch, &app, &owner, SIDE_NAME).await;
    launch.expect(
        launch.deleted_within(&schedule, 10).await,
        format!(
            "the closed private side chat's schedule is deleted (it is {:?})",
            launch.phase(&schedule).await
        ),
    );
    launch.expect(
        !launch.schedule_ids().await.contains(&schedule),
        "the closed private side chat's schedule is no longer listed by the service",
    );
    // What its occurrences do: none waits to fire, and none is delivered.
    let since = launch.seen.all().len();
    tokio::time::sleep(Duration::from_secs(5)).await;
    let waiting: Vec<String> = launch
        .attached
        .service
        .list_occurrences(&meerkat::ScheduleId::parse(&schedule).expect("schedule id"))
        .await
        .expect("list the occurrences")
        .into_iter()
        .filter(|occurrence| {
            matches!(
                occurrence.phase,
                meerkat::OccurrencePhase::Pending
                    | meerkat::OccurrencePhase::Claimed
                    | meerkat::OccurrencePhase::Dispatching
                    | meerkat::OccurrencePhase::AwaitingCompletion
            )
        })
        .map(|occurrence| {
            format!(
                "#{} {:?}",
                occurrence.occurrence_ordinal.0, occurrence.phase
            )
        })
        .collect();
    launch.expect(
        waiting.is_empty(),
        format!("no occurrence of the deleted schedule waits to fire (waiting: {waiting:?})"),
    );
    launch.expect(
        !launch.marker_since(since),
        "no model request made after the delete holds the schedule's prompt",
    );
    launch.finish().await;
}

/// The owner closes her private side chat, then the shared agent `alpha`
/// seats a member under its name through `door`.
async fn a_shared_agent_takes_a_closed_private_side_chats_name(door: Door) {
    let launch = boot("door").await;
    let (app, owner) = private_side_chat(&launch).await;
    let schedule = launch.make_schedule(SIDE_NAME, 2).await;
    close(&launch, &app, &owner, SIDE_NAME).await;
    launch.expect(
        launch.deleted_within(&schedule, 10).await,
        format!(
            "the closed private side chat's schedule is deleted (it is {:?})",
            launch.phase(&schedule).await
        ),
    );

    let since = launch.seen.all().len();
    let later = launch.agent_takes_the_name("alpha", door).await;
    launch.expect(
        later.is_some(),
        format!("{door:?}: the shared agent seats a member under the name"),
    );
    if let Some(later) = later {
        launch.expect(
            later.private_to.is_none(),
            format!("{door:?}: the later holder of the name is private to no one"),
        );
        // Four intervals of the schedule.
        tokio::time::sleep(Duration::from_secs(8)).await;
        launch.expect(
            !launch.marker_since(since),
            format!(
                "{door:?}: no model request made after the name was taken holds the closed \
                 side chat's schedule prompt"
            ),
        );
        if let Some(session) = later.session.as_ref() {
            launch.expect(
                !launch.history_holds_marker(session).await,
                format!(
                    "{door:?}: the later holder's transcript does not hold the closed side \
                     chat's schedule prompt"
                ),
            );
        }
    }
    launch.finish().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_closed_private_side_chats_schedule_fires_into_no_member_an_agents_mob_spawn_member_seats()
 {
    a_shared_agent_takes_a_closed_private_side_chats_name(Door::MobSpawnMember).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_closed_private_side_chats_schedule_fires_into_no_member_an_agents_spawn_member_seats() {
    a_shared_agent_takes_a_closed_private_side_chats_name(Door::SpawnMember).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_closed_private_side_chats_schedule_fires_into_no_member_an_agents_fork_off_seats() {
    a_shared_agent_takes_a_closed_private_side_chats_name(Door::ForkOff).await;
}

/// How a private side chat's fork child ends.
#[derive(Debug, Clone, Copy)]
enum ChildEnd {
    /// The child is retired once it has been idle, while its side chat
    /// lives.
    IdlesOut,
    /// The owner closes the side chat, and the child ends with it (or is
    /// closed by her too, where the close leaves it seated).
    ClosedWithItsSideChat,
}

/// A private side chat forks a child under an ordinary name; the child
/// makes a schedule for itself and ends as `end` says. Then a shared agent
/// forks a child of its own under the same name.
async fn a_private_side_chats_fork_child_ends(end: ChildEnd) {
    let launch = boot("child").await;
    let (app, owner) = private_side_chat(&launch).await;
    launch.shared("planner", "gamma").await;
    let child = "report-helper";

    // The private side chat forks a child whose task is to make a schedule
    // for itself.
    let idle = match end {
        ChildEnd::IdlesOut => "idle=yes",
        ChildEnd::ClosedWithItsSideChat => "idle=no",
    };
    let before = launch.schedule_ids().await;
    launch
        .turn(
            SIDE_NAME,
            &format!("{FORK_A_CHILD} child={child} make=yes {idle} please"),
        )
        .await;
    let schedule = launch.new_schedule(&before, 20).await;
    match end {
        ChildEnd::IdlesOut => {
            launch.expect(
                launch.gone_within(child, 40).await,
                "the fork child is retired once it has been idle",
            );
            launch.expect(
                launch.entry(SIDE_NAME).await.is_some(),
                "the private side chat itself is still seated",
            );
        }
        ChildEnd::ClosedWithItsSideChat => {
            let seat = launch.entry(child).await;
            launch.expect(
                seat.as_ref().is_some_and(|seat| seat.private_to.is_some()),
                "the private side chat's fork child is seated, private to the same person",
            );
            close(&launch, &app, &owner, SIDE_NAME).await;
            if launch.entry(child).await.is_some() {
                close(&launch, &app, &owner, child).await;
            }
        }
    }
    launch.expect(
        launch.deleted_within(&schedule, 10).await,
        format!(
            "{end:?}: the ended fork child's schedule is deleted (it is {:?})",
            launch.phase(&schedule).await
        ),
    );

    // A shared agent forks a child of its own under the same name.
    let since = launch.seen.all().len();
    let later = launch
        .agent_seats(
            "gamma",
            &format!("{FORK_A_CHILD} child={child} make=no please"),
            child,
        )
        .await;
    launch.expect(
        later.is_some(),
        format!("{end:?}: a shared agent's fork_off seats a child under the same name"),
    );
    if let Some(later) = later {
        launch.expect(
            later.private_to.is_none(),
            format!("{end:?}: the shared agent's child is private to no one"),
        );
        // Two intervals of the ended child's schedule.
        tokio::time::sleep(Duration::from_secs(13)).await;
        launch.expect(
            !launch.marker_since(since),
            format!(
                "{end:?}: no model request made after the name was taken holds the ended \
                 child's schedule prompt"
            ),
        );
        if let Some(session) = later.session.as_ref() {
            launch.expect(
                !launch.history_holds_marker(session).await,
                format!(
                    "{end:?}: the later child's transcript does not hold the ended child's \
                     schedule prompt"
                ),
            );
        }
    }
    launch.finish().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_private_side_chats_fork_childs_schedule_is_deleted_when_the_child_idles_out() {
    a_private_side_chats_fork_child_ends(ChildEnd::IdlesOut).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_private_side_chats_fork_childs_schedule_is_deleted_when_the_side_chat_is_closed() {
    a_private_side_chats_fork_child_ends(ChildEnd::ClosedWithItsSideChat).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn another_agent_is_listed_no_schedule_of_a_closed_private_side_chat_once_its_name_is_held() {
    let launch = boot("reads").await;
    let (app, owner) = private_side_chat(&launch).await;
    launch.shared("planner", "beta").await;
    let schedule = launch.make_schedule(SIDE_NAME, 2).await;

    // While the side chat lives: its own read holds the schedule, another
    // agent's holds nothing of it.
    let own = launch
        .schedule_read(SIDE_NAME, &format!("{LIST_SCHEDULES} please"))
        .await;
    launch.expect(
        own.contains(&schedule),
        "the private side chat lists its own schedule while it lives",
    );
    let others = launch
        .schedule_read("beta", &format!("{LIST_SCHEDULES} please"))
        .await;
    launch.expect(
        !others.contains(&schedule) && !others.contains(MARKER),
        "another agent's list holds nothing of a live private side chat's schedule",
    );

    close(&launch, &app, &owner, SIDE_NAME).await;
    launch.expect(
        launch.deleted_within(&schedule, 10).await,
        format!(
            "the closed private side chat's schedule is deleted (it is {:?})",
            launch.phase(&schedule).await
        ),
    );

    // A shared member holds the name.
    let since = launch.seen.all().len();
    launch
        .mob()
        .handle()
        .spawn_spec(meerkat_mob::SpawnMemberSpec::new(
            "worker",
            AgentIdentity::from(SIDE_NAME),
        ))
        .await
        .expect("spawn a shared member under the name");
    let listed = launch
        .schedule_read("beta", &format!("{LIST_SCHEDULES} please"))
        .await;
    launch.expect(
        !listed.contains(&schedule) && !listed.contains(MARKER),
        "another agent's meerkat_schedule_list holds nothing of the closed side chat's \
         schedule once a shared member holds the name",
    );
    // No list shows the schedule, so an agent comes by its id on no surface
    // of its tools. By its id the record of a deleted schedule is still
    // read: the tools answer it with its phase.
    let got = launch
        .schedule_read("beta", &format!("{GET_SCHEDULE} id={schedule} please"))
        .await;
    println!(
        "ROW: meerkat_schedule_get of the deleted schedule by its id answers: {}",
        got.chars().take(600).collect::<String>()
    );
    launch.expect(
        got.contains(&schedule) && got.contains("deleted"),
        format!("a get by its id answers the deleted schedule's record: {got}"),
    );
    // The same agent's writes by the id: the schedule lifecycle refuses a
    // pause and a resume of a deleted schedule, and accepts an update and a
    // repeated delete of its record; after each the schedule is deleted
    // still.
    for (tool, refused) in [
        ("meerkat_schedule_update", false),
        ("meerkat_schedule_pause", true),
        ("meerkat_schedule_resume", true),
        ("meerkat_schedule_delete", false),
    ] {
        let answer = launch
            .schedule_read(
                "beta",
                &format!("{CALL_SCHEDULE_TOOL} tool={tool} id={schedule} please"),
            )
            .await;
        println!(
            "ROW: {tool} of the deleted schedule by its id answers: {}",
            answer.chars().take(400).collect::<String>()
        );
        launch.expect(
            answer.contains("is_error: true") == refused,
            format!("{tool} of the deleted schedule by its id: refused is {refused}: {answer}"),
        );
        launch.expect(
            launch.phase(&schedule).await == Some(meerkat::SchedulePhase::Deleted),
            format!(
                "after another agent's {tool} the schedule stays deleted (it is {:?})",
                launch.phase(&schedule).await
            ),
        );
    }
    // Nothing of the deleted schedule fires, into the member that holds the
    // name or anywhere else.
    tokio::time::sleep(Duration::from_secs(6)).await;
    let fired: Vec<String> = launch
        .seen
        .all()
        .into_iter()
        .skip(since)
        .filter(|request| request.last_user_text.contains(MARKER))
        .map(|request| request.last_user_text)
        .collect();
    launch.expect(
        fired.is_empty(),
        format!("no occurrence of the deleted schedule fires once the name is held: {fired:?}"),
    );
    launch.finish().await;
}

/// A member a private side chat spawned with its member tool makes a
/// schedule for itself. When the side chat is closed, the member either
/// ends with it or goes on; its schedule is kept while it lives and deleted
/// when it ends.
#[tokio::test(flavor = "multi_thread")]
async fn a_member_a_private_side_chat_spawned_loses_its_schedule_when_it_ends() {
    let launch = boot("spawned").await;
    let (app, owner) = private_side_chat(&launch).await;
    let child = "report-helper";
    let seat = launch
        .agent_seats(
            SIDE_NAME,
            &format!("{SPAWN_A_CHILD} child={child} please"),
            child,
        )
        .await;
    launch.expect(
        seat.as_ref().is_some_and(|seat| seat.private_to.is_some()),
        "the member a private side chat spawned is seated, private to the same person",
    );
    if seat.is_none() {
        launch.finish().await;
        return;
    }
    let before = launch.seen.all().len();
    let schedule = launch.make_schedule(child, 2).await;
    launch.expect(
        launch.fired_within(before, 15).await,
        "the schedule fires into the spawned member while it lives",
    );

    close(&launch, &app, &owner, SIDE_NAME).await;
    let ended_with_it = launch.gone_within(child, 5).await;
    println!(
        "ROW: the member a private side chat spawned ended with the side chat: {ended_with_it}"
    );
    if !ended_with_it {
        // It goes on, and its schedule with it.
        let since = launch.seen.all().len();
        tokio::time::sleep(Duration::from_secs(6)).await;
        launch.expect(
            launch.phase(&schedule).await == Some(meerkat::SchedulePhase::Active),
            format!(
                "the spawned member's schedule stays active while it lives (it is {:?})",
                launch.phase(&schedule).await
            ),
        );
        println!(
            "ROW: after the side chat's close the spawned member's schedule fired into it \
             again within six seconds: {} ; its occurrences: {:?}",
            launch.fired_since(since),
            launch.occurrences(&schedule).await
        );
        launch
            .mob()
            .handle()
            .retire(AgentIdentity::from(child))
            .await
            .expect("retire the spawned member");
        launch.expect(
            launch.gone_within(child, 10).await,
            "the roster no longer holds the retired spawned member",
        );
    }
    launch.expect(
        launch.deleted_within(&schedule, 10).await,
        format!(
            "the ended spawned member's schedule is deleted (it is {:?})",
            launch.phase(&schedule).await
        ),
    );
    launch.finish().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_shared_members_schedule_is_kept_when_the_member_is_retired() {
    let launch = boot("shared").await;
    launch.shared("planner", "gamma").await;
    let schedule = launch.make_schedule("gamma", 2).await;
    launch
        .mob()
        .handle()
        .retire(AgentIdentity::from("gamma"))
        .await
        .expect("retire the shared member");
    assert!(
        launch.gone_within("gamma", 10).await,
        "the roster no longer holds the retired shared member"
    );
    tokio::time::sleep(Duration::from_secs(3)).await;
    launch.expect(
        launch.phase(&schedule).await == Some(meerkat::SchedulePhase::Active),
        format!(
            "a retired shared member's schedule stays as it was (it is {:?})",
            launch.phase(&schedule).await
        ),
    );
    launch.finish().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_respawned_private_side_chat_keeps_its_schedule() {
    let launch = boot("respawn").await;
    let (_app, _owner) = private_side_chat(&launch).await;
    let before = launch.seen.all().len();
    let schedule = launch.make_schedule(SIDE_NAME, 2).await;
    launch.expect(
        launch.fired_within(before, 15).await,
        "the schedule fires into the private side chat while it lives",
    );

    let respawned = launch
        .mob()
        .handle()
        .respawn(AgentIdentity::from(SIDE_NAME), None)
        .await;
    launch.expect(
        respawned.is_ok(),
        format!(
            "the private side chat is respawned: {:?}",
            respawned.as_ref().err()
        ),
    );
    let seat = launch.entry(SIDE_NAME).await;
    launch.expect(
        seat.as_ref().is_some_and(|seat| seat.private_to.is_some()),
        "the respawned side chat is seated, private to the same person",
    );
    let since = launch.seen.all().len();
    tokio::time::sleep(Duration::from_secs(3)).await;
    launch.expect(
        launch.phase(&schedule).await == Some(meerkat::SchedulePhase::Active),
        format!(
            "a respawned private side chat's schedule stays active (it is {:?})",
            launch.phase(&schedule).await
        ),
    );
    launch.expect(
        launch.fired_within(since, 15).await,
        "the schedule fires into the respawned private side chat",
    );
    launch.finish().await;
}

/// A follower that keeps the ends it is handed.
#[derive(Default)]
struct KeptEnds(Mutex<Vec<crate::private_owners::EndedPrivateMember>>);

#[async_trait::async_trait]
impl crate::private_owners::PrivateMemberEndFollower for KeptEnds {
    async fn member_ended(&self, ended: &crate::private_owners::EndedPrivateMember) {
        self.0.lock().unwrap().push(ended.clone());
    }
}

impl KeptEnds {
    fn members(&self) -> Vec<String> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .map(|ended| ended.member.clone())
            .collect()
    }
}

/// The whole journal of the launch's mob.
async fn journal(launch: &Launch) -> Vec<meerkat_mob::MobEvent> {
    let handle = launch.mob().handle();
    let mut events = Vec::new();
    loop {
        let after = events
            .last()
            .map_or(0, |event: &meerkat_mob::MobEvent| event.cursor);
        let page = handle
            .poll_events(after, 512)
            .await
            .expect("poll the journal");
        if page.is_empty() {
            return events;
        }
        events.extend(page);
    }
}

/// The retirement start of a closed private side chat, shown to the record
/// only after a shared member was built under its name (a pump that had
/// fallen behind): the record of the members this process built names
/// nobody for the name by then, and the end is still handed to the
/// follower, because Core's member history holds the labels of the
/// incarnation the retirement names.
#[tokio::test(flavor = "multi_thread")]
async fn an_end_shown_after_a_shared_member_holds_the_name_is_still_a_private_members_end() {
    let launch = boot("history").await;
    let (app, owner) = private_side_chat(&launch).await;
    close(&launch, &app, &owner, SIDE_NAME).await;
    launch.shared("worker", SIDE_NAME).await;
    let owners = launch.mob().private_owners().clone();
    assert_eq!(
        owners.owner_of(&launch.mob_id, SIDE_NAME),
        None,
        "the record of the built members names nobody for the name once a shared member \
         holds it"
    );

    let kept = Arc::new(KeptEnds::default());
    owners.follow_member_ends(kept.clone()).await;
    let started = journal(&launch)
        .await
        .into_iter()
        .find(|event| {
            matches!(
                &event.kind,
                meerkat_mob::MobEventKind::MemberRetirementStarted { agent_identity, .. }
                    if agent_identity.as_str() == SIDE_NAME
            )
        })
        .expect("the journal holds the closed side chat's retirement start");
    owners
        .observe_mob_event(&launch.mob().handle(), &started)
        .await;
    launch.expect(
        kept.members() == vec![SIDE_NAME.to_string()],
        format!(
            "the closed private side chat's end is handed to the follower (handed: {:?})",
            kept.members()
        ),
    );
    launch.finish().await;
}

/// A reset of the gateway's own mob retires every member: a private side
/// chat ends with it, and so does its schedule.
#[tokio::test(flavor = "multi_thread")]
async fn a_private_side_chats_schedule_is_deleted_when_its_mob_is_reset() {
    let launch = boot("mobreset").await;
    let (_app, _owner) = private_side_chat(&launch).await;
    let schedule = launch.make_schedule(SIDE_NAME, 2).await;
    let reset = launch.mob().handle().reset().await;
    launch.expect(
        reset.is_ok(),
        format!("the mob is reset: {:?}", reset.as_ref().err()),
    );
    launch.expect(
        launch.gone_within(SIDE_NAME, 15).await,
        "the roster no longer holds the private side chat after the reset",
    );
    launch.expect(
        launch.deleted_within(&schedule, 15).await,
        format!(
            "the private side chat's schedule is deleted by the reset of its mob (it is {:?})",
            launch.phase(&schedule).await
        ),
    );
    launch.finish().await;
}

/// The labels of a member the host seats private to a person, below the
/// console's doors.
fn private_labels() -> std::collections::BTreeMap<String, String> {
    std::collections::BTreeMap::from([(
        PRIVATE_TO.to_string(),
        crate::personal_accounts::private_owner_key(
            "https://issuer.example.test",
            "oidc-sub-a-person",
        ),
    )])
}

/// The spec of the member `name` of profile `profile`, private to a
/// person; seated as a long-term agent (a durable identity) when `durable`.
fn private_member_spec(profile: &str, name: &str, durable: bool) -> meerkat_mob::SpawnMemberSpec {
    let mut spec = meerkat_mob::SpawnMemberSpec::new(profile, AgentIdentity::from(name))
        .with_labels(private_labels());
    if durable {
        spec.continuity_intent = meerkat_mob::SpawnContinuityIntent::DurableIdentity {
            continuity_key: name.to_string(),
        };
    }
    spec
}

/// A member private to a person under `name`, seated by the host, with a
/// schedule of its own that has fired into it: the schedule's id.
async fn private_member_with_a_schedule(launch: &Launch, name: &str, durable: bool) -> String {
    launch
        .mob()
        .handle()
        .spawn_spec(private_member_spec("planner", name, durable))
        .await
        .expect("seat a member private to a person");
    let before = launch.seen.all().len();
    let schedule = launch.make_schedule(name, 2).await;
    launch.expect(
        launch.fired_within(before, 15).await,
        "the schedule fires into the private member while it lives",
    );
    schedule
}

/// After `what`, the schedule is still active and fires into the member
/// that holds the name.
async fn expect_kept(launch: &Launch, schedule: &str, what: &str) {
    let since = launch.seen.all().len();
    tokio::time::sleep(Duration::from_secs(3)).await;
    launch.expect(
        launch.phase(schedule).await == Some(meerkat::SchedulePhase::Active),
        format!(
            "{what}: the private member's schedule stays active (it is {:?})",
            launch.phase(schedule).await
        ),
    );
    launch.expect(
        launch.fired_within(since, 15).await,
        format!("{what}: the schedule fires into the member again"),
    );
}

/// A rebuild that keeps a member private to a person under its name.
#[derive(Debug, Clone, Copy)]
enum Rebuild {
    /// A respawn: the call the Console's respawn and reset make for a live
    /// member that is no side chat.
    Respawn,
    /// A respawn onto another profile.
    ProfileMove,
    /// A rotation onto a fresh session: a reset.
    Reset,
}

async fn a_private_members_schedule_is_kept_across(rebuild: Rebuild) {
    let launch = boot("rebuild").await;
    let name = "notes";
    let schedule = private_member_with_a_schedule(&launch, name, false).await;
    let handle = launch.mob().handle();
    let rebuilt = match rebuild {
        Rebuild::Respawn => handle
            .respawn(AgentIdentity::from(name), None)
            .await
            .map(|_| ())
            .map_err(|error| format!("{error:?}")),
        Rebuild::ProfileMove => handle
            .respawn_with_successor_spec(private_member_spec("reviewer", name, false))
            .await
            .map(|_| ())
            .map_err(|error| format!("{error:?}")),
        Rebuild::Reset => handle
            .rotate_member(private_member_spec("planner", name, false))
            .await
            .map(|_| ())
            .map_err(|error| format!("{error:?}")),
    };
    launch.expect(
        rebuilt.is_ok(),
        format!("{rebuild:?}: the member is rebuilt: {rebuilt:?}"),
    );
    let seat = launch.entry(name).await;
    launch.expect(
        seat.as_ref().is_some_and(|seat| seat.private_to.is_some()),
        format!("{rebuild:?}: the rebuilt member is seated, private to the same person"),
    );
    expect_kept(&launch, &schedule, &format!("{rebuild:?}")).await;
    launch.finish().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_private_members_schedule_is_kept_across_a_respawn() {
    a_private_members_schedule_is_kept_across(Rebuild::Respawn).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_private_members_schedule_is_kept_across_a_profile_move() {
    a_private_members_schedule_is_kept_across(Rebuild::ProfileMove).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_private_members_schedule_is_kept_across_a_reset() {
    a_private_members_schedule_is_kept_across(Rebuild::Reset).await;
}

/// An ephemeral member private to a person that is no side chat, retired by
/// its host: it ends, and so does its schedule.
#[tokio::test(flavor = "multi_thread")]
async fn an_ephemeral_private_members_schedule_is_deleted_when_the_member_is_retired() {
    let launch = boot("retired").await;
    let name = "notes";
    let schedule = private_member_with_a_schedule(&launch, name, false).await;
    launch
        .mob()
        .handle()
        .retire(AgentIdentity::from(name))
        .await
        .expect("retire the private member");
    launch.expect(
        launch.gone_within(name, 10).await,
        "the roster no longer holds the retired member",
    );
    launch.expect(
        launch.deleted_within(&schedule, 10).await,
        format!(
            "the retired private member's schedule is deleted (it is {:?})",
            launch.phase(&schedule).await
        ),
    );
    launch.finish().await;
}

/// A long-term agent private to a person whose member is retired and seated
/// again on its name, as a rebuild that keeps the agent does where Core
/// seats no successor in one step (a forced compaction, a reload): the
/// retirement is no end of the agent, and its schedule is kept.
#[tokio::test(flavor = "multi_thread")]
async fn a_private_long_term_agents_schedule_is_kept_when_its_member_is_retired_and_seated_again() {
    let launch = boot("longterm").await;
    let name = "keeper";
    let schedule = private_member_with_a_schedule(&launch, name, true).await;
    launch
        .mob()
        .handle()
        .retire(AgentIdentity::from(name))
        .await
        .expect("retire the long-term agent's member");
    launch.expect(
        launch.gone_within(name, 10).await,
        "the roster no longer holds the retired member",
    );
    tokio::time::sleep(Duration::from_secs(3)).await;
    launch.expect(
        launch.phase(&schedule).await == Some(meerkat::SchedulePhase::Active),
        format!(
            "the long-term agent's schedule stays active while no member holds its name (it \
             is {:?})",
            launch.phase(&schedule).await
        ),
    );
    let seated = launch
        .mob()
        .handle()
        .spawn_spec(private_member_spec("planner", name, true))
        .await;
    launch.expect(
        seated.is_ok(),
        format!(
            "the long-term agent's member is seated again: {:?}",
            seated.as_ref().err()
        ),
    );
    expect_kept(&launch, &schedule, "seated again").await;
    launch.finish().await;
}

/// The Console refuses to respawn or reset a side chat (it is closed and
/// forked again); a refused call ends nothing, and the private side chat's
/// schedule stays.
#[tokio::test(flavor = "multi_thread")]
async fn a_private_side_chats_schedule_stays_when_the_console_refuses_its_respawn_and_reset() {
    let launch = boot("refused").await;
    let (app, owner) = private_side_chat(&launch).await;
    let before = launch.seen.all().len();
    let schedule = launch.make_schedule(SIDE_NAME, 2).await;
    launch.expect(
        launch.fired_within(before, 15).await,
        "the schedule fires into the private side chat while it lives",
    );
    for (method, params) in [
        ("meta/respawn_member", json!({ "member_id": SIDE_NAME })),
        ("meta/reset", json!({ "identity": SIDE_NAME })),
    ] {
        let answer = console_rpc_as(&app, Some(&owner), method, params).await;
        launch.expect(
            !answer["error"].is_null(),
            format!("{method} of a side chat is refused: {answer}"),
        );
        launch.expect(
            launch.entry(SIDE_NAME).await.is_some(),
            format!("{method}: the side chat is still seated"),
        );
    }
    expect_kept(&launch, &schedule, "refused respawn and reset").await;
    launch.finish().await;
}
