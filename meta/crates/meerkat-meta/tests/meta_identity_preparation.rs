#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    clippy::uninlined_format_args
)]

//! The host's preparation of an agent created at run time.
//!
//! A launch's identity policy may name a preparation for a profile
//! (`preparation.<profile>.mode: "required"`): an agent of that profile is
//! committed pending its preparation, has no member, runs nothing and wakes
//! nothing until its host settles it (`meta/identity/settle_preparation`).
//! A host that gives up on an agent removes it (`meta/identity/remove`).
//!
//! The rows of the first half speak JSON-RPC to a real `rpc_gateway` process
//! as the stdio host (the operator). The rows of the second half boot the
//! gateway's runtime composition in process, with a members' model that
//! records every request it is asked, so that a row can say what ran and
//! what did not.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::pin::Pin;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use meerkat_client::{LlmError, LlmEvent, LlmRequest};
use meerkat_core::Message;
use meerkat_core::types::StopReason;
use meerkat_meta::identity_creation::{
    ChangeRequester, GetAgent, IdentityChangeCall, IdentityChangeSpec, IdentityCreationPolicy,
    IdentityFamilyOptions, IdentityRead, IdentityReadAnswer, OperationId, SettlePreparationCall,
};
use meerkat_meta::identity_first::contracts::RosterProvider;
use meerkat_meta::identity_first::{
    AgentAddressability, AgentIdentity, DurableAgentSpec, MutableRosterProvider, RosterContext,
    RosterError,
};
use meerkat_meta::{UnifiedRuntime, UnifiedRuntimeBuilder};
use meerkat_mob::{EntryStanding, IdentityEntryRead, MobDefinition, ProfileName};
use serde_json::{Value, json};

#[path = "support/llm_usage.rs"]
mod llm_usage;

/// A configured agent.
const HOST_A: &str = "host-a";

const MOB_CONFIG: &str = r#"
[mob]
id = "meta-identity-preparation-test"

[profiles.worker]
model = "gpt-5.5"
external_addressable = true
runtime_mode = "turn_driven"

[profiles.worker.tools]
comms = true
"#;

/// The JSON-RPC code of every refusal of the family.
const REFUSED: i64 = -32058;
const LINE_WAIT: Duration = Duration::from_mins(3);
/// Bounds a failure only; a passing row never waits for it.
const TURN_BOUND: Duration = Duration::from_secs(90);
/// How long a row watches for something that must not happen.
const QUIET: Duration = Duration::from_millis(1500);

const SETTLE: &str = "meta/identity/settle_preparation";

/// The rows' policy: the host prepares every agent of the profile `worker`
/// before its first use, and the stdio host may make every change of such
/// an agent at once.
fn prepared_policy() -> Value {
    json!({
        "enabled": true,
        "preparation": {"worker": {"mode": "required"}},
        "approval": {"rules": [{
            "change": ["create", "descriptive", "capability", "peers", "profile", "archive",
                       "remove", "adopt"],
            "requester": {"kind": "host"},
            "target": {"profile": ["worker"]},
            "outcome": {"kind": "allow"},
        }]},
        "peers": {"default": "allow"},
    })
}

/// One `rpc_gateway` process on a pipe. Requests go to its stdin; a reader
/// thread hands every stdout line to the row, and the row answers the
/// gateway's roster callback as the SDK host.
struct Gateway {
    child: Child,
    stdin: ChildStdin,
    lines: std::sync::mpsc::Receiver<String>,
    next_id: u64,
}

