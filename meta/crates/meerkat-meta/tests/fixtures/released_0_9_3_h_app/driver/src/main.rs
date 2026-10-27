//! Mints Meta 0.9.3 embedded-builder state directories: the persistent path
//! an application takes when it embeds `UnifiedRuntimeBuilder` with
//! `persistent_state` (shape S2 of the #306 plan), as inputs for the
//! storage-migrate-native census and the upgrade acceptance tests.
//!
//! Built only against meerkat-meta =0.9.3 and its meerkat crates from
//! crates.io (see Cargo.toml and the Cargo.lock written beside it); never
//! rebuild it against current code. The model is a scripted in-process
//! client, so no provider key is involved.
//!
//! Usage: s2-mint-093 <roster|roster-durable|roster-durable-mob-sqlite|plain> <work-dir>
//! roster keeps the builder's default in-memory mob storage; roster-durable
//! supplies persistent mob storage at <state>/event_log.sqlite3 (the gateway's
//! file name) through `mob_storage`; roster-durable-mob-sqlite does the same at
//! <state>/mob.sqlite3 (the name 0.9.3's own non-gateway call sites use); plain
//! has no roster provider.
//! Writes <work-dir>/state (the state directory), <work-dir>/events.jsonl
//! (every operation with its result or error) and <work-dir>/probe.json
//! (whether 0.9.3 builds again on a copy of the final state).

mod llm_usage;

use std::collections::BTreeMap;
use std::error::Error;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use meerkat_client::{LlmClient, LlmError, LlmEvent, LlmRequest};
use meerkat_core::{ContentInput, Provider, StopReason, Usage};
use meerkat_meta::identity_first::contracts::RosterProvider;
use meerkat_meta::identity_first::{
    AgentAddressability, AgentIdentity, DurableAgentSpec, RosterContext, RosterError,
};
use meerkat_meta::{IdentityBootstrapMode, UnifiedRuntime, UnifiedRuntimeBuilder};
use meerkat_mob::{MobDefinition, ProfileName};
use serde_json::{Value, json};

const MOB_ID: &str = "app-corpus-mob";
const PROFILE: &str = "assistant";
/// A user message carrying this marker makes the answering request report a
/// provider input cost above the auto-compaction threshold, once per message.
const COMPACT_MARKER: &str = "[compact-now]";
const COMPACT_THRESHOLD: u64 = 50_000;
const TURN_DEADLINE: Duration = Duration::from_secs(60);

fn identity(name: &str) -> AgentIdentity {
    AgentIdentity::parse(name).expect("valid corpus identity")
}

fn spec(name: &str) -> DurableAgentSpec {
    DurableAgentSpec {
        identity: identity(name),
        profile: ProfileName::from(PROFILE),
        addressability: AgentAddressability::Addressable,
        display_name: None,
        labels: BTreeMap::new(),
        context: None,
        additional_instructions: Vec::new(),
        initial_message: None,
        runtime_mode_override: None,
        backend: None,
        binding: None,
        placement: None,
    }
}

fn definition() -> MobDefinition {
    let toml = format!(
        "[mob]\nid = \"{MOB_ID}\"\n\n[profiles.{PROFILE}]\nmodel = \"gpt-5.5\"\n\
         external_addressable = true\nruntime_mode = \"turn_driven\"\n\n\
         [profiles.{PROFILE}.tools]\ncomms = true\n"
    );
    MobDefinition::from_toml(&toml).expect("parse the corpus mob definition")
}

/// The roster an application serves, replaceable between boots.
#[derive(Default)]
struct AppRoster {
    specs: Mutex<Vec<DurableAgentSpec>>,
}

impl AppRoster {
    fn set(&self, names: &[&str]) {
        *self.specs.lock().expect("roster lock") = names.iter().map(|name| spec(name)).collect();
    }
}

#[async_trait]
impl RosterProvider for AppRoster {
    async fn roster(&self, _context: &RosterContext) -> Result<Vec<DurableAgentSpec>, RosterError> {
        Ok(self.specs.lock().expect("roster lock").clone())
    }
}

/// A scripted model: answers "app reply <n>" with normalized accounting.
#[derive(Clone, Default)]
struct ScriptedModel {
    calls: Arc<AtomicU64>,
}

