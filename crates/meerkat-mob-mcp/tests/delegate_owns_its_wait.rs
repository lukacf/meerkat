//! `delegate` owns its wait.
//!
//! A `delegate` call that ends without its helper's terminal result, for any
//! reason, cancels and retires its helper: by its own limit (`max_run_secs`,
//! or the call's outside bound), or because the call's future was dropped
//! (the calling turn was interrupted). Where somebody still listens, the
//! answer is a typed tool error (`data.kind` is `delegate_helper_ended`) that
//! names the helper, its mob and its session and says what became of it.
//! The cancel reaches exactly the helper this call seated.
//!
//! Every `delegate` here is a real tool call of a real member turn, over the
//! production runtime-backed composition; only the model is scripted.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

mod support;

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use meerkat_client::LlmRequest;
use meerkat_core::types::{ContentInput, HandlingMode, ToolCallView, ToolDef, ToolResult};
use meerkat_core::{AgentToolDispatcher, Message, SessionId, ToolDispatchOutcome, ToolError};
use meerkat_mob::{
    AgentIdentity, BoundedResultSpec, MobBackendKind, MobControlPrincipal, MobDefinition,
    MobHandle, MobId, MobRuntimeMode, ProfileBinding, ProfileName, WorkOrigin, WorkSpec,
    WorkTurnHandle,
};
use meerkat_mob_mcp::MobMcpState;
use serde_json::{Value, json};
use support::{ScriptedCouncilClient, ScriptedTurn, TurnGate, last_user_text, participant_profile};

/// The bound every wait of a row sits under: a hang guard, far above what a
/// passing row needs.
const ROW_BOUND: Duration = Duration::from_mins(1);

/// A host tool that never answers: a helper's slow tool.
const HOST_WAIT: &str = "host_wait";

/// A host tool that answers after the `ms` its call names.
const HOST_PAUSE: &str = "host_pause";

/// The marker that opens the tool calls a scripted turn makes, and the one
/// that closes them.
const CALLS: &str = "DT-CALLS:";
const CALLS_END: &str = ":DT-CALLS-END";

/// What the scripted model of every session does, told apart by the text a
/// turn was given:
///
/// - `DT-CALLS:<json array of {id, name, args}>:DT-CALLS-END` makes those
///   tool calls in one batch, records each result under its call id, and
///   answers `done <json of the results>`;
/// - `DT-HANG:<name>:` blocks in the model call until the gate `name` opens,
///   then answers `late answer <name>`;
/// - `DT-FAIL` fails the model call;
/// - anything else answers `quick answer`.
#[derive(Clone, Default)]
struct Script {
    gates: Arc<Mutex<HashMap<String, Arc<TurnGate>>>>,
    results: Arc<Mutex<BTreeMap<String, String>>>,
}

impl Script {
    fn gate(&self, name: &str) -> Arc<TurnGate> {
        Arc::clone(
            self.gates
                .lock()
                .unwrap()
                .entry(name.to_string())
                .or_insert_with(TurnGate::new),
        )
    }

    fn turn(&self, request: &LlmRequest) -> ScriptedTurn {
        let user = last_user_text(request);
        if let (Some(start), Some(end)) = (user.find(CALLS), user.rfind(CALLS_END)) {
            let calls: Vec<Value> =
                serde_json::from_str(&user[start + CALLS.len()..end]).expect("scripted calls");
            let seen = tool_results(&request.messages);
            let ids: Vec<String> = calls
                .iter()
                .map(|call| call["id"].as_str().expect("call id").to_string())
                .collect();
            if ids.iter().all(|id| seen.contains_key(id)) {
                let mut results = self.results.lock().unwrap();
                let mut answered = serde_json::Map::new();
                for id in ids {
                    results.insert(id.clone(), seen[&id].clone());
                    answered.insert(id.clone(), Value::String(seen[&id].clone()));
                }
                return ScriptedTurn::Text(format!("done {}", Value::Object(answered)));
            }
            return ScriptedTurn::ToolCalls(
                calls
                    .into_iter()
                    .map(|call| {
                        (
                            call["id"].as_str().expect("call id").to_string(),
                            call["name"].as_str().expect("call name").to_string(),
                            call["args"].clone(),
                        )
                    })
                    .collect(),
            );
        }
        if let Some(start) = user.find("DT-HANG:") {
            let name = user[start + "DT-HANG:".len()..]
                .split(':')
                .next()
                .expect("gate name");
            return ScriptedTurn::Gated(self.gate(name), format!("late answer {name}"));
        }
        if user.contains("DT-FAIL") {
            return ScriptedTurn::Fail("the provider refused the request".to_string());
        }
        ScriptedTurn::Text("quick answer".to_string())
    }
}

/// A task whose helper blocks until the gate `name` opens.
fn hang(name: &str) -> String {
    format!("DT-HANG:{name}:")
}

/// A turn text (or a helper task) that makes `calls` in one batch.
fn calls_text(calls: &[Value]) -> String {
    format!("{CALLS}{}{CALLS_END}", Value::Array(calls.to_vec()))
}

/// The tool-result text of each call in `messages`, keyed by call id.
fn tool_results(messages: &[Message]) -> BTreeMap<String, String> {
    messages
        .iter()
        .filter_map(|message| match message {
            Message::ToolResults { results, .. } => Some(results),
            _ => None,
        })
        .flatten()
        .map(|result| (result.tool_use_id.clone(), result.text_content()))
        .collect()
}

/// The `tooling` of a helper on an inline profile that switches on
/// `families` (plus comms, which every mob member needs).
fn inline_tooling(families: &[&str]) -> Value {
    let mut tools = serde_json::Map::new();
    tools.insert("comms".to_string(), true.into());
    for family in families {
        tools.insert((*family).to_string(), true.into());
    }
    json!({
        "mode": "profile",
        "source": {
            "type": "inline",
            "model": "claude-haiku-4-5-20251001",
            "tools": tools
        }
    })
}

/// One `delegate` call, under the call id `id`, for the helper `member_id`.
fn delegate_call(id: &str, task: &str, member_id: &str, max_run_secs: Option<u64>) -> Value {
    let mut args = json!({
        "task": task,
        "member_id": member_id,
        "result_label": "helper_result",
        "max_text_bytes": 8192,
        "tooling": inline_tooling(&[]),
    });
    if let Some(max_run_secs) = max_run_secs {
        args["max_run_secs"] = json!(max_run_secs);
    }
    json!({ "id": id, "name": "delegate", "args": args })
}

/// [`delegate_call`] whose helper mounts the agent mob tools too, so that it
/// can delegate itself.
fn delegating_helper_call(
    id: &str,
    task: &str,
    member_id: &str,
    max_run_secs: Option<u64>,
) -> Value {
    let mut call = delegate_call(id, task, member_id, max_run_secs);
    call["args"]["tooling"] = inline_tooling(&["mob"]);
    call
}

/// The host's own tools, offered to every member: `host_wait` never
/// answers, `host_pause` answers after the time its call names. Counts the
/// `host_wait` calls that entered it and those whose future was dropped.
struct HostWait {
    entered: Arc<AtomicUsize>,
    dropped: Arc<AtomicUsize>,
}

struct CountDrop(Arc<AtomicUsize>);

impl Drop for CountDrop {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[async_trait::async_trait]
impl AgentToolDispatcher for HostWait {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        let host_tool = |name: &str, description: &str, schema: Value| {
            Arc::new(ToolDef::new(name, description, schema).with_provenance(
                meerkat_core::ToolProvenance {
                    kind: meerkat_core::ToolSourceKind::Callback,
                    source_id: name.into(),
                    artifact: None,
                },
            ))
        };
        vec![
            host_tool(
                HOST_WAIT,
                "A host tool that never answers.",
                json!({"type": "object", "properties": {}}),
            ),
            host_tool(
                HOST_PAUSE,
                "A host tool that answers after `ms` milliseconds.",
                json!({
                    "type": "object",
                    "properties": {"ms": {"type": "integer", "minimum": 0}},
                    "required": ["ms"]
                }),
            ),
        ]
        .into()
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        if call.name == HOST_PAUSE {
            let args: Value = serde_json::from_str(call.args.get())
                .map_err(|error| ToolError::invalid_arguments(call.name, error.to_string()))?;
            let ms = args["ms"].as_u64().unwrap_or_default();
            tokio::time::sleep(Duration::from_millis(ms)).await;
            return Ok(ToolDispatchOutcome::sync_result(ToolResult::new(
                call.id.to_string(),
                format!("{{\"paused_ms\":{ms}}}"),
                false,
            )));
        }
        self.entered.fetch_add(1, Ordering::SeqCst);
        let _dropped = CountDrop(Arc::clone(&self.dropped));
        std::future::pending::<()>().await;
        Ok(ToolDispatchOutcome::sync_result(ToolResult::new(
            call.id.to_string(),
            "{}".to_string(),
            false,
        )))
    }

    fn capabilities(&self) -> meerkat_core::agent::DispatcherCapabilities {
        meerkat_core::agent::DispatcherCapabilities::default()
    }
}

