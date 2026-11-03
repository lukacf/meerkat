#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! A keyed `meta/send` with the caller's handling mode, through the gateway
//! on real persistent stores: the identity runtime over the gateway's own
//! mob, whose agent mob-tool state binds Core's continuation service.
//!
//! The mode is committed with the content under the key, as Core keeps it:
//! a keyed steer joins the member's running turn at its next boundary and
//! starts a turn on an idle member, a replay answers the first receipt and
//! never delivers twice, and the key refuses another mode as it refuses
//! other content. A keyed send without a mode still queues.
//!
//! The model double holds the first call of the turn whose prompt is
//! [`BUSY_PROMPT`] until the row releases it, then calls one tool, so that
//! turn runs a second model call after its tool round. Every other call
//! answers at once. Each request's messages are recorded in call order.

use std::collections::BTreeMap;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use meerkat_client::{LlmError, LlmEvent, LlmRequest};
use meerkat_core::types::{HandlingMode, StopReason};
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
use serde_json::{Value, json};
use tokio::sync::Semaphore;

#[path = "support/llm_usage.rs"]
mod llm_usage;

const WAIT: Duration = Duration::from_secs(30);
const IDENTITY: &str = "personal:alice";
const ANSWER: &str = "steer-answer";
const BUSY_PROMPT: &str = "BUSY-TURN-ROMEO-4: work through the round.";
const ROUND_CALL_ID: &str = "busy-round-call";

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
    format!("keyed-steer-{}", uuid::Uuid::new_v4())
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
        .expect("parse keyed-steer mob definition")
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

/// The member's model; see the module docs.
#[derive(Clone)]
struct SteerProbeClient {
    requests: Arc<Mutex<Vec<String>>>,
    held_call: Arc<Mutex<Option<usize>>>,
    entered: Arc<Semaphore>,
    release: Arc<Semaphore>,
}

impl SteerProbeClient {
    fn new() -> Self {
        Self {
            requests: Arc::new(Mutex::new(Vec::new())),
            held_call: Arc::new(Mutex::new(None)),
            entered: Arc::new(Semaphore::new(0)),
            release: Arc::new(Semaphore::new(0)),
        }
    }

    fn requests(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }

    /// The call indexes of the recorded requests that mention `needle`.
    fn calls_mentioning(&self, needle: &str) -> Vec<usize> {
        self.requests()
            .iter()
            .enumerate()
            .filter(|(_, request)| request.contains(needle))
            .map(|(index, _)| index)
            .collect()
    }

    /// The index of the held call, once the busy turn reached it.
    async fn wait_held(&self) -> usize {
        tokio::time::timeout(WAIT, self.entered.acquire())
            .await
            .expect("the busy turn reaches its first model call")
            .expect("the model gate")
            .forget();
        self.held_call
            .lock()
            .unwrap()
            .expect("the held call is recorded")
    }

    fn release_held(&self) {
        self.release.add_permits(1);
    }
}