impl ScriptedModel {
    fn calls(&self) -> u64 {
        self.calls.load(Ordering::SeqCst)
    }

    /// The input after the last assistant message: what this turn added.
    fn new_input(request: &LlmRequest) -> String {
        let from = request
            .messages
            .iter()
            .rposition(|message| matches!(message, meerkat_core::Message::BlockAssistant(_)))
            .map_or(0, |last_reply| last_reply + 1);
        request.messages[from..]
            .iter()
            .filter_map(|message| serde_json::to_string(message).ok())
            .collect()
    }
}

#[async_trait]
impl LlmClient for ScriptedModel {
    fn project_replay_messages(
        &self,
        messages: &[meerkat_core::Message],
    ) -> Result<Vec<meerkat_core::Message>, LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
    ) -> std::pin::Pin<Box<dyn futures::Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>>
    {
        let n = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        let input_tokens = if Self::new_input(request).contains(COMPACT_MARKER) {
            COMPACT_THRESHOLD + 10_000
        } else {
            10
        };
        let usage = Usage {
            input_tokens,
            output_tokens: 1,
            ..Usage::default()
        };
        let [usage, done] =
            llm_usage::usage_then_done_with(request, Provider::OpenAI, usage, StopReason::EndTurn);
        Box::pin(async_stream::stream! {
            yield Ok(LlmEvent::TextDelta { delta: format!("app reply {n}"), meta: None });
            yield Ok(usage);
            yield Ok(done);
        })
    }

    fn provider(&self) -> Provider {
        Provider::OpenAI
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }
}

/// Every operation of the run, one JSON object per line.
struct Events {
    out: File,
}

impl Events {
    fn record(
        &mut self,
        boot: u32,
        op: &str,
        identity: Option<&str>,
        outcome: Result<Value, String>,
    ) {
        let mut row = json!({ "boot": boot, "op": op, "identity": identity });
        match outcome {
            Ok(result) => row["result"] = result,
            Err(error) => row["error"] = Value::String(error),
        }
        writeln!(self.out, "{row}").expect("write an event");
        self.out.flush().expect("flush events");
        println!("[boot {boot}] {op} {}: {}", identity.unwrap_or("-"), row);
    }
}

struct App {
    roster: Arc<AppRoster>,
    model: ScriptedModel,
    with_roster: bool,
    /// The persistent mob store's file name inside the state directory, if any.
    durable_mob: Option<&'static str>,
}

impl App {
    async fn boot(&self, state: &Path) -> Result<UnifiedRuntime, String> {
        let mut builder = UnifiedRuntimeBuilder::default()
            .definition(definition())
            .persistent_state(state)
            .comms(true)
            .compaction(meerkat_core::config::CompactionRuntimeConfig {
                auto_compact_threshold: COMPACT_THRESHOLD,
                auto_compact_threshold_explicit: true,
                recent_turn_budget: 1,
                ..Default::default()
            })
            .default_llm_client(Arc::new(self.model.clone()));
        if self.with_roster {
            builder = builder
                .roster_provider(self.roster.clone())
                .identity_bootstrap_mode(IdentityBootstrapMode::EagerMaterialize);
        }
        if let Some(file_name) = self.durable_mob {
            let (storage, provenance) = meerkat_meta::mob_composition_manifest::persistent_mob_storage(
                state.join(file_name),
            )
            .map_err(|error| error.to_string())?;
            builder = builder.mob_storage(storage, provenance);
        }
        Box::pin(builder.build())
            .await
            .map_err(|error| error.to_string())
    }