/// One host mob with a member (`parent`) whose profile mounts the agent mob
/// tools, over the production runtime-backed composition.
struct Rig {
    state: Arc<MobMcpState>,
    mob_id: MobId,
    session: SessionId,
    script: Script,
    host_wait_entered: Arc<AtomicUsize>,
    host_wait_dropped: Arc<AtomicUsize>,
    _temp: tempfile::TempDir,
}

fn parent_definition(mob_id: &MobId) -> MobDefinition {
    let mut profile = participant_profile("delegating parent");
    profile.tools.mob = true;
    profile.external_addressable = true;
    let mut definition = MobDefinition::explicit(mob_id.clone());
    definition.profiles.insert(
        ProfileName::from("participant"),
        ProfileBinding::Inline(Box::new(profile)),
    );
    definition
}

fn turn_spec() -> BoundedResultSpec {
    BoundedResultSpec::new("turn", 8192).expect("bounded result spec")
}

impl Rig {
    async fn new() -> Self {
        Self::with_config(meerkat::Config::default()).await
    }

    async fn with_config(config: meerkat::Config) -> Self {
        Self::build(config, |state, _root| state).await
    }

    /// [`Self::new`] on a host that unloads idle mobs: its mobs are stored
    /// under a persistent root, and the state restores them on demand.
    async fn unloading_idle_mobs() -> Self {
        Self::build(meerkat::Config::default(), |state, root| {
            state
                .with_mob_restore_policy(meerkat_mob_mcp::MobRestorePolicy::OnDemand)
                .try_with_persistent_storage_root(Some(root.join("state")))
                .expect("a persistent mob root")
        })
        .await
    }

    async fn build(
        config: meerkat::Config,
        configure: impl FnOnce(MobMcpState, &std::path::Path) -> MobMcpState,
    ) -> Self {
        let temp = tempfile::tempdir().expect("temp dir");
        let root = temp.path().to_path_buf();
        let script = Script::default();
        let model = script.clone();
        let client = Arc::new(ScriptedCouncilClient::new(move |request| {
            model.turn(request)
        }));
        let host_wait_entered = Arc::new(AtomicUsize::new(0));
        let host_wait_dropped = Arc::new(AtomicUsize::new(0));
        let host: Arc<dyn AgentToolDispatcher> = Arc::new(HostWait {
            entered: Arc::clone(&host_wait_entered),
            dropped: Arc::clone(&host_wait_dropped),
        });
        let state =
            support::agent_mob_tools_state_with_config(temp.path(), client, config, move |state| {
                let provider: meerkat_mob::ExternalToolsProvider =
                    Arc::new(move || Some(Arc::clone(&host)));
                configure(state.with_external_tools_provider(Some(provider)), &root)
            });
        let mob_id = MobId::from(format!("delegating-{}", uuid::Uuid::new_v4().simple()));
        state
            .mob_create_definition(parent_definition(&mob_id))
            .await
            .expect("create the host mob");
        state
            .mob_spawn(
                &mob_id,
                ProfileName::from("participant"),
                AgentIdentity::from("parent"),
                Some(MobRuntimeMode::TurnDriven),
                Some(MobBackendKind::Session),
                None,
            )
            .await
            .expect("spawn the parent");
        let session = state
            .handle_for(&mob_id)
            .await
            .expect("host mob handle")
            .resolve_bridge_session_id(&AgentIdentity::from("parent"))
            .await
            .expect("the parent has a bridge session");
        Self {
            state,
            mob_id,
            session,
            script,
            host_wait_entered,
            host_wait_dropped,
            _temp: temp,
        }
    }

    async fn host(&self) -> MobHandle {
        self.state.handle_for(&self.mob_id).await.expect("host mob")
    }