impl Gateway {
    fn start(scratch: &Path) -> Self {
        let stderr = File::create(scratch.join(format!(
            "gateway-{}.stderr",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        )))
        .expect("gateway stderr file");
        let mut child = Command::new(env!("CARGO_BIN_EXE_rpc_gateway"))
            .arg("--persistent")
            .env("OPENAI_API_KEY", "sk-regression-test")
            .env("XDG_STATE_HOME", scratch.join("xdg-state"))
            .env("XDG_CONFIG_HOME", scratch.join("xdg-config"))
            .current_dir(scratch)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::from(stderr))
            .spawn()
            .expect("spawn rpc_gateway");
        let stdin = child.stdin.take().expect("gateway stdin");
        let stdout = child.stdout.take().expect("gateway stdout");
        let (sender, lines) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if sender.send(line).is_err() {
                    break;
                }
            }
        });
        Self {
            child,
            stdin,
            lines,
            next_id: 0,
        }
    }

    fn send(&mut self, message: &Value) {
        writeln!(self.stdin, "{message}").expect("write");
        self.stdin.flush().expect("flush");
    }

    /// Send `method` and read stdout until its answer, answering the
    /// gateway's roster callback on the way.
    fn rpc(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let id = format!("preparation-{}", self.next_id);
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        loop {
            let line = self
                .lines
                .recv_timeout(LINE_WAIT)
                .unwrap_or_else(|_| panic!("no answer to {method}"));
            let Some(message) = line
                .find('{')
                .and_then(|start| serde_json::from_str::<Value>(line[start..].trim()).ok())
            else {
                continue;
            };
            if message.get("id") == Some(&json!(id)) && message.get("method").is_none() {
                return message;
            }
            if let (Some(callback), Some(callback_id)) = (
                message.get("method").and_then(Value::as_str),
                message.get("id").cloned(),
            ) {
                let reply = match callback {
                    "callback/roster_provider/roster" => json!({
                        "jsonrpc": "2.0",
                        "id": callback_id,
                        "result": [{"identity": HOST_A, "profile": "worker"}],
                    }),
                    other => json!({
                        "jsonrpc": "2.0",
                        "id": callback_id,
                        "error": {"code": -32601, "message": format!("no handler for {other}")},
                    }),
                };
                self.send(&reply);
            }
        }
    }

    /// `meta/init` on `state` with the identity roster and the identity
    /// policy `policy`.
    fn init(&mut self, state: &Path, policy: Value) -> Value {
        self.rpc(
            "meta/init",
            json!({
                "mob_config": MOB_CONFIG,
                "persistent_state": state,
                "has_roster_provider": true,
                "runtime_options": {"demo_llm": true, "identity_creation": policy},
            }),
        )
    }

    /// A gateway launched on `scratch` with `policy`, its init accepted.
    fn launched(scratch: &Path, policy: Value) -> Self {
        let mut gateway = Self::start(scratch);
        let init = gateway.init(&scratch.join("state"), policy);
        assert!(
            init["error"].is_null() && init["result"]["contract_version"].is_string(),
            "the launch is accepted: {init:#}"
        );
        gateway
    }

    /// The result of `method`, which must answer one.
    fn ok(&mut self, method: &str, params: Value) -> Value {
        let answer = self.rpc(method, params.clone());
        assert!(
            answer["error"].is_null(),
            "{method} {params} answers a result: {answer:#}"
        );
        answer["result"].clone()
    }

    /// The refusal of `method`: the `data` of the family's one error.
    fn refused(&mut self, method: &str, params: Value) -> Value {
        let answer = self.rpc(method, params.clone());
        assert_eq!(
            answer["error"]["code"],
            json!(REFUSED),
            "{method} {params} is refused with the family's code: {answer:#}"
        );
        let data = answer["error"]["data"].clone();
        assert_eq!(
            answer["error"]["message"], data["detail"],
            "the error's message is the refusal's sentence: {answer:#}"
        );
        data
    }

    fn get(&mut self, name: &str) -> Value {
        self.ok("meta/identity/get", json!({"name": name}))
    }

    /// The readiness a list gives `name`.
    fn listed_readiness(&mut self, name: &str) -> Value {
        let listed = self.ok("meta/identity/list", json!({}));
        let agents = listed["agents"].as_array().expect("a list names agents");
        agents
            .iter()
            .find(|agent| agent["name"] == json!(name))
            .unwrap_or_else(|| panic!("the list names {name}: {agents:?}"))["readiness"]
            .clone()
    }

    /// The refusal of a send to `name`: the error's data.
    fn send_refused(&mut self, name: &str) -> Value {
        let sent = self.rpc(
            "meta/send",
            json!({"identity": name, "content": "anything in the queue?"}),
        );
        assert!(sent["result"].is_null(), "{sent:#}");
        sent["error"]["data"].clone()
    }

    /// One tracked turn of `name`, waited until it completes.
    fn turn(&mut self, name: &str, text: &str) {
        let sent = self.ok(
            "meta/send",
            json!({"identity": name, "content": text, "track_turn": true}),
        );
        let ticket = sent["turn"]["ticket"]
            .as_str()
            .unwrap_or_else(|| panic!("an untracked turn: {sent}"))
            .to_string();
        let waited = self.ok(
            "meta/wait_for_turn",
            json!({"identity": name, "ticket": ticket, "timeout_ms": 90_000}),
        );
        assert_eq!(
            waited["state"],
            json!("completed"),
            "the turn {text:?} completes: {waited}"
        );
    }

    /// Create the agent `name` of the prepared profile as the request
    /// `request_id`: it is committed pending its preparation.
    fn create_pending(&mut self, request_id: &str, name: &str) -> Value {
        let created = self.ok(
            "meta/identity/create",
            json!({"request_id": request_id, "agents": [{"name": name, "profile": "worker"}]}),
        );
        assert_eq!(
            created["outcome"],
            json!("pending_preparation"),
            "{created:#}"
        );
        let agent = &created["agents"][0];
        assert_eq!(agent["state"], json!("pending_preparation"), "{created:#}");
        assert!(
            agent.get("seat").is_none() && agent.get("session_id").is_none(),
            "a pending agent has no seat and no session: {created:#}"
        );
        created
    }
}

