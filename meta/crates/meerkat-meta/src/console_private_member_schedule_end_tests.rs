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
/// A turn whose prompt holds this word makes a schedule whose target is a
/// session created on demand.
const MAKE_FRESH: &str = "MAKE-A-FRESH-SESSION-SCHEDULE-6R";
/// The prompt of the schedule [`MAKE_FRESH`] makes: the first user text of
/// the session its fire creates.
const FRESH_MARKER: &str = "FRESH-SESSION-NOTE-6R";
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
/// `id=<schedule id>` calls that tool for that id (an update renames it);
/// `with=trigger` and `with=target` make the update one of that part, and
/// `with=full` asks for the full view.
const CALL_SCHEDULE_TOOL: &str = "CALL-A-SCHEDULE-TOOL-6R";
/// The prompt of a target an update writes.
const REAIMED: &str = "REAIMED-6R-by-an-update";
/// A turn whose prompt holds this word and `child=<name>` calls the member
/// tool `spawn_member` for a member of that name.
const SPAWN_A_CHILD: &str = "SPAWN-A-CHILD-6R";
/// A turn whose prompt holds this word and `mob=<id>` calls `mob_create`
/// for a mob of that id with one turn-driven `worker` profile.
const CREATE_A_MOB: &str = "CREATE-A-MOB-6R";
/// The call id of a scripted `mob_create`.
const CREATE_MOB_CALL: &str = "create-a-mob";
/// A turn whose prompt holds this word, `mob=<id>` and `child=<name>` calls
/// `mob_spawn_member` for a `worker` member of that name in that mob.
const SPAWN_INTO_A_MOB: &str = "SPAWN-INTO-A-MOB-6R";
/// The call id of a scripted `mob_spawn_member` into a named mob.
const SPAWN_INTO_MOB_CALL: &str = "spawn-into-a-mob";
const MOB_CREATE: &str = "mob_create";
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
    /// The names of the tools the request offers, sorted.
    tools: Vec<String>,
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
        let mut tools: Vec<String> = request
            .tools
            .iter()
            .map(|tool| tool.name.to_string())
            .collect();
        tools.sort();
        self.seen.0.lock().unwrap().push(SeenRequest {
            last_user_text: last.clone(),
            all_messages: format!("{:?}", request.messages),
            tool_results: results.join(" | "),
            tools,
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
            let schedule_id = word(&last, "id=");
            let args = match word(&last, "with=").as_str() {
                // A trigger that is due at once and every second after.
                "trigger" => json!({
                    "schedule_id": schedule_id,
                    "trigger": {
                        "type": "interval",
                        "start_at_utc": chrono::Utc::now().to_rfc3339(),
                        "every_seconds": 1
                    }
                }),
                // A target that names the calling member's own session.
                "target" => json!({
                    "schedule_id": schedule_id,
                    "target": {
                        "target_kind": "session",
                        "type": "current_session",
                        "action": { "type": "prompt", "prompt": REAIMED }
                    }
                }),
                "full" => json!({ "schedule_id": schedule_id, "view": "full" }),
                _ if tool == "meerkat_schedule_update" => {
                    json!({ "schedule_id": schedule_id, "name": "renamed" })
                }
                _ => json!({ "schedule_id": schedule_id }),
            };
            call(READ_CALL, &tool, args)
        } else if first && last.contains(CREATE_A_MOB) && holds(MOB_CREATE) {
            call(
                CREATE_MOB_CALL,
                MOB_CREATE,
                json!({ "definition": {
                    "id": word(&last, "mob="),
                    "profiles": { "worker": {
                        "model": WORKER_MODEL,
                        "runtime_mode": "turn_driven",
                        "tools": { "comms": true }
                    } }
                } }),
            )
        } else if first && last.contains(SPAWN_INTO_A_MOB) && holds(MOB_SPAWN_MEMBER) {
            let mut args = json!({
                "mob_id": word(&last, "mob="),
                "profile": "worker",
                "member_id": word(&last, "child=")
            });
            // `tier=` gives the spawn one label of its own.
            if last.contains("tier=") {
                args["labels"] = json!({ "tier": word(&last, "tier=") });
            }
            call(SPAWN_INTO_MOB_CALL, MOB_SPAWN_MEMBER, args)
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
        } else if first && last.contains(MAKE_FRESH) && holds(SCHEDULE_CREATE) {
            call(
                "make-fresh-schedule",
                SCHEDULE_CREATE,
                json!({
                    "name": "a fresh session",
                    "trigger": {
                        "type": "interval",
                        "start_at_utc": chrono::Utc::now().to_rfc3339(),
                        "every_seconds": 2
                    },
                    "target": {
                        "target_kind": "session",
                        "type": "materialize_on_demand_session",
                        "create": { "model": PLANNER_MODEL },
                        "action": { "type": "prompt", "prompt": FRESH_MARKER }
                    },
                    "missing_target_policy": "mark_misfired",
                    "planning_horizon_occurrences": 1
                }),
            )
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
    boot_on(label, false).await
}

/// [`boot`]; with `stored_mob` the mob's journal is kept in a SQLite store,
/// so that a read of it answers what the store holds.
async fn boot_on(label: &str, stored_mob: bool) -> Launch {
    boot_as(label, stored_mob, None).await
}

/// [`boot_on`]; with `gateway` the agent factory has the switches the
/// gateway binary gives its own (`bin/meta_gateway.rs`: built-in tools,
/// shell, mob, comms and memory on, the schedule tools not set), and the
/// scripted model is the builder's default, so that a session a schedule's
/// fire creates, which no mob builds, runs on it too. Its value is the
/// host's configuration key `[tools] agent_schedules_may_create_sessions`:
/// `false` is the stock configuration.
async fn boot_as(label: &str, stored_mob: bool, gateway: Option<bool>) -> Launch {
    let gateway_switches = gateway.is_some();
    use crate::mob_composition_manifest::CompositionAuthority;

    let authority = CompositionAuthority::Authoritative;
    let stores = tempfile::tempdir().expect("stores dir");
    // The store files are where a launch over this state directory keeps
    // them.
    let layout = crate::storage_layout::MetaStorageLayout::with_injected_roots(
        stores.path().to_path_buf(),
        None,
    );
    let mob_storage = if stored_mob {
        meerkat_mob::MobStorage::persistent(layout.event_log_db()).expect("mob store")
    } else {
        meerkat_mob::MobStorage::in_memory()
    };
    let schedule_state = tempfile::tempdir().expect("schedule state dir");
    let session_store: Arc<dyn meerkat::SessionStore> = Arc::new(
        meerkat_store::SqliteSessionStore::open(
            layout.session_db().expect("the sessions database").path,
        )
        .expect("session store"),
    );
    let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> = Arc::new(
        meerkat_runtime::store::SqliteRuntimeStore::new(layout.runtime_db())
            .expect("runtime store"),
    );

    let blob_store: Arc<dyn meerkat_core::BlobStore> =
        Arc::new(meerkat_store::MemoryBlobStore::new());
    let seen = Arc::new(Seen::default());
    let factory = if gateway_switches {
        meerkat::AgentFactory::new(stores.path())
            .session_store(session_store.clone())
            .runtime_root(stores.path())
            .project_root(stores.path())
            .builtins(true)
            .shell(true)
            .mob(true)
            .comms(true)
            .memory(true)
    } else {
        meerkat::AgentFactory::new(stores.path()).comms(true)
    };
    let mut config = meerkat::Config::default();
    config.tools.agent_schedules_may_create_sessions = gateway.unwrap_or(false);
    let mut builder = meerkat::FactoryAgentBuilder::new(factory, config);
    if gateway_switches {
        builder.default_llm_client = Some(Arc::new(ScriptedClient {
            seen: Arc::clone(&seen),
        }));
    }
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

[profiles.keeper]
model = "{PLANNER_MODEL}"
runtime_mode = "turn_driven"
external_addressable = true
durable = true

[profiles.keeper.tools]
comms = true
schedule = true
mob = true

[profiles.worker]
model = "{WORKER_MODEL}"
runtime_mode = "turn_driven"
external_addressable = true

[profiles.worker.tools]
comms = true

[profiles.clerk]
model = "{PLANNER_MODEL}"
runtime_mode = "turn_driven"
external_addressable = true

[profiles.clerk.tools]
comms = true
schedule = true
"#
    ))
    .expect("definition");
    let spec =
        crate::mob_handle_runtime::MobBootstrapSpec::new(definition, mob_storage, sessions.clone())
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
    /// Whether its labels carry a side chat's marker.
    side_chat_label: bool,
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
                side_chat_label: entry
                    .labels
                    .contains_key(crate::side_chat::SIDE_CHAT_OF_LABEL),
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