    /// The parent session's implicit mob, once a `delegate` has made it.
    async fn implicit_mob_id(&self) -> MobId {
        let deadline = Instant::now() + ROW_BOUND;
        loop {
            if let Some(mob_id) = self
                .state
                .find_implicit_mob_for_bridge_session(&self.session.to_string())
                .await
            {
                return mob_id;
            }
            assert!(
                Instant::now() < deadline,
                "the parent session got no implicit mob"
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    async fn implicit(&self) -> MobHandle {
        let mob_id = self.implicit_mob_id().await;
        self.state.handle_for(&mob_id).await.expect("implicit mob")
    }

    /// Start one turn of the parent member with `text`.
    async fn start_parent_turn(&self, text: &str) -> WorkTurnHandle {
        self.host()
            .await
            .start_work_for_identity_bounded(
                AgentIdentity::from("parent"),
                WorkSpec::new(ContentInput::Text(text.to_string()), WorkOrigin::Internal),
                HandlingMode::Queue,
                turn_spec(),
            )
            .await
            .expect("start a parent turn")
    }

    /// Start one turn of the parent member that makes `calls` in one batch.
    async fn parent_calls(&self, calls: &[Value]) -> WorkTurnHandle {
        self.start_parent_turn(&calls_text(calls)).await
    }

    /// The result the model that made the call `id` got back for it.
    async fn result_of(&self, id: &str) -> String {
        let started = Instant::now();
        loop {
            if let Some(result) = self.script.results.lock().unwrap().get(id).cloned() {
                return result;
            }
            assert!(
                started.elapsed() < ROW_BOUND,
                "call `{id}` got no result within {ROW_BOUND:?}"
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    async fn teardown(self) {
        let handles = self.state.mob_handles_snapshot().await.unwrap_or_default();
        for (mob_id, handle) in handles {
            if self.state.mob_destroy(&mob_id).await.is_err() {
                let _ = handle.shutdown().await;
            }
        }
    }
}

/// Wait for `turn` to end and answer its final text.
async fn finish(turn: WorkTurnHandle, what: &str) -> String {
    let Ok(ended) = tokio::time::timeout(ROW_BOUND, turn.wait_bounded(turn_spec())).await else {
        panic!("{what} did not end within {ROW_BOUND:?}");
    };
    match ended {
        Ok(turn) => turn.result().result().text().to_string(),
        Err(error) => panic!("{what} failed: {error:?}"),
    }
}

async fn is_member(mob: &MobHandle, member: &str) -> bool {
    mob.get_member(&AgentIdentity::from(member))
        .await
        .expect("read the roster")
        .is_some()
}

/// Wait until `member` is no member of `mob`, and answer how long it took.
async fn wait_gone(mob: &MobHandle, member: &str, why: &str) -> Duration {
    let started = Instant::now();
    while is_member(mob, member).await {
        assert!(
            started.elapsed() < ROW_BOUND,
            "helper `{member}` is still a member of its mob after {:?}: {why}",
            started.elapsed()
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    started.elapsed()
}

/// Wait until `mob` reads no foreground owner: a run that ended holds its
/// mob no longer. Polled, since the run's task lets go of its lease just
/// after it answered; a lease that leaked never reads none.
async fn wait_no_foreground_owner(mob: &MobHandle, why: &str) {
    let started = Instant::now();
    loop {
        let observed = mob.observe_idle().await.expect("an idle observation");
        if observed.busy.foreground_owners == 0 {
            return;
        }
        assert!(
            started.elapsed() < ROW_BOUND,
            "the mob still reads a foreground owner {:?} after the run ended ({why}): \
             {observed:?}",
            started.elapsed()
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// The typed data of a `delegate` error, from the result the model saw.
fn error_data(result: &str) -> Value {
    let payload: Value = serde_json::from_str(result)
        .unwrap_or_else(|error| panic!("a delegate error is JSON ({error}): {result}"));
    assert_eq!(
        payload["error"], "execution_failed",
        "a delegate that ends without its helper's result answers execution_failed: {result}"
    );
    assert!(
        payload["data"].is_object(),
        "the delegate error carries typed data: {result}"
    );
    payload["data"].clone()
}

/// The wait a call reports under an outside bound: what the bound allows
/// (`allowed`, in whole seconds), less what the call used of the bound
/// before it reached the tool. `spent_at_most` is the row's own measure of
/// that: the time from before the calling turn was started to a moment at
/// which the call was in the tool already.
fn assert_wait_secs(data: &Value, allowed: u64, spent_at_most: Duration) {
    let reported = data["max_run_secs"]
        .as_u64()
        .unwrap_or_else(|| panic!("max_run_secs: {data}"));
    let least = allowed.saturating_sub(spent_at_most.as_secs() + 1);
    assert!(
        (least..=allowed).contains(&reported),
        "the wait is {allowed} s, less at most the {spent_at_most:?} the call used before the \
         tool: {data}"
    );
}

fn assert_helper_ended(data: &Value, reason: &str, helper: &str) {
    assert_eq!(data["kind"], "delegate_helper_ended", "{data}");
    assert_eq!(data["reason"], reason, "{data}");
    assert_eq!(data["helper"], helper, "{data}");
}

/// The call's own limit. The helper's run is cancelled, the helper is
/// retired, and the error names the helper, its mob, its session and what
/// was done. The calling turn goes on.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_delegate_past_its_max_run_cancels_and_retires_its_helper_and_names_it() {
    let rig = Rig::new().await;
    let turn = rig
        .parent_calls(&[delegate_call("call", &hang("slow"), "slow", Some(5))])
        .await;
    let result = rig.result_of("call").await;
    let data = error_data(&result);
    assert_helper_ended(&data, "max_run_elapsed", "cancelled_and_retired");

    let implicit_id = rig.implicit_mob_id().await;
    assert_eq!(data["mob_id"], implicit_id.as_str(), "{data}");
    assert_eq!(data["member_id"], "slow", "{data}");
    assert!(
        data["member_ref"].as_str().is_some_and(|r| !r.is_empty()),
        "{data}"
    );
    assert!(
        data["session_id"]
            .as_str()
            .is_some_and(|id| SessionId::parse(id).is_ok()),
        "the error names the helper's session: {data}"
    );
    assert_eq!(data["max_run_secs"], 5, "{data}");
    assert_eq!(data["retryable"], false, "{data}");
    assert!(data.get("max_run_limited_by").is_none(), "{data}");
    for needle in ["slow", implicit_id.as_str(), "cancelled and retired"] {
        assert!(result.contains(needle), "{needle} missing from: {result}");
    }
    assert!(
        !is_member(&rig.implicit().await, "slow").await,
        "the helper is retired when the call answers"
    );
    assert_eq!(rig.script.gate("slow").entered(), 1);
    assert!(
        finish(turn, "the calling turn").await.starts_with("done "),
        "a delegate that ends without a result does not fail the calling turn"
    );
    rig.teardown().await;
}

/// The incident: the helper is stuck in a tool call of its own when the
/// limit fires. Its run is ended all the same (the tool call is dropped),
/// the helper is retired, and the answer is the typed one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_helper_stuck_in_a_tool_call_is_ended_at_the_limit() {
    let rig = Rig::new().await;
    let helper_task = calls_text(&[json!({ "id": "helper-wait", "name": HOST_WAIT, "args": {} })]);
    let started = Instant::now();
    let turn = rig
        .parent_calls(&[delegate_call("call", &helper_task, "stuck", Some(5))])
        .await;
    let result = rig.result_of("call").await;
    let answered = started.elapsed();
    let data = error_data(&result);
    assert_helper_ended(&data, "max_run_elapsed", "cancelled_and_retired");
    assert!(!is_member(&rig.implicit().await, "stuck").await);
    assert_eq!(
        rig.host_wait_entered.load(Ordering::SeqCst),
        1,
        "the helper was in its tool call"
    );
    let dropped = Instant::now();
    while rig.host_wait_dropped.load(Ordering::SeqCst) == 0 {
        assert!(
            dropped.elapsed() < ROW_BOUND,
            "the helper's tool call was not ended"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    eprintln!(
        "DT-STUCK-TOOL: the call with max_run_secs 5 answered after {answered:?}; the helper's \
         tool call was dropped {:?} after the answer was read",
        dropped.elapsed()
    );
    finish(turn, "the calling turn").await;
    rig.teardown().await;
}

/// Interrupting the calling turn while its `delegate` waits drops the
/// call's future. Nobody is left to answer; the helper is cancelled and
/// retired all the same.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_caller_whose_turn_is_interrupted_leaves_no_helper_seated() {
    let rig = Rig::new().await;
    let turn = rig
        .parent_calls(&[delegate_call("call", &hang("orphan"), "orphan", None)])
        .await;
    rig.script.gate("orphan").wait_entered(1).await;
    let implicit = rig.implicit().await;
    assert!(is_member(&implicit, "orphan").await, "the helper is seated");

    let cancelled = Instant::now();
    // The hard interrupt: the mob's force cancel is a cancel after the
    // run's next boundary, which a turn inside a tool call does not reach.
    rig.host()
        .await
        .hard_cancel_member(
            MobControlPrincipal::Owner,
            AgentIdentity::from("parent"),
            "the row interrupts the calling turn",
        )
        .await
        .expect("interrupt the calling turn");
    let ended = tokio::time::timeout(ROW_BOUND, turn.wait_bounded(turn_spec()))
        .await
        .expect("the interrupted turn ends");
    let turn_ended = cancelled.elapsed();
    assert!(
        ended.is_err(),
        "the calling turn was interrupted: {ended:?}"
    );
    let waited = wait_gone(
        &implicit,
        "orphan",
        "the turn that delegated to it was interrupted",
    )
    .await;
    eprintln!(
        "DT-INTERRUPT: the cancelled calling turn ended {turn_ended:?} after the cancel; the \
         helper left its roster {waited:?} after that"
    );
    rig.teardown().await;
}

/// A pin of what is not closed: the mob's force cancel is a cancel after the
/// run's next boundary, and a turn that waits inside `delegate` reaches no
/// boundary until the call returns. A waiting tool cannot see the pending
/// cancel, so the call ends at its own wait: only then is the helper
/// cancelled and retired, and the caller's turn cancelled.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_force_cancelled_caller_waits_inside_delegate_until_the_calls_wait_ends() {
    let mut config = meerkat::Config::default();
    // The call's wait: 12 s less its quarter.
    config.tools.default_timeout = Duration::from_secs(12);
    let rig = Rig::with_config(config).await;
    let turn = rig
        .parent_calls(&[delegate_call("call", &hang("waited"), "waited", None)])
        .await;
    rig.script.gate("waited").wait_entered(1).await;
    let implicit = rig.implicit().await;

    let cancelled = Instant::now();
    let host = rig.host().await;
    let cancel = tokio::spawn(async move {
        host.force_cancel_member(AgentIdentity::from("parent"))
            .await
    });
    // Well after the cancel was asked, the call still waits and its helper
    // is still seated.
    tokio::time::sleep(Duration::from_secs(4)).await;
    assert!(
        is_member(&implicit, "waited").await,
        "the helper is seated while the call's wait runs"
    );
    assert!(rig.script.results.lock().unwrap().is_empty());

    let ended = tokio::time::timeout(ROW_BOUND, turn.wait_bounded(turn_spec()))
        .await
        .expect("the cancelled turn ends once the call's wait has ended");
    let turn_ended = cancelled.elapsed();
    assert!(ended.is_err(), "the calling turn was cancelled: {ended:?}");
    assert!(
        turn_ended >= Duration::from_secs(7),
        "the turn ended only when the call's wait of 9 s had ended: {turn_ended:?}"
    );
    let waited = wait_gone(&implicit, "waited", "its call's wait has ended").await;
    let cancel_answer = tokio::time::timeout(ROW_BOUND, cancel)
        .await
        .expect("the force cancel answers")
        .expect("the force cancel's task");
    eprintln!(
        "DT-FORCE-CANCEL: asked while the call waited (wait 9 s); the calling turn ended \
         {turn_ended:?} after the cancel was asked; the helper was off its roster {waited:?} \
         after that; the force cancel answered {cancel_answer:?}"
    );
    rig.teardown().await;
}

/// Wherever the calling turn is interrupted (before the helper is
/// seated, between its seat and its turn's admission, or after), no helper
/// stays seated.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_caller_interrupted_at_any_point_leaves_no_helper_seated() {
    let rig = Rig::new().await;
    // The implicit mob exists from the first call on.
    let first = rig
        .parent_calls(&[delegate_call("first", "say hello", "first", None)])
        .await;
    finish(first, "the first calling turn").await;
    let implicit = rig.implicit().await;

    for delay_ms in [0_u64, 2, 5, 10, 20, 40, 80, 160, 320, 640] {
        let member = format!("sweep-{delay_ms}");
        let call = format!("call-{delay_ms}");
        let turn = rig
            .parent_calls(&[delegate_call(&call, &hang(&member), &member, None)])
            .await;
        tokio::time::sleep(Duration::from_millis(delay_ms)).await;
        // An interrupt that finds no run yet is refused and leaves nothing
        // behind, so it is repeated until the turn has ended.
        let host = rig.host().await;
        let wait = turn.wait_bounded(turn_spec());
        tokio::pin!(wait);
        let landed = Instant::now();
        let ended = loop {
            let _ = host
                .hard_cancel_member(
                    MobControlPrincipal::Owner,
                    AgentIdentity::from("parent"),
                    "the row interrupts the calling turn",
                )
                .await;
            match tokio::time::timeout(Duration::from_millis(20), &mut wait).await {
                Ok(ended) => break ended,
                Err(_) => assert!(
                    landed.elapsed() < ROW_BOUND,
                    "the turn cancelled after {delay_ms} ms did not end"
                ),
            }
        };
        assert!(ended.is_err(), "interrupted after {delay_ms} ms: {ended:?}");
        wait_gone(
            &implicit,
            &member,
            &format!("its calling turn was interrupted after {delay_ms} ms"),
        )
        .await;
    }
    // A run whose calling turn was interrupted before its helper was seated
    // seats it afterwards and ends it then: the check above passes at once
    // for a helper that is not on the roster yet. Every run has had time to
    // seat and end its helper by now, and none is left.
    tokio::time::sleep(Duration::from_secs(5)).await;
    let left: Vec<String> = implicit
        .list_members()
        .await
        .into_iter()
        .map(|member| member.agent_identity.to_string())
        .filter(|member| member.starts_with("sweep-"))
        .collect();
    assert!(
        left.is_empty(),
        "helpers of interrupted calls are still seated: {left:?}"
    );
    rig.teardown().await;
}

/// The helper finishes as the limit fires. The limit was observed first,
/// but the helper's turn completed before the cancel landed: the call is
/// answered the completed result, and the helper is retired as after any
/// completed call.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_helper_that_finishes_as_the_limit_fires_is_answered_its_result() {
    let rig = Rig::new().await;
    let (reached, release) =
        meerkat_mob::hold_delegation_limit_for_test(AgentIdentity::from("photo-finish"));
    let turn = rig
        .parent_calls(&[delegate_call(
            "call",
            &hang("photo-finish"),
            "photo-finish",
            Some(5),
        )])
        .await;
    tokio::time::timeout(ROW_BOUND, reached)
        .await
        .expect("the run reaches its limit")
        .expect("the run is held at its limit");
    // The helper's turn completes while the run is held before its cancel.
    rig.script.gate("photo-finish").open();
    let implicit = rig.implicit().await;
    let finished = Instant::now();
    loop {
        let status = implicit
            .member_status(&AgentIdentity::from("photo-finish"))
            .await
            .expect("helper status");
        if status
            .output_preview
            .as_deref()
            .is_some_and(|text| text.contains("late answer"))
        {
            break;
        }
        assert!(
            finished.elapsed() < ROW_BOUND,
            "the helper's turn did not complete: {status:?}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let _ = release.send(());

    let result = rig.result_of("call").await;
    let payload: Value = serde_json::from_str(&result).expect("delegate result JSON");
    assert!(
        payload.get("error").is_none(),
        "a turn that completed before the cancel is answered its result: {result}"
    );
    assert!(
        payload["output"]
            .as_str()
            .is_some_and(|output| output.contains("late answer photo-finish")),
        "{result}"
    );
    assert!(
        !is_member(&implicit, "photo-finish").await,
        "the helper is retired"
    );
    finish(turn, "the calling turn").await;
    rig.teardown().await;
}

/// A host that unloads idle mobs, and the gap between a helper's end and its
/// retirement. The helper's turn completed while the run is held at its
/// limit, so nothing is in flight in the session's implicit mob when the
/// host's idle unload asks; the run still owes the helper's retirement. The
/// unload does not close the mob under the run: the call is answered the
/// completed result with no retirement owed, and the helper is retired.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_idle_unload_before_a_helpers_retirement_leaves_no_helper_seated() {
    let rig = Rig::unloading_idle_mobs().await;
    let helper = AgentIdentity::from("held-through-unload");
    let (reached, release) = meerkat_mob::hold_delegation_limit_for_test(helper.clone());
    let turn = rig
        .parent_calls(&[delegate_call(
            "call",
            &hang("held-through-unload"),
            "held-through-unload",
            Some(5),
        )])
        .await;
    tokio::time::timeout(ROW_BOUND, reached)
        .await
        .expect("the run reaches its limit")
        .expect("the run is held at its limit");
    // The helper's turn completes while the run is held before its cancel.
    rig.script.gate("held-through-unload").open();
    let implicit_id = rig.implicit_mob_id().await;
    let finished = Instant::now();
    loop {
        let status = rig
            .state
            .handle_for(&implicit_id)
            .await
            .expect("implicit mob")
            .member_status(&helper)
            .await
            .expect("helper status");
        if status
            .output_preview
            .as_deref()
            .is_some_and(|text| text.contains("late answer"))
        {
            break;
        }
        assert!(
            finished.elapsed() < ROW_BOUND,
            "the helper's turn did not complete: {status:?}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    // One pass of the host's idle unload while the run is held: the helper
    // is seated, its turn is over, and its retirement is still owed.
    let unloaded = rig.state.unload_idle_mobs_for_tests().await;
    let _ = release.send(());

    let result = rig.result_of("call").await;
    let payload: Value = serde_json::from_str(&result).expect("delegate result JSON");
    assert!(
        payload.get("error").is_none(),
        "a turn that completed before the cancel is answered its result: {result}"
    );
    assert!(
        payload["output"]
            .as_str()
            .is_some_and(|output| output.contains("late answer held-through-unload")),
        "{result}"
    );
    let implicit = rig.implicit().await;
    assert!(
        !is_member(&implicit, "held-through-unload").await,
        "the helper is retired. The unload pass unloaded {unloaded:?}; the call answered \
         retirement_error {}",
        payload["retirement_error"]
    );
    assert!(
        payload["retirement_error"].is_null(),
        "no retirement is owed: {result}"
    );
    assert!(
        !unloaded.contains(&implicit_id),
        "an idle unload leaves a mob whose bounded run still owes a retirement: {unloaded:?}"
    );
    finish(turn, "the calling turn").await;
    // The run has ended: it holds its mob no longer, and the host's idle
    // unload takes the mob once nothing of the retirement is in flight.
    wait_no_foreground_owner(&implicit, "the helper's turn completed").await;
    drop(implicit);
    let started = Instant::now();
    loop {
        if rig
            .state
            .unload_idle_mobs_for_tests()
            .await
            .contains(&implicit_id)
        {
            break;
        }
        assert!(
            started.elapsed() < ROW_BOUND,
            "the session's implicit mob was not unloaded after its run had ended"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    rig.teardown().await;
}

/// The owner lease of a bounded run, held and let go at the limit. While
/// the run waits for its helper the session's implicit mob reads one
/// foreground owner; once the run has ended at its limit and retired the
/// helper, it reads none.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_run_that_ended_at_its_limit_holds_its_mob_no_longer() {
    let rig = Rig::new().await;
    let turn = rig
        .parent_calls(&[delegate_call("call", &hang("leased"), "leased", Some(5))])
        .await;
    // The helper is inside its model call: the run is live.
    tokio::time::timeout(ROW_BOUND, rig.script.gate("leased").wait_entered(1))
        .await
        .expect("the helper's turn starts");
    let implicit = rig.implicit().await;
    let held = implicit.observe_idle().await.expect("an idle observation");
    assert_eq!(
        held.busy.foreground_owners, 1,
        "a live bounded run holds one owner lease of its mob: {held:?}"
    );
    let result = rig.result_of("call").await;
    assert_helper_ended(
        &error_data(&result),
        "max_run_elapsed",
        "cancelled_and_retired",
    );
    wait_no_foreground_owner(&implicit, "the run ended at its limit").await;
    assert!(
        finish(turn, "the calling turn").await.starts_with("done "),
        "a delegate that ends without a result does not fail the calling turn"
    );
    rig.teardown().await;
}

/// The owner lease of a run that was lost: the guard that ends the lost
/// run's helper carries the lease, and lets it go once the helper is ended.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_lost_run_holds_its_mob_no_longer_once_its_helper_is_ended() {
    let rig = Rig::new().await;
    meerkat_mob::lose_delegation_run_for_test(AgentIdentity::from("lost-leased"));
    let turn = rig
        .parent_calls(&[delegate_call(
            "call",
            &hang("lost-leased"),
            "lost-leased",
            None,
        )])
        .await;
    let result = rig.result_of("call").await;
    assert_helper_ended(&error_data(&result), "run_lost", "unknown");
    let implicit = rig.implicit().await;
    wait_gone(&implicit, "lost-leased", "the run that seated it was lost").await;
    wait_no_foreground_owner(&implicit, "the lost run's guard ended its helper").await;
    assert!(
        finish(turn, "the calling turn").await.starts_with("done "),
        "a lost run does not fail the calling turn"
    );
    rig.teardown().await;
}

/// The stale fence: the helper is gone when the limit fires, and
/// another member sits under its member id by then. The call says its
/// helper was already gone; the later member, its run and its roster entry
/// are untouched.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_later_member_under_the_helpers_id_is_untouched_when_the_limit_fires() {
    let rig = Rig::new().await;
    let reborn = AgentIdentity::from("reborn");
    let (reached, release) = meerkat_mob::hold_delegation_limit_for_test(reborn.clone());
    let turn = rig
        .parent_calls(&[delegate_call("call", &hang("first"), "reborn", Some(5))])
        .await;
    tokio::time::timeout(ROW_BOUND, reached)
        .await
        .expect("the run reaches its limit")
        .expect("the run is held at its limit");

    // While the run is held before its cancel: the helper is retired by
    // another hand, and a new member takes its id and starts a run.
    let implicit_id = rig.implicit_mob_id().await;
    let implicit = rig.implicit().await;
    let first = implicit
        .get_member(&reborn)
        .await
        .expect("roster")
        .expect("the helper is seated");
    implicit.retire(reborn.clone()).await.expect("retire");
    rig.state
        .mob_spawn(
            &implicit_id,
            ProfileName::from("delegate"),
            reborn.clone(),
            Some(MobRuntimeMode::TurnDriven),
            Some(MobBackendKind::Session),
            None,
        )
        .await
        .expect("a new member under the helper's id");
    let second = implicit
        .get_member(&reborn)
        .await
        .expect("roster")
        .expect("the later member is seated");
    assert_ne!(
        (first.generation, first.fence_token),
        (second.generation, second.fence_token),
        "the later member is another incarnation"
    );
    let second_turn = implicit
        .start_work_for_identity_bounded(
            reborn.clone(),
            WorkSpec::new(ContentInput::Text(hang("second")), WorkOrigin::Internal),
            HandlingMode::Queue,
            turn_spec(),
        )
        .await
        .expect("the later member's own run");
    rig.script.gate("second").wait_entered(1).await;
    let _ = release.send(());

    let data = error_data(&rig.result_of("call").await);
    assert_helper_ended(&data, "max_run_elapsed", "already_gone");

    let still = implicit
        .get_member(&reborn)
        .await
        .expect("roster")
        .expect("the later member is still seated");
    assert_eq!(
        (still.generation, still.fence_token),
        (second.generation, second.fence_token),
        "the later member's incarnation is the one that was spawned"
    );
    // Its run was not cancelled: it completes once its own gate opens.
    rig.script.gate("second").open();
    assert_eq!(
        finish(second_turn, "the later member's run").await,
        "late answer second"
    );
    finish(turn, "the calling turn").await;
    rig.teardown().await;
}

/// The cancel reaches exactly the helper this call seated: the caller's own
/// running turn and its queued work, and the helper of another call of the
/// same caller, go on.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_delegates_cancel_touches_only_its_own_helper() {
    let rig = Rig::new().await;
    // One round, two calls: helper a meets its limit, helper b runs on.
    let turn = rig
        .parent_calls(&[
            delegate_call("call-b", &hang("b"), "helper-b", None),
            delegate_call("call-a", &hang("a"), "helper-a", Some(5)),
        ])
        .await;
    rig.script.gate("a").wait_entered(1).await;
    rig.script.gate("b").wait_entered(1).await;
    // The caller's other work: a turn queued behind the running one.
    let queued = rig.start_parent_turn("the queued turn").await;

    let implicit = rig.implicit().await;
    wait_gone(&implicit, "helper-a", "its call's limit is 5 s").await;
    // Give a wrong cancel time to show.
    tokio::time::sleep(Duration::from_secs(2)).await;

    // Another call's helper is seated and still in its one model call.
    assert!(is_member(&implicit, "helper-b").await);
    assert_eq!(
        rig.script.gate("b").entered(),
        1,
        "helper b's model call was neither ended nor restarted"
    );
    // The caller's round still waits for call b, so call a has no recorded
    // result yet, and the caller's turn was not cancelled.
    assert!(rig.script.results.lock().unwrap().is_empty());

    rig.script.gate("b").open();
    let data = error_data(&rig.result_of("call-a").await);
    assert_helper_ended(&data, "max_run_elapsed", "cancelled_and_retired");
    assert_eq!(data["member_id"], "helper-a", "{data}");
    let result_b: Value =
        serde_json::from_str(&rig.result_of("call-b").await).expect("delegate result JSON");
    assert_eq!(result_b["output"], "late answer b", "{result_b}");
    finish(turn, "the caller's running turn").await;
    assert_eq!(
        finish(queued, "the caller's queued turn").await,
        "quick answer",
        "the caller's queued work ran"
    );
    rig.teardown().await;
}

/// By default the call waits what its outside bound allows: with only the
/// agent loop's default tool deadline, it ends a little before that
/// deadline, where it used to be cut at it, and its answer is the typed one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_default_delegate_ends_typed_before_the_loops_default_deadline() {
    let mut config = meerkat::Config::default();
    // The call's wait: 32 s less its quarter. The 8 s left hold the
    // helper's end several times over.
    config.tools.default_timeout = Duration::from_secs(32);
    let rig = Rig::with_config(config).await;
    let started = Instant::now();
    let turn = rig
        .parent_calls(&[delegate_call(
            "call",
            &hang("by-default"),
            "by-default",
            None,
        )])
        .await;
    // The helper is in its model call: the call reached the tool before.
    rig.script.gate("by-default").wait_entered(1).await;
    let in_tool = started.elapsed();
    let result = rig.result_of("call").await;
    assert!(
        !result.contains("\"error\":\"timeout\""),
        "the typed answer, not the loop's generic timeout: {result}"
    );
    let data = error_data(&result);
    assert_helper_ended(&data, "max_run_elapsed", "cancelled_and_retired");
    // 32 s less its quarter.
    assert_wait_secs(&data, 24, in_tool);
    assert!(data.get("max_run_limited_by").is_none(), "{data}");
    assert!(!is_member(&rig.implicit().await, "by-default").await);
    finish(turn, "the calling turn").await;
    rig.teardown().await;
}

/// An operator's `tools.tool_timeouts.delegate` bounds the call from
/// outside. The tool ends before it, so the model sees the typed answer and
/// not the loop's generic timeout, and a `max_run_secs` above what the
/// bound allows is clamped and said so.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_delegate_ends_typed_before_an_operators_bound_and_says_it_was_clamped() {
    let mut config = meerkat::Config::default();
    config
        .tools
        .tool_timeouts
        .insert("delegate".to_string(), Duration::from_secs(32));
    let rig = Rig::with_config(config).await;
    let started = Instant::now();
    let turn = rig
        .parent_calls(&[delegate_call(
            "call",
            &hang("clamped"),
            "clamped",
            Some(3600),
        )])
        .await;
    // The helper is in its model call: the call reached the tool before.
    rig.script.gate("clamped").wait_entered(1).await;
    let in_tool = started.elapsed();
    let result = rig.result_of("call").await;
    assert!(
        !result.contains("\"error\":\"timeout\""),
        "the typed answer, not the loop's generic timeout: {result}"
    );
    let data = error_data(&result);
    assert_helper_ended(&data, "max_run_elapsed", "cancelled_and_retired");
    // 32 s less its quarter.
    assert_wait_secs(&data, 24, in_tool);
    assert_eq!(data["max_run_limited_by"], "call_deadline", "{data}");
    assert!(!is_member(&rig.implicit().await, "clamped").await);
    finish(turn, "the calling turn").await;
    rig.teardown().await;
}

/// Nesting, with no carrier between the levels: the outer wait ends while an
/// inner helper runs. The outer helper is cancelled and retired, and the
/// inner helper ends with it, promptly. Nothing is left on a roster.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_outer_delegate_that_ends_takes_the_inner_helper_with_it() {
    let rig = Rig::new().await;
    let outer_task = calls_text(&[delegate_call("inner-call", &hang("inner"), "inner", None)]);
    let turn = rig
        .parent_calls(&[delegating_helper_call(
            "outer-call",
            &outer_task,
            "outer",
            Some(8),
        )])
        .await;
    // The inner helper is in its model call.
    rig.script.gate("inner").wait_entered(1).await;
    let outer_mob = rig.implicit().await;
    let outer_session = outer_mob
        .resolve_bridge_session_id(&AgentIdentity::from("outer"))
        .await
        .expect("the outer helper's session");
    let inner_mob_id = rig
        .state
        .find_implicit_mob_for_bridge_session(&outer_session.to_string())
        .await
        .expect("the outer helper's own implicit mob");
    let inner_mob = rig
        .state
        .handle_for(&inner_mob_id)
        .await
        .expect("inner mob");
    assert!(
        is_member(&inner_mob, "inner").await,
        "the inner helper runs"
    );

    let data = error_data(&rig.result_of("outer-call").await);
    let outer_answered = Instant::now();
    assert_helper_ended(&data, "max_run_elapsed", "cancelled_and_retired");
    assert!(!is_member(&outer_mob, "outer").await);

    let waited = wait_gone(
        &inner_mob,
        "inner",
        "the helper that delegated to it was cancelled and retired",
    )
    .await;
    let inner_mob_listed = rig
        .state
        .mob_list()
        .await
        .expect("mob list")
        .iter()
        .any(|(id, _)| *id == inner_mob_id);
    eprintln!(
        "DT-NESTING: the inner helper was off its roster {:?} after the outer call's answer was \
         read (the first roster read after it took {waited:?}); the retired outer helper's \
         implicit mob {inner_mob_id} is still listed: {inner_mob_listed}",
        outer_answered.elapsed(),
    );
    assert_eq!(
        rig.script.gate("inner").entered(),
        1,
        "the inner helper's run was not restarted"
    );
    finish(turn, "the calling turn").await;
    rig.teardown().await;
}

/// (B) the mob id on every `delegate` answer from the point the mob exists:
/// the result, a refused spawn (a member under the id is seated already)
/// and a failed helper turn name the session's implicit mob. A refusal
/// before any mob exists names none.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_delegate_answer_names_the_helpers_mob_once_the_mob_exists() {
    let rig = Rig::new().await;

    // Before any mob exists: an argument refusal names none.
    let turn = rig
        .parent_calls(&[
            json!({ "id": "no-member", "name": "delegate", "args": {
                "task": "no member id",
                "result_label": "helper_result",
                "max_text_bytes": 4096,
            }}),
            delegate_call("zero", "say hello", "zero", Some(0)),
        ])
        .await;
    for id in ["no-member", "zero"] {
        let early: Value = serde_json::from_str(&rig.result_of(id).await).expect("error JSON");
        assert_eq!(early["error"], "invalid_arguments", "{early}");
        assert!(early.get("data").is_none(), "{early}");
    }
    finish(turn, "the refused turn").await;
    assert!(
        rig.state
            .find_implicit_mob_for_bridge_session(&rig.session.to_string())
            .await
            .is_none(),
        "a refused call makes no mob"
    );

    // The result.
    let started = Instant::now();
    let turn = rig
        .parent_calls(&[delegate_call("quick", "say hello", "quick", None)])
        .await;
    let payload: Value =
        serde_json::from_str(&rig.result_of("quick").await).expect("delegate result JSON");
    // The call has answered: it reached the tool before.
    let in_tool = started.elapsed();
    let implicit_id = rig.implicit_mob_id().await;
    assert_eq!(payload["mob_id"], implicit_id.as_str(), "{payload}");
    assert_eq!(payload["output"], "quick answer", "{payload}");
    // The loop's default tool deadline of 10 min less 15 s.
    assert_wait_secs(&payload, 585, in_tool);
    finish(turn, "the quick turn").await;

    // A refused spawn: a member under the id is seated already.
    rig.state
        .mob_spawn(
            &implicit_id,
            ProfileName::from("delegate"),
            AgentIdentity::from("taken"),
            Some(MobRuntimeMode::TurnDriven),
            Some(MobBackendKind::Session),
            None,
        )
        .await
        .expect("a member that holds the id");
    let turn = rig
        .parent_calls(&[delegate_call("refused", "say hello", "taken", None)])
        .await;
    let data = error_data(&rig.result_of("refused").await);
    assert_eq!(data["mob_id"], implicit_id.as_str(), "{data}");
    assert_eq!(data["member_id"], "taken", "{data}");
    assert!(
        data.get("kind") != Some(&json!("delegate_helper_ended")),
        "no helper was seated by the refused call: {data}"
    );
    finish(turn, "the refused-spawn turn").await;
    assert!(
        is_member(&rig.implicit().await, "taken").await,
        "the refused call did not touch the member that holds the id"
    );

    // A failed helper turn.
    let turn = rig
        .parent_calls(&[delegate_call("failed", "DT-FAIL", "failing", None)])
        .await;
    let data = error_data(&rig.result_of("failed").await);
    assert_helper_ended(&data, "turn_failed", "retired");
    assert_eq!(data["mob_id"], implicit_id.as_str(), "{data}");
    assert_eq!(data["member_id"], "failing", "{data}");
    assert!(data["session_id"].as_str().is_some(), "{data}");
    finish(turn, "the failed-helper turn").await;
    assert!(!is_member(&rig.implicit().await, "failing").await);
    rig.teardown().await;
}

/// `member_not_found` is unchanged: a member id a mob does not hold answers
/// for the mob that was searched, with the data it always had.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn member_not_found_is_unchanged() {
    let rig = Rig::new().await;
    let turn = rig
        .parent_calls(&[delegate_call("quick", "say hello", "quick", None)])
        .await;
    finish(turn, "the quick turn").await;
    let implicit_id = rig.implicit_mob_id().await;

    let turn = rig
        .parent_calls(
            &[json!({ "id": "check", "name": "mob_check_member", "args": {
                "mob_id": implicit_id.as_str(),
                "member_id": "nobody",
            }})],
        )
        .await;
    let payload: Value = serde_json::from_str(&rig.result_of("check").await).expect("error JSON");
    assert_eq!(
        payload["data"],
        json!({
            "kind": "member_not_found",
            "mob_id": implicit_id.as_str(),
            "member_id": "nobody",
            "retryable": false,
        }),
        "{payload}"
    );
    finish(turn, "the checking turn").await;
    rig.teardown().await;
}

/// The one knob: an operator who raises `tools.tool_timeouts.delegate`
/// above the other tool deadlines gets a helper whose own slow tool times
/// out first, so the helper reports it and the `delegate` completes.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_helpers_tool_times_out_before_a_delegate_with_a_longer_bound_does() {
    let mut config = meerkat::Config::default();
    config.tools.default_timeout = Duration::from_secs(3);
    config
        .tools
        .tool_timeouts
        .insert("delegate".to_string(), Duration::from_secs(25));
    let rig = Rig::with_config(config).await;
    let helper_task = calls_text(&[json!({ "id": "helper-wait", "name": HOST_WAIT, "args": {} })]);
    let started = Instant::now();
    let turn = rig
        .parent_calls(&[delegate_call("call", &helper_task, "reporter", None)])
        .await;
    // The helper is in its tool call: the call reached the tool before.
    while rig.host_wait_entered.load(Ordering::SeqCst) == 0 {
        assert!(
            started.elapsed() < ROW_BOUND,
            "the helper did not reach its tool call"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let in_tool = started.elapsed();
    let payload: Value =
        serde_json::from_str(&rig.result_of("call").await).expect("delegate result JSON");
    let output = payload["output"].as_str().unwrap_or_default();
    assert!(
        output.starts_with("done ") && output.contains("timed out after 3000ms"),
        "the helper reports its own tool's timeout: {payload}"
    );
    // 25 s less its quarter is 18.75 s, reported to the nearest second.
    assert_wait_secs(&payload, 19, in_tool);
    finish(turn, "the calling turn").await;
    rig.teardown().await;
}

/// K calls of one caller in one round, with mixed endings, each settle
/// exactly once and leave no helper behind.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn k_delegates_of_one_caller_each_settle_once_and_leave_no_helper() {
    const K: usize = 6;
    let rig = Rig::new().await;
    let calls: Vec<Value> = (0..K)
        .map(|index| {
            let member = format!("k-{index}");
            // Even calls hang and meet their limit, odd calls answer.
            let task = if index % 2 == 0 {
                hang(&member)
            } else {
                "say hello".to_string()
            };
            delegate_call(&format!("call-{index}"), &task, &member, Some(6))
        })
        .collect();
    let turn = rig.parent_calls(&calls).await;
    for index in 0..K {
        let result = rig.result_of(&format!("call-{index}")).await;
        if index % 2 == 0 {
            let data = error_data(&result);
            assert_helper_ended(&data, "max_run_elapsed", "cancelled_and_retired");
            assert_eq!(data["member_id"], format!("k-{index}"), "{data}");
            assert_eq!(rig.script.gate(&format!("k-{index}")).entered(), 1);
        } else {
            let payload: Value = serde_json::from_str(&result).expect("delegate result JSON");
            assert_eq!(payload["output"], "quick answer", "{payload}");
        }
    }
    finish(turn, "the calling turn").await;
    let implicit = rig.implicit().await;
    for index in 0..K {
        assert!(
            !is_member(&implicit, &format!("k-{index}")).await,
            "helper k-{index} is retired"
        );
    }
    rig.teardown().await;
}

/// A run whose own task is lost after it seated its helper (it unwound):
/// the call is told that it does not know what became of the helper, with
/// the mob and the member id to look at, and the task's guard ends the
/// helper all the same.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_delegate_whose_run_is_lost_says_so_and_its_helper_is_ended() {
    let rig = Rig::new().await;
    meerkat_mob::lose_delegation_run_for_test(AgentIdentity::from("lost"));
    let turn = rig
        .parent_calls(&[delegate_call("call", &hang("lost"), "lost", None)])
        .await;
    let result = rig.result_of("call").await;
    let data = error_data(&result);
    assert_helper_ended(&data, "run_lost", "unknown");
    let implicit_id = rig.implicit_mob_id().await;
    assert_eq!(data["mob_id"], implicit_id.as_str(), "{data}");
    assert_eq!(data["member_id"], "lost", "{data}");
    assert!(
        !result.contains("not_started"),
        "a lost run is not answered as a helper that never started: {result}"
    );
    let waited = wait_gone(
        &rig.implicit().await,
        "lost",
        "the run that seated it was lost",
    )
    .await;
    eprintln!(
        "DT-LOST-RUN: the helper of the lost run was off its roster {waited:?} after the answer \
         was read"
    );
    assert!(
        finish(turn, "the calling turn").await.starts_with("done "),
        "a lost run does not fail the calling turn"
    );
    rig.teardown().await;
}

/// A start that outlasts the call's limit: the helper is seated and its
/// turn's admission has not returned when the limit elapses. The call is
/// answered at its limit, typed, and the helper is ended once the start
/// returns.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_start_that_outlasts_the_limit_is_answered_at_the_limit_and_its_helper_is_ended() {
    let rig = Rig::new().await;
    // The implicit mob exists from a first call on, so the limit of the
    // call that follows covers one seat and nothing else.
    let first = rig
        .parent_calls(&[delegate_call("first", "say hello", "first", None)])
        .await;
    finish(first, "the first calling turn").await;
    let (reached, release) =
        meerkat_mob::hold_delegation_start_for_test(AgentIdentity::from("held"));
    let turn = rig
        .parent_calls(&[delegate_call("call", &hang("held"), "held", Some(5))])
        .await;
    tokio::time::timeout(ROW_BOUND, reached)
        .await
        .expect("the run seats its helper")
        .expect("the run is held in its start");

    // The start is still held: the answer does not wait for it.
    let data = error_data(&rig.result_of("call").await);
    assert_helper_ended(&data, "max_run_elapsed", "end_pending");
    assert_eq!(data["max_run_secs"], 5, "{data}");
    assert_eq!(data["member_id"], "held", "{data}");
    assert!(
        data["session_id"].as_str().is_some(),
        "the helper was read when it was seated: {data}"
    );
    let implicit = rig.implicit().await;
    assert!(
        is_member(&implicit, "held").await,
        "the helper is seated while its start is held"
    );
    assert_eq!(
        rig.script.gate("held").entered(),
        0,
        "its turn was not admitted yet"
    );

    let _ = release.send(());
    let waited = wait_gone(
        &implicit,
        "held",
        "its start returned after the call's limit",
    )
    .await;
    eprintln!("DT-HELD-START: the helper was off its roster {waited:?} after its start was let go");
    finish(turn, "the calling turn").await;
    rig.teardown().await;
}

/// A helper's end that outlasts what the call waits for it: the call is
/// answered typed before its outside bound, saying that the end is still
/// under way, and the helper is ended without the call.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_helpers_end_that_outlasts_its_bound_is_answered_before_the_calls_deadline() {
    let mut config = meerkat::Config::default();
    // Wait 12 s, then 3 s for the helper's end, under a bound of 16 s.
    config
        .tools
        .tool_timeouts
        .insert("delegate".to_string(), Duration::from_secs(16));
    let rig = Rig::with_config(config).await;
    let (reached, release) =
        meerkat_mob::hold_delegation_limit_for_test(AgentIdentity::from("slow-end"));
    let started = Instant::now();
    let turn = rig
        .parent_calls(&[delegate_call("call", &hang("slow-end"), "slow-end", None)])
        .await;
    // The helper is in its model call: the call reached the tool before.
    rig.script.gate("slow-end").wait_entered(1).await;
    let in_tool = started.elapsed();
    tokio::time::timeout(ROW_BOUND, reached)
        .await
        .expect("the run reaches its limit")
        .expect("the run is held at its limit");