impl Drop for Gateway {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A launch whose policy names the preparation `required` for a profile is
/// accepted. A `constructed` preparation is still refused at init, and what
/// a launch serves of a policy says so by naming `required`.
#[test]
fn a_launch_serves_a_required_preparation_and_refuses_a_constructed_one() {
    let scratch = tempfile::tempdir().unwrap();
    let mut gateway = Gateway::start(scratch.path());
    let init = gateway.init(&scratch.path().join("state"), prepared_policy());
    assert!(
        init["error"].is_null() && init["result"]["contract_version"].is_string(),
        "a launch that prepares a profile's agents is accepted: {init:#}"
    );
    drop(gateway);

    // The stdio gateway is no native host: a held construction is refused
    // for that, first.
    let scratch = tempfile::tempdir().unwrap();
    let mut gateway = Gateway::start(scratch.path());
    let mut constructed = prepared_policy();
    constructed["preparation"]["worker"] =
        json!({"mode": "constructed", "construction_deadline_ms": 60_000});
    let init = gateway.init(&scratch.path().join("state"), constructed.clone());
    assert_eq!(init["error"]["code"], json!(-32602), "{init:#}");
    let message = init["error"]["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("identity_creation.preparation.worker")
            && message.contains("needs a native host"),
        "{message}"
    );

    // What a launch serves of a policy, on any host: `required` is served,
    // `constructed` is not, and its refusal names the mode a host writes.
    let served = |policy: &Value| {
        IdentityCreationPolicy::parse(policy)
            .expect("the policy parses")
            .expect("the policy is enabled")
            .check_served()
    };
    assert!(served(&prepared_policy()).is_ok());
    let refused = served(&constructed).expect_err("a held construction is not served");
    let text = refused.to_string();
    assert!(
        text.contains("preparation.worker.mode")
            && text.contains("constructed")
            && text.contains("required"),
        "{text}"
    );
}

/// An agent of a profile its host prepares is committed pending its
/// preparation: its create answers `pending_preparation`; get and list read
/// it `preparation_pending`; the directory names it as an agent like any
/// other; a send to it is refused retryable and its archive is refused
/// retryable. The host's settle seats it: the settle answers `done` with
/// the agent `committed` and `seated`, the agent reads `dormant`, the
/// create's operation reads `done`, and the agent takes its first delivery.
/// The same settle again answers the same; a settle of an agent that awaits
/// none, of a configured agent and of a name no agent holds is refused
/// typed, and a settle under another change's request id is
/// `operation_reused`.
#[test]
fn a_pending_agent_is_listed_as_pending_and_is_seated_by_its_hosts_settle() {
    let scratch = tempfile::tempdir().unwrap();
    let mut gateway = Gateway::launched(scratch.path(), prepared_policy());
    let capabilities = gateway.ok("meta/capabilities", json!({}));
    assert!(
        capabilities.to_string().contains(SETTLE),
        "the launch lists the settle among its methods: {capabilities:#}"
    );
    let created = gateway.create_pending("create-1", "scout");

    let scout = gateway.get("scout");
    assert_eq!(
        scout["readiness"],
        json!("preparation_pending"),
        "{scout:#}"
    );
    assert_eq!(scout["archived"], json!(false), "{scout:#}");
    assert_eq!(
        gateway.listed_readiness("scout"),
        json!("preparation_pending")
    );
    let directory = gateway.ok("meta/identity/directory", json!({}));
    assert!(
        directory["peers"]
            .as_array()
            .is_some_and(|peers| peers.iter().any(|peer| peer["name"] == json!("scout"))),
        "the directory names the pending agent: {directory:#}"
    );
    let operation = gateway.ok("meta/identity/operation", json!({"request_id": "create-1"}));
    assert_eq!(operation, created, "the create's operation reads pending");

    let refused = gateway.send_refused("scout");
    assert_eq!(
        (refused["kind"].clone(), refused["retryable"].clone()),
        (json!("identity_wake_retryable"), json!(true)),
        "a send to a pending agent is refused retryable: {refused:#}"
    );
    let archive = gateway.refused(
        "meta/identity/archive",
        json!({"request_id": "archive-1", "name": "scout",
               "expected_spec_digest": scout["spec_digest"]}),
    );
    assert_eq!(
        (archive["kind"].clone(), archive["retryable"].clone()),
        (json!("host_preparation_pending"), json!(true)),
        "{archive:#}"
    );

    // What is not the host's to settle.
    let unknown = gateway.refused(SETTLE, json!({"request_id": "s-0", "name": "nobody"}));
    assert_eq!(unknown["kind"], json!("identity_unknown"), "{unknown:#}");
    assert_eq!(unknown["field"], json!("name"), "{unknown:#}");
    let configured = gateway.refused(SETTLE, json!({"request_id": "s-0", "name": HOST_A}));
    assert_eq!(
        configured["kind"],
        json!("preparation_not_pending"),
        "{configured:#}"
    );
    let reused = gateway.refused(SETTLE, json!({"request_id": "create-1", "name": "scout"}));
    assert_eq!(reused["kind"], json!("operation_reused"), "{reused:#}");
    let unnamed = gateway.refused(SETTLE, json!({"name": "scout"}));
    assert_eq!(unnamed["kind"], json!("invalid_request"), "{unnamed:#}");
    let extra = gateway.refused(
        SETTLE,
        json!({"request_id": "s-0", "name": "scout", "outcome": "failed"}),
    );
    assert_eq!(extra["kind"], json!("invalid_request"), "{extra:#}");
    assert_eq!(
        gateway.get("scout")["readiness"],
        json!("preparation_pending"),
        "a refused settle settles nothing"
    );

    // The settle.
    let settled = gateway.ok(SETTLE, json!({"request_id": "s-1", "name": "scout"}));
    assert_eq!(settled["outcome"], json!("done"), "{settled:#}");
    assert!(settled["operation_id"].is_string(), "{settled:#}");
    let agent = &settled["agents"][0];
    assert_eq!(agent["name"], json!("scout"), "{settled:#}");
    assert_eq!(agent["state"], json!("committed"), "{settled:#}");
    assert_eq!(agent["seat"], json!("seated"), "{settled:#}");
    assert_eq!(agent["spec_digest"], scout["spec_digest"], "{settled:#}");
    assert_eq!(agent["spec_revision"], json!(1), "{settled:#}");
    assert!(agent["session_id"].is_string(), "{settled:#}");
    assert_eq!(gateway.get("scout")["readiness"], json!("dormant"));
    assert_eq!(gateway.listed_readiness("scout"), json!("dormant"));

    // The create's operation follows the agent.
    let operation = gateway.ok("meta/identity/operation", json!({"request_id": "create-1"}));
    assert_eq!(operation["outcome"], json!("done"), "{operation:#}");
    assert_eq!(
        operation["agents"][0]["state"],
        json!("committed"),
        "{operation:#}"
    );
    assert_eq!(
        operation["agents"][0]["seat"],
        json!("seated"),
        "{operation:#}"
    );

    // The same settle again, and a second one.
    let again = gateway.ok(SETTLE, json!({"request_id": "s-1", "name": "scout"}));
    assert_eq!(again, settled, "a retry of the settle replays");
    let read = gateway.ok("meta/identity/operation", json!({"request_id": "s-1"}));
    assert_eq!(read, settled, "the settle's operation reads its outcome");
    let second = gateway.refused(SETTLE, json!({"request_id": "s-2", "name": "scout"}));
    assert_eq!(
        second["kind"],
        json!("preparation_not_pending"),
        "{second:#}"
    );
    assert_eq!(second["retryable"], json!(false), "{second:#}");

    // It takes its first delivery like any agent.
    gateway.turn("scout", "anything in the queue?");
    assert_eq!(gateway.get("scout")["readiness"], json!("live"));
}

/// A restart while an agent awaits its preparation keeps it pending: the
/// new process lists it `preparation_pending`, refuses a send to it, and
/// its settle seats it there.
#[test]
fn a_restart_keeps_a_pending_agent_pending_and_its_settle_seats_it() {
    let scratch = tempfile::tempdir().unwrap();
    let mut gateway = Gateway::launched(scratch.path(), prepared_policy());
    gateway.create_pending("create-1", "scout");
    let digest = gateway.get("scout")["spec_digest"].clone();
    drop(gateway);

    let mut gateway = Gateway::launched(scratch.path(), prepared_policy());
    let scout = gateway.get("scout");
    assert_eq!(
        scout["readiness"],
        json!("preparation_pending"),
        "{scout:#}"
    );
    assert_eq!(scout["spec_digest"], digest, "{scout:#}");
    assert_eq!(
        gateway.listed_readiness("scout"),
        json!("preparation_pending")
    );
    let refused = gateway.send_refused("scout");
    assert_eq!(refused["retryable"], json!(true), "{refused:#}");

    let settled = gateway.ok(SETTLE, json!({"request_id": "s-1", "name": "scout"}));
    assert_eq!(settled["outcome"], json!("done"), "{settled:#}");
    assert_eq!(settled["agents"][0]["seat"], json!("seated"), "{settled:#}");
    assert_eq!(gateway.get("scout")["readiness"], json!("dormant"));
    gateway.turn("scout", "anything in the queue?");

    // And the settled agent is seated after another restart.
    drop(gateway);
    let mut gateway = Gateway::launched(scratch.path(), prepared_policy());
    assert_eq!(gateway.get("scout")["readiness"], json!("dormant"));
    let second = gateway.refused(SETTLE, json!({"request_id": "s-2", "name": "scout"}));
    assert_eq!(
        second["kind"],
        json!("preparation_not_pending"),
        "{second:#}"
    );
}

// ---------------------------------------------------------------------------
// In process: what ran, and what did not
// ---------------------------------------------------------------------------

static NEXT_MOB: AtomicU64 = AtomicU64::new(0);

fn definition(mob_id: &str) -> MobDefinition {
    MobDefinition::from_toml(&format!(
        r#"
[mob]
id = "{mob_id}"

[profiles.worker]
model = "gpt-5.5"
external_addressable = true
runtime_mode = "turn_driven"

[profiles.worker.tools]
comms = true
"#
    ))
    .expect("parse the definition")
}

fn fresh_mob_id() -> String {
    format!(
        "meta-identity-preparation-{}-{}",
        std::process::id(),
        NEXT_MOB.fetch_add(1, Ordering::Relaxed)
    )
}

fn roster_spec(name: &str) -> DurableAgentSpec {
    DurableAgentSpec {
        identity: AgentIdentity::parse(name).unwrap(),
        profile: ProfileName::from("worker"),
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

/// The host's roster: the configured agent alone. It never lists an agent
/// a row creates.
struct ConfiguredRoster;

#[async_trait]
impl RosterProvider for ConfiguredRoster {
    async fn roster(&self, _context: &RosterContext) -> Result<Vec<DurableAgentSpec>, RosterError> {
        Ok(vec![roster_spec(HOST_A)])
    }
}

fn controller() -> meerkat_core::ControllerModelSelection {
    meerkat_core::ControllerModelSelection::new(
        meerkat_core::SessionLlmIdentity {
            model: "gpt-5.5".into(),
            provider: meerkat_core::Provider::OpenAI,
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: None,
        },
        serde_json::from_value(
            serde_json::json!({"realm": "meta-identity-preparation", "account": "recording"}),
        )
        .expect("nonsecret identity of the fixture transport"),
        "recording-profile".into(),
        "recording-transport".into(),
    )
}

/// The members' model: the last message of each request is kept, in the
/// order the requests were made, and every request answers.
#[derive(Clone)]
struct RecordingClient {
    asked: Arc<Mutex<Vec<String>>>,
}

impl meerkat_client::LlmClient for RecordingClient {
    fn controller_model_selection(&self) -> Option<meerkat_core::ControllerModelSelection> {
        Some(controller())
    }

    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
    ) -> Pin<Box<dyn futures::Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>> {
        if let Some(last) = request
            .messages
            .iter()
            .rev()
            .find(|message| !matches!(message, Message::SystemNotice(_)))
        {
            self.asked
                .lock()
                .unwrap()
                .push(serde_json::to_string(last).unwrap_or_default());
        }
        let [usage, done] =
            llm_usage::usage_then_done(request, meerkat::Provider::OpenAI, StopReason::EndTurn);
        Box::pin(async_stream::stream! {
            yield Ok(LlmEvent::TextDelta { delta: "noted".to_string(), meta: None });
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

/// The approver id the approval rows' rule names.
const APPROVERS: &str = "team:owners";
/// The chat person the approver map lets decide for [`APPROVERS`].
const OWNER: &str = "U123";

/// One booted gateway runtime, with the rows' policy attached, and what its
/// members' model was asked.
struct Rig {
    runtime: UnifiedRuntime,
    asked: Arc<Mutex<Vec<String>>>,
}

/// Boot the gateway's runtime composition on `state` for a fresh mob and
/// attach `policy` to it. The launch's approver map lets the chat person
/// [`OWNER`] decide for [`APPROVERS`] through a declared ingress.
async fn boot(state: &Path, policy: Value) -> Rig {
    let asked = Arc::new(Mutex::new(Vec::new()));
    let access: meerkat_meta::AccessControlConfig = toml::from_str(
        r#"
enabled = false

[approvers."team:owners"]
subjects = ["owner@example.org"]
ingress_subjects = [{ ingress = "chat", subject = "U123" }]
"#,
    )
    .expect("the approver map");
    let ingresses: meerkat_meta::DecisionIngressDeclarations =
        serde_json::from_value(json!({"ingresses": [
            {"ingress": "chat", "attests": "person", "max_clock_skew_ms": 5000},
        ]}))
        .expect("ingress declarations");
    let builder = UnifiedRuntimeBuilder::default()
        .definition(definition(&fresh_mob_id()))
        .persistent_state(state)
        .continuity_from_state_dir(state)
        .await
        .expect("open the state-dir identity substrate")
        .roster_provider(Arc::new(ConfiguredRoster))
        .identity_runtime_instance_id("meta-identity-preparation")
        .default_llm_client(Arc::new(RecordingClient {
            asked: Arc::clone(&asked),
        }))
        .decision_ingresses(ingresses)
        .access_controller(meerkat_meta::AccessController::new(access).expect("controller"));
    let runtime = Box::pin(builder.build())
        .await
        .expect("build the gateway-shaped UnifiedRuntime");
    let policy = IdentityCreationPolicy::parse(&policy)
        .expect("the policy parses")
        .expect("the policy is enabled");
    assert!(
        runtime.attach_identity_policy(Arc::new(policy), IdentityFamilyOptions::new()),
        "the runtime serves identities and takes the policy"
    );
    Rig { runtime, asked }
}

fn now_ms() -> u64 {
    u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_millis(),
    )
    .expect("ms")
}

fn member(name: &str) -> meerkat_mob::AgentIdentity {
    meerkat_mob::AgentIdentity::from(name)
}

impl Rig {
    fn mob(&self) -> meerkat_mob::MobHandle {
        self.runtime.mob_handle()
    }

    /// One JSON-RPC call through the dispatcher the stdio gateway serves,
    /// as its host's operator.
    async fn rpc(&self, method: &str, params: Value) -> Value {
        let ctx = meerkat_meta::rpc::IdentityFirstContext {
            runtime: self
                .runtime
                .identity_runtime()
                .expect("identity runtime installed"),
            roster_provider: Arc::new(MutableRosterProvider::new(vec![roster_spec(HOST_A)])),
            topology_provider: None,
            customizer: None,
            agent_memory_provider: None,
            mob_definition: None,
            transcript_edit_service: None,
            compaction_floors: None,
        };
        let raw = meerkat_meta::handle_unified_rpc_json(
            &self.runtime,
            &json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).to_string(),
            Duration::from_mins(1),
            None,
            Some(&ctx),
        )
        .await;
        serde_json::from_str(&raw).expect("json-rpc response")
    }

    /// The result of `method`, which must answer one.
    async fn ok(&self, method: &str, params: Value) -> Value {
        let answer = self.rpc(method, params.clone()).await;
        assert!(
            answer["error"].is_null(),
            "{method} {params} answers a result: {answer:#}"
        );
        answer["result"].clone()
    }

    /// The refusal of `method`: the `data` of the family's one error.
    async fn refused(&self, method: &str, params: Value) -> Value {
        let answer = self.rpc(method, params.clone()).await;
        assert_eq!(
            answer["error"]["code"],
            json!(REFUSED),
            "{method} {params} is refused with the family's code: {answer:#}"
        );
        answer["error"]["data"].clone()
    }

    /// Create `agent` as the request `request_id`: it is committed pending
    /// its preparation.
    async fn create_pending(&self, request_id: &str, agent: Value) -> Value {
        let created = self
            .ok(
                "meta/identity/create",
                json!({"request_id": request_id, "agents": [agent]}),
            )
            .await;
        assert_eq!(
            created["outcome"],
            json!("pending_preparation"),
            "{created:#}"
        );
        created
    }

    async fn get(&self, name: &str) -> Value {
        self.ok("meta/identity/get", json!({"name": name})).await
    }

    async fn operation(&self, request_id: &str) -> Value {
        self.ok("meta/identity/operation", json!({"request_id": request_id}))
            .await
    }

    /// Where Core holds `name`'s member, when `name` is an identity entry.
    fn standing(&self, name: &str) -> Option<EntryStanding> {
        match self.mob().identity_entry(&member(name)) {
            Ok(IdentityEntryRead::Entry(entry)) => Some(entry.standing),
            _ => None,
        }
    }

    fn pending(&self, name: &str) -> bool {
        self.standing(name)
            == Some(EntryStanding::Unseated(
                meerkat_mob::UnseatedEntryCause::PreparationPending,
            ))
    }

    /// Whether Core holds a member of `name`.
    async fn is_member(&self, name: &str) -> bool {
        let name = member(name);
        self.mob()
            .list_members_including_retiring()
            .await
            .iter()
            .any(|entry| entry.agent_identity == name)
    }

    /// How many requests of the members' model ended in `marker` so far.
    fn asked(&self, marker: &str) -> usize {
        self.asked
            .lock()
            .unwrap()
            .iter()
            .filter(|asked| asked.contains(marker))
            .count()
    }

    /// Wait until a member's model was asked `marker`; `false` when it was
    /// not within the bound.
    async fn asked_within(&self, marker: &str, bound: Duration) -> bool {
        let deadline = std::time::Instant::now() + bound;
        while std::time::Instant::now() < deadline {
            if self.asked(marker) > 0 {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        self.asked(marker) > 0
    }

    /// Settle `name`'s preparation through Core's own settle, as its host's
    /// operator: the settle every surface reaches.
    async fn core_settle(&self, name: &str, operation: &str) {
        self.mob()
            .settle_identity_preparation(
                meerkat_mob::IdentityPreparationSettle::new(
                    meerkat_mob::IdentityOperation::new(operation, "sha256:row"),
                    meerkat_mob::IdentityChangeRequesterInput::host_operator("rpc"),
                    member(name),
                ),
                meerkat_mob::IdentityChangeConsentInput::Policy(
                    meerkat_mob::IdentityPolicyOutcome::Allow,
                ),
            )
            .await
            .unwrap_or_else(|error| panic!("Core settles {name}: {error}"));
    }

    async fn shutdown(self) {
        let _ = self.runtime.shutdown().await;
    }
}

/// A first message waits for the preparation: while the agent is pending
/// nothing is built for it and its first message is asked of no model; the
/// settle seats the agent and its first message runs, once. The same settle
/// asked again, and a later delivery, do not run it again.
#[tokio::test(flavor = "multi_thread")]
async fn a_first_message_runs_once_after_the_settle_and_nothing_runs_before_it() {
    let temp = tempfile::tempdir().unwrap();
    let rig = boot(temp.path(), prepared_policy()).await;
    let created = rig
        .create_pending(
            "create-1",
            json!({"name": "scout", "profile": "worker",
                   "first_message": "Begin the watch."}),
        )
        .await;
    assert_eq!(
        created["agents"][0]["state"],
        json!("pending_preparation"),
        "{created:#}"
    );
    assert!(
        !rig.asked_within("Begin the watch.", QUIET).await,
        "no model is asked the first message of an agent that is not prepared"
    );
    assert!(rig.pending("scout"), "{:?}", rig.standing("scout"));
    assert!(
        !rig.is_member("scout").await,
        "Core holds no member of an agent that is not prepared"
    );
    assert!(
        rig.asked.lock().unwrap().is_empty(),
        "the create woke nobody: {:?}",
        rig.asked.lock().unwrap()
    );

    let settled = rig
        .ok(SETTLE, json!({"request_id": "s-1", "name": "scout"}))
        .await;
    assert_eq!(settled["outcome"], json!("done"), "{settled:#}");
    assert_eq!(settled["agents"][0]["seat"], json!("seated"), "{settled:#}");
    assert!(
        rig.asked_within("Begin the watch.", TURN_BOUND).await,
        "the settle runs the agent's first message"
    );

    let again = rig
        .ok(SETTLE, json!({"request_id": "s-1", "name": "scout"}))
        .await;
    assert_eq!(again["outcome"], json!("done"), "{again:#}");
    let sent = rig
        .rpc(
            "meta/send",
            json!({"identity": "scout", "content": "SECOND-DELIVERY"}),
        )
        .await;
    assert!(sent["error"].is_null(), "{sent:#}");
    assert!(
        rig.asked_within("SECOND-DELIVERY", TURN_BOUND).await,
        "the settled agent takes a delivery"
    );
    assert_eq!(
        rig.asked("Begin the watch."),
        1,
        "the first message ran once: {:?}",
        rig.asked.lock().unwrap()
    );
    rig.shutdown().await;
}

/// A create a person approved is prepared like any create: once the
/// approval is carried out the agent awaits its preparation, with no
/// member, and the host's settle seats it.
#[tokio::test(flavor = "multi_thread")]
async fn an_approved_create_of_a_prepared_profile_waits_for_its_preparation() {
    use meerkat_core::approval::decision::{
        ApproverId, DecisionId, DecisionReference, PresentationAudience, PresentationRecord,
    };

    let temp = tempfile::tempdir().unwrap();
    let rig = boot(
        temp.path(),
        json!({
            "enabled": true,
            "preparation": {"worker": {"mode": "required"}},
            "approval": {"rules": [{
                "change": ["create"],
                "requester": {"kind": "host"},
                "target": {"profile": ["worker"]},
                "outcome": {"kind": "require_approval", "approver": {"id": APPROVERS}},
            }]},
        }),
    )
    .await;
    let runtime = &rig.runtime;
    let family = runtime.identity_family().expect("the family");
    let mob = rig.mob();
    let host = ChangeRequester::host("ops");
    let operation = OperationId::for_request(&host.recorded(), "held-1").expect("a request id");
    let create = IdentityChangeSpec::Create(
        serde_json::from_value(json!({"agents": [{"name": "scout", "profile": "worker"}]}))
            .expect("a create as a surface writes it"),
    );
    let held = family
        .change(&mob, &host, IdentityChangeCall::new(operation, create))
        .await;
    assert_eq!(held.token(), "pending_decision", "{held:?}");
    let decision_id = held.to_json()["decision_id"]
        .as_str()
        .expect("a decision id")
        .to_owned();

    // A host channel shows the decision to its approvers, and the person
    // the approver map names approves it.
    let owner = runtime.durable_decisions().owner().clone();
    let row = owner
        .get(&DecisionId::new(decision_id.as_str()).expect("a decision id"))
        .expect("the decision is read");
    owner
        .present(
            &row.decision_id,
            &PresentationRecord::complete(
                PresentationAudience::Approver {
                    approver_id: ApproverId::new(APPROVERS).expect("approver"),
                },
                &row.descriptor.rendering,
                None,
                DecisionReference::new("chat:owners").expect("channel"),
                DecisionReference::new(format!("message-{}", now_ms())).expect("message"),
                now_ms(),
            ),
        )
        .expect("present");
    struct NoNotices;
    #[async_trait]
    impl meerkat_meta::DecisionNoticeSink for NoNotices {
        async fn deliver(&self, _line: String) -> bool {
            true
        }
    }
    let connection = meerkat_meta::StdioDecisionConnection::new(
        runtime.durable_decisions().realm(),
        Arc::new(NoNotices) as Arc<dyn meerkat_meta::DecisionNoticeSink>,
    );
    let line =
        json!({"jsonrpc": "2.0", "id": "decide", "method": "meta/decisions/decide", "params": {
            "decision_id": decision_id,
            "approve": true,
            "presented_digest": row.descriptor.subject_digest.to_string(),
            "attestation": {
                "ingress": "chat",
                "subject": OWNER,
                "scope": "chat-session-1",
                "event_ref": format!("chat:event:{decision_id}"),
                "observed_at_ms": now_ms(),
            },
        }})
        .to_string();
    let answer: Value = serde_json::from_str(
        &connection
            .handle_line(runtime, &line)
            .await
            .expect("a decision method answers"),
    )
    .expect("answer JSON");
    assert!(answer["error"].is_null(), "{answer:#}");

    // The family's driver carries the create out.
    let deadline = std::time::Instant::now() + TURN_BOUND;
    let created = loop {
        match family
            .read(&mob, &host, IdentityRead::Get(GetAgent::new("scout")))
            .await
        {
            Ok(IdentityReadAnswer::Get(record)) => break *record,
            _ => {
                assert!(
                    std::time::Instant::now() < deadline,
                    "the approved create was not carried out"
                );
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    };
    assert_eq!(
        serde_json::to_value(created.agent.readiness).unwrap(),
        json!("preparation_pending"),
        "an approved create is prepared like any create: {created:?}"
    );
    assert!(rig.pending("scout"), "{:?}", rig.standing("scout"));
    assert!(!rig.is_member("scout").await);

    let settled = rig
        .ok(SETTLE, json!({"request_id": "s-1", "name": "scout"}))
        .await;
    assert_eq!(settled["agents"][0]["seat"], json!("seated"), "{settled:#}");
    assert_eq!(rig.get("scout").await["readiness"], json!("dormant"));
    rig.shutdown().await;
}

/// An update asks no preparation: the update of an agent that awaits its
/// preparation changes its record and leaves it pending, and the update of
/// a settled agent of a prepared profile changes it like any agent's.
#[tokio::test(flavor = "multi_thread")]
async fn an_update_of_an_agent_on_a_prepared_profile_asks_no_preparation() {
    let temp = tempfile::tempdir().unwrap();
    let rig = boot(temp.path(), prepared_policy()).await;
    rig.create_pending("create-1", json!({"name": "scout", "profile": "worker"}))
        .await;
    let digest = rig.get("scout").await["spec_digest"].clone();

    let updated = rig
        .ok(
            "meta/identity/update",
            json!({"request_id": "update-1", "name": "scout",
                   "expected_spec_digest": digest,
                   "set": {"display_name": "Scout"}}),
        )
        .await;
    assert_eq!(updated["outcome"], json!("done"), "{updated:#}");
    let scout = rig.get("scout").await;
    assert_eq!(scout["display_name"], json!("Scout"), "{scout:#}");
    assert_eq!(scout["spec_revision"], json!(2), "{scout:#}");
    assert_eq!(
        scout["readiness"],
        json!("preparation_pending"),
        "an update settles no preparation: {scout:#}"
    );
    assert!(rig.pending("scout"));

    // Settled, the agent is updated like any agent.
    rig.core_settle("scout", "row-settle-1").await;
    let scout = rig.get("scout").await;
    assert_eq!(scout["readiness"], json!("dormant"), "{scout:#}");
    assert_eq!(
        scout["display_name"],
        json!("Scout"),
        "the settle seats the record as the update left it: {scout:#}"
    );
    let updated = rig
        .ok(
            "meta/identity/update",
            json!({"request_id": "update-2", "name": "scout",
                   "expected_spec_digest": scout["spec_digest"],
                   "set": {"display_name": "Scout Two"}}),
        )
        .await;
    assert_eq!(updated["outcome"], json!("done"), "{updated:#}");
    assert_eq!(
        rig.get("scout").await["display_name"],
        json!("Scout Two"),
        "{updated:#}"
    );
    rig.shutdown().await;
}

/// The operation of a create reads what became of its agent: pending while
/// the agent awaits its preparation, and `done` with the agent `committed`
/// and `seated` once it is settled. The create asked again answers the
/// same.
#[tokio::test(flavor = "multi_thread")]
async fn a_creates_operation_reads_done_once_its_agent_is_settled() {
    let temp = tempfile::tempdir().unwrap();
    let rig = boot(temp.path(), prepared_policy()).await;
    let agent = json!({"name": "scout", "profile": "worker"});
    let created = rig.create_pending("create-1", agent.clone()).await;
    assert_eq!(rig.operation("create-1").await, created);

    rig.core_settle("scout", "row-settle-1").await;
    let operation = rig.operation("create-1").await;
    assert_eq!(operation["outcome"], json!("done"), "{operation:#}");
    let read = &operation["agents"][0];
    assert_eq!(read["state"], json!("committed"), "{operation:#}");
    assert_eq!(read["seat"], json!("seated"), "{operation:#}");
    assert!(read["session_id"].is_string(), "{operation:#}");
    assert_eq!(
        read["spec_digest"], created["agents"][0]["spec_digest"],
        "{operation:#}"
    );
    let again = rig
        .ok(
            "meta/identity/create",
            json!({"request_id": "create-1", "agents": [agent]}),
        )
        .await;
    assert_eq!(again, operation, "the create asked again answers the same");
    rig.shutdown().await;
}

/// A host that gives up on an agent it cannot prepare removes it: the
/// removal of a pending agent commits, the agent is gone, the create's
/// operation reads `done` with the agent `removed`, its name is retired
/// like any removed name, a settle finds no agent, and nothing was ever
/// built for it: no member, and no model was asked its first message.
#[tokio::test(flavor = "multi_thread")]
async fn giving_up_on_a_pending_agent_is_its_removal_and_nothing_was_built() {
    let temp = tempfile::tempdir().unwrap();
    let rig = boot(temp.path(), prepared_policy()).await;
    let agent = json!({"name": "scout", "profile": "worker",
                       "first_message": "Begin the watch."});
    rig.create_pending("create-1", agent).await;
    let digest = rig.get("scout").await["spec_digest"].clone();

    let removed = rig
        .ok(
            "meta/identity/remove",
            json!({"request_id": "remove-1", "name": "scout", "expected_spec_digest": digest}),
        )
        .await;
    assert_eq!(removed["outcome"], json!("done"), "{removed:#}");
    assert_eq!(
        removed["agents"][0]["state"],
        json!("removed"),
        "{removed:#}"
    );

    let gone = rig
        .refused("meta/identity/get", json!({"name": "scout"}))
        .await;
    assert_eq!(gone["kind"], json!("identity_unknown"), "{gone:#}");
    let operation = rig.operation("create-1").await;
    assert_eq!(operation["outcome"], json!("done"), "{operation:#}");
    assert_eq!(
        operation["agents"][0]["state"],
        json!("removed"),
        "the create's operation reads what became of its agent: {operation:#}"
    );
    assert!(
        operation["agents"][0].get("seat").is_none(),
        "{operation:#}"
    );
    let taken = rig
        .refused(
            "meta/identity/create",
            json!({"request_id": "create-2",
                   "agents": [{"name": "scout", "profile": "worker"}]}),
        )
        .await;
    assert_eq!(taken["kind"], json!("name_taken"), "{taken:#}");
    let settle = rig
        .refused(SETTLE, json!({"request_id": "s-1", "name": "scout"}))
        .await;
    assert_eq!(settle["kind"], json!("identity_unknown"), "{settle:#}");

    assert!(!rig.is_member("scout").await);
    assert!(
        !rig.asked_within("Begin the watch.", QUIET).await,
        "nothing ran for an agent that was never prepared"
    );
    assert!(
        rig.asked.lock().unwrap().is_empty(),
        "nobody was woken: {:?}",
        rig.asked.lock().unwrap()
    );
    rig.shutdown().await;
}

/// Only the host's operator settles a preparation: the host on a surface
/// that is not its operator's, a signed-in person and an agent are refused
/// `change_denied`, and the agent stays pending. The settle is a change
/// like any other to the mob's hold of its changes: while they are held it
/// is refused the retryable `changes_held`, and it seats the agent once
/// they are released.
#[tokio::test(flavor = "multi_thread")]
async fn only_the_hosts_operator_settles_a_preparation() {
    let temp = tempfile::tempdir().unwrap();
    let rig = boot(temp.path(), prepared_policy()).await;
    rig.create_pending("create-1", json!({"name": "scout", "profile": "worker"}))
        .await;
    let family = rig.runtime.identity_family().expect("the family");
    let mob = rig.mob();
    for requester in [
        ChangeRequester::host("rpc"),
        ChangeRequester::host("console"),
        ChangeRequester::human("person-1"),
        ChangeRequester::agent_recorded(HOST_A),
    ] {
        let operation =
            OperationId::for_request(&requester.recorded(), "s-1").expect("a request id");
        let outcome = family
            .settle_preparation(
                &mob,
                &requester,
                SettlePreparationCall::new(operation, "scout"),
            )
            .await;
        let refusal = outcome
            .refusal()
            .unwrap_or_else(|| panic!("{requester:?} settles nothing: {outcome:?}"));
        assert_eq!(refusal.kind_token(), "change_denied", "{requester:?}");
        assert!(
            refusal.detail().contains("host's operator"),
            "{requester:?}: {refusal}"
        );
    }
    assert!(rig.pending("scout"), "{:?}", rig.standing("scout"));

    rig.ok("meta/identity/changes_hold", json!({"held": true}))
        .await;
    let held = rig
        .refused(SETTLE, json!({"request_id": "s-1", "name": "scout"}))
        .await;
    assert_eq!(
        (held["kind"].clone(), held["retryable"].clone()),
        (json!("changes_held"), json!(true)),
        "{held:#}"
    );
    assert!(rig.pending("scout"));
    rig.ok("meta/identity/changes_hold", json!({"held": false}))
        .await;
    let settled = rig
        .ok(SETTLE, json!({"request_id": "s-1", "name": "scout"}))
        .await;
    assert_eq!(settled["agents"][0]["seat"], json!("seated"), "{settled:#}");
    assert!(
        rig.asked.lock().unwrap().is_empty(),
        "a settle wakes nobody: {:?}",
        rig.asked.lock().unwrap()
    );
    rig.shutdown().await;
}

/// A created agent that awaits its preparation stands as a configured agent
/// does when its host declares it with a preparation: Core holds each with
/// no member for the same cause, refuses a delivery to each the same
/// retryable way, and seats each dormant once its preparation is settled.
/// Each is settled by its owner's way: a configured agent by its host's
/// next declaration, which the settle method does not stand in for
/// (`owner_read_only`), and a created agent by the settle method.
#[tokio::test(flavor = "multi_thread")]
async fn a_created_agent_awaits_its_preparation_as_a_configured_one_does() {
    use meerkat_mob::{DeclaredEntry, DeclaredRoster, IdentityPreparationMode, SpawnMemberSpec};

    const CONFIGURED: &str = "twin-configured";
    const CREATED: &str = "twin-created";
    let temp = tempfile::tempdir().unwrap();
    let rig = boot(temp.path(), prepared_policy()).await;
    let mob = rig.mob();
    let declared = |preparation| {
        DeclaredRoster::partial([DeclaredEntry::new(SpawnMemberSpec::new(
            "worker",
            member(CONFIGURED),
        ))
        .with_preparation(preparation)])
    };
    mob.apply_declared_roster(declared(IdentityPreparationMode::Required))
        .await
        .expect("the host declares an agent it prepares");
    rig.create_pending("create-1", json!({"name": CREATED, "profile": "worker"}))
        .await;

    let wake_refusal = |name: &'static str| {
        let mob = mob.clone();
        async move {
            match mob.ensure_member_live(&member(name)).await {
                Err(meerkat_mob::MobError::MemberWakeRefused { refusal, .. }) => {
                    format!("{refusal:?}")
                }
                other => panic!("a delivery to {name} is refused: {other:?}"),
            }
        }
    };
    for name in [CONFIGURED, CREATED] {
        assert!(rig.pending(name), "{name}: {:?}", rig.standing(name));
        assert!(!rig.is_member(name).await, "{name}");
    }
    assert_eq!(
        wake_refusal(CONFIGURED).await,
        wake_refusal(CREATED).await,
        "a delivery to either is refused the same way"
    );
    assert_eq!(wake_refusal(CREATED).await, "HostPreparationPending");

    // Each is settled by its owner's way.
    let not_the_methods = rig
        .refused(SETTLE, json!({"request_id": "s-0", "name": CONFIGURED}))
        .await;
    assert_eq!(
        not_the_methods["kind"],
        json!("owner_read_only"),
        "{not_the_methods:#}"
    );
    mob.apply_declared_roster(declared(IdentityPreparationMode::None))
        .await
        .expect("the host declares the agent prepared");
    let settled = rig
        .ok(SETTLE, json!({"request_id": "s-1", "name": CREATED}))
        .await;
    assert_eq!(settled["outcome"], json!("done"), "{settled:#}");
    for name in [CONFIGURED, CREATED] {
        assert!(
            matches!(rig.standing(name), Some(EntryStanding::Seated(_))),
            "{name}: {:?}",
            rig.standing(name)
        );
    }
    assert!(
        rig.asked.lock().unwrap().is_empty(),
        "a settle wakes nobody: {:?}",
        rig.asked.lock().unwrap()
    );
    rig.shutdown().await;
}