/// OBSERVATION of what a session created by a schedule's fire is offered, on
/// a launch whose agent factory has the gateway binary's switches and the
/// stock configuration. A member whose profile switches on comms (which
/// every mob member needs) and the schedule tools, and nothing else, writes
/// a schedule whose target is a session created on demand: refused, nothing
/// stored. The host's own schedule of the same form fires as before, and the
/// session the fire creates is offered its own tool list, which the row
/// prints beside the member's: the shell and the file tools the member is
/// not offered, and no schedule tool. The row makes no tool call but the
/// member's refused write.
#[tokio::test(flavor = "multi_thread")]
async fn a_fired_on_demand_sessions_tool_list_is_the_hosts_not_the_writers() {
    let launch = boot_as("fresh", false, Some(false)).await;
    launch.shared("clerk", "clerk").await;
    let before = launch.seen.all().len();
    launch.turn("clerk", &format!("{MAKE_FRESH} please")).await;
    let writer = launch
        .seen
        .all()
        .into_iter()
        .skip(before)
        .find(|request| request.last_user_text.contains(MAKE_FRESH))
        .expect("the member's model request");
    let refusal = launch
        .seen
        .all()
        .into_iter()
        .skip(before)
        .map(|request| request.tool_results)
        .find(|results| results.contains("make-fresh-schedule"))
        .unwrap_or_default();
    launch.expect(
        refusal.contains("schedule_session_creation_not_allowed")
            || refusal.contains("cannot create a session"),
        format!("the member's write is refused as a schedule that creates a session: {refusal}"),
    );
    launch.expect(
        launch.schedule_ids().await.is_empty(),
        "nothing is stored for the member's write",
    );

    // The host's own schedule of that form: stored, and it fires as before.
    let hosts: meerkat::CreateScheduleRequest = serde_json::from_value(json!({
        "name": "the host's fresh session",
        "trigger": {
            "type": "interval",
            "start_at_utc": chrono::Utc::now().to_rfc3339(),
            "every_seconds": 2
        },
        "target": {
            "target_kind": "session",
            "type": "materialize_on_demand_session",
            "create": { "model": PLANNER_MODEL },
            "action": { "type": "prompt", "prompt": FRESH_MARKER }
        },
        "missing_target_policy": "mark_misfired",
        "planning_horizon_occurrences": 1
    }))
    .expect("the host's schedule request");
    let hosts = launch
        .attached
        .service
        .create(hosts)
        .await
        .expect("the host's own schedule is stored");
    // The session the schedule's target names: none until a fire creates
    // one and binds it.
    let bound = |schedule: &meerkat::Schedule| {
        serde_json::to_value(&schedule.target)
            .ok()
            .and_then(|target| {
                target
                    .get("bound_session_id")
                    .and_then(|id| id.as_str().map(str::to_owned))
            })
    };
    launch.expect(
        bound(&hosts).is_none(),
        "the schedule names no session before its fire",
    );
    let writers_session = launch
        .entry("clerk")
        .await
        .and_then(|seat| seat.session)
        .map(|id| id.to_string());
    let mut fired = None;
    for _ in 0..120 {
        fired = launch
            .seen
            .all()
            .into_iter()
            .skip(before)
            .find(|request| request.last_user_text.contains(FRESH_MARKER));
        if fired.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    let mut fired_session = None;
    for _ in 0..40 {
        fired_session = launch
            .attached
            .service
            .get(&hosts.schedule_id)
            .await
            .ok()
            .as_ref()
            .and_then(bound);
        if fired_session.is_some() || fired.is_none() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    let occurrences = launch.occurrences(&hosts.schedule_id.to_string()).await;
    println!(
        "OBSERVED version={} writer_profile_tools=[comms, schedule] config=Config::default()",
        env!("CARGO_PKG_VERSION")
    );
    println!("OBSERVED writer tools: {}", writer.tools.join(", "));
    match &fired {
        Some(fired) => {
            println!("OBSERVED fired session tools: {}", fired.tools.join(", "));
            let beyond: Vec<&str> = fired
                .tools
                .iter()
                .filter(|tool| !writer.tools.contains(tool))
                .map(String::as_str)
                .collect();
            println!(
                "OBSERVED fired session tools the writer does not hold: {}",
                beyond.join(", ")
            );
            println!(
                "OBSERVED fired session: {fired_session:?}, named by the schedule only after \
                 its fire; the writer's session: {writers_session:?}"
            );
        }
        None => println!("OBSERVED no fire within 30 s; occurrences: {occurrences:?}"),
    }
    launch.expect(
        fired.is_none() || fired_session.is_some(),
        "the fire binds the session it created to the schedule",
    );
    launch.expect(
        fired_session.is_none() || fired_session != writers_session,
        format!("the fired session is not the writer's: {fired_session:?} and {writers_session:?}"),
    );
    launch.expect(
        writer.tools.iter().any(|tool| tool == SCHEDULE_CREATE),
        format!("the member holds the schedule tools: {:?}", writer.tools),
    );
    launch.expect(
        fired.is_some(),
        format!("the host's schedule fires into a session created on demand: {occurrences:?}"),
    );
    if let Some(fired) = &fired {
        let holds = |tools: &[String], name: &str| tools.iter().any(|tool| tool == name);
        launch.expect(
            holds(&fired.tools, "shell") && holds(&fired.tools, "apply_patch"),
            format!(
                "the fired session is offered the shell and the file tools: {:?}",
                fired.tools
            ),
        );
        launch.expect(
            !holds(&writer.tools, "shell") && !holds(&writer.tools, "apply_patch"),
            format!(
                "the member is offered neither the shell nor the file tools: {:?}",
                writer.tools
            ),
        );
        launch.expect(
            !fired
                .tools
                .iter()
                .any(|tool| tool.starts_with("meerkat_schedule_")),
            format!(
                "the fired session is offered no schedule tool on this launch: {:?}",
                fired.tools
            ),
        );
    }
    launch.finish().await;
}

/// The switches [`boot_as`] gives its agent factory are the gateway
/// binary's, read from its source: `bin/meta_gateway.rs` builds its agent
/// factory twice (the persistent launch and the process-local one), each
/// with the built-in tools, the shell, mob, comms and memory on and the
/// schedule switch not set. A change of either construction fails here, so
/// the observation above is not kept for a binary it no longer describes.
#[test]
fn the_observed_factory_switches_are_the_gateway_binarys() {
    let source: String = include_str!("bin/meta_gateway.rs")
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    let switches = ".builtins(true).shell(true).mob(true).comms(true).memory(true)";
    assert_eq!(
        source.matches("letmutfactory=AgentFactory::new(").count(),
        2,
        "the gateway binary builds its agent factory twice"
    );
    assert_eq!(
        source.matches(switches).count(),
        2,
        "each construction sets exactly these switches: {switches}"
    );
    for switch in [".builtins(", ".shell(", ".mob(", ".comms(", ".memory("] {
        assert_eq!(
            source.matches(switch).count(),
            2,
            "{switch} is set nowhere else in the gateway binary"
        );
    }
    assert_eq!(
        source.matches(".schedule(").count(),
        0,
        "the gateway binary leaves the factory's schedule switch as it is"
    );
    let this = include_str!("console_private_member_schedule_end_tests.rs")
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>();
    assert!(
        this.contains(&format!(
            "meerkat::AgentFactory::new(stores.path()).session_store(session_store.clone())\
             .runtime_root(stores.path()).project_root(stores.path()){switches}}}else"
        )),
        "the row's launch sets the same switches and no other"
    );
}

/// On a launch whose configuration allows it by name
/// (`[tools] agent_schedules_may_create_sessions = true`) the same member's
/// schedule that creates a session is taken, and it fires.
#[tokio::test(flavor = "multi_thread")]
async fn a_launch_that_allows_it_takes_a_members_schedule_that_creates_a_session() {
    let launch = boot_as("fresh-allowed", false, Some(true)).await;
    launch.shared("clerk", "clerk").await;
    let before = launch.seen.all().len();
    launch.turn("clerk", &format!("{MAKE_FRESH} please")).await;
    let schedules = launch.schedule_ids().await;
    launch.expect(
        schedules.len() == 1,
        format!("the member's schedule is stored: {schedules:?}"),
    );
    let mut fired = false;
    for _ in 0..120 {
        fired = launch
            .seen
            .all()
            .into_iter()
            .skip(before)
            .any(|request| request.last_user_text.contains(FRESH_MARKER));
        if fired {
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    launch.expect(fired, "the member's schedule fires");
    launch.finish().await;
}

/// The console with sign-in over `launch`, and the first person's side chat
/// of the shared agent `alpha` under [`SIDE_NAME`], private to her.
async fn private_side_chat(launch: &Launch) -> (Router, String) {
    private_side_chat_of(launch, "planner", "alpha", SIDE_NAME).await
}

/// The console with sign-in over `launch`, and the first person's side chat
/// `name` of a shared agent `agent` of profile `profile`, private to her.
async fn private_side_chat_of(
    launch: &Launch,
    profile: &str,
    agent: &str,
    name: &str,
) -> (Router, String) {
    launch.shared(profile, agent).await;
    let app = console_json_router_with_state(enforced_console(launch.mob(), None));
    let owner = bearer(alice_claims());
    let forked = fork_on_console(&app, Some(&owner), agent, name).await;
    assert!(forked["error"].is_null(), "{forked:#}");
    let side = launch.entry(name).await.expect("the side chat is seated");
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
            let ended_with_it = launch.gone_within(child, 5).await;
            println!("ROW: the fork child ended with its side chat: {ended_with_it}");
            if !ended_with_it {
                // The child is a member of its own: its schedule is there
                // while it is, and ends with it.
                launch.expect(
                    launch.phase(&schedule).await == Some(meerkat::SchedulePhase::Active),
                    format!(
                        "the fork child's schedule stays while the child is seated (it is {:?})",
                        launch.phase(&schedule).await
                    ),
                );
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

/// What another agent's schedule tools answer about a deleted schedule when
/// they name it by its id: each answer is printed. A pause and a resume are
/// refused (the schedule lifecycle has neither for a deleted schedule), and
/// after every call the schedule is deleted still.
async fn by_id_answers(launch: &Launch, agent: &str, schedule: &str) {
    for (tool, with) in [
        (SCHEDULE_GET, ""),
        ("meerkat_schedule_occurrences", "full"),
        ("meerkat_schedule_update", "name"),
        ("meerkat_schedule_update", "trigger"),
        ("meerkat_schedule_update", "target"),
        ("meerkat_schedule_pause", ""),
        ("meerkat_schedule_resume", ""),
        ("meerkat_schedule_delete", ""),
    ] {
        let answer = launch
            .schedule_read(
                agent,
                &format!("{CALL_SCHEDULE_TOOL} tool={tool} with={with} id={schedule} please"),
            )
            .await;
        println!(
            "ROW: {tool} {with} by the id answers: {}",
            answer.chars().take(500).collect::<String>()
        );
        if tool.ends_with("pause") || tool.ends_with("resume") {
            launch.expect(
                answer.contains("is_error: true"),
                format!("{tool} of a deleted schedule is refused: {answer}"),
            );
        }
        launch.expect(
            launch.phase(schedule).await == Some(meerkat::SchedulePhase::Deleted),
            format!(
                "after {tool} {with} by the id the schedule is deleted still (it is {:?})",
                launch.phase(schedule).await
            ),
        );
    }
}

/// A deleted schedule is on no agent's list, in either state of its
/// member's name (no member holds it; a shared member holds it). No call
/// that names it by its id plans an occurrence or delivers anything: an
/// update that asks for a trigger due at once, or for a target that names
/// the caller, leaves it deleted. What each call answers is printed.
#[tokio::test(flavor = "multi_thread")]
async fn a_deleted_schedule_is_on_no_agents_list_and_no_call_by_its_id_makes_it_fire() {
    let launch = boot("byid").await;
    let (app, owner) = private_side_chat(&launch).await;
    launch.shared("planner", "beta").await;
    let schedule = launch.make_schedule(SIDE_NAME, 2).await;

    // While the side chat lives its own list holds the schedule, and
    // another agent's holds nothing of it.
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
            "the ended private side chat's schedule is deleted (it is {:?})",
            launch.phase(&schedule).await
        ),
    );
    let since = launch.seen.all().len();
    let before = launch.occurrences(&schedule).await;
    println!("ROW: the deleted schedule's occurrences before the calls: {before:?}");

    // No member holds the name.
    println!("ROW: ===== no member holds the name =====");
    let listed = launch
        .schedule_read("beta", &format!("{LIST_SCHEDULES} please"))
        .await;
    launch.expect(
        !listed.contains(&schedule) && !listed.contains(MARKER),
        "no member holds the name: another agent's list holds nothing of the deleted schedule",
    );
    by_id_answers(&launch, "beta", &schedule).await;

    // A shared member holds the name.
    println!("ROW: ===== a shared member holds the name =====");
    launch
        .mob()
        .handle()
        .spawn_spec(meerkat_mob::SpawnMemberSpec::new(
            "worker",
            AgentIdentity::from(SIDE_NAME),
        ))
        .await
        .expect("seat a shared member under the name");
    let listed = launch
        .schedule_read("beta", &format!("{LIST_SCHEDULES} please"))
        .await;
    launch.expect(
        !listed.contains(&schedule) && !listed.contains(MARKER),
        "a shared member holds the name: another agent's list holds nothing of the deleted \
         schedule",
    );
    by_id_answers(&launch, "beta", &schedule).await;

    // Whatever the calls answered: nothing was planned, and nothing is
    // delivered to the member that holds the name, to the caller or to
    // anyone else.
    tokio::time::sleep(Duration::from_secs(6)).await;
    let occurrences = launch.occurrences(&schedule).await;
    println!("ROW: the deleted schedule's occurrences at the end: {occurrences:?}");
    let ordinal = |occurrence: &String| {
        occurrence
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .to_string()
    };
    let known: Vec<String> = before.iter().map(ordinal).collect();
    launch.expect(
        occurrences
            .iter()
            .all(|occurrence| known.contains(&ordinal(occurrence))),
        format!(
            "no occurrence of the deleted schedule is planned by a call by its id (there \
             were {before:?}): {occurrences:?}"
        ),
    );
    let completed = |occurrences: &[String]| {
        occurrences
            .iter()
            .filter(|occurrence| occurrence.contains("Completed"))
            .count()
    };
    launch.expect(
        completed(&occurrences) == completed(&before)
            && !occurrences.iter().any(|occurrence| {
                ["Claimed", "Dispatching", "AwaitingCompletion"]
                    .iter()
                    .any(|underway| occurrence.contains(underway))
            }),
        format!(
            "no occurrence of the deleted schedule is delivered or under way (there were \
             {before:?}): {occurrences:?}"
        ),
    );
    let delivered: Vec<String> = launch
        .seen
        .all()
        .into_iter()
        .skip(since)
        .filter(|request| {
            request.last_user_text.contains(MARKER) || request.last_user_text.contains(REAIMED)
        })
        .map(|request| request.last_user_text)
        .collect();
    launch.expect(
        delivered.is_empty(),
        format!("nothing of the deleted schedule is delivered to anyone: {delivered:?}"),
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
    launch.expect(
        !ended_with_it,
        "the member a private side chat spawned is a member of its own: it stays seated when \
         the side chat is closed",
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

/// A follower that keeps the ends it is handed and hands each on to the
/// launch's schedules, as the gateway's own follower does.
struct KeptAndFollowed {
    kept: Arc<KeptEnds>,
    schedules: crate::schedule_wiring::EndedPrivateMemberSchedules,
}

#[async_trait::async_trait]
impl crate::private_owners::PrivateMemberEndFollower for KeptAndFollowed {
    async fn member_ended(&self, ended: &crate::private_owners::EndedPrivateMember) {
        self.kept.member_ended(ended).await;
        self.schedules.member_ended(ended).await;
    }
}

/// Install a [`KeptAndFollowed`] over the launch's schedules as the
/// follower of its private members' ends: the ends it will have kept.
async fn keep_and_follow(launch: &Launch) -> Arc<KeptEnds> {
    let kept = Arc::new(KeptEnds::default());
    launch
        .mob()
        .private_owners()
        .follow_member_ends(Arc::new(KeptAndFollowed {
            kept: kept.clone(),
            schedules: crate::schedule_wiring::EndedPrivateMemberSchedules::new(
                launch.attached.service.clone(),
            ),
        }))
        .await;
    kept
}

/// The retirement starts of `name` that the mob's journal holds, oldest
/// first, each printed as it was read.
async fn retirement_starts(launch: &Launch, name: &str) -> Vec<meerkat_mob::MobEvent> {
    let starts: Vec<meerkat_mob::MobEvent> = journal(launch)
        .await
        .into_iter()
        .filter(|event| {
            matches!(
                &event.kind,
                meerkat_mob::MobEventKind::MemberRetirementStarted { agent_identity, .. }
                    if agent_identity.as_str() == name
            )
        })
        .collect();
    for event in &starts {
        println!(
            "ROW: read from the journal: cursor {} at {}: {:?}",
            event.cursor, event.timestamp, event.kind
        );
    }
    starts
}

/// The mob's journal is kept in a SQLite store. A private side chat ends;
/// a later member under its name makes a schedule of its own; then the
/// ended side chat's retirement start is read from the store and shown to
/// the record again, as a pump shows it that starts again before its cursor
/// moved past it. The end is handed to the follower again, and the later
/// member's schedule is not the ended member's: it stays and goes on
/// firing. The ended side chat's schedule is deleted.
#[tokio::test(flavor = "multi_thread")]
async fn an_end_shown_again_from_the_store_deletes_no_schedule_a_later_member_made() {
    let launch = boot_on("replay", true).await;
    let (app, owner) = private_side_chat(&launch).await;
    let ended = launch.make_schedule(SIDE_NAME, 2).await;
    close(&launch, &app, &owner, SIDE_NAME).await;
    launch.expect(
        launch.deleted_within(&ended, 10).await,
        format!(
            "the ended private side chat's schedule is deleted (it is {:?})",
            launch.phase(&ended).await
        ),
    );

    // A later member under the name, with a schedule of its own.
    launch.shared("planner", SIDE_NAME).await;
    let before = launch.seen.all().len();
    let later = launch.make_schedule(SIDE_NAME, 2).await;
    launch.expect(
        launch.fired_within(before, 15).await,
        "the later member's schedule fires into it",
    );

    let kept = keep_and_follow(&launch).await;
    let starts = retirement_starts(&launch, SIDE_NAME).await;
    launch.expect(
        starts.len() == 1,
        format!(
            "the store holds the ended side chat's one retirement start (it holds {})",
            starts.len()
        ),
    );
    for start in &starts {
        launch
            .mob()
            .private_owners()
            .observe_mob_event(&launch.mob().handle(), start)
            .await;
    }
    launch.expect(
        kept.members() == vec![SIDE_NAME.to_string()],
        format!(
            "the end read from the store is handed to the follower again (handed: {:?})",
            kept.members()
        ),
    );
    expect_kept(&launch, &later, "the end shown again").await;
    launch.expect(
        launch.phase(&ended).await == Some(meerkat::SchedulePhase::Deleted),
        format!(
            "the ended private side chat's schedule is deleted still (it is {:?})",
            launch.phase(&ended).await
        ),
    );
    launch.finish().await;
}

/// The mob's journal is kept in a SQLite store. An ephemeral member private
/// to a person is respawned and then rotated onto a fresh session (a
/// reset); each retires the incarnation before it. Read from the store,
/// neither retirement start is an end: shown to the record, none is handed
/// to the follower, and the member's schedule stays and goes on firing. A
/// plain retirement of the same member is an end: its start is handed
/// over, and the schedule is deleted.
#[tokio::test(flavor = "multi_thread")]
async fn a_respawns_and_a_resets_retirement_start_read_from_the_store_is_no_end() {
    let launch = boot_on("stored", true).await;
    let name = "notes";
    let schedule = private_member_with_a_schedule(&launch, name, false).await;
    let handle = launch.mob().handle();
    let respawned = handle.respawn(AgentIdentity::from(name), None).await;
    launch.expect(
        respawned.is_ok(),
        format!("the member is respawned: {:?}", respawned.as_ref().err()),
    );
    let rotated = handle
        .rotate_member(private_member_spec("planner", name, false))
        .await;
    launch.expect(
        rotated.is_ok(),
        format!(
            "the member is rotated onto a fresh session: {:?}",
            rotated.as_ref().err()
        ),
    );

    let kept = keep_and_follow(&launch).await;
    let rebuilds = retirement_starts(&launch, name).await;
    launch.expect(
        rebuilds.len() >= 2,
        format!(
            "the store holds a retirement start of the respawn and of the reset (it holds {})",
            rebuilds.len()
        ),
    );
    for start in &rebuilds {
        launch
            .mob()
            .private_owners()
            .observe_mob_event(&handle, start)
            .await;
    }
    launch.expect(
        kept.members().is_empty(),
        format!(
            "no retirement start of a respawn or a reset is handed to the follower (handed: \
             {:?})",
            kept.members()
        ),
    );
    expect_kept(
        &launch,
        &schedule,
        "a respawn and a reset read from the store",
    )
    .await;

    handle
        .retire(AgentIdentity::from(name))
        .await
        .expect("retire the private member");
    launch.expect(
        launch.gone_within(name, 10).await,
        "the roster no longer holds the retired member",
    );
    let starts = retirement_starts(&launch, name).await;
    launch.expect(
        starts.len() == rebuilds.len() + 1,
        format!(
            "the store holds the plain retirement's start too (it holds {})",
            starts.len()
        ),
    );
    if let Some(plain) = starts.last() {
        launch
            .mob()
            .private_owners()
            .observe_mob_event(&handle, plain)
            .await;
    }
    launch.expect(
        kept.members().iter().any(|member| member == name),
        format!(
            "the plain retirement's start is handed to the follower (handed: {:?})",
            kept.members()
        ),
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

/// A shared member is seated under a private side chat's name as soon as
/// the roster stops holding the side chat, before anything else is waited
/// for. The ended side chat's schedule is deleted, and nothing of it
/// reaches the member.
#[tokio::test(flavor = "multi_thread")]
async fn a_member_seated_under_the_name_at_once_gets_no_schedule_of_the_ended_private_side_chat() {
    let launch = boot("atonce").await;
    let (app, owner) = private_side_chat(&launch).await;
    let schedule = launch.make_schedule(SIDE_NAME, 4).await;
    close(&launch, &app, &owner, SIDE_NAME).await;
    let when_the_name_was_free = launch.phase(&schedule).await;
    launch
        .mob()
        .handle()
        .spawn_spec(meerkat_mob::SpawnMemberSpec::new(
            "worker",
            AgentIdentity::from(SIDE_NAME),
        ))
        .await
        .expect("seat a shared member under the name");
    let when_the_member_was_seated = launch.phase(&schedule).await;
    println!(
        "ROW: the ended side chat's schedule was {when_the_name_was_free:?} when the roster \
         stopped holding the side chat, and {when_the_member_was_seated:?} when a member was \
         seated under the name"
    );
    launch.expect(
        when_the_member_was_seated == Some(meerkat::SchedulePhase::Deleted),
        format!(
            "the ended side chat's schedule is deleted by the time a member is seated under \
             the name (it was {when_the_member_was_seated:?})"
        ),
    );
    let since = launch.seen.all().len();
    launch.expect(
        launch.deleted_within(&schedule, 10).await,
        format!(
            "the ended private side chat's schedule is deleted (it is {:?})",
            launch.phase(&schedule).await
        ),
    );
    let seat = launch.entry(SIDE_NAME).await;
    launch.expect(
        seat.as_ref().is_some_and(|seat| seat.private_to.is_none()),
        "the member seated under the name is private to no one",
    );
    // Two intervals of the schedule.
    tokio::time::sleep(Duration::from_secs(9)).await;
    launch.expect(
        !launch.marker_since(since),
        "no model request made once the member was seated holds the ended side chat's \
         schedule prompt",
    );
    if let Some(session) = seat.and_then(|seat| seat.session) {
        launch.expect(
            !launch.history_holds_marker(&session).await,
            "the member's transcript does not hold the ended side chat's schedule prompt",
        );
    }
    launch.finish().await;
}

/// A side chat of an agent whose profile is durable is a side chat still:
/// private to its person, it ends when she closes it, and its schedule ends
/// with it. What becomes of a child it forked is printed.
#[tokio::test(flavor = "multi_thread")]
async fn a_private_side_chat_of_a_durable_profiles_agent_loses_its_schedule_when_closed() {
    let launch = boot("durable").await;
    let side = "omega-side";
    let (app, owner) = private_side_chat_of(&launch, "keeper", "omega", side).await;
    launch.expect(
        launch
            .entry(side)
            .await
            .is_some_and(|seat| seat.side_chat_label),
        "the side chat carries a side chat's label",
    );
    let before = launch.seen.all().len();
    let schedule = launch.make_schedule(side, 2).await;
    launch.expect(
        launch.fired_within(before, 15).await,
        "the schedule fires into the private side chat while it lives",
    );

    // A child the side chat forks, with a schedule of its own.
    let child = "report-helper";
    let stored = launch.schedule_ids().await;
    launch
        .turn(
            side,
            &format!("{FORK_A_CHILD} child={child} make=yes idle=no please"),
        )
        .await;
    let childs = launch.new_schedule(&stored, 20).await;
    let child_seat = launch.entry(child).await;
    println!(
        "ROW: the fork child of a durable profile's side chat: seated {}, private {}, side \
         chat label {}",
        child_seat.is_some(),
        child_seat
            .as_ref()
            .is_some_and(|seat| seat.private_to.is_some()),
        child_seat.as_ref().is_some_and(|seat| seat.side_chat_label)
    );

    close(&launch, &app, &owner, side).await;
    launch.expect(
        launch.deleted_within(&schedule, 10).await,
        format!(
            "the closed side chat's schedule is deleted (it is {:?})",
            launch.phase(&schedule).await
        ),
    );
    let ended_with_it = launch.gone_within(child, 5).await;
    if !ended_with_it {
        let closed = console_rpc_as(
            &app,
            Some(&owner),
            "meta/retire_member",
            json!({ "member_id": child }),
        )
        .await;
        println!("ROW: the owner's close of that fork child answers: {closed}");
        let gone = launch.gone_within(child, 10).await;
        println!("ROW: the roster stopped holding that fork child: {gone}");
    }
    tokio::time::sleep(Duration::from_secs(3)).await;
    println!(
        "ROW: the fork child of a durable profile's side chat ended with it: {ended_with_it}; \
         its schedule three seconds after its own end is {:?}",
        launch.phase(&childs).await
    );
    launch.finish().await;
}

/// The event cursor `cursors` holds for the mob, when one was written.
async fn stored_cursor(
    cursors: &Arc<dyn crate::PersistentMetadataStore>,
    mob_id: &str,
) -> Option<u64> {
    cursors
        .get_subscription_cursor(mob_id)
        .await
        .expect("read the cursor")
}

/// A launch that does not speak for its composition is shown the end of a
/// private side chat by its event pump: no follower is installed there, so
/// the end is only held, and the pump writes its event cursor no further.
/// The pump of the launch that speaks, started on the same stored cursor,
/// is shown that event, and the side chat's schedule is deleted.
#[tokio::test(flavor = "multi_thread")]
async fn an_end_a_launch_only_held_is_shown_to_the_launch_that_speaks_for_the_composition() {
    use crate::PersistentMetadataStore;
    use crate::unified_runtime::mob_events::MobEventsStore;
    use crate::unified_runtime::run_mob_events_subscription;

    let launch = boot("candidate").await;
    let (app, owner) = private_side_chat(&launch).await;
    let schedule = launch.make_schedule(SIDE_NAME, 4).await;
    // This launch's own follower only keeps the ends it is handed, so that
    // what deletes the schedule below is the second pump's follower.
    launch
        .mob()
        .private_owners()
        .follow_member_ends(Arc::new(KeptEnds::default()))
        .await;

    // The pump of a launch that does not speak for its composition, with a
    // cursor store of its own and a record that has no follower.
    let cursors: Arc<dyn PersistentMetadataStore> = Arc::new(crate::InMemoryMetadataStore::new());
    let holds_only = tokio::spawn(run_mob_events_subscription(
        launch.mob().handle(),
        MobEventsStore::new(),
        Arc::clone(&cursors),
        None,
        crate::private_owners::PrivateOwners::default(),
        false,
    ));
    // An event that is no end: the pump writes its cursor for it.
    tokio::time::sleep(Duration::from_millis(500)).await;
    launch.shared("worker", "beta").await;
    let mut before_the_end = None;
    for _ in 0..50 {
        before_the_end = stored_cursor(&cursors, &launch.mob_id).await;
        if before_the_end.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    launch.expect(
        before_the_end.is_some(),
        "the pump writes its cursor for an event that is no end",
    );

    close(&launch, &app, &owner, SIDE_NAME).await;
    let starts = retirement_starts(&launch, SIDE_NAME).await;
    let Some(start) = starts.last() else {
        launch.expect(false, "the journal holds the side chat's retirement start");
        holds_only.abort();
        launch.finish().await;
        return;
    };
    tokio::time::sleep(Duration::from_secs(2)).await;
    let held_at = stored_cursor(&cursors, &launch.mob_id).await;
    println!(
        "ROW: the end's event is at cursor {}; the pump that only held it left its cursor at \
         {held_at:?} (it was {before_the_end:?} before the end)",
        start.cursor
    );
    launch.expect(
        held_at.is_some_and(|cursor| cursor < start.cursor),
        format!(
            "the pump that only held the end left its cursor before the end's event (cursor \
             {held_at:?}, the end's event {})",
            start.cursor
        ),
    );
    launch.expect(
        launch.phase(&schedule).await == Some(meerkat::SchedulePhase::Active),
        format!(
            "no follower was shown the end yet, and the schedule is there (it is {:?})",
            launch.phase(&schedule).await
        ),
    );
    holds_only.abort();
    let _ = holds_only.await;

    // The pump of the launch that speaks, on the same stored cursor: its
    // record has the follower the gateway installs.
    let speaking = crate::private_owners::PrivateOwners::default();
    speaking
        .follow_member_ends(Arc::new(
            crate::schedule_wiring::EndedPrivateMemberSchedules::new(
                launch.attached.service.clone(),
            ),
        ))
        .await;
    let speaks = tokio::spawn(run_mob_events_subscription(
        launch.mob().handle(),
        MobEventsStore::new(),
        Arc::clone(&cursors),
        None,
        speaking,
        true,
    ));
    launch.expect(
        launch.deleted_within(&schedule, 10).await,
        format!(
            "the launch that speaks is shown the end, and the schedule is deleted (it is {:?})",
            launch.phase(&schedule).await
        ),
    );
    let mut moved_to = stored_cursor(&cursors, &launch.mob_id).await;
    for _ in 0..50 {
        if moved_to.is_some_and(|cursor| cursor >= start.cursor) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        moved_to = stored_cursor(&cursors, &launch.mob_id).await;
    }
    println!("ROW: the pump of the launch that speaks moved the cursor to {moved_to:?}");
    launch.expect(
        moved_to.is_some_and(|cursor| cursor >= start.cursor),
        format!(
            "the pump of the launch that speaks moves the cursor past the end's event (cursor \
             {moved_to:?}, the end's event {})",
            start.cursor
        ),
    );
    speaks.abort();
    let _ = speaks.await;
    launch.finish().await;
}

/// A member's name is its mob's own. An agent that seats a member in a mob
/// of its own under the name of a member of the launch's mob changes nothing
/// of what the Console holds of the launch's member: what its doors and the
/// access view answer a signed-in caller about that member is the same
/// before and after. Each row prints what it read (`ROW ...`).
mod spawned_member_names_across_mobs {
    use super::super::console_private_side_chat_tests::{
        ALICE, ROOT, bob_claims, row, send_on_console, sidebar,
    };
    use super::*;

    /// A shared agent whose turns make the mob tool calls.
    const CALLER: &str = "alpha";
    /// A shared member of the launch's mob that the host seated.
    const NAMED: &str = "beta";

    fn clip(text: &str) -> String {
        text.chars().take(600).collect()
    }

    /// The answer of the scripted call `call_id` made since request `since`,
    /// as its model saw it: it sees it only after the call returned.
    async fn answer_since(launch: &Launch, since: usize, call_id: &str) -> String {
        for _ in 0..100 {
            if let Some(request) = launch
                .seen
                .all()
                .into_iter()
                .skip(since)
                .find(|request| request.tool_results.contains(call_id))
            {
                return request.tool_results;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        String::new()
    }

    /// `agent`'s own turns create the mob `mob` and seat a member `name` in
    /// it with `mob_spawn_member`. Whether that mob then holds the member.
    async fn seats_in_its_own_mob(launch: &Launch, agent: &str, mob: &str, name: &str) -> bool {
        let since = launch.seen.all().len();
        launch
            .turn(agent, &format!("{CREATE_A_MOB} mob={mob} please"))
            .await;
        let created = answer_since(launch, since, CREATE_MOB_CALL).await;
        println!("ROW: {agent}'s mob_create answered {}", clip(&created));
        let since = launch.seen.all().len();
        launch
            .turn(
                agent,
                &format!("{SPAWN_INTO_A_MOB} mob={mob} child={name} please"),
            )
            .await;
        let spawned = answer_since(launch, since, SPAWN_INTO_MOB_CALL).await;
        println!(
            "ROW: {agent}'s mob_spawn_member answered {}",
            clip(&spawned)
        );
        let mut holds = false;
        if let Some(mob_tools) = launch.mob().agent_mob_mcp_state() {
            for (mob_id, handle) in Box::pin(mob_tools.mob_handles_snapshot())
                .await
                .unwrap_or_default()
            {
                if mob_id.as_str() == mob {
                    holds = handle
                        .get_member(&AgentIdentity::from(name))
                        .await
                        .expect("roster read")
                        .is_some();
                }
            }
        }
        println!("ROW: {agent}'s own mob {mob} holds a member {name}: {holds}");
        holds
    }

    /// What the Console's doors answer one caller about one agent.
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct Doors {
        /// The caller's sidebar lists the agent.
        listed: bool,
        /// `meta/get_member` answers the agent.
        read: bool,
        /// `POST /console/send` passed the send gate: this Console serves no
        /// aggregator, so a send its gate admits answers `unavailable`.
        send_admitted: bool,
        verbatim: String,
    }

    impl Doors {
        fn answers(&self) -> (bool, bool, bool) {
            (self.listed, self.read, self.send_admitted)
        }
    }

    async fn doors(app: &Router, bearer: &str, agent: &str) -> Doors {
        let listed = row(&sidebar(app, Some(bearer)).await, agent).is_some();
        let got = console_rpc_as(
            app,
            Some(bearer),
            "meta/get_member",
            json!({ "member_id": agent }),
        )
        .await;
        let read = got["error"].is_null();
        let (status, sent) = send_on_console(app, bearer, agent).await;
        let send_admitted = sent["error"] == json!("unavailable");
        let verbatim = format!(
            "sidebar={listed} get_member={} send={} {}",
            if read {
                "ok".to_string()
            } else {
                clip(&format!(
                    "{} {}",
                    got["error"]["code"], got["error"]["message"]
                ))
            },
            status.as_u16(),
            sent["error"]
        );
        Doors {
            listed,
            read,
            send_admitted,
            verbatim,
        }
    }

    /// The access view's own answers for `subject` on `agent` after a
    /// priming from the runtime: `agent.view`, `agent.send`, and the view
    /// decision.
    async fn gate(
        launch: &Launch,
        access: &AccessController,
        subject: &str,
        agent: &str,
    ) -> (bool, bool, String) {
        prime_access_cache_from_runtime(launch.mob(), access).await;
        let view = access.view_for_subject(Some(subject));
        (
            view.can_view_agent(agent),
            view.allows_agent(ACTION_AGENT_SEND, agent),
            format!("{:?}", view.decide_agent(ACTION_AGENT_VIEW, agent)),
        )
    }

    /// Rules under which the first person may view and send to `agent`
    /// only, plus `more`; `root` is the one admin.
    fn first_person_on(agent: &str, more: Vec<AccessRule>) -> AccessController {
        let mut rules = vec![AccessRule {
            id: "first-person-on-one-agent".to_string(),
            subjects: vec![ALICE.to_string()],
            actions: vec![ACTION_AGENT_VIEW.to_string(), ACTION_AGENT_SEND.to_string()],
            agents: vec![agent.to_string()],
            ..AccessRule::default()
        }];
        rules.extend(more);
        AccessController::new(crate::access::AccessControlConfig {
            enabled: true,
            admins: vec![ROOT.to_string()],
            rules,
            ..crate::access::AccessControlConfig::default()
        })
        .expect("access controller")
    }

    /// A deny of every agent action for the first person on `agent`.
    fn first_person_off(agent: &str) -> AccessRule {
        AccessRule {
            id: "first-person-off-one-agent".to_string(),
            effect: crate::access::AccessEffect::Deny,
            subjects: vec![ALICE.to_string()],
            actions: vec!["agent.*".to_string()],
            agents: vec![agent.to_string()],
            ..AccessRule::default()
        }
    }

    /// The rules allow the first person the calling agent only. The calling
    /// agent seats a member under the launch member's name in a mob of its
    /// own: the doors and the access view refuse her the launch's member
    /// before, and after.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_name_seated_in_an_agents_own_mob_leaves_the_launch_members_doors() {
        let launch = boot("own-mob").await;
        launch.shared("planner", CALLER).await;
        launch.shared("worker", NAMED).await;
        let access = first_person_on(CALLER, Vec::new());
        let app =
            console_json_router_with_state(enforced_console(launch.mob(), Some(access.clone())));
        let alice = bearer(alice_claims());

        let on_caller = doors(&app, &alice, CALLER).await;
        let before = doors(&app, &alice, NAMED).await;
        let gate_before = gate(&launch, &access, ALICE, NAMED).await;
        println!("ROW 1: on the calling agent: {}", on_caller.verbatim);
        println!("ROW 1: before, on {NAMED}: {}", before.verbatim);
        println!("ROW 1: before, the access view on {NAMED}: {gate_before:?}");
        launch.expect(
            on_caller.listed && on_caller.send_admitted,
            format!(
                "the first person is shown the calling agent and her send to it passes the gate \
                 ({})",
                on_caller.verbatim
            ),
        );
        launch.expect(
            before.answers() == (false, false, false) && !gate_before.0 && !gate_before.1,
            "before, the doors and the access view refuse the first person the launch's member",
        );

        let own_mob = format!("{}-own", launch.mob_id);
        let seated = seats_in_its_own_mob(&launch, CALLER, &own_mob, NAMED).await;
        launch.expect(
            seated,
            "the calling agent's own mob holds the member it seated there",
        );

        let after = doors(&app, &alice, NAMED).await;
        let gate_after = gate(&launch, &access, ALICE, NAMED).await;
        println!("ROW 1: after, on {NAMED}: {}", after.verbatim);
        println!("ROW 1: after, the access view on {NAMED}: {gate_after:?}");
        launch.expect(
            after.answers() == (false, false, false) && !gate_after.0 && !gate_after.1,
            format!(
                "after, the doors and the access view still refuse the first person the \
                 launch's member ({}; {gate_after:?})",
                after.verbatim
            ),
        );
        launch.finish().await;
    }

    /// The other order: the calling agent seats the name in a mob of its own
    /// first, and the host seats a member of that name in the launch's mob
    /// afterwards. The doors and the access view refuse the first person
    /// the launch's member.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_member_seated_after_its_name_was_used_in_an_agents_own_mob_keeps_its_doors() {
        const LATER: &str = "zeta";
        let launch = boot("later").await;
        launch.shared("planner", CALLER).await;
        let access = first_person_on(CALLER, Vec::new());
        let app =
            console_json_router_with_state(enforced_console(launch.mob(), Some(access.clone())));
        let alice = bearer(alice_claims());

        let own_mob = format!("{}-own", launch.mob_id);
        let seated = seats_in_its_own_mob(&launch, CALLER, &own_mob, LATER).await;
        launch.expect(
            seated,
            "the calling agent's own mob holds the member it seated there",
        );
        launch.shared("worker", LATER).await;

        let after = doors(&app, &alice, LATER).await;
        let gate_after = gate(&launch, &access, ALICE, LATER).await;
        println!(
            "ROW 2: on the later launch member {LATER}: {}",
            after.verbatim
        );
        println!("ROW 2: the access view on {LATER}: {gate_after:?}");
        launch.expect(
            after.answers() == (false, false, false) && !gate_after.0 && !gate_after.1,
            format!(
                "the doors and the access view refuse the first person the launch's member \
                 seated later ({}; {gate_after:?})",
                after.verbatim
            ),
        );
        launch.finish().await;
    }

    /// The calling agent is the first person's private side chat. It seats a
    /// member under the shared launch member's name in a mob of its own: the
    /// doors answer a second person the shared member before, and after.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_name_seated_in_a_private_members_own_mob_leaves_the_launch_member_shared() {
        let launch = boot("private").await;
        launch.shared("worker", NAMED).await;
        let (app, _alice) = private_side_chat(&launch).await;
        let bob = bearer(bob_claims());

        let before = doors(&app, &bob, NAMED).await;
        println!(
            "ROW 3: before, the second person on {NAMED}: {}",
            before.verbatim
        );
        launch.expect(
            before.answers() == (true, true, true),
            format!(
                "before, the doors answer the second person the shared launch member ({})",
                before.verbatim
            ),
        );

        let own_mob = format!("{}-side", launch.mob_id);
        let seated = seats_in_its_own_mob(&launch, SIDE_NAME, &own_mob, NAMED).await;
        launch.expect(
            seated,
            "the private side chat's own mob holds the member it seated there",
        );

        let after = doors(&app, &bob, NAMED).await;
        println!(
            "ROW 3: after, the second person on {NAMED}: {}",
            after.verbatim
        );
        launch.expect(
            after.answers() == (true, true, true),
            format!(
                "after, the doors still answer the second person the shared launch member ({})",
                after.verbatim
            ),
        );
        launch.finish().await;
    }

    /// A launch member that one agent spawned is refused to the first person
    /// by a deny on that agent. A second agent, which the rules allow her,
    /// seats a member of the same name in a mob of its own: the deny still
    /// refuses her the launch's member.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_deny_through_a_members_spawner_holds_after_its_name_is_seated_in_another_mob() {
        const SPAWNER: &str = "alpha";
        const OTHER: &str = "omega";
        const SPAWNED: &str = "gamma";
        let launch = boot("spawner").await;
        launch.shared("planner", SPAWNER).await;
        launch.shared("planner", OTHER).await;
        let access = first_person_on(OTHER, vec![first_person_off(SPAWNER)]);
        let app =
            console_json_router_with_state(enforced_console(launch.mob(), Some(access.clone())));
        let alice = bearer(alice_claims());

        // The spawner seats its member in the launch's mob.
        let since = launch.seen.all().len();
        launch
            .turn(
                SPAWNER,
                &format!(
                    "{SPAWN_INTO_A_MOB} mob={} child={SPAWNED} please",
                    launch.mob_id
                ),
            )
            .await;
        let spawned = answer_since(&launch, since, SPAWN_INTO_MOB_CALL).await;
        println!(
            "ROW 4: the spawner's mob_spawn_member answered {}",
            clip(&spawned)
        );
        launch.expect(
            launch.entry(SPAWNED).await.is_some(),
            "the launch's mob holds the member its agent spawned",
        );

        let before = doors(&app, &alice, SPAWNED).await;
        let gate_before = gate(&launch, &access, ALICE, SPAWNED).await;
        println!("ROW 4: before, on {SPAWNED}: {}", before.verbatim);
        println!("ROW 4: before, the access view on {SPAWNED}: {gate_before:?}");
        launch.expect(
            before.answers() == (false, false, false)
                && gate_before.2.contains("first-person-off-one-agent"),
            format!(
                "before, the deny on its spawner refuses the first person the spawned member \
                 ({}; {gate_before:?})",
                before.verbatim
            ),
        );

        let own_mob = format!("{}-own", launch.mob_id);
        let seated = seats_in_its_own_mob(&launch, OTHER, &own_mob, SPAWNED).await;
        launch.expect(
            seated,
            "the second agent's own mob holds the member it seated there",
        );

        let after = doors(&app, &alice, SPAWNED).await;
        let gate_after = gate(&launch, &access, ALICE, SPAWNED).await;
        println!("ROW 4: after, on {SPAWNED}: {}", after.verbatim);
        println!("ROW 4: after, the access view on {SPAWNED}: {gate_after:?}");
        launch.expect(
            after.answers() == (false, false, false)
                && gate_after.2.contains("first-person-off-one-agent"),
            format!(
                "after, the deny on its spawner still refuses the first person the spawned \
                 member ({}; {gate_after:?})",
                after.verbatim
            ),
        );
        launch.finish().await;
    }

    /// A deny that names the launch's member refuses the first person
    /// before the calling agent seats the name in a mob of its own, and
    /// after.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_deny_on_a_launch_member_holds_after_its_name_is_seated_in_another_mob() {
        let launch = boot("named-deny").await;
        launch.shared("planner", CALLER).await;
        launch.shared("worker", NAMED).await;
        let access = first_person_on(CALLER, vec![first_person_off(NAMED)]);
        let app =
            console_json_router_with_state(enforced_console(launch.mob(), Some(access.clone())));
        let alice = bearer(alice_claims());

        let before = doors(&app, &alice, NAMED).await;
        let gate_before = gate(&launch, &access, ALICE, NAMED).await;
        println!("ROW 5: before, on {NAMED}: {}", before.verbatim);
        println!("ROW 5: before, the access view on {NAMED}: {gate_before:?}");

        let own_mob = format!("{}-own", launch.mob_id);
        let seated = seats_in_its_own_mob(&launch, CALLER, &own_mob, NAMED).await;
        launch.expect(
            seated,
            "the calling agent's own mob holds the member it seated there",
        );

        let after = doors(&app, &alice, NAMED).await;
        let gate_after = gate(&launch, &access, ALICE, NAMED).await;
        println!("ROW 5: after, on {NAMED}: {}", after.verbatim);
        println!("ROW 5: after, the access view on {NAMED}: {gate_after:?}");
        for (when, doors, gate) in [
            ("before", &before, &gate_before),
            ("after", &after, &gate_after),
        ] {
            launch.expect(
                doors.answers() == (false, false, false)
                    && gate.2.contains("first-person-off-one-agent"),
                format!(
                    "{when}, the deny that names the launch's member refuses the first person \
                     ({}; {gate:?})",
                    doors.verbatim
                ),
            );
        }
        launch.finish().await;
    }
}

/// What a tool spawn registered is of the one member it seated. A member
/// seated later under that member's name, in the same mob, is another
/// member: nothing the earlier spawn carried decides it. The member the
/// spawn seated keeps what its spawn carried when it is respawned or
/// rebuilt under its name. Each row prints what it read (`ROW ...`).
mod spawned_member_names_within_a_mob {
    use super::super::console_private_side_chat_tests::{ALICE, ROOT};
    use super::*;

    /// A shared agent whose turns make the mob tool calls.
    const SPAWNER: &str = "alpha";
    /// The name its spawn seats in the launch's mob.
    const SPAWNED: &str = "gamma";

    /// The answer of the scripted spawn made since request `since`, as its
    /// model saw it: it sees it only after the call returned.
    async fn spawn_answer_since(launch: &Launch, since: usize) -> String {
        for _ in 0..100 {
            if let Some(request) = launch
                .seen
                .all()
                .into_iter()
                .skip(since)
                .find(|request| request.tool_results.contains(SPAWN_INTO_MOB_CALL))
            {
                return request.tool_results.chars().take(600).collect();
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        String::new()
    }

    /// [`SPAWNER`]'s own turn seats [`SPAWNED`] in the launch's mob with
    /// `mob_spawn_member`, with the label `tier` when one is given. Whether
    /// the launch's mob then holds the member.
    async fn spawner_seats(launch: &Launch, tier: Option<&str>) -> bool {
        let since = launch.seen.all().len();
        let tier = tier.map(|tier| format!(" tier={tier}")).unwrap_or_default();
        launch
            .turn(
                SPAWNER,
                &format!(
                    "{SPAWN_INTO_A_MOB} mob={} child={SPAWNED}{tier} please",
                    launch.mob_id
                ),
            )
            .await;
        println!(
            "ROW: the spawner's mob_spawn_member answered {}",
            spawn_answer_since(launch, since).await
        );
        launch.entry(SPAWNED).await.is_some()
    }

    /// The host ends [`SPAWNED`]. Whether the launch's mob stops holding it.
    async fn host_retires(launch: &Launch) -> bool {
        let retired = launch
            .mob()
            .handle()
            .retire(AgentIdentity::from(SPAWNED))
            .await;
        println!("ROW: the host's retire of {SPAWNED} answered {retired:?}");
        launch.gone_within(SPAWNED, 20).await
    }

    /// The access view's own answers for the first person on [`SPAWNED`]
    /// after a priming from the runtime: `agent.view`, `agent.send`, and the
    /// view decision.
    async fn first_person_view(launch: &Launch, access: &AccessController) -> (bool, bool, String) {
        prime_access_cache_from_runtime(launch.mob(), access).await;
        let view = access.view_for_subject(Some(ALICE));
        (
            view.can_view_agent(SPAWNED),
            view.allows_agent(ACTION_AGENT_SEND, SPAWNED),
            format!("{:?}", view.decide_agent(ACTION_AGENT_VIEW, SPAWNED)),
        )
    }

    fn enforced(rules: Vec<AccessRule>) -> AccessController {
        AccessController::new(crate::access::AccessControlConfig {
            enabled: true,
            admins: vec![ROOT.to_string()],
            rules,
            ..crate::access::AccessControlConfig::default()
        })
        .expect("access controller")
    }

    /// The first person may view and send where the rule's selector matches.
    fn first_person_may(id: &str, rule: AccessRule) -> AccessRule {
        AccessRule {
            id: id.to_string(),
            subjects: vec![ALICE.to_string()],
            actions: vec![ACTION_AGENT_VIEW.to_string(), ACTION_AGENT_SEND.to_string()],
            ..rule
        }
    }

    /// The rules allow the first person the spawning agent only, which
    /// reaches the member it spawned. That member ends, and the host seats a
    /// member of its own under the name: the allow on the earlier member's
    /// spawner does not reach it.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_member_the_host_seats_under_an_ended_spawned_members_name_is_its_own() {
        let launch = boot("seated-anew").await;
        launch.shared("planner", SPAWNER).await;
        let access = enforced(vec![first_person_may(
            "first-person-on-the-spawner",
            AccessRule {
                agents: vec![SPAWNER.to_string()],
                ..AccessRule::default()
            },
        )]);

        launch.expect(
            spawner_seats(&launch, None).await,
            "the launch's mob holds the member its agent spawned",
        );
        let before = first_person_view(&launch, &access).await;
        println!("ROW 1: the access view on the spawned {SPAWNED}: {before:?}");
        launch.expect(
            before.0 && before.1,
            format!("the allow on its spawner reaches the member the agent spawned ({before:?})"),
        );

        launch.expect(
            host_retires(&launch).await,
            "the spawned member leaves the roster",
        );
        launch.shared("worker", SPAWNED).await;
        let after = first_person_view(&launch, &access).await;
        println!("ROW 1: the access view on the host's {SPAWNED}: {after:?}");
        launch.expect(
            !after.0 && !after.1,
            format!(
                "the allow on the earlier member's spawner does not reach the member the host \
                 seated under its name ({after:?})"
            ),
        );
        launch.finish().await;
    }

    /// The rules allow the first person an agent that carries one label.
    /// A spawn seats a member with it; the member ends; a second spawn seats
    /// a member under the name without it: the second member does not carry
    /// the first spawn's label.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_second_spawn_under_an_ended_members_name_carries_its_own_labels_only() {
        let launch = boot("spawned-twice").await;
        launch.shared("planner", SPAWNER).await;
        let access = enforced(vec![first_person_may(
            "first-person-on-one-label",
            AccessRule {
                match_labels: [("tier".to_string(), "vault".to_string())]
                    .into_iter()
                    .collect(),
                ..AccessRule::default()
            },
        )]);

        launch.expect(
            spawner_seats(&launch, Some("vault")).await,
            "the launch's mob holds the member the first spawn seated",
        );
        let before = first_person_view(&launch, &access).await;
        println!("ROW 2: the access view on the first {SPAWNED}: {before:?}");
        launch.expect(
            before.0 && before.1,
            format!("the allow on the label reaches the member that carries it ({before:?})"),
        );

        launch.expect(
            host_retires(&launch).await,
            "the first member leaves the roster",
        );
        launch.expect(
            spawner_seats(&launch, None).await,
            "the launch's mob holds the member the second spawn seated",
        );
        let after = first_person_view(&launch, &access).await;
        println!("ROW 2: the access view on the second {SPAWNED}: {after:?}");
        launch.expect(
            !after.0 && !after.1,
            format!(
                "the allow on the first spawn's label does not reach the member the second \
                 spawn seated without it ({after:?})"
            ),
        );
        launch.finish().await;
    }

    /// A step that seats the same member again under its name.
    #[derive(Debug, Clone, Copy)]
    enum Again {
        /// A respawn: the member's next incarnation in a step of its own.
        Respawn,
        /// A rotation onto a fresh session: a reset.
        Reset,
    }

    /// The rules allow the first person the spawned member by its name and
    /// deny her its spawner, which refuses her the member it spawned. The
    /// member is seated again by `again`: the deny on its spawner still
    /// refuses her the member.
    async fn a_deny_on_its_spawner_still_refuses_a_spawned_member_after(again: Again) {
        let launch = boot("seated-again").await;
        launch.shared("planner", SPAWNER).await;
        let access = enforced(vec![
            first_person_may(
                "first-person-on-the-spawned-name",
                AccessRule {
                    agents: vec![SPAWNED.to_string()],
                    ..AccessRule::default()
                },
            ),
            AccessRule {
                id: "first-person-off-the-spawner".to_string(),
                effect: crate::access::AccessEffect::Deny,
                subjects: vec![ALICE.to_string()],
                actions: vec!["agent.*".to_string()],
                agents: vec![SPAWNER.to_string()],
                ..AccessRule::default()
            },
        ]);

        launch.expect(
            spawner_seats(&launch, None).await,
            "the launch's mob holds the member its agent spawned",
        );
        let before = first_person_view(&launch, &access).await;
        println!("ROW {again:?}: before, the access view on {SPAWNED}: {before:?}");
        launch.expect(
            !before.0 && before.2.contains("first-person-off-the-spawner"),
            format!("before, the deny on its spawner refuses the spawned member ({before:?})"),
        );

        let handle = launch.mob().handle();
        let seated_again = match again {
            Again::Respawn => handle
                .respawn(AgentIdentity::from(SPAWNED), None)
                .await
                .map(|_| ())
                .map_err(|error| format!("{error:?}")),
            Again::Reset => handle
                .rotate_member(meerkat_mob::SpawnMemberSpec::new(
                    "worker",
                    AgentIdentity::from(SPAWNED),
                ))
                .await
                .map(|_| ())
                .map_err(|error| format!("{error:?}")),
        };
        println!("ROW {again:?}: the step answered {seated_again:?}");
        launch.expect(
            seated_again.is_ok() && launch.entry(SPAWNED).await.is_some(),
            format!("{again:?}: the member is seated again under its name ({seated_again:?})"),
        );

        let after = first_person_view(&launch, &access).await;
        println!("ROW {again:?}: after, the access view on {SPAWNED}: {after:?}");
        launch.expect(
            !after.0 && after.2.contains("first-person-off-the-spawner"),
            format!(
                "{again:?}: the deny on its spawner still refuses the member seated again \
                 ({after:?})"
            ),
        );
        launch.finish().await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_deny_on_its_spawner_still_refuses_a_spawned_member_after_a_respawn() {
        a_deny_on_its_spawner_still_refuses_a_spawned_member_after(Again::Respawn).await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_deny_on_its_spawner_still_refuses_a_spawned_member_after_a_reset() {
        a_deny_on_its_spawner_still_refuses_a_spawned_member_after(Again::Reset).await;
    }
}

/// The same order (a spawn seats a member, the member ends, the host seats a
/// member of its own under the name) read at the Console's other readers of
/// what a spawn registered: the member list, the aggregator's identity
/// record and identity list. One more row reads a member an agent forked.
mod spawned_member_names_within_a_mob_at_other_readers {
    use super::super::console_private_side_chat_tests::{ALICE, ROOT};
    use super::*;

    /// A shared agent whose turns make the mob tool calls.
    const SPAWNER: &str = "alpha";
    /// The name its spawn seats in the launch's mob.
    const SPAWNED: &str = "gamma";

    /// The rules allow the first person the spawning agent only; `root` is
    /// the one admin.
    fn first_person_on_the_spawner() -> AccessController {
        AccessController::new(crate::access::AccessControlConfig {
            enabled: true,
            admins: vec![ROOT.to_string()],
            rules: vec![AccessRule {
                id: "first-person-on-the-spawner".to_string(),
                subjects: vec![ALICE.to_string()],
                actions: vec![ACTION_AGENT_VIEW.to_string(), ACTION_AGENT_SEND.to_string()],
                agents: vec![SPAWNER.to_string()],
                ..AccessRule::default()
            }],
            ..crate::access::AccessControlConfig::default()
        })
        .expect("access controller")
    }

    /// [`SPAWNER`]'s own turn seats [`SPAWNED`] in the launch's mob with
    /// `mob_spawn_member`. Whether the launch's mob then holds the member.
    async fn spawner_seats(launch: &Launch) -> bool {
        launch
            .turn(
                SPAWNER,
                &format!(
                    "{SPAWN_INTO_A_MOB} mob={} child={SPAWNED} please",
                    launch.mob_id
                ),
            )
            .await;
        for _ in 0..100 {
            if launch.entry(SPAWNED).await.is_some() {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        false
    }

    /// The spawned member ends and the host seats a member of its own under
    /// the name.
    async fn the_host_seats_the_name_anew(launch: &Launch) {
        let retired = launch
            .mob()
            .handle()
            .retire(AgentIdentity::from(SPAWNED))
            .await;
        println!("ROW: the host's retire of {SPAWNED} answered {retired:?}");
        launch.expect(
            launch.gone_within(SPAWNED, 20).await,
            "the spawned member leaves the roster",
        );
        launch.shared("worker", SPAWNED).await;
    }

    /// Whether `meta/list_members` shows the first person [`SPAWNED`].
    async fn listed_to_the_first_person(app: &Router, bearer: &str) -> bool {
        let listed = console_rpc_as(app, Some(bearer), "meta/list_members", json!({})).await;
        let names = |key: &str| {
            listed["result"]
                .as_array()
                .is_some_and(|rows| rows.iter().any(|row| row[key] == json!(SPAWNED)))
        };
        let shown = names("member_id") || names("agent_identity") || names("identity");
        println!(
            "ROW: meta/list_members shows the first person {SPAWNED}: {shown} (error {})",
            listed["error"]
        );
        shown
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_member_list_shows_a_member_the_host_seats_under_an_ended_spawned_members_name_as_its_own()
     {
        let launch = boot("list-anew").await;
        launch.shared("planner", SPAWNER).await;
        let access = first_person_on_the_spawner();
        let app =
            console_json_router_with_state(enforced_console(launch.mob(), Some(access.clone())));
        let alice = bearer(alice_claims());

        launch.expect(
            spawner_seats(&launch).await,
            "the launch's mob holds the member its agent spawned",
        );
        launch.expect(
            listed_to_the_first_person(&app, &alice).await,
            "the member list shows the first person the member the agent she is allowed spawned",
        );
        the_host_seats_the_name_anew(&launch).await;
        launch.expect(
            !listed_to_the_first_person(&app, &alice).await,
            "the member list does not show the first person the member the host seated under \
             the ended member's name",
        );
        launch.finish().await;
    }

    /// An aggregator over the launch's runtime alone, under no namespace,
    /// reading the runtime's own Console event store.
    fn aggregator_over(launch: &Launch) -> crate::console_aggregator::MetaConsoleAggregator {
        let aggregator = crate::console_aggregator::MetaConsoleAggregator::in_memory();
        aggregator.register_runtime_handles_with_policy(
            "default",
            "",
            launch.mob().clone(),
            None,
            launch.runtime.console_events(),
            Arc::new(crate::console_aggregator::AllowAllConsoleVisibilityPolicy),
        );
        aggregator
    }

    /// What a fresh aggregator over the launch reads of [`SPAWNED`]: the
    /// spawner its identity record names, and whether the identity list
    /// shows the member to the first person.
    async fn aggregator_reads(
        launch: &Launch,
        access: &AccessController,
    ) -> (Option<String>, bool) {
        let aggregator = aggregator_over(launch);
        let spawner = aggregator
            .list_identities_fresh()
            .await
            .expect("the aggregator's identity records")
            .into_iter()
            .find(|record| record.identity == SPAWNED)
            .and_then(|record| {
                record
                    .labels
                    .get(crate::console_spawn::SPAWNED_BY_LABEL)
                    .cloned()
            });
        prime_access_cache_from_runtime(launch.mob(), access).await;
        let view = access.view_for_subject(Some(ALICE));
        let listed = Box::pin(visible_identity_records(&aggregator, Some(&view)))
            .await
            .expect("the identity list")
            .iter()
            .any(|record| record.identity == SPAWNED);
        println!(
            "ROW: the aggregator's record of {SPAWNED} names the spawner {spawner:?}; the \
             identity list shows it to the first person: {listed}"
        );
        (spawner, listed)
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_aggregators_record_of_a_member_the_host_seats_under_an_ended_spawned_members_name_names_no_spawner()
     {
        let launch = boot("record-anew").await;
        launch.shared("planner", SPAWNER).await;
        let access = first_person_on_the_spawner();
        launch.expect(
            spawner_seats(&launch).await,
            "the launch's mob holds the member its agent spawned",
        );
        let before = aggregator_reads(&launch, &access).await;
        launch.expect(
            before.0.as_deref() == Some(SPAWNER),
            format!(
                "the record of the spawned member names the agent that spawned it ({:?})",
                before.0
            ),
        );
        the_host_seats_the_name_anew(&launch).await;
        let after = aggregator_reads(&launch, &access).await;
        launch.expect(
            after.0.is_none(),
            format!(
                "the record of the member the host seated names no spawner ({:?})",
                after.0
            ),
        );
        launch.finish().await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_identity_list_shows_a_member_the_host_seats_under_an_ended_spawned_members_name_as_its_own()
     {
        let launch = boot("identities-anew").await;
        launch.shared("planner", SPAWNER).await;
        let access = first_person_on_the_spawner();
        launch.expect(
            spawner_seats(&launch).await,
            "the launch's mob holds the member its agent spawned",
        );
        let before = aggregator_reads(&launch, &access).await;
        launch.expect(
            before.1,
            "the identity list shows the first person the member the agent she is allowed \
             spawned",
        );
        the_host_seats_the_name_anew(&launch).await;
        let after = aggregator_reads(&launch, &access).await;
        launch.expect(
            !after.1,
            "the identity list does not show the first person the member the host seated under \
             the ended member's name",
        );
        launch.finish().await;
    }

    /// A member an agent forked is reached through that agent, as a member
    /// it spawned is: the rules allow the first person the forked member by
    /// its name and deny her the agent that forked it.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_deny_on_the_agent_that_forked_it_refuses_a_forked_member() {
        const FORKED: &str = "delta";
        let launch = boot("forked").await;
        launch.shared("planner", SPAWNER).await;
        let access = AccessController::new(crate::access::AccessControlConfig {
            enabled: true,
            admins: vec![ROOT.to_string()],
            rules: vec![
                AccessRule {
                    id: "first-person-on-the-forked-name".to_string(),
                    subjects: vec![ALICE.to_string()],
                    actions: vec![ACTION_AGENT_VIEW.to_string(), ACTION_AGENT_SEND.to_string()],
                    agents: vec![FORKED.to_string()],
                    ..AccessRule::default()
                },
                AccessRule {
                    id: "first-person-off-the-forking-agent".to_string(),
                    effect: crate::access::AccessEffect::Deny,
                    subjects: vec![ALICE.to_string()],
                    actions: vec!["agent.*".to_string()],
                    agents: vec![SPAWNER.to_string()],
                    ..AccessRule::default()
                },
            ],
            ..crate::access::AccessControlConfig::default()
        })
        .expect("access controller");

        launch
            .turn(
                SPAWNER,
                &format!("{FORK_A_CHILD} child={FORKED} make=no please"),
            )
            .await;
        launch.expect(
            launch.entry(FORKED).await.is_some(),
            "the launch's mob holds the member its agent forked",
        );
        prime_access_cache_from_runtime(launch.mob(), &access).await;
        let view = access.view_for_subject(Some(ALICE));
        let decision = format!("{:?}", view.decide_agent(ACTION_AGENT_VIEW, FORKED));
        println!("ROW: the access view on the forked {FORKED}: {decision}");
        launch.expect(
            !view.can_view_agent(FORKED) && decision.contains("first-person-off-the-forking-agent"),
            format!(
                "the deny on the agent that forked it refuses the first person the forked \
                 member ({decision})"
            ),
        );
        launch.finish().await;
    }
}

/// One request's read of the roster and its record of what it read are two
/// steps. A spawned member that ends, and whose name the host seats anew,
/// between them is not recorded with what the earlier spawn carried.
mod spawned_member_names_within_a_mob_between_two_reads {
    use super::super::console_private_side_chat_tests::{ALICE, ROOT};
    use super::*;

    /// A shared agent whose turns make the mob tool calls.
    const SPAWNER: &str = "alpha";
    /// The name its spawn seats in the launch's mob.
    const SPAWNED: &str = "gamma";

    #[tokio::test(flavor = "multi_thread")]
    async fn a_name_seated_anew_while_a_request_holds_its_roster_read_is_not_recorded_with_the_earlier_spawn()
     {
        let launch = boot("between-reads").await;
        launch.shared("planner", SPAWNER).await;
        let access = AccessController::new(crate::access::AccessControlConfig {
            enabled: true,
            admins: vec![ROOT.to_string()],
            rules: vec![AccessRule {
                id: "first-person-on-the-spawner".to_string(),
                subjects: vec![ALICE.to_string()],
                actions: vec![ACTION_AGENT_VIEW.to_string(), ACTION_AGENT_SEND.to_string()],
                agents: vec![SPAWNER.to_string()],
                ..AccessRule::default()
            }],
            ..crate::access::AccessControlConfig::default()
        })
        .expect("access controller");

        launch
            .turn(
                SPAWNER,
                &format!(
                    "{SPAWN_INTO_A_MOB} mob={} child={SPAWNED} please",
                    launch.mob_id
                ),
            )
            .await;
        launch.expect(
            launch.entry(SPAWNED).await.is_some(),
            "the launch's mob holds the member its agent spawned",
        );
        prime_access_cache_from_runtime(launch.mob(), &access).await;
        let before = access.view_for_subject(Some(ALICE)).can_view_agent(SPAWNED);
        println!("ROW: before, the first person may view the spawned {SPAWNED}: {before}");
        launch.expect(
            before,
            "the allow on its spawner reaches the member the agent spawned",
        );

        // A priming reads the roster and waits before it records.
        let (open, gate) = tokio::sync::watch::channel(false);
        let reached = super::super::unseated_read_hold::hold(&launch.mob_id, gate);
        let priming = tokio::spawn({
            let runtime = launch.mob().clone();
            let access = access.clone();
            async move { prime_access_cache_from_runtime(&runtime, &access).await }
        });
        for _ in 0..200 {
            if reached.load(std::sync::atomic::Ordering::SeqCst) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        launch.expect(
            reached.load(std::sync::atomic::Ordering::SeqCst),
            "the priming waits after its roster read",
        );

        // Between its two steps the spawned member ends and the host seats
        // a member of its own under the name.
        let retired = launch
            .mob()
            .handle()
            .retire(AgentIdentity::from(SPAWNED))
            .await;
        println!("ROW: the host's retire of {SPAWNED} answered {retired:?}");
        launch.expect(
            launch.gone_within(SPAWNED, 20).await,
            "the spawned member leaves the roster",
        );
        launch.shared("worker", SPAWNED).await;

        let _ = open.send(true);
        priming.await.expect("the held priming ends");
        let view = access.view_for_subject(Some(ALICE));
        let after = view.can_view_agent(SPAWNED);
        println!(
            "ROW: after the held priming recorded, the first person may view {SPAWNED}: {after} \
             ({:?})",
            view.decide_agent(ACTION_AGENT_VIEW, SPAWNED)
        );
        launch.expect(
            !after,
            "the held priming does not record the name with the earlier spawn's spawner",
        );
        launch.finish().await;
    }
}

/// The settle of the spawn registry against a mob's own member lifecycle
/// journal, read on a launch: what each kind of seat means for an entry, how
/// a reader with a member in hand is answered, and how many journal reads a
/// request makes. Each row prints what it read (`ROW ...`).
mod spawn_registry_settle {
    use super::*;
    use crate::console_spawn::{SeatFact, SpawnEntryNotTold, seat_fact};

    /// A shared agent whose turns make the mob tool calls.
    const SPAWNER: &str = "alpha";
    /// The name its spawn seats in the launch's mob.
    const SPAWNED: &str = "gamma";

    /// Every fact of the launch's member lifecycle journal, classified.
    async fn seat_facts(launch: &Launch) -> Vec<SeatFact> {
        let handle = launch.mob().handle();
        let mut cursor = meerkat_mob::MobJournalCursor::START;
        let mut facts = Vec::new();
        loop {
            let page = handle
                .member_lifecycle_events(cursor, 256)
                .await
                .expect("the member lifecycle journal");
            facts.extend(page.records.iter().map(|record| seat_fact(&record.event)));
            if page.next.get() == cursor.get() {
                return facts;
            }
            cursor = page.next;
        }
    }

    /// The generation the roster holds `name` at.
    async fn generation_of(launch: &Launch, name: &str) -> u64 {
        launch
            .mob()
            .handle()
            .get_member(&AgentIdentity::from(name))
            .await
            .expect("roster read")
            .expect("the member is seated")
            .generation
            .get()
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn each_kind_of_seat_is_read_from_the_mobs_own_journal() {
        const NAME: &str = "delta";
        let launch = boot("seat-facts").await;
        let handle = launch.mob().handle();
        launch.shared("worker", NAME).await;
        let respawned = handle.respawn(AgentIdentity::from(NAME), None).await;
        let rotated = handle
            .rotate_member(meerkat_mob::SpawnMemberSpec::new(
                "worker",
                AgentIdentity::from(NAME),
            ))
            .await;
        println!(
            "ROW: the respawn answered ok={}, the rotation answered ok={}",
            respawned.is_ok(),
            rotated.is_ok()
        );
        launch.expect(
            respawned.is_ok() && rotated.is_ok(),
            "the member is respawned and rotated",
        );
        let facts = seat_facts(&launch).await;
        let seats: Vec<bool> = facts
            .iter()
            .filter_map(|fact| match fact {
                SeatFact::Seat {
                    member_id, fresh, ..
                } if member_id == NAME => Some(*fresh),
                _ => None,
            })
            .collect();
        println!("ROW: the seats of {NAME}, each as made by no rotation: {seats:?}");
        launch.expect(
            seats.first() == Some(&true),
            format!("the host's seat is a new member's ({seats:?})"),
        );
        launch.expect(
            seats.len() >= 3 && seats.iter().skip(1).all(|fresh| !fresh),
            format!("a respawn's and a rotation's seat are the same member's ({seats:?})"),
        );
        launch.expect(
            !facts.contains(&SeatFact::Unclassified),
            "the journal holds no fact the reading does not know",
        );

        let retired = handle.retire(AgentIdentity::from(NAME)).await;
        println!("ROW: the retire answered ok={}", retired.is_ok());
        launch.expect(
            launch.gone_within(NAME, 20).await,
            "the member leaves the roster",
        );
        launch.shared("worker", NAME).await;
        let seats_after: Vec<bool> = seat_facts(&launch)
            .await
            .iter()
            .filter_map(|fact| match fact {
                SeatFact::Seat {
                    member_id, fresh, ..
                } if member_id == NAME => Some(*fresh),
                _ => None,
            })
            .collect();
        println!("ROW: after an end and a new seat: {seats_after:?}");
        launch.expect(
            seats_after.last() == Some(&true),
            format!("a seat after the member's end is a new member's ({seats_after:?})"),
        );

        let reset = handle.reset().await;
        launch.expect(
            reset.is_ok(),
            format!("the mob is reset: {:?}", reset.as_ref().err()),
        );
        let facts = seat_facts(&launch).await;
        println!(
            "ROW: after the mob's reset the journal holds a mob reset fact: {}",
            facts.contains(&SeatFact::MobReset)
        );
        launch.expect(
            facts.contains(&SeatFact::MobReset) && !facts.contains(&SeatFact::Unclassified),
            "the mob's reset is read as one, and no fact is unknown",
        );
        launch.finish().await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_reader_with_a_member_in_hand_is_answered_for_that_incarnation_and_reads_the_journal_once_per_seat()
     {
        let launch = boot("in-hand").await;
        launch.shared("planner", SPAWNER).await;
        launch
            .turn(
                SPAWNER,
                &format!(
                    "{SPAWN_INTO_A_MOB} mob={} child={SPAWNED} please",
                    launch.mob_id
                ),
            )
            .await;
        launch.expect(
            launch.entry(SPAWNED).await.is_some(),
            "the launch's mob holds the member its agent spawned",
        );
        let handle = launch.mob().handle();
        let store = launch.runtime.console_events();
        let sink = launch.mob().console_spawn_sink().expect("the spawn sink");
        let spawner_of = |answer: &Result<
            Option<std::collections::BTreeMap<String, String>>,
            SpawnEntryNotTold,
        >| {
            answer.clone().map(|labels| {
                labels
                    .and_then(|labels| labels.get(crate::console_spawn::SPAWNED_BY_LABEL).cloned())
            })
        };

        // The spawned member, as seated: its own entry, with no journal read
        // for a seat already read.
        let first = generation_of(&launch, SPAWNED).await;
        let reads = store.spawn_journal_reads().await;
        let own = sink
            .labels_of_seated_row(&handle, SPAWNED, SPAWNED, Some(first))
            .await;
        let again = sink
            .labels_of_seated_row(&handle, SPAWNED, SPAWNED, Some(first))
            .await;
        let reads_for_a_read_seat = store.spawn_journal_reads().await - reads;
        println!(
            "ROW: the spawned member at generation {first}: {:?}; journal reads for two asks: \
             {reads_for_a_read_seat}",
            spawner_of(&own)
        );
        launch.expect(
            spawner_of(&own) == Ok(Some(SPAWNER.to_string())) && own == again,
            "the spawned member's row takes its own entry",
        );
        launch.expect(
            reads_for_a_read_seat == 0,
            format!("a seat already read costs no journal read ({reads_for_a_read_seat})"),
        );

        // One request's priming: how many journal reads it makes.
        let access = AccessController::new(crate::access::AccessControlConfig::default())
            .expect("access controller");
        let reads = store.spawn_journal_reads().await;
        prime_access_cache_from_runtime(launch.mob(), &access).await;
        let reads_per_priming = store.spawn_journal_reads().await - reads;
        println!(
            "ROW: journal reads of one priming with one mob that holds an entry: \
             {reads_per_priming}"
        );
        launch.expect(
            reads_per_priming == 2,
            format!(
                "a priming reads the journal once before its roster read and once after \
                 ({reads_per_priming})"
            ),
        );

        // The same member, respawned: its entry stays, one settle reads the
        // new seat.
        let respawned = handle.respawn(AgentIdentity::from(SPAWNED), None).await;
        launch.expect(respawned.is_ok(), "the spawned member is respawned");
        let second = generation_of(&launch, SPAWNED).await;
        let after_respawn = sink
            .labels_of_seated_row(&handle, SPAWNED, SPAWNED, Some(second))
            .await;
        println!(
            "ROW: the respawned member at generation {second}: {:?}",
            spawner_of(&after_respawn)
        );
        launch.expect(
            second > first && spawner_of(&after_respawn) == Ok(Some(SPAWNER.to_string())),
            "the respawned member's row takes the entry its spawn registered",
        );

        // The member ends and the host seats a member of its own under the
        // name. A row of the ended member that a reader still has in hand
        // takes the ended member's entry until the new seat is read; the new
        // member's row takes none; after that the ended member's row cannot
        // be told.
        let retired = handle.retire(AgentIdentity::from(SPAWNED)).await;
        println!("ROW: the retire answered ok={}", retired.is_ok());
        launch.expect(
            launch.gone_within(SPAWNED, 20).await,
            "the spawned member leaves the roster",
        );
        launch.shared("worker", SPAWNED).await;
        let third = generation_of(&launch, SPAWNED).await;
        let ended_before = sink
            .labels_of_seated_row(&handle, SPAWNED, SPAWNED, Some(second))
            .await;
        let new_member = sink
            .labels_of_seated_row(&handle, SPAWNED, SPAWNED, Some(third))
            .await;
        let ended_after = sink
            .labels_of_seated_row(&handle, SPAWNED, SPAWNED, Some(second))
            .await;
        println!(
            "ROW: the ended member's row before the new seat is read: {:?}; the new member at \
             generation {third}: {:?}; the ended member's row after: {:?}",
            spawner_of(&ended_before),
            spawner_of(&new_member),
            spawner_of(&ended_after)
        );
        launch.expect(
            third > second && spawner_of(&ended_before) == Ok(Some(SPAWNER.to_string())),
            "the ended member's row takes its own entry while the entry stands",
        );
        launch.expect(
            new_member == Ok(None),
            "the member the host seated under the name takes no entry",
        );
        launch.expect(
            ended_after == Err(SpawnEntryNotTold),
            "the ended member's row cannot be told once its entry is gone",
        );
        launch.finish().await;
    }
}

/// A mob whose member lifecycle journal the spawn registry's settle could not
/// stand behind is a mob whose entries cannot be told: no reader takes an
/// entry of it for a row. And the snapshot walk, which takes the registry
/// before it reads the rosters, does not show a name seated anew in between
/// with the earlier member's spawner. Each row prints what it read
/// (`ROW ...`).
mod spawn_registry_entries_not_told {
    use super::super::console_private_side_chat_tests::{ALICE, ROOT};
    use super::*;

    /// A shared agent whose turns make the mob tool calls.
    const SPAWNER: &str = "alpha";
    /// The name its spawn seats in the launch's mob.
    const SPAWNED: &str = "gamma";

    /// A launch whose shared agent has spawned [`SPAWNED`] into its mob.
    async fn launch_with_a_spawned_member(label: &str) -> Launch {
        let launch = boot(label).await;
        launch.shared("planner", SPAWNER).await;
        launch
            .turn(
                SPAWNER,
                &format!(
                    "{SPAWN_INTO_A_MOB} mob={} child={SPAWNED} please",
                    launch.mob_id
                ),
            )
            .await;
        launch.expect(
            launch.entry(SPAWNED).await.is_some(),
            "the launch's mob holds the member its agent spawned",
        );
        launch
    }

    /// The mob's entries cannot be told from now on.
    async fn mark_not_told(launch: &Launch) {
        launch
            .runtime
            .console_events()
            .mark_spawn_journal_untold(&launch.mob_id)
            .await;
    }

    /// The spawner the snapshot walk's row of [`SPAWNED`] names.
    fn walked_spawner(state: &ConsoleSnapshotReadModelState) -> Option<String> {
        state
            .primary_members
            .iter()
            .find(|member| member.agent_identity == SPAWNED)
            .and_then(|member| {
                member
                    .labels
                    .get(crate::console_spawn::SPAWNED_BY_LABEL)
                    .cloned()
            })
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_member_list_takes_no_entry_of_a_mob_that_cannot_be_told() {
        let launch = boot("list-not-told").await;
        launch.shared("planner", SPAWNER).await;
        // The first person may view and send to the spawning agent only. The
        // access records hold the spawning agent and not the member it spawns
        // afterwards, so the list's own reading of the row decides alone.
        let access = AccessController::new(crate::access::AccessControlConfig {
            enabled: true,
            admins: vec![ROOT.to_string()],
            rules: vec![AccessRule {
                id: "first-person-on-the-spawner".to_string(),
                subjects: vec![ALICE.to_string()],
                actions: vec![ACTION_AGENT_VIEW.to_string(), ACTION_AGENT_SEND.to_string()],
                agents: vec![SPAWNER.to_string()],
                ..AccessRule::default()
            }],
            ..crate::access::AccessControlConfig::default()
        })
        .expect("access controller");
        prime_access_cache_from_runtime(launch.mob(), &access).await;
        launch
            .turn(
                SPAWNER,
                &format!(
                    "{SPAWN_INTO_A_MOB} mob={} child={SPAWNED} please",
                    launch.mob_id
                ),
            )
            .await;
        launch.expect(
            launch.entry(SPAWNED).await.is_some(),
            "the launch's mob holds the member its agent spawned",
        );

        let mut shown = Vec::new();
        for told in [true, false] {
            if !told {
                mark_not_told(&launch).await;
            }
            let entries = launch
                .mob()
                .handle()
                .list_members_including_retiring()
                .await;
            let mut rows: Vec<Value> = entries
                .iter()
                .map(|entry| {
                    json!({
                        "agent_identity": entry.agent_identity.as_str(),
                        "role": entry.role.to_string(),
                        "labels": entry.labels,
                    })
                })
                .collect();
            let view = access.view_for_subject(Some(ALICE));
            retain_visible_member_rows(&mut rows, launch.mob(), &entries, Some(&view)).await;
            shown.push(
                rows.iter()
                    .any(|row| row["agent_identity"] == json!(SPAWNED)),
            );
        }
        println!(
            "ROW: the member list's reader keeps the spawned member's row, told then not told: \
             {shown:?}"
        );
        launch.expect(
            shown[0],
            "with its entry told, the allow on its spawner keeps the spawned member's row",
        );
        launch.expect(
            !shown[1],
            "with the mob not told, the list's reader takes no entry and the row is not kept",
        );
        launch.finish().await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_aggregators_label_read_takes_no_entry_of_a_mob_that_cannot_be_told() {
        let launch = launch_with_a_spawned_member("labels-not-told").await;
        let aggregator = crate::console_aggregator::MetaConsoleAggregator::in_memory();
        aggregator.register_runtime_handles_with_policy(
            "default",
            "",
            launch.mob().clone(),
            None,
            launch.runtime.console_events(),
            Arc::new(crate::console_aggregator::AllowAllConsoleVisibilityPolicy),
        );
        let records: Vec<_> = aggregator
            .list_identities_fresh()
            .await
            .expect("the aggregator's identity records")
            .into_iter()
            .filter(|record| record.identity == SPAWNED)
            .collect();
        launch.expect(
            records.len() == 1,
            "the aggregator holds a record of the spawned member",
        );
        let mut read = Vec::new();
        for told in [true, false] {
            if !told {
                mark_not_told(&launch).await;
            }
            let labels = aggregator
                .spawn_registry_labels_for_identities(&records)
                .await
                .expect("the label read");
            read.push(labels.first().is_some_and(Option::is_some));
        }
        println!(
            "ROW: the aggregator's label read gives the record an entry, told then not told: \
             {read:?}"
        );
        launch.expect(read[0], "with its entry told, the record takes it");
        launch.expect(
            !read[1],
            "with the mob not told, the label read gives the record no entry",
        );
        launch.finish().await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_snapshot_walk_takes_no_entry_of_a_mob_that_cannot_be_told() {
        let launch = launch_with_a_spawned_member("walk-not-told").await;
        let told = walked_spawner(&walk_console_snapshot_read_model(launch.mob(), None).await);
        mark_not_told(&launch).await;
        let not_told = walked_spawner(&walk_console_snapshot_read_model(launch.mob(), None).await);
        println!(
            "ROW: the snapshot walk's row names the spawner, told then not told: {told:?}, \
             {not_told:?}"
        );
        launch.expect(
            told.as_deref() == Some(SPAWNER),
            "with its entry told, the walk's row names the agent that spawned the member",
        );
        launch.expect(
            not_told.is_none(),
            "with the mob not told, the walk's row names no spawner",
        );
        launch.finish().await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_snapshot_walk_does_not_show_a_name_seated_anew_between_its_reads_with_the_earlier_spawner()
     {
        let launch = launch_with_a_spawned_member("walk-between").await;
        // A walk reads the rosters and waits before it settles again.
        let (open, gate) = tokio::sync::watch::channel(false);
        let reached =
            super::super::unseated_read_hold::hold(&format!("walk:{}", launch.mob_id), gate);
        let walk = tokio::spawn({
            let runtime = launch.mob().clone();
            async move { walk_console_snapshot_read_model(&runtime, None).await }
        });
        for _ in 0..200 {
            if reached.load(std::sync::atomic::Ordering::SeqCst) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        launch.expect(
            reached.load(std::sync::atomic::Ordering::SeqCst),
            "the walk waits after its roster reads",
        );
        let retired = launch
            .mob()
            .handle()
            .retire(AgentIdentity::from(SPAWNED))
            .await;
        println!("ROW: the host's retire of {SPAWNED} answered {retired:?}");
        launch.expect(
            launch.gone_within(SPAWNED, 20).await,
            "the spawned member leaves the roster",
        );
        launch.shared("worker", SPAWNED).await;
        let _ = open.send(true);
        let walked = walked_spawner(&walk.await.expect("the held walk ends"));
        println!("ROW: the held walk's row of {SPAWNED} names the spawner {walked:?}");
        launch.expect(
            walked.is_none(),
            "the held walk does not show the name with the earlier member's spawner",
        );
        launch.finish().await;
    }
}

/// A spawned member adopted as an identity keeps what its spawn registered:
/// the adoption seats it on by a rotation, which is the same member.
mod spawned_member_adopted_as_an_identity {
    use super::super::console_private_side_chat_tests::{ALICE, ROOT, open_console};
    use super::*;

    /// A shared agent whose turns make the mob tool calls.
    const SPAWNER: &str = "alpha";
    /// The name its spawn seats in the launch's mob.
    const SPAWNED: &str = "gamma";

    #[tokio::test(flavor = "multi_thread")]
    async fn a_deny_on_its_spawner_still_refuses_a_spawned_member_adopted_as_an_identity() {
        let launch = boot("adopted").await;
        launch.shared("planner", SPAWNER).await;
        let access = AccessController::new(crate::access::AccessControlConfig {
            enabled: true,
            admins: vec![ROOT.to_string()],
            rules: vec![
                AccessRule {
                    id: "first-person-on-the-spawned-name".to_string(),
                    subjects: vec![ALICE.to_string()],
                    actions: vec![ACTION_AGENT_VIEW.to_string(), ACTION_AGENT_SEND.to_string()],
                    agents: vec![SPAWNED.to_string()],
                    ..AccessRule::default()
                },
                AccessRule {
                    id: "first-person-off-the-spawner".to_string(),
                    effect: crate::access::AccessEffect::Deny,
                    subjects: vec![ALICE.to_string()],
                    actions: vec!["agent.*".to_string()],
                    agents: vec![SPAWNER.to_string()],
                    ..AccessRule::default()
                },
            ],
            ..crate::access::AccessControlConfig::default()
        })
        .expect("access controller");
        launch
            .turn(
                SPAWNER,
                &format!(
                    "{SPAWN_INTO_A_MOB} mob={} child={SPAWNED} please",
                    launch.mob_id
                ),
            )
            .await;
        let session = launch
            .entry(SPAWNED)
            .await
            .and_then(|seat| seat.session)
            .map(|session| session.to_string());
        launch.expect(
            session.is_some(),
            "the launch's mob holds the member its agent spawned, on a session",
        );
        let session = session.unwrap_or_default();

        let mut refused = Vec::new();
        for adopted in [false, true] {
            if adopted {
                let app = console_json_router_with_state(open_console(launch.mob()));
                let answer = console_rpc_as(
                    &app,
                    None,
                    crate::rpc::mob_methods::ADOPT_MEMBER_IDENTITY_DECLARATION,
                    json!({
                        "mob_id": launch.mob_id,
                        "agent_identity": SPAWNED,
                        "request_id": "adopt-a-spawned-member",
                        "precondition": "expected_absent",
                        "declaration_scope": "spawned-adoption",
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
                println!(
                    "ROW: the adoption answered error={} outcome={}",
                    answer["error"], answer["result"]["adoption"]["outcome"]
                );
                launch.expect(
                    answer["result"]["adoption"]["outcome"] == json!("adopted"),
                    format!("the spawned member is adopted ({})", answer["error"]),
                );
                tokio::time::sleep(Duration::from_secs(3)).await;
            }
            prime_access_cache_from_runtime(launch.mob(), &access).await;
            let view = access.view_for_subject(Some(ALICE));
            let decision = format!("{:?}", view.decide_agent(ACTION_AGENT_VIEW, SPAWNED));
            println!("ROW: adopted={adopted}: the access view on {SPAWNED}: {decision}");
            refused.push(
                !view.can_view_agent(SPAWNED) && decision.contains("first-person-off-the-spawner"),
            );
        }
        launch.expect(
            refused[0],
            "before the adoption, the deny on its spawner refuses the spawned member",
        );
        launch.expect(
            refused[1],
            "after the adoption, the deny on its spawner still refuses the member",
        );
        launch.finish().await;
    }
}

/// The rows on the question sets of a member private to a person once it
/// ends, on this rig.
#[path = "console_private_member_question_set_end_tests.rs"]
mod question_set_end;