    // The run is held before its cancel: the helper's end does not finish.
    let result = rig.result_of("call").await;
    assert!(
        !result.contains("\"error\":\"timeout\""),
        "the typed answer, not the loop's generic timeout: {result}"
    );
    let data = error_data(&result);
    assert_helper_ended(&data, "max_run_elapsed", "end_pending");
    // 16 s less its quarter.
    assert_wait_secs(&data, 12, in_tool);
    let implicit = rig.implicit().await;
    assert!(
        is_member(&implicit, "slow-end").await,
        "the helper is still seated when the call answers"
    );

    let _ = release.send(());
    wait_gone(&implicit, "slow-end", "its end was let go").await;
    finish(turn, "the calling turn").await;
    rig.teardown().await;
}

/// The helper's session binding moves between its seat and its end (the
/// same incarnation: same runtime id, same fence). The retire acts under
/// the scope MobMachine holds for the helper when the run ends, so
/// MobMachine admits it: the helper's retirement begins. Under the scope
/// read at the seat the retire is refused (the session moved), nothing
/// begins, and the helper stays active.
///
/// The seam that moves the binding here moves it in MobMachine only; the
/// helper's runtime keeps the route of the session it was seated on (a
/// real binding recovery moves both). So the retirement that was admitted
/// cannot finish in this rig, and the row asserts the exact answer of that:
/// the helper is reported as not retired, and it is retiring, not active.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_retire_of_a_helper_whose_session_was_bound_again_is_made_under_its_current_scope() {
    let rig = Rig::new().await;
    // The implicit mob, and in it a session for the binding to move to:
    // another member's, which a retire can archive.
    let first = rig
        .parent_calls(&[delegate_call("first", "say hello", "first", None)])
        .await;
    finish(first, "the first calling turn").await;
    let implicit_id = rig.implicit_mob_id().await;
    let implicit = rig.implicit().await;
    let donor = AgentIdentity::from("donor");
    rig.state
        .mob_spawn(
            &implicit_id,
            ProfileName::from("delegate"),
            donor.clone(),
            Some(MobRuntimeMode::TurnDriven),
            Some(MobBackendKind::Session),
            None,
        )
        .await
        .expect("a member whose session the binding moves to");
    let moved_to = implicit
        .resolve_bridge_session_id(&donor)
        .await
        .expect("the donor's session");

    let rebound = AgentIdentity::from("rebound");
    let (reached, release) = meerkat_mob::hold_delegation_limit_for_test(rebound.clone());
    let turn = rig
        .parent_calls(&[delegate_call("call", &hang("rebound"), "rebound", Some(5))])
        .await;
    tokio::time::timeout(ROW_BOUND, reached)
        .await
        .expect("the run reaches its limit")
        .expect("the run is held at its limit");
    let seated_on = implicit
        .resolve_bridge_session_id(&rebound)
        .await
        .expect("the helper's session");
    assert_ne!(seated_on, moved_to);
    let before = implicit
        .get_member(&rebound)
        .await
        .expect("roster")
        .expect("the helper is seated");
    implicit
        .rebind_member_session_for_test(&rebound, moved_to.clone())
        .await
        .expect("bind the helper's session again");
    let after = implicit
        .get_member(&rebound)
        .await
        .expect("roster")
        .expect("the helper is still seated");
    assert_eq!(
        (before.generation, before.fence_token),
        (after.generation, after.fence_token),
        "the binding moved under the same incarnation"
    );
    eprintln!(
        "DT-REBOUND: the helper was seated on session {seated_on} and is bound to {:?} now, \
         under the same runtime id and fence",
        implicit.resolve_bridge_session_id(&rebound).await
    );
    let _ = release.send(());

    let result = rig.result_of("call").await;
    let data = error_data(&result);
    let status = implicit
        .member_status(&rebound)
        .await
        .expect("helper status")
        .status;
    eprintln!(
        "DT-REBOUND-END: the helper is reported `{}` ({}); its member status is {status:?}",
        data["helper"], data["retirement_error"]
    );
    assert_ne!(
        data["helper"], "already_gone",
        "a seated helper is not reported gone: {data}"
    );
    assert_eq!(
        status,
        meerkat_mob::MobMemberStatus::Retiring,
        "the retire was made under the scope MobMachine holds for the helper now, and was \
         admitted: {data}"
    );
    // The retirement that began cannot finish here (see above). The answer
    // says that the helper is still a member (`retirement_failed`) or, when
    // the retries of the retirement outlast what the call waits for the
    // helper's end, that its end is still under way (`end_pending`); either
    // way it says how to retire the helper.
    assert_eq!(data["kind"], "delegate_helper_ended", "{data}");
    assert_eq!(data["reason"], "max_run_elapsed", "{data}");
    assert!(
        matches!(
            data["helper"].as_str(),
            Some("retirement_failed" | "end_pending")
        ),
        "a helper that is retiring and not retired: {data}"
    );
    assert!(
        result.contains("mob_retire_member"),
        "the answer says what to do with the helper: {result}"
    );
    finish(turn, "the calling turn").await;
    rig.teardown().await;
}

