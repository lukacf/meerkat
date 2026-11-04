#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! `meta/reset` of an identity whose member has a turn in flight and input
//! queued behind it, through the gateway, on real persistent stores.
//!
//! A reset retires the member's incarnation in one Core rotation and starts
//! the identity on a fresh session. Work the retired incarnation held does
//! not move to the fresh session: Core's retirement interrupts the turn in
//! flight and ends every input still queued behind it, each with a durable
//! terminal and a terminal receipt in the retired session's input ledger.
//! The queued input never runs, on the retired incarnation or on the fresh
//! one. What a submitter can read of that terminal depends on how it sent:
//! a keyed send's admitted pair answers `meta/send_status`, a tracked send's
//! ticket settles `failed`, and an unkeyed, untracked send answers no input
//! id at all, so its submitter sees only the identity-wide wait end.
//!
//! These rows pin that custody. The reset's own answer names none of the
//! inputs it ended; the rows print it, with each terminal and receipt, for
//! the triage record.
//!
//! The model double holds the turn of a message naming `alpha` until the
//! test releases it, so that turn is in flight while the next input queues
//! (the member is turn driven, so its turns run one at a time). It answers
//! every other call at once and counts the calls per name.

use std::collections::{BTreeMap, HashMap};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use meerkat_client::{LlmError, LlmEvent, LlmRequest};
use meerkat_core::types::StopReason;
use meerkat_meta::UnifiedRuntimeBuilder;
use meerkat_meta::identity_first::contracts::{AgentCustomizer, LeaseProvider, TopologyProvider};
use meerkat_meta::identity_first::orchestrator::RestoreOutcome;
use meerkat_meta::identity_first::{
    AgentAddressability, AgentBuildContext, AgentBuildDraft, AgentIdentity, AgentRuntimeServices,
    ContinuityStore, CustomizerError, DurabilityPolicy, DurableAgentSpec,
    IdentityFirstRuntimeContext, IdentityRuntime, IdentityRuntimeConfig, LocalContinuityStore,
    LocalLeaseProvider, ManagedPeerEdge, MobSessionBridge, MutableRosterProvider, SessionBridge,
    TopologyContext, TopologyError,
};
use meerkat_meta::mob_handle_runtime::SessionCreatedContext;
use meerkat_mob::{MobDefinition, ProfileName};
use meerkat_runtime::SessionServiceRuntimeExt as _;
use meerkat_runtime::input_state::{InputLifecycleState, InputTerminalOutcome};
use meerkat_runtime::terminal_status::{InputTerminalReceiptObservation, InteractionSelector};
use serde_json::{Value, json};

#[path = "support/llm_usage.rs"]
mod llm_usage;

const WAIT: Duration = Duration::from_secs(30);
const IDENTITY: &str = "personal:alice";

/// The names the model double tells apart.
const NAMES: [&str; 2] = ["alpha", "beta"];

fn spec() -> DurableAgentSpec {
    DurableAgentSpec {
        identity: AgentIdentity::parse(IDENTITY).unwrap(),
        profile: ProfileName::from("personal"),
        addressability: AgentAddressability::Addressable,
        display_name: None,
        labels: BTreeMap::new(),
        context: None,
        additional_instructions: Vec::new(),
        initial_message: None,
        runtime_mode_override: Some(meerkat_mob::MobRuntimeMode::TurnDriven),
        backend: None,
        binding: None,
        placement: None,
    }
}

fn unique_mob_id() -> String {
    format!("reset-in-flight-{}", uuid::Uuid::new_v4())
}

const MOB_TOML: &str = r#"
[mob]
id = "MOB_ID_PLACEHOLDER"

[profiles.personal]
model = "gpt-5.5"
external_addressable = true
runtime_mode = "turn_driven"

[profiles.personal.tools]
comms = true
"#;

fn definition(mob_id: &str) -> MobDefinition {
    MobDefinition::from_toml(&MOB_TOML.replace("MOB_ID_PLACEHOLDER", mob_id))
        .expect("parse reset-in-flight mob definition")
}

struct EmptyTopology;
#[async_trait]
impl TopologyProvider for EmptyTopology {
    async fn compute_edges(
        &self,
        _target_identities: &[AgentIdentity],
        _context: &TopologyContext,
    ) -> Result<Vec<ManagedPeerEdge>, TopologyError> {
        Ok(vec![])
    }
}

struct NoopCustomizer;
#[async_trait]
impl AgentCustomizer for NoopCustomizer {
    async fn customize_build(
        &self,
        _context: &AgentBuildContext,
        _spec: &DurableAgentSpec,
        _draft: &mut AgentBuildDraft,
    ) -> Result<(), CustomizerError> {
        Ok(())
    }
    async fn after_create(
        &self,
        _identity: &AgentIdentity,
        _session_id: &meerkat_core::types::SessionId,
        _context: &SessionCreatedContext,
    ) -> Result<(), CustomizerError> {
        Ok(())
    }
}