    /// Wait until the model saw `at_least` calls, then let the answer commit.
    async fn settle(&self, at_least: u64) -> Result<(), String> {
        let deadline = Instant::now() + TURN_DEADLINE;
        while self.model.calls() < at_least {
            if Instant::now() > deadline {
                return Err(format!(
                    "no model call #{at_least} within {TURN_DEADLINE:?}"
                ));
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        tokio::time::sleep(Duration::from_millis(700)).await;
        Ok(())
    }
}

async fn status(runtime: &UnifiedRuntime, events: &mut Events, boot: u32, name: &str) {
    let identities = runtime.identity_runtime().expect("identity runtime");
    let outcome = identities
        .status(&identity(name))
        .await
        .map(|status| serde_json::to_value(status).expect("status json"))
        .map_err(|error| error.to_string());
    events.record(boot, "status", Some(name), outcome);
}

async fn turn(
    app: &App,
    runtime: &UnifiedRuntime,
    events: &mut Events,
    boot: u32,
    name: &str,
    text: &str,
) {
    let identities = runtime.identity_runtime().expect("identity runtime");
    let before = app.model.calls();
    let sent = identities
        .send(&identity(name), &ContentInput::Text(text.to_string()))
        .await;
    let outcome = match sent {
        Ok(token) => app
            .settle(before + 1)
            .await
            .map(|()| json!({ "text": text, "fencing_token": format!("{token:?}") })),
        Err(error) => Err(error.to_string()),
    };
    events.record(boot, "turn", Some(name), outcome);
}

async fn shutdown(runtime: UnifiedRuntime, events: &mut Events, boot: u32) {
    let report = runtime.shutdown().await;
    events.record(boot, "shutdown", None, Ok(json!(format!("{report:?}"))));
    drop(runtime);
}

const IDENTITIES: [&str; 6] = [
    "app:app-01",
    "app:app-02",
    "app:app-03",
    "app:app-04",
    "app:app-05",
    "app:app-06",
];

async fn run_roster(app: &App, state: &Path, events: &mut Events) -> Result<(), Box<dyn Error>> {
    // Boot 1: every identity takes a turn; app-01 compacts; app-03 is
    // retired and woken; app-04 is reset; app-05 is deleted and recreated.
    app.roster.set(&IDENTITIES);
    let runtime = app.boot(state).await?;
    events.record(1, "build", None, Ok(json!("ok")));
    for name in IDENTITIES {
        status(&runtime, events, 1, name).await;
        turn(
            app,
            &runtime,
            events,
            1,
            name,
            &format!("boot 1 hello from the app to {name}"),
        )
        .await;
    }
    turn(
        app,
        &runtime,
        events,
        1,
        "app:app-01",
        &format!("{COMPACT_MARKER} boot 1 long context for app:app-01"),
    )
    .await;
    turn(
        app,
        &runtime,
        events,
        1,
        "app:app-01",
        "boot 1 after the compaction for app:app-01",
    )
    .await;
    let identities = runtime.identity_runtime().expect("identity runtime");
    let retired = identities.retire(&identity("app:app-03")).await;
    events.record(
        1,
        "retire",
        Some("app:app-03"),
        retired
            .map(|t| json!(format!("{t:?}")))
            .map_err(|e| e.to_string()),
    );
    turn(
        app,
        &runtime,
        events,
        1,
        "app:app-03",
        "boot 1 wake after retire for app:app-03",
    )
    .await;
    let reset = identities.reset(&identity("app:app-04")).await;
    events.record(
        1,
        "reset",
        Some("app:app-04"),
        reset
            .map(|record| serde_json::to_value(record).expect("record json"))
            .map_err(|e| e.to_string()),
    );
    turn(
        app,
        &runtime,
        events,
        1,
        "app:app-04",
        "boot 1 after the reset for app:app-04",
    )
    .await;
    let deleted = identities.delete_identity(&identity("app:app-05")).await;
    events.record(
        1,
        "delete_identity",
        Some("app:app-05"),
        deleted.map(|()| json!("ok")).map_err(|e| e.to_string()),
    );
    turn(
        app,
        &runtime,
        events,
        1,
        "app:app-05",
        "boot 1 after the delete for app:app-05",
    )
    .await;
    for name in IDENTITIES {
        status(&runtime, events, 1, name).await;
    }
    drop(identities);
    shutdown(runtime, events, 1).await;

    // Boot 2: the same roster; resumed turns.
    let runtime = app.boot(state).await?;
    events.record(2, "build", None, Ok(json!("ok")));
    for name in IDENTITIES {
        status(&runtime, events, 2, name).await;
    }
    for name in ["app:app-01", "app:app-02", "app:app-03"] {
        turn(
            app,
            &runtime,
            events,
            2,
            name,
            &format!("boot 2 follow-up for {name}"),
        )
        .await;
    }
    shutdown(runtime, events, 2).await;

    // Boot 3: the roster drops app-06.
    app.roster.set(&IDENTITIES[..5]);
    let runtime = app.boot(state).await?;
    events.record(3, "build", None, Ok(json!("ok")));
    for name in IDENTITIES {
        status(&runtime, events, 3, name).await;
    }
    turn(
        app,
        &runtime,
        events,
        3,
        "app:app-01",
        "boot 3 follow-up for app:app-01",
    )
    .await;
    shutdown(runtime, events, 3).await;
    Ok(())
}

async fn run_plain(app: &App, state: &Path, events: &mut Events) -> Result<(), Box<dyn Error>> {
    // No roster provider: the application spawns members itself and drives
    // their turns through the mob, as the plain persistent arm does.
    for boot in 1..=2u32 {
        let runtime = app.boot(state).await?;
        events.record(boot, "build", None, Ok(json!("ok")));
        for member in ["worker-1", "worker-2"] {
            let spawned = runtime
                .spawn(meerkat_mob::SpawnMemberSpec::new(PROFILE, member))
                .await;
            events.record(
                boot,
                "spawn",
                Some(member),
                spawned
                    .map(|result| json!(format!("{result:?}")))
                    .map_err(|e| e.to_string()),
            );
            let admitted = runtime
                .start_member_turn(
                    member,
                    ContentInput::Text(format!("boot {boot} hello to {member}")),
                    meerkat_core::types::HandlingMode::Queue,
                    meerkat_mob::MemberTurnOptions::default(),
                    None,
                )
                .await;
            let outcome = match admitted {
                Ok(admission) => {
                    let session_id = admission.session_id.clone();
                    match tokio::time::timeout(TURN_DEADLINE, admission.turn.wait()).await {
                        Ok(Ok(result)) => {
                            Ok(json!({ "session_id": session_id, "result": format!("{result:?}") }))
                        }
                        Ok(Err(error)) => Err(format!("session {session_id}: {error}")),
                        Err(_) => Err(format!(
                            "session {session_id}: no terminal within {TURN_DEADLINE:?}"
                        )),
                    }
                }
                Err(error) => Err(error.to_string()),
            };
            events.record(boot, "turn", Some(member), outcome);
        }
        shutdown(runtime, events, boot).await;
    }
    Ok(())
}

fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = std::env::args().collect();
    let (variant, work) = match args.as_slice() {
        [_, variant, work] => (variant.clone(), PathBuf::from(work)),
        _ => {
            return Err(
                "usage: s2-mint-093 <roster|roster-durable|roster-durable-mob-sqlite|plain> <work-dir>"
                    .into(),
            );
        }
    };
    std::fs::create_dir(&work)?;
    let state = work.join("state");
    let mut events = Events {
        out: File::create_new(work.join("events.jsonl"))?,
    };
    let app = App {
        roster: Arc::new(AppRoster::default()),
        model: ScriptedModel::default(),
        with_roster: variant.starts_with("roster"),
        durable_mob: match variant.as_str() {
            "roster-durable" => Some("event_log.sqlite3"),
            "roster-durable-mob-sqlite" => Some("mob.sqlite3"),
            _ => None,
        },
    };
    match variant.as_str() {
        "roster" | "roster-durable" | "roster-durable-mob-sqlite" => {
            run_roster(&app, &state, &mut events).await?
        }
        "plain" => run_plain(&app, &state, &mut events).await?,
        other => return Err(format!("unknown variant {other}").into()),
    }
    // Restart probe: 0.9.3 builds once more on a copy of the final state.
    let probe_state = work.join("probe-state");
    copy_dir(&state, &probe_state)?;
    let probe = match app.boot(&probe_state).await {
        Ok(runtime) => {
            let report = runtime.shutdown().await;
            json!({ "started": true, "shutdown": format!("{report:?}") })
        }
        Err(error) => json!({ "started": false, "error": error }),
    };
    std::fs::write(
        work.join("probe.json"),
        serde_json::to_string_pretty(&probe)? + "\n",
    )?;
    println!("probe: {probe}");
    Ok(())
}