/// `not_started`: the helper is seated and its turn is refused admission
/// (another hand retired it in between). The answer is the typed one, with
/// the mob and the member id, and nothing is left seated.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_delegate_whose_helpers_turn_is_not_admitted_answers_not_started() {
    let rig = Rig::new().await;
    let unadmitted = AgentIdentity::from("unadmitted");
    let (reached, release) = meerkat_mob::hold_delegation_start_for_test(unadmitted.clone());
    let turn = rig
        .parent_calls(&[delegate_call("call", "say hello", "unadmitted", None)])
        .await;
    tokio::time::timeout(ROW_BOUND, reached)
        .await
        .expect("the run seats its helper")
        .expect("the run is held in its start");
    let implicit = rig.implicit().await;
    let implicit_id = rig.implicit_mob_id().await;
    implicit
        .retire(unadmitted.clone())
        .await
        .expect("another hand retires the helper before its turn is admitted");
    let _ = release.send(());

    let data = error_data(&rig.result_of("call").await);
    assert_helper_ended(&data, "not_started", "retired");
    assert_eq!(data["mob_id"], implicit_id.as_str(), "{data}");
    assert_eq!(data["member_id"], "unadmitted", "{data}");
    assert!(data.get("max_run_secs").is_none(), "{data}");
    assert!(!is_member(&implicit, "unadmitted").await);
    finish(turn, "the calling turn").await;
    rig.teardown().await;
}