/// Holds the turn of a message naming a held name until the test releases
/// it; answers every other call at once. Counts the model calls per name,
/// read from the request's new input (the messages after the last reply).
#[derive(Clone, Default)]
struct HoldingClient {
    holds: Arc<Mutex<HashMap<&'static str, tokio::sync::oneshot::Receiver<()>>>>,
    calls_by_name: Arc<Mutex<HashMap<&'static str, usize>>>,
    /// Names whose first call answers one tool round, so the turn is mid
    /// tool loop: the call after that round waits on the receiver.
    tool_rounds: Arc<Mutex<HashMap<&'static str, tokio::sync::oneshot::Receiver<()>>>>,
    held_after_round: Arc<Mutex<Option<tokio::sync::oneshot::Receiver<()>>>>,
    calls_after_round: Arc<std::sync::atomic::AtomicUsize>,
}

impl HoldingClient {
    fn hold(&self, name: &'static str) -> tokio::sync::oneshot::Sender<()> {
        let (release, held) = tokio::sync::oneshot::channel();
        self.holds.lock().unwrap().insert(name, held);
        release
    }

    /// The first call naming `name` answers one tool round (a call to the
    /// first tool offered); the call after the round is held until the
    /// returned sender is dropped or sent.
    fn hold_after_tool_round(&self, name: &'static str) -> tokio::sync::oneshot::Sender<()> {
        let (release, held) = tokio::sync::oneshot::channel();
        self.tool_rounds.lock().unwrap().insert(name, held);
        release
    }