impl meerkat_client::LlmClient for SteerProbeClient {
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
        let holds = request.messages.last().is_some_and(|last| {
            serde_json::to_string(last).is_ok_and(|text| text.contains(BUSY_PROMPT))
        });
        let index = {
            let mut requests = self.requests.lock().unwrap();
            requests.push(
                serde_json::to_string(&request.messages).expect("request messages serialize"),
            );
            requests.len() - 1
        };
        if holds {
            *self.held_call.lock().unwrap() = Some(index);
            let tool = request
                .tools
                .iter()
                .find(|tool| tool.name == "peers")
                .or_else(|| request.tools.first())
                .map(|tool| tool.name.to_string())
                .expect("the busy turn offers a tool");
            let [usage, done] =
                llm_usage::usage_then_done(request, meerkat::Provider::OpenAI, StopReason::ToolUse);
            let entered = Arc::clone(&self.entered);
            let release = Arc::clone(&self.release);
            return Box::pin(async_stream::stream! {
                entered.add_permits(1);
                if let Ok(permit) = release.acquire().await {
                    permit.forget();
                }
                yield Ok(LlmEvent::ToolCallComplete {
                    id: ROUND_CALL_ID.to_string(),
                    name: tool,
                    args: json!({}),
                    meta: None,
                });
                yield Ok(usage);
                yield Ok(done);
            });
        }
        let [usage, done] =
            llm_usage::usage_then_done(request, meerkat::Provider::OpenAI, StopReason::EndTurn);
        Box::pin(async_stream::stream! {
            yield Ok(LlmEvent::TextDelta { delta: ANSWER.to_string(), meta: None });
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

/// One boot of the gateway: the unified runtime, its identity runtime and
/// the RPC context, with the identity restored.
struct Boot {
    unified: meerkat_meta::UnifiedRuntime,
    ctx: meerkat_meta::rpc::IdentityFirstContext,
}

impl Boot {
    async fn start(state_path: &std::path::Path, client: &SteerProbeClient) -> Self {
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
            identity_rt,
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
                runtime_instance_id: "keyed-steer".to_string(),
                has_runtime_store: true,
                durability_policy: DurabilityPolicy::SyncWriteThrough,
                bridge: Some(bridge),
                default_timeout: None,
            })
            .with_runtime_services(AgentRuntimeServices::new(unified.mob_handle())),
        )
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

    /// A keyed `meta/send` of `text` under `key`, with `mode` as its
    /// `handling_mode` when given; the whole reply.
    async fn keyed_call(&self, text: &str, key: &str, mode: Option<&str>) -> Value {
        let mut params = json!({"identity": IDENTITY, "content": text, "idempotency_key": key});
        if let Some(mode) = mode {
            params["handling_mode"] = json!(mode);
        }
        self.call("meta/send", params).await
    }

    /// A keyed `meta/send` that must answer a continuation; the result.
    async fn keyed(&self, text: &str, key: &str, mode: Option<&str>) -> Value {
        let reply = self.keyed_call(text, key, mode).await;
        assert!(
            reply.get("error").is_none(),
            "keyed meta/send failed: {reply}"
        );
        let result = reply["result"].clone();
        assert!(
            result["continuation"]["delivery_id"].is_string(),
            "a keyed send answers its continuation: {result}"
        );
        result
    }

    /// Replay `key` until its status is `admitted`; that answer.
    async fn replay_until_admitted(&self, text: &str, key: &str, mode: Option<&str>) -> Value {
        tokio::time::timeout(WAIT, async {
            loop {
                let result = self.keyed(text, key, mode).await;
                if state(&result) == "admitted" {
                    break result;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("the keyed send {key} is admitted"))
    }

    /// `meta/send_status` of an admitted pair until its input is terminal;
    /// that result.
    async fn status_until_terminal(&self, status: &Value) -> Value {
        tokio::time::timeout(WAIT, async {
            loop {
                let read = self
                    .rpc(
                        "meta/send_status",
                        json!({
                            "identity": IDENTITY,
                            "session_id": status["session_id"],
                            "input_id": status["input_id"],
                        }),
                    )
                    .await;
                if read["state"]["terminal_outcome"].is_string() {
                    break read;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .expect("the input reaches a terminal state")
    }

    /// Start the turn whose first model call holds; the held call's index.
    async fn start_busy_turn(&self, client: &SteerProbeClient) -> usize {
        self.rpc(
            "meta/send",
            json!({"identity": IDENTITY, "content": BUSY_PROMPT}),
        )
        .await;
        client.wait_held().await
    }

    async fn shutdown(self) {
        self.unified.shutdown().await;
    }
}

fn identity() -> AgentIdentity {
    AgentIdentity::parse(IDENTITY).unwrap()
}

/// The two fields a replay answers unchanged.
fn delivery(result: &Value) -> (String, u64) {
    (
        result["continuation"]["delivery_id"]
            .as_str()
            .expect("delivery id")
            .to_string(),
        result["continuation"]["committed_at_ms"]
            .as_u64()
            .expect("committed_at_ms"),
    )
}

/// [`delivery`] of a receipt the Rust surface answered.
fn receipt_delivery(receipt: &meerkat_contracts::WireContinuationReceipt) -> (String, u64) {
    (receipt.delivery_id.clone(), receipt.committed_at_ms)
}

fn state(result: &Value) -> &str {
    result["continuation"]["status"]["state"]
        .as_str()
        .unwrap_or_default()
}

/// A keyed steer to a member whose turn is running joins that turn: the
/// turn's next model call, the one after its tool round, carries the
/// steered content. The steer is committed through the Rust surface; its
/// replay through `meta/send` with `handling_mode: "steer"` answers the
/// same receipt, so both surfaces commit the same mode.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_keyed_steer_to_a_busy_member_joins_its_running_turn() {
    const STEER: &str = "STEER-SIERRA-1: also check the second ledger.";
    let temp = tempfile::TempDir::new().unwrap();
    let client = SteerProbeClient::new();
    let boot = Boot::start(&temp.path().join("state"), &client).await;
    let held = boot.start_busy_turn(&client).await;

    let committed = boot
        .ctx
        .runtime
        .send_keyed_with_handling_mode(
            &identity(),
            None,
            &meerkat_core::ContentInput::Text(STEER.to_string()),
            HandlingMode::Steer,
            "steer-busy",
        )
        .await
        .expect("the keyed steer commits");
    let admitted = boot
        .replay_until_admitted(STEER, "steer-busy", Some("steer"))
        .await;
    assert_eq!(
        delivery(&admitted),
        receipt_delivery(&committed),
        "the replay over meta/send answers the Rust surface's receipt"
    );
    client.release_held();
    let terminal = boot
        .status_until_terminal(&admitted["continuation"]["status"])
        .await;
    boot.shutdown().await;

    assert_eq!(
        terminal["state"]["terminal_outcome"], "completed",
        "{terminal}"
    );
    let requests = client.requests();
    let next = requests
        .get(held + 1)
        .expect("the busy turn ran a model call after its tool round");
    assert!(
        next.contains(ROUND_CALL_ID),
        "the call after the held one continues the busy turn: {next}"
    );
    assert_eq!(
        client.calls_mentioning(STEER).first(),
        Some(&(held + 1)),
        "the steer joins the running turn at its next boundary: {requests:#?}"
    );
}

/// A keyed steer to an idle member starts its turn, once.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_keyed_steer_to_an_idle_member_starts_its_turn() {
    const STEER: &str = "STEER-SIERRA-2: start on the inventory.";
    let temp = tempfile::TempDir::new().unwrap();
    let client = SteerProbeClient::new();
    let boot = Boot::start(&temp.path().join("state"), &client).await;

    let admitted = boot
        .replay_until_admitted(STEER, "steer-idle", Some("steer"))
        .await;
    let terminal = boot
        .status_until_terminal(&admitted["continuation"]["status"])
        .await;
    boot.shutdown().await;

    assert_eq!(
        terminal["state"]["terminal_outcome"], "completed",
        "{terminal}"
    );
    let requests = client.requests();
    let calls = client.calls_mentioning(STEER);
    assert_eq!(calls.len(), 1, "one turn ran the steer: {requests:#?}");
    let last = serde_json::from_str::<Vec<Value>>(&requests[calls[0]])
        .expect("recorded messages")
        .last()
        .cloned()
        .expect("a message");
    assert!(
        last.to_string().contains(STEER),
        "the steer is the turn's own input: {last}"
    );
}

/// A host that lost a keyed steer's answer replays the key: the replay
/// answers the first receipt (its delivery id and commit time), before and
/// after the steer ran, and the content is delivered once.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_replayed_keyed_steer_answers_its_first_receipt_and_delivers_once() {
    const STEER: &str = "STEER-SIERRA-3: reconcile the totals.";
    let temp = tempfile::TempDir::new().unwrap();
    let client = SteerProbeClient::new();
    let boot = Boot::start(&temp.path().join("state"), &client).await;

    // The gateway answered; the host lost the answer.
    let lost = boot.keyed(STEER, "steer-replay", Some("steer")).await;
    let replay = boot.keyed(STEER, "steer-replay", Some("steer")).await;
    let admitted = boot
        .replay_until_admitted(STEER, "steer-replay", Some("steer"))
        .await;
    boot.status_until_terminal(&admitted["continuation"]["status"])
        .await;
    let after = boot.keyed(STEER, "steer-replay", Some("steer")).await;
    boot.shutdown().await;

    assert_eq!(delivery(&replay), delivery(&lost), "the first receipt");
    assert_eq!(delivery(&admitted), delivery(&lost), "the first receipt");
    assert_eq!(delivery(&after), delivery(&lost), "the first receipt");
    assert_eq!(
        after["continuation"]["status"], admitted["continuation"]["status"],
        "the first admission, never a second one"
    );
    assert_eq!(
        client.calls_mentioning(STEER).len(),
        1,
        "delivered once: {:#?}",
        client.requests()
    );
}

/// A key a steer used refuses the same content in another mode, and other
/// content in the same mode, as `input_idempotency_conflict` naming the
/// first delivery, which stands.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_keyed_steer_key_refuses_another_mode_and_other_content() {
    const STEER: &str = "STEER-SIERRA-4: hold the release.";
    let temp = tempfile::TempDir::new().unwrap();
    let client = SteerProbeClient::new();
    let boot = Boot::start(&temp.path().join("state"), &client).await;

    let first = boot.keyed(STEER, "steer-conflict", Some("steer")).await;
    let (delivery_id, committed_at_ms) = delivery(&first);
    let refusals = [
        (
            "the default mode",
            boot.keyed_call(STEER, "steer-conflict", None).await,
        ),
        (
            "queue",
            boot.keyed_call(STEER, "steer-conflict", Some("queue"))
                .await,
        ),
        (
            "other content",
            boot.keyed_call("Something else.", "steer-conflict", Some("steer"))
                .await,
        ),
    ];
    let replay = boot.keyed(STEER, "steer-conflict", Some("steer")).await;
    boot.shutdown().await;

    for (case, refused) in refusals {
        assert_eq!(refused["error"]["code"], -32602, "{case}: {refused}");
        assert_eq!(
            refused["error"]["data"],
            json!({
                "kind": "input_idempotency_conflict",
                "delivery_id": delivery_id,
                "committed_at_ms": committed_at_ms,
            }),
            "{case}: {refused}"
        );
    }
    assert_eq!(
        delivery(&replay),
        delivery(&first),
        "the first delivery stands"
    );
}

/// The keyed send without a mode still queues: sent while the member's turn
/// runs, through the Rust surface's `send_keyed` and replayed over
/// `meta/send` without `handling_mode`, it runs after that turn, never in
/// it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_keyed_send_without_a_mode_still_queues_behind_the_running_turn() {
    const QUEUED: &str = "QUEUED-QUEBEC-5: file the report afterwards.";
    let temp = tempfile::TempDir::new().unwrap();
    let client = SteerProbeClient::new();
    let boot = Boot::start(&temp.path().join("state"), &client).await;
    let held = boot.start_busy_turn(&client).await;

    let committed = boot
        .ctx
        .runtime
        .send_keyed(
            &identity(),
            None,
            &meerkat_core::ContentInput::Text(QUEUED.to_string()),
            "queue-busy",
        )
        .await
        .expect("the keyed send commits");
    let admitted = boot.replay_until_admitted(QUEUED, "queue-busy", None).await;
    assert_eq!(delivery(&admitted), receipt_delivery(&committed));
    client.release_held();
    boot.status_until_terminal(&admitted["continuation"]["status"])
        .await;
    boot.shutdown().await;

    let requests = client.requests();
    let next = requests
        .get(held + 1)
        .expect("the busy turn ran a model call after its tool round");
    assert!(next.contains(ROUND_CALL_ID), "{next}");
    let first = client.calls_mentioning(QUEUED).first().copied();
    assert!(
        first.is_some_and(|call| call > held + 1),
        "the queued send runs after the busy turn: {requests:#?}"
    );
}

/// A steer requires a key, and `"queue"` is the default: an unkeyed
/// `meta/send` takes `handling_mode: "queue"` and sends as before, and
/// refuses `"steer"` (an unkeyed send queues) and an unknown mode, keyed or
/// not, committing nothing. The gateway advertises the keyed mode for
/// clients to check before they steer.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn meta_send_takes_an_unkeyed_queue_and_refuses_an_unkeyed_steer_or_unknown_mode() {
    let temp = tempfile::TempDir::new().unwrap();
    let client = SteerProbeClient::new();
    let boot = Boot::start(&temp.path().join("state"), &client).await;
    let unkeyed = |mode: &str| {
        json!({
            "identity": IDENTITY,
            "content": format!("STEER-SIERRA-6: unkeyed {mode}."),
            "handling_mode": mode,
        })
    };

    let queued = boot.call("meta/send", unkeyed("queue")).await;
    let steer = boot.call("meta/send", unkeyed("steer")).await;
    let unknown_unkeyed = boot.call("meta/send", unkeyed("sideways")).await;
    let unknown_keyed = boot
        .keyed_call("STEER-SIERRA-6: sideways.", "steer-mode", Some("sideways"))
        .await;
    // The refused keyed call committed nothing: other content under its key
    // commits, never answered as a conflict.
    let committed = boot
        .keyed(
            "STEER-SIERRA-6: after the refusals.",
            "steer-mode",
            Some("steer"),
        )
        .await;
    let capabilities = boot.rpc("meta/capabilities", json!({})).await;
    boot.shutdown().await;

    assert!(queued.get("error").is_none(), "{queued}");
    assert!(
        queued["result"]["fencing_token"].is_u64(),
        "an unkeyed queue sends as before: {queued}"
    );
    assert_eq!(steer["error"]["code"], -32602, "{steer}");
    let message = steer["error"]["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("idempotency_key") && message.contains("steer"),
        "{steer}"
    );
    for unknown in [&unknown_unkeyed, &unknown_keyed] {
        assert_eq!(unknown["error"]["code"], -32602, "{unknown}");
        assert!(
            unknown["error"]["message"]
                .as_str()
                .is_some_and(|message| message.contains("handling_mode")),
            "{unknown}"
        );
    }
    assert!(
        ["pending", "admitted"].contains(&state(&committed)),
        "{committed}"
    );
    assert!(
        capabilities["feature_capabilities"]
            .as_array()
            .is_some_and(|features| features.contains(&json!("keyed_send_handling_mode"))),
        "{capabilities}"
    );
}