/// `mob_not_running`: the helper's mob stops taking commands while the
/// call waits (it is shut down). Neither the cancel nor the retire can act,
/// and the answer says so, with the mob and the member id.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_delegate_whose_mob_takes_no_commands_any_more_answers_mob_not_running() {
    let rig = Rig::new().await;
    let stranded = AgentIdentity::from("stranded");
    let (reached, release) = meerkat_mob::hold_delegation_limit_for_test(stranded.clone());
    let turn = rig
        .parent_calls(&[delegate_call(
            "call",
            &hang("stranded"),
            "stranded",
            Some(5),
        )])
        .await;
    tokio::time::timeout(ROW_BOUND, reached)
        .await
        .expect("the run reaches its limit")
        .expect("the run is held at its limit");
    let implicit = rig.implicit().await;
    let implicit_id = rig.implicit_mob_id().await;
    tokio::time::timeout(ROW_BOUND, implicit.shutdown())
        .await
        .expect("the helper's mob shuts down in time")
        .expect("the helper's mob shuts down");
    let _ = release.send(());

    let result = rig.result_of("call").await;
    let data = error_data(&result);
    assert_helper_ended(&data, "max_run_elapsed", "mob_not_running");
    assert_eq!(data["mob_id"], implicit_id.as_str(), "{data}");
    assert_eq!(data["member_id"], "stranded", "{data}");
    assert!(
        result.contains("mob_retire_member"),
        "the answer says what to do with a helper the mob may still hold: {result}"
    );
    finish(turn, "the calling turn").await;
    rig.teardown().await;
}