    async fn wait_for_call_after_round(&self) {
        tokio::time::timeout(WAIT, async {
            while self
                .calls_after_round
                .load(std::sync::atomic::Ordering::SeqCst)
                == 0
            {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("the model is called after the tool round");
    }

    fn calls_for(&self, name: &str) -> usize {
        self.calls_by_name
            .lock()
            .unwrap()
            .get(name)
            .copied()
            .unwrap_or(0)
    }

    async fn wait_for_call(&self, name: &str) {
        tokio::time::timeout(WAIT, async {
            while self.calls_for(name) == 0 {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("the model never saw {name:?}"));
    }
}

impl meerkat_client::LlmClient for HoldingClient {
    fn project_replay_messages(
        &self,
        messages: &[meerkat_core::Message],
    ) -> Result<Vec<meerkat_core::Message>, LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
    ) -> Pin<Box<dyn futures::Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>> {
        let new_input_from = request
            .messages
            .iter()
            .rposition(|message| matches!(message, meerkat_core::Message::BlockAssistant(_)))
            .map_or(0, |last_reply| last_reply + 1);
        let new_input = request.messages[new_input_from..]
            .iter()
            .filter_map(|message| serde_json::to_string(message).ok())
            .collect::<String>();
        let names = NAMES
            .into_iter()
            .filter(|name| new_input.contains(name))
            .collect::<Vec<_>>();
        {
            let mut calls = self.calls_by_name.lock().unwrap();
            for name in &names {
                *calls.entry(name).or_default() += 1;
            }
        }
        let round = names
            .iter()
            .find_map(|name| self.tool_rounds.lock().unwrap().remove(name));
        if let Some(after_round) = round
            && let Some(tool) = request.tools.first()
        {
            *self.held_after_round.lock().unwrap() = Some(after_round);
            let [usage, done] =
                llm_usage::usage_then_done(request, meerkat::Provider::OpenAI, StopReason::ToolUse);
            let call = LlmEvent::ToolCallComplete {
                id: "reset-round-call".to_string(),
                name: tool.name.to_string(),
                args: json!({}),
                meta: None,
            };
            return Box::pin(futures::stream::iter([Ok(call), Ok(usage), Ok(done)]));
        }
        let held = if names.is_empty() {
            let after_round = self.held_after_round.lock().unwrap().take();
            if after_round.is_some() {
                self.calls_after_round
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
            after_round
        } else {
            names
                .iter()
                .find_map(|name| self.holds.lock().unwrap().remove(name))
        };
        let name = names.first().copied().unwrap_or("other");
        let [usage, done] =
            llm_usage::usage_then_done(request, meerkat::Provider::OpenAI, StopReason::EndTurn);
        Box::pin(async_stream::stream! {
            if let Some(held) = held {
                let _ = held.await;
            }
            yield Ok(LlmEvent::TextDelta { delta: format!("reply to: {name}"), meta: None });
            yield Ok(usage);
            yield Ok(done);
        })
    }

    fn provider(&self) -> meerkat::Provider {
        meerkat::Provider::OpenAI
    }

    fn health_check<'life0, 'async_trait>(
        &'life0 self,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<(), LlmError>> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async { Ok(()) })
    }
}

/// One boot of the gateway over a persistent state directory: the unified
/// runtime, its identity runtime and the RPC context, the identity restored.
struct Boot {
    unified: meerkat_meta::UnifiedRuntime,
    ctx: meerkat_meta::rpc::IdentityFirstContext,
}

impl Boot {
    async fn start(state_path: &std::path::Path, client: &HoldingClient) -> Self {
        let definition = definition(&unique_mob_id());
        let specs = vec![spec()];
        let mut unified = UnifiedRuntimeBuilder::default()
            .definition(definition.clone())
            .persistent_state(state_path)
            .comms(true)
            .default_llm_client(Arc::new(client.clone()))
            .build()
            .await
            .expect("build UnifiedRuntime");
        let identity_rt = Self::identity_runtime(&unified, state_path);
        let context = Arc::new(IdentityFirstRuntimeContext::new(
            Arc::clone(&identity_rt),
            Arc::new(MutableRosterProvider::new(specs.clone())),
            Some(Arc::new(EmptyTopology) as Arc<dyn TopologyProvider>),
            Some(Arc::new(NoopCustomizer) as Arc<dyn AgentCustomizer>),
            Some(definition.clone()),
        ));
        let result = unified
            .install_and_bootstrap_identity_first_context(context, &specs)
            .await
            .expect("install and restore the identity-first runtime");
        assert!(
            matches!(
                result.outcomes.get(&spec().identity),
                Some(RestoreOutcome::Created { .. } | RestoreOutcome::Resumed { .. })
            ),
            "{:?}",
            result.outcomes
        );
        let ctx = meerkat_meta::rpc::IdentityFirstContext {
            runtime: unified
                .identity_runtime()
                .expect("identity runtime installed"),
            roster_provider: Arc::new(MutableRosterProvider::new(specs)),
            topology_provider: None,
            customizer: None,
            agent_memory_provider: None,
            mob_definition: Some(definition),
            transcript_edit_service: None,
            compaction_floors: None,
        };
        Self { unified, ctx }
    }

    fn identity_runtime(
        unified: &meerkat_meta::UnifiedRuntime,
        state_path: &std::path::Path,
    ) -> Arc<IdentityRuntime> {
        let bridge: Arc<dyn SessionBridge> = Arc::new(MobSessionBridge::with_session_service(
            unified.mob_handle(),
            unified
                .mob_runtime()
                .session_service()
                .cloned()
                .expect("the persistent runtime has a session service"),
        ));
        let store = Arc::new(
            LocalContinuityStore::open(state_path.join("continuity.db")).expect("continuity store"),
        );
        Arc::new(
            IdentityRuntime::new(IdentityRuntimeConfig {
                continuity_store: store as Arc<dyn ContinuityStore>,
                lease_provider: Arc::new(LocalLeaseProvider::new()) as Arc<dyn LeaseProvider>,
                runtime_instance_id: "reset-in-flight".to_string(),
                has_runtime_store: true,
                durability_policy: DurabilityPolicy::SyncWriteThrough,
                bridge: Some(bridge),
                default_timeout: None,
            })
            .with_runtime_services(AgentRuntimeServices::new(unified.mob_handle())),
        )
    }

    /// The session service's runtime machine: Core's input ledger.
    fn machine(&self) -> Arc<meerkat_runtime::MeerkatMachine> {
        self.unified
            .mob_runtime()
            .session_service()
            .expect("the persistent runtime has a session service")
            .acquire_runtime_adapter(None)
            .expect("the runtime machine is acquired")
            .expect("the session service owns a runtime machine")
    }

    /// One JSON-RPC call; the whole reply.
    async fn call(&self, method: &str, params: Value) -> Value {
        let request = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
        let reply = meerkat_meta::rpc::handle_unified_rpc_json(
            &self.unified,
            &request.to_string(),
            WAIT,
            None,
            Some(&self.ctx),
        )
        .await;
        serde_json::from_str(&reply).expect("json-rpc reply")
    }

    /// One JSON-RPC call that must succeed; its result.
    async fn rpc(&self, method: &str, params: Value) -> Value {
        let reply = self.call(method, params).await;
        assert!(reply.get("error").is_none(), "{method} failed: {reply}");
        reply["result"].clone()
    }

    /// `meta/reset` of the identity, timed; the whole reply and the time it
    /// took.
    async fn reset(&self) -> (Value, Duration) {
        let started = std::time::Instant::now();
        let reply = self.call("meta/reset", json!({"identity": IDENTITY})).await;
        let elapsed = started.elapsed();
        assert!(
            reply["result"]["session_id"].is_string(),
            "the reset succeeds: {reply}"
        );
        (reply, elapsed)
    }

    /// Replay the keyed send of `text` under `key` until it is admitted; its
    /// admitted pair (`session_id`, `input_id`).
    async fn keyed_send_admitted(&self, text: &str, key: &str) -> Value {
        tokio::time::timeout(WAIT, async {
            loop {
                let result = self
                    .rpc(
                        "meta/send",
                        json!({"identity": IDENTITY, "content": text, "idempotency_key": key}),
                    )
                    .await;
                if result["continuation"]["status"]["state"] == "admitted" {
                    break result["continuation"]["status"].clone();
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("the keyed send {key} is admitted"))
    }

    /// `meta/send_status` of an admitted pair.
    async fn send_status(&self, pair: &Value) -> Value {
        self.rpc(
            "meta/send_status",
            json!({
                "identity": IDENTITY,
                "session_id": pair["session_id"],
                "input_id": pair["input_id"],
            }),
        )
        .await
    }

    /// `meta/send_status` of an admitted pair until its input is terminal or
    /// `WAIT` passes; the last read, terminal or not.
    async fn send_status_settled(&self, pair: &Value) -> Value {
        let deadline = tokio::time::Instant::now() + WAIT;
        loop {
            let read = self.send_status(pair).await;
            if read["state"]["terminal_outcome"].is_string()
                || tokio::time::Instant::now() >= deadline
            {
                return read;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    async fn session_of(&self) -> String {
        let status = self
            .rpc("meta/status_identity", json!({"identity": IDENTITY}))
            .await;
        status["session_id"]
            .as_str()
            .expect("the identity's session")
            .to_string()
    }

    /// Core's input state of `input_id` in `session`, polled until terminal
    /// or `WAIT` passes; the last read.
    async fn core_input_settled(
        &self,
        session: &meerkat_core::types::SessionId,
        input_id: &meerkat_core::lifecycle::InputId,
    ) -> Option<meerkat_runtime::input_state::StoredInputState> {
        let machine = self.machine();
        let deadline = tokio::time::Instant::now() + WAIT;
        loop {
            let read = machine
                .input_state(session, input_id)
                .await
                .expect("Core's input state reads");
            if read
                .as_ref()
                .is_some_and(|state| state.seed.terminal_outcome.is_some())
                || tokio::time::Instant::now() >= deadline
            {
                return read;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    /// Core's terminal receipt observation of `input_id` in `session`.
    async fn core_receipt(
        &self,
        session: &meerkat_core::types::SessionId,
        input_id: &meerkat_core::lifecycle::InputId,
    ) -> Option<InputTerminalReceiptObservation> {
        self.machine()
            .input_terminal_receipt_observation(
                session,
                InteractionSelector::InputId(input_id.clone()),
            )
            .await
            .expect("Core's terminal receipt reads")
            .map(|sourced| sourced.report)
    }

    /// The retired session's committed transcript, one JSON line per
    /// message.
    async fn transcript(&self, session: &meerkat_core::types::SessionId) -> Vec<String> {
        let service = self
            .unified
            .mob_runtime()
            .session_service()
            .cloned()
            .expect("the persistent runtime has a session service");
        match service
            .read_history(
                session,
                meerkat_core::service::SessionHistoryQuery::default(),
            )
            .await
        {
            Ok(page) => page
                .messages
                .iter()
                .map(|message| serde_json::to_string(message).unwrap_or_default())
                .collect(),
            Err(error) => vec![format!("read_history failed: {error}")],
        }
    }

    async fn shutdown(self) {
        self.unified.shutdown().await;
    }
}

fn session_id(value: &Value) -> meerkat_core::types::SessionId {
    meerkat_core::types::SessionId::parse(value.as_str().expect("a session id"))
        .expect("a valid session id")
}

fn input_id(value: &Value) -> meerkat_core::lifecycle::InputId {
    meerkat_core::lifecycle::InputId::from_uuid(
        uuid::Uuid::parse_str(value.as_str().expect("an input id")).expect("a valid input id"),
    )
}

/// The reset's answer names nothing of the work it ended: the five fields
/// it has always answered.
fn assert_answer_names_no_ended_work(reset: &Value) {
    let mut fields = reset["result"]
        .as_object()
        .expect("the reset answers an object")
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    fields.sort();
    assert_eq!(
        fields,
        [
            "agent_runtime_id",
            "cleanup_warning",
            "generation",
            "identity",
            "session_id"
        ],
        "{reset}"
    );
}

/// An input the reset ended: terminal, never consumed, with Core's receipt.
fn assert_ended_without_completion(
    label: &str,
    state: Option<&meerkat_runtime::input_state::StoredInputState>,
    receipt: Option<&InputTerminalReceiptObservation>,
) {
    let state = state.unwrap_or_else(|| panic!("{label}: Core holds no state of the input"));
    assert!(
        matches!(
            state.seed.terminal_outcome,
            Some(InputTerminalOutcome::Abandoned { .. })
        ),
        "{label}: the input ends abandoned, never consumed: {:?}",
        state.seed
    );
    assert_eq!(state.seed.phase, InputLifecycleState::Abandoned, "{label}");
    assert!(
        matches!(
            receipt,
            Some(
                InputTerminalReceiptObservation::Finalized(_)
                    | InputTerminalReceiptObservation::TerminalWithoutReceipt { .. }
            )
        ),
        "{label}: Core's terminal read is final: {receipt:?}"
    );
}

/// C1 (keyed sends): the reset ends the turn in flight and the input queued
/// behind it. Each ends abandoned in the retired session's ledger, with a
/// terminal Core reads back, and `meta/send_status` reads it from the pair;
/// the queued input never runs; the keys stay the identity's, so a replay
/// answers the retired pair and admits nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_reset_ends_the_keyed_turn_in_flight_and_the_queued_input_with_durable_terminals() {
    let temp = tempfile::TempDir::new().unwrap();
    let client = HoldingClient::default();
    let release_alpha = client.hold("alpha");
    let boot = Boot::start(&temp.path().join("state"), &client).await;
    let predecessor = boot.session_of().await;

    let alpha = boot
        .keyed_send_admitted("alpha: the turn in flight", "k-alpha")
        .await;
    client.wait_for_call("alpha").await;
    let beta = boot
        .keyed_send_admitted("beta: the input queued behind it", "k-beta")
        .await;
    assert_eq!(alpha["session_id"], predecessor, "{alpha}");
    assert_eq!(beta["session_id"], predecessor, "{beta}");
    let queued = boot.send_status(&beta).await;
    assert_eq!(
        queued["state"]["current_state"], "queued",
        "beta waits behind alpha's turn: {queued}"
    );

    let (reset, elapsed) = boot.reset().await;
    assert_ne!(reset["result"]["session_id"], predecessor, "{reset}");
    drop(release_alpha);

    let alpha_read = boot.send_status_settled(&alpha).await;
    let beta_read = boot.send_status_settled(&beta).await;
    let retired = session_id(&alpha["session_id"]);
    let alpha_state = boot
        .core_input_settled(&retired, &input_id(&alpha["input_id"]))
        .await;
    let beta_state = boot
        .core_input_settled(&retired, &input_id(&beta["input_id"]))
        .await;
    let alpha_receipt = boot
        .core_receipt(&retired, &input_id(&alpha["input_id"]))
        .await;
    let beta_receipt = boot
        .core_receipt(&retired, &input_id(&beta["input_id"]))
        .await;
    let transcript = boot.transcript(&retired).await;
    // The triage record: what the caller, the submitter and Core each see.
    eprintln!("C1 EVIDENCE reset_ms={} reset={reset}", elapsed.as_millis());
    eprintln!("C1 EVIDENCE send_status alpha={alpha_read}");
    eprintln!("C1 EVIDENCE send_status beta={beta_read}");
    eprintln!(
        "C1 EVIDENCE core alpha seed={:?} receipt={alpha_receipt:?}",
        alpha_state.as_ref().map(|state| &state.seed)
    );
    eprintln!(
        "C1 EVIDENCE core beta seed={:?} receipt={beta_receipt:?}",
        beta_state.as_ref().map(|state| &state.seed)
    );
    for (index, line) in transcript.iter().enumerate() {
        eprintln!("C1 EVIDENCE retired transcript[{index}]={line}");
    }

    for read in [&alpha_read, &beta_read] {
        assert!(
            matches!(
                read["state"]["terminal_outcome"].as_str(),
                Some("cancelled" | "abandoned")
            ),
            "send_status reads a terminal that is no completion: {read}"
        );
    }
    assert_ended_without_completion("alpha", alpha_state.as_ref(), alpha_receipt.as_ref());
    assert_ended_without_completion("beta", beta_state.as_ref(), beta_receipt.as_ref());
    // The queued input never runs: not on the retired incarnation while it
    // retires, and not on the fresh one.
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(client.calls_for("beta"), 0, "the queued input never ran");
    assert!(
        transcript.iter().all(|line| !line.contains("beta")),
        "the queued input is in no transcript of the retired session"
    );
    assert_answer_names_no_ended_work(&reset);

    // The keys stay the identity's: a replay answers the retired pair, never
    // a second admission onto the fresh session.
    let replay = boot
        .rpc(
            "meta/send",
            json!({
                "identity": IDENTITY,
                "content": "beta: the input queued behind it",
                "idempotency_key": "k-beta",
            }),
        )
        .await;
    assert_eq!(replay["continuation"]["status"], beta, "{replay}");
    boot.shutdown().await;
}

/// C2 (tracked sends): a tracked turn in flight and a tracked turn queued
/// behind it each settle `failed` once the reset ends them, never pending and
/// never completed; the queued one never runs.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_reset_settles_the_tickets_of_the_turn_in_flight_and_the_queued_turn_failed() {
    let temp = tempfile::TempDir::new().unwrap();
    let client = HoldingClient::default();
    let release_alpha = client.hold("alpha");
    let boot = Boot::start(&temp.path().join("state"), &client).await;

    let ticket = |sent: &Value| {
        sent["turn"]["ticket"]
            .as_str()
            .unwrap_or_else(|| panic!("the send is tracked: {sent}"))
            .to_string()
    };
    let alpha = boot
        .rpc(
            "meta/send",
            json!({"identity": IDENTITY, "content": "alpha: tracked, in flight", "track_turn": true}),
        )
        .await;
    let alpha_ticket = ticket(&alpha);
    client.wait_for_call("alpha").await;
    let beta = boot
        .rpc(
            "meta/send",
            json!({"identity": IDENTITY, "content": "beta: tracked, queued", "track_turn": true}),
        )
        .await;
    let beta_ticket = ticket(&beta);

    let (reset, elapsed) = boot.reset().await;
    drop(release_alpha);

    let wait_for_turn = |ticket: String| {
        let boot = &boot;
        async move {
            boot.rpc(
                "meta/wait_for_turn",
                json!({"identity": IDENTITY, "ticket": ticket, "timeout_ms": 20_000}),
            )
            .await
        }
    };
    let alpha_turn = wait_for_turn(alpha_ticket).await;
    let beta_turn = wait_for_turn(beta_ticket).await;
    eprintln!("C2 EVIDENCE reset_ms={} reset={reset}", elapsed.as_millis());
    eprintln!("C2 EVIDENCE turn alpha={alpha_turn}");
    eprintln!("C2 EVIDENCE turn beta={beta_turn}");
    assert_eq!(
        alpha_turn["state"], "failed",
        "the interrupted turn settles failed: {alpha_turn}"
    );
    assert_eq!(
        beta_turn["state"], "failed",
        "the queued turn the reset ended settles failed, never pending: {beta_turn}"
    );
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(client.calls_for("beta"), 0, "the queued turn never ran");
    assert_answer_names_no_ended_work(&reset);
    boot.shutdown().await;
}

/// C3 (unkeyed, untracked sends): the queued input ends abandoned in Core's
/// ledger like any other, but its send answered no input id, so its
/// submitter can read none of it: the identity-wide wait on the send's
/// baseline ends `incarnation_changed`, the only sign of the loss.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unkeyed_untracked_queued_send_ends_abandoned_but_answers_its_submitter_no_handle() {
    let temp = tempfile::TempDir::new().unwrap();
    let client = HoldingClient::default();
    let release_alpha = client.hold("alpha");
    let boot = Boot::start(&temp.path().join("state"), &client).await;
    let predecessor = boot.session_of().await;
    let retired = session_id(&json!(predecessor));
    let machine = boot.machine();

    boot.rpc(
        "meta/send",
        json!({"identity": IDENTITY, "content": "alpha: untracked, in flight"}),
    )
    .await;
    client.wait_for_call("alpha").await;
    let active_before = machine
        .list_active_inputs(&retired)
        .await
        .expect("Core lists the active inputs");
    let beta = boot
        .rpc(
            "meta/send",
            json!({"identity": IDENTITY, "content": "beta: untracked, queued"}),
        )
        .await;
    assert!(
        beta.get("input_id").is_none() && beta["turn"].is_null(),
        "an unkeyed, untracked send answers no handle on its input: {beta}"
    );
    // The queued input, as only Core can name it: the one active input the
    // send added.
    let beta_id = tokio::time::timeout(WAIT, async {
        loop {
            let active = machine
                .list_active_inputs(&retired)
                .await
                .expect("Core lists the active inputs");
            if let Some(added) = active.iter().find(|id| !active_before.contains(id)) {
                break added.clone();
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the send adds one active input");
    let queued = machine
        .input_state(&retired, &beta_id)
        .await
        .expect("Core's input state reads")
        .expect("Core holds the queued input");
    assert_eq!(queued.seed.phase, InputLifecycleState::Queued, "{queued:?}");

    let (reset, elapsed) = boot.reset().await;
    drop(release_alpha);
    let beta_state = boot.core_input_settled(&retired, &beta_id).await;
    let beta_receipt = boot.core_receipt(&retired, &beta_id).await;
    let completion = boot
        .rpc(
            "meta/wait_for_completion",
            json!({
                "identity": IDENTITY,
                "after": beta["completion_baseline"],
                "timeout_ms": 10_000,
            }),
        )
        .await;
    eprintln!("C3 EVIDENCE reset_ms={} reset={reset}", elapsed.as_millis());
    eprintln!("C3 EVIDENCE send beta={beta}");
    eprintln!(
        "C3 EVIDENCE core beta seed={:?} receipt={beta_receipt:?}",
        beta_state.as_ref().map(|state| &state.seed)
    );
    eprintln!("C3 EVIDENCE wait_for_completion={completion}");

    assert_ended_without_completion("beta", beta_state.as_ref(), beta_receipt.as_ref());
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(client.calls_for("beta"), 0, "the queued input never ran");
    assert_eq!(
        completion["outcome"], "incarnation_changed",
        "the submitter's only sign: the identity-wide wait ends on the rotation: {completion}"
    );
    assert_answer_names_no_ended_work(&reset);
    boot.shutdown().await;
}

/// C4 (keyed, mid tool loop): the turn in flight has committed a tool round
/// and waits in its next model call, as a turn mid tool loop does, when the
/// reset ends it. Its input ends with a durable terminal and Core's receipt,
/// the queued input ends abandoned and never runs, and the retired session's
/// transcript keeps the committed round.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_reset_mid_tool_loop_ends_the_turn_and_the_queued_input_with_durable_terminals() {
    let temp = tempfile::TempDir::new().unwrap();
    let client = HoldingClient::default();
    let release_round = client.hold_after_tool_round("alpha");
    let boot = Boot::start(&temp.path().join("state"), &client).await;

    let alpha = boot
        .keyed_send_admitted("alpha: the turn mid tool loop", "k-alpha")
        .await;
    client.wait_for_call_after_round().await;
    let beta = boot
        .keyed_send_admitted("beta: the input queued behind it", "k-beta")
        .await;
    let queued = boot.send_status(&beta).await;
    assert_eq!(
        queued["state"]["current_state"], "queued",
        "beta waits behind alpha's turn: {queued}"
    );

    let (reset, elapsed) = boot.reset().await;
    drop(release_round);

    let retired = session_id(&alpha["session_id"]);
    let alpha_read = boot.send_status_settled(&alpha).await;
    let beta_read = boot.send_status_settled(&beta).await;
    let alpha_state = boot
        .core_input_settled(&retired, &input_id(&alpha["input_id"]))
        .await;
    let beta_state = boot
        .core_input_settled(&retired, &input_id(&beta["input_id"]))
        .await;
    let alpha_receipt = boot
        .core_receipt(&retired, &input_id(&alpha["input_id"]))
        .await;
    let beta_receipt = boot
        .core_receipt(&retired, &input_id(&beta["input_id"]))
        .await;
    let transcript = boot.transcript(&retired).await;
    eprintln!("C4 EVIDENCE reset_ms={} reset={reset}", elapsed.as_millis());
    eprintln!("C4 EVIDENCE send_status alpha={alpha_read}");
    eprintln!("C4 EVIDENCE send_status beta={beta_read}");
    eprintln!(
        "C4 EVIDENCE core alpha seed={:?} receipt={alpha_receipt:?}",
        alpha_state.as_ref().map(|state| &state.seed)
    );
    eprintln!(
        "C4 EVIDENCE core beta seed={:?} receipt={beta_receipt:?}",
        beta_state.as_ref().map(|state| &state.seed)
    );
    for (index, line) in transcript.iter().enumerate().skip(1) {
        eprintln!("C4 EVIDENCE retired transcript[{index}]={line}");
    }

    assert!(
        alpha_state
            .as_ref()
            .is_some_and(|state| state.seed.terminal_outcome.is_some()),
        "the turn mid tool loop ends with a durable terminal: {alpha_state:?}"
    );
    assert!(
        matches!(
            alpha_receipt,
            Some(
                InputTerminalReceiptObservation::Finalized(_)
                    | InputTerminalReceiptObservation::TerminalWithoutReceipt { .. }
            )
        ),
        "Core's terminal read of the interrupted turn is final: {alpha_receipt:?}"
    );
    assert_ended_without_completion("beta", beta_state.as_ref(), beta_receipt.as_ref());
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(client.calls_for("beta"), 0, "the queued input never ran");
    assert!(
        transcript.iter().all(|line| !line.contains("beta")),
        "the queued input is in no transcript of the retired session"
    );
    assert_answer_names_no_ended_work(&reset);
    boot.shutdown().await;
}

/// C5 (the idle-only reset a host can compose today): `meta/retire` refuses,
/// typed and retryable, while the member has a turn in flight and input
/// queued, and interrupts nothing; once the member is quiescent the retire
/// releases it and the reset that follows ends no input.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn retire_first_refuses_while_work_is_in_flight_and_interrupts_nothing() {
    let temp = tempfile::TempDir::new().unwrap();
    let client = HoldingClient::default();
    let release_alpha = client.hold("alpha");
    let boot = Boot::start(&temp.path().join("state"), &client).await;

    let alpha = boot
        .keyed_send_admitted("alpha: the turn in flight", "k-alpha")
        .await;
    client.wait_for_call("alpha").await;
    let beta = boot
        .keyed_send_admitted("beta: the input queued behind it", "k-beta")
        .await;

    let refused = boot
        .call("meta/retire", json!({"identity": IDENTITY}))
        .await;
    eprintln!("C5 EVIDENCE retire while busy={refused}");
    assert_eq!(refused["error"]["code"], -32052, "{refused}");
    assert_eq!(
        refused["error"]["data"]["kind"], "identity_wake_retryable",
        "{refused}"
    );
    let alpha_now = boot.send_status(&alpha).await;
    let beta_now = boot.send_status(&beta).await;
    assert!(
        alpha_now["state"]["terminal_outcome"].is_null(),
        "the refused retire interrupted nothing: {alpha_now}"
    );
    assert_eq!(
        beta_now["state"]["current_state"], "queued",
        "the refused retire left the queued input queued: {beta_now}"
    );

    drop(release_alpha);
    let alpha_done = boot.send_status_settled(&alpha).await;
    let beta_done = boot.send_status_settled(&beta).await;
    assert_eq!(
        alpha_done["state"]["terminal_outcome"], "completed",
        "{alpha_done}"
    );
    assert_eq!(
        beta_done["state"]["terminal_outcome"], "completed",
        "{beta_done}"
    );
    assert_eq!(client.calls_for("beta"), 1, "the queued input ran once");

    // Quiescent now: the retire releases the member, and the reset that
    // follows rotates an idle incarnation.
    let retired = tokio::time::timeout(WAIT, async {
        loop {
            let reply = boot
                .call("meta/retire", json!({"identity": IDENTITY}))
                .await;
            if reply.get("error").is_none() {
                break reply;
            }
            assert_eq!(reply["error"]["code"], -32052, "{reply}");
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("the retire succeeds once the member is quiescent");
    let (reset, elapsed) = boot.reset().await;
    eprintln!("C5 EVIDENCE retire when idle={retired}");
    eprintln!("C5 EVIDENCE reset_ms={} reset={reset}", elapsed.as_millis());
    assert_ne!(
        reset["result"]["session_id"], alpha["session_id"],
        "{reset}"
    );
    boot.shutdown().await;
}

/// C6 (keyed, boundary cancel): the turn in flight reaches its boundary
/// inside the retirement's cooperative grace, so the boundary cancel ends it
/// without a hard cancel. It still ends cancelled, and the input queued
/// behind it still ends cancelled without running: Core cancels the queued
/// input before the runtime starts it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_reset_whose_turn_reaches_its_boundary_in_the_grace_still_ends_both_inputs() {
    let temp = tempfile::TempDir::new().unwrap();
    let client = HoldingClient::default();
    let release_alpha = client.hold("alpha");
    let boot = Boot::start(&temp.path().join("state"), &client).await;

    let alpha = boot
        .keyed_send_admitted("alpha: the turn in flight", "k-alpha")
        .await;
    client.wait_for_call("alpha").await;
    let beta = boot
        .keyed_send_admitted("beta: the input queued behind it", "k-beta")
        .await;

    // The model answers 300 ms into the reset, well inside the two-second
    // grace before the retirement's hard cancel.
    let ((reset, elapsed), ()) = tokio::join!(boot.reset(), async move {
        tokio::time::sleep(Duration::from_millis(300)).await;
        drop(release_alpha);
    });
    let retired = session_id(&alpha["session_id"]);
    let alpha_read = boot.send_status_settled(&alpha).await;
    let beta_read = boot.send_status_settled(&beta).await;
    let beta_state = boot
        .core_input_settled(&retired, &input_id(&beta["input_id"]))
        .await;
    let beta_receipt = boot
        .core_receipt(&retired, &input_id(&beta["input_id"]))
        .await;
    eprintln!("C6 EVIDENCE reset_ms={} reset={reset}", elapsed.as_millis());
    eprintln!("C6 EVIDENCE send_status alpha={alpha_read}");
    eprintln!("C6 EVIDENCE send_status beta={beta_read}");
    eprintln!(
        "C6 EVIDENCE core beta seed={:?} receipt={beta_receipt:?}",
        beta_state.as_ref().map(|state| &state.seed)
    );

    assert_eq!(
        alpha_read["state"]["terminal_outcome"], "cancelled",
        "the boundary cancel ends the turn cancelled, not completed: {alpha_read}"
    );
    assert_ended_without_completion("beta", beta_state.as_ref(), beta_receipt.as_ref());
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(client.calls_for("beta"), 0, "the queued input never ran");
    assert_answer_names_no_ended_work(&reset);
    boot.shutdown().await;
}