/// The call's outside bound counts from the start of its dispatch. A call
/// that waited for a dispatch slot before it reached the tool waits that
/// much less, so that it still ends before the bound.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_delegate_takes_off_the_time_its_call_waited_for_a_dispatch_slot() {
    let mut config = meerkat::Config::default();
    // One call at a time: the second call of a round waits for the first.
    config.tools.max_concurrent = 1;
    // 40 s less its 10 s margin: 30 s from the dispatch's start.
    config
        .tools
        .tool_timeouts
        .insert("delegate".to_string(), Duration::from_secs(40));
    let rig = Rig::with_config(config).await;
    let turn = rig
        .parent_calls(&[
            json!({ "id": "pause", "name": HOST_PAUSE, "args": { "ms": 3000 } }),
            delegate_call("call", "say hello", "queued", None),
        ])
        .await;
    let payload: Value =
        serde_json::from_str(&rig.result_of("call").await).expect("delegate result JSON");
    assert_eq!(payload["output"], "quick answer", "{payload}");
    let reported = payload["max_run_secs"].as_u64().expect("max_run_secs");
    eprintln!(
        "DT-DISPATCH-START: a call that waited 3 s for a dispatch slot under a 40 s bound \
         reports max_run_secs {reported} (30 with nothing taken off)"
    );
    assert!(
        (15..=27).contains(&reported),
        "the 3 s the call waited for its slot are taken off its 30 s: {payload}"
    );
    finish(turn, "the calling turn").await;
    rig.teardown().await;
}

/// The host dies while a `delegate` waits. Nothing process-local
/// survives, and the engine keeps no durable fact of the call, so nothing
/// ends the helper: a restarted host finds it seated and idle. It does not
/// run its interrupted turn again, by itself or when woken, and the same
/// call made again is refused with the mob and the member id to retire.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_helper_whose_call_died_with_the_host_after_the_restart() {
    use support::CouncilFixture;

    let script = Script::default();
    let model = script.clone();
    let mut fixture = CouncilFixture::new(|_| ScriptedTurn::Text("unused".to_string()));
    // A first lifetime whose members mount the real agent mob tools.
    fixture
        .restart_cold_with_agent_mob_tools(move |request| model.turn(request), |state, _| state)
        .await;
    let mob_id = fixture.source_mob_id();
    let mut definition = parent_definition(&mob_id);
    definition.id = mob_id.clone();
    fixture.seed_source_mob_from(definition, &["parent"]).await;
    let host = fixture.state.handle_for(&mob_id).await.expect("host mob");
    let session = host
        .resolve_bridge_session_id(&AgentIdentity::from("parent"))
        .await
        .expect("the parent has a bridge session");
    let turn = host
        .start_work_for_identity_bounded(
            AgentIdentity::from("parent"),
            WorkSpec::new(
                ContentInput::Text(calls_text(&[delegate_call(
                    "interrupted-call",
                    &hang("orphan"),
                    "orphan",
                    None,
                )])),
                WorkOrigin::Internal,
            ),
            HandlingMode::Queue,
            turn_spec(),
        )
        .await
        .expect("start the delegating turn");
    script.gate("orphan").wait_entered(1).await;
    let implicit_id = fixture
        .state
        .find_implicit_mob_for_bridge_session(&session.to_string())
        .await
        .expect("the parent's implicit mob");
    let helper_session = fixture
        .state
        .handle_for(&implicit_id)
        .await
        .expect("implicit mob")
        .resolve_bridge_session_id(&AgentIdentity::from("orphan"))
        .await
        .expect("the helper's session");

    // The host dies: the mobs stop as a crash stops them, and the call,
    // with whatever owned its wait, dies with the process.
    let reran = Arc::new(AtomicUsize::new(0));
    let rerun_count = Arc::clone(&reran);
    let parent_reran = Arc::new(AtomicUsize::new(0));
    let parent_rerun_count = Arc::clone(&parent_reran);
    let after = Script::default();
    let after_model = after.clone();
    fixture
        .restart_cold_after_crash_with_agent_mob_tools(
            move |request| {
                let user = last_user_text(request);
                if user.contains(CALLS) {
                    parent_rerun_count.fetch_add(1, Ordering::SeqCst);
                    return after_model.turn(request);
                }
                if user.contains("DT-HANG:orphan:") {
                    rerun_count.fetch_add(1, Ordering::SeqCst);
                    return ScriptedTurn::Text("answered after the restart".to_string());
                }
                ScriptedTurn::Text("quick answer".to_string())
            },
            |state, _root| state,
        )
        .await;
    drop(turn);

    let listed = tokio::time::timeout(ROW_BOUND, fixture.state.mob_list())
        .await
        .expect("the restarted host lists its mobs")
        .expect("mob list");
    let implicit_listed = listed.iter().any(|(id, _)| *id == implicit_id);
    let implicit = fixture.state.handle_for(&implicit_id).await;
    let (seated, status_before_wake) = match &implicit {
        Ok(implicit) => {
            let seated = is_member(implicit, "orphan").await;
            let status = implicit
                .member_status(&AgentIdentity::from("orphan"))
                .await
                .map(|status| format!("{:?}", status.status))
                .unwrap_or_else(|error| format!("unreadable: {error}"));
            (seated, status)
        }
        Err(error) => (false, format!("no implicit mob handle: {error}")),
    };
    // Left alone: does its interrupted turn run again by itself?
    tokio::time::sleep(Duration::from_secs(5)).await;
    let reran_alone = reran.load(Ordering::SeqCst);
    // Woken as any dormant member is woken: does it run then?
    let wake = match &implicit {
        Ok(implicit) if seated => Some(
            tokio::time::timeout(
                ROW_BOUND,
                implicit.ensure_member_live(&AgentIdentity::from("orphan")),
            )
            .await
            .map(|woken| woken.map_err(|error| error.to_string())),
        ),
        _ => None,
    };
    tokio::time::sleep(Duration::from_secs(5)).await;
    let reran_after_wake = reran.load(Ordering::SeqCst);
    let after_wake = match &implicit {
        Ok(implicit) if seated => implicit
            .member_status(&AgentIdentity::from("orphan"))
            .await
            .map(|status| format!("{:?} preview={:?}", status.status, status.output_preview))
            .unwrap_or_else(|error| format!("unreadable: {error}")),
        _ => "not seated".to_string(),
    };

    // The caller after the restart: woken, does its interrupted round run
    // again, and what does the same `delegate` call answer then?
    let host = fixture.state.handle_for(&mob_id).await.expect("host mob");
    let parent_wake = tokio::time::timeout(
        ROW_BOUND,
        host.ensure_member_live(&AgentIdentity::from("parent")),
    )
    .await
    .map(|woken| woken.map_err(|error| error.to_string()));
    tokio::time::sleep(Duration::from_secs(5)).await;
    let parent_reran_alone = parent_reran.load(Ordering::SeqCst);
    let again = host
        .start_work_for_identity_bounded(
            AgentIdentity::from("parent"),
            WorkSpec::new(
                ContentInput::Text(calls_text(&[delegate_call(
                    "same-call-again",
                    &hang("orphan"),
                    "orphan",
                    None,
                )])),
                WorkOrigin::Internal,
            ),
            HandlingMode::Queue,
            turn_spec(),
        )
        .await;
    let again = match again {
        Ok(turn) => {
            let _ = tokio::time::timeout(ROW_BOUND, turn.wait_bounded(turn_spec())).await;
            after
                .results
                .lock()
                .unwrap()
                .get("same-call-again")
                .cloned()
                .unwrap_or_else(|| "no result within the row bound".to_string())
        }
        Err(error) => format!("the turn was not admitted: {error}"),
    };
    let seated_at_end = match &implicit {
        Ok(implicit) => is_member(implicit, "orphan").await,
        Err(_) => false,
    };
    eprintln!(
        "DT-RESTART: implicit mob listed={implicit_listed}; helper seated={seated} (status \
         {status_before_wake}, session {helper_session}); its interrupted turn ran again by \
         itself within 5 s: {reran_alone} time(s); wake={wake:?}; ran after the wake: \
         {reran_after_wake} time(s) ({after_wake}); caller wake={parent_wake:?}, its \
         interrupted round ran again by itself: {parent_reran_alone} time(s); the same delegate \
         call made again answered: {again}; helper seated at the end={seated_at_end}"
    );
    assert!(
        implicit_listed && seated && seated_at_end,
        "the helper stays"
    );
    assert_eq!(
        (reran_alone, reran_after_wake),
        (0, 0),
        "the helper does not run its interrupted turn again"
    );
    assert_eq!(
        parent_reran_alone, 0,
        "the caller's round does not run again"
    );
    let again: Value = serde_json::from_str(&again).expect("the refusal is JSON");
    assert_eq!(again["error"], "execution_failed", "{again}");
    assert!(
        again["message"]
            .as_str()
            .is_some_and(|message| message.contains("already exists")),
        "{again}"
    );
    assert_eq!(again["data"]["mob_id"], implicit_id.as_str(), "{again}");
    assert_eq!(again["data"]["member_id"], "orphan", "{again}");
    fixture.teardown().await;
}
