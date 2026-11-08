#![cfg(feature = "mob")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::large_futures
)]

//! How a host learns that a member's gated tool call waits on a decision.
//!
//! A host that gates a member's tool calls the way Meerkat Connect does (a
//! `SpawnMemberCustomizer` installs a `ToolDispatchAdmission` that waits for
//! a decision) leaves the turn waiting on a decision only a person can make.
//!
//! - A plain approval record (`ApprovalService::request`) emits no
//!   notification: a client polls `approval/list` or `approval/get` and
//!   decides with `approval/decide`.
//! - A durable decision (`DurableDecisionService::hold`, through
//!   `SessionRuntime::decision_service`) is announced to the host when it
//!   opens: the host's adapter subscribes to the decisions its runtime's
//!   owner opens, shows the person what to decide, and relays their
//!   authenticated decision through
//!   `MethodRouter::dispatch_with_decision_event`. The unauthenticated TCP
//!   connection is no decider: it is told nothing and cannot decide.
//!
//! The provider is scripted; the RPC router, TCP connection, mob, member
//! session, tool dispatch, admission, approval service and decision owner are
//! real.

use async_trait::async_trait;
use meerkat::AgentFactory;
use meerkat_client::{LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest};
use meerkat_core::approval::consent::{
    AuthenticatedApprover, ConsentEligibility, ConsentIneligible, HostAuthenticationScope,
};
use meerkat_core::approval::decision::{
    ApproverId, AttestedDecision, AuthenticatedDecisionEvent, DecisionHoldRequest, DecisionIngress,
    DecisionKind, DecisionOperationId, DecisionPhase, DecisionPrincipalKey, DecisionReference,
    DecisionRenderer, DecisionRendering, DecisionRequestBody, DecisionSubjectKey,
    PresentationAudience, PresentationRecord, Sha256Digest, StoredDecision,
};
use meerkat_core::{
    ApprovalActionKind, ApprovalAssurance, ApprovalDecision, ApprovalOwnerRef, ApprovalPrincipalId,
    ApprovalProposedAction, ApprovalRequest, ApprovalResourceId, ApprovalResourceKind,
    ApprovalResourceRef, ApprovalRisk, ApprovalService, ApprovalStatus, BlobStore, Config,
    ConfigRuntime, ConfigStore, MemoryConfigStore, Message, PrincipalKind, PrincipalRef,
    StopReason, ToolCallView, ToolError, TrustDomainId,
};
use meerkat_rpc::protocol::{RpcId, RpcRequest};
use meerkat_rpc::router::{DECISION_DECIDE_METHOD, MethodRouter, NotificationSink};
use meerkat_rpc::server::serve_tcp_connection;
use meerkat_rpc::session_runtime::SessionRuntime;
use meerkat_runtime::decision::DurableDecisionService;
use serde_json::{Value, json};
use std::collections::{BTreeSet, HashMap};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::tcp::OwnedWriteHalf;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

const LIMIT: Duration = Duration::from_secs(10);
/// How long a connection must stay silent before it counts as drained.
const QUIET: Duration = Duration::from_millis(500);
const MOB: &str = "approval_notice_mob";
const MEMBER: &str = "w1";
/// The member tool the host gates; a comms tool every member has.
const GATED_TOOL: &str = "peers";
const GATED_CALL: &str = "call-peers";
const SUMMARY: &str = "list the mob's peers";
/// The realm whose persistence keeps the durable decisions.
const REALM: &str = "decision-notice";
/// The approver id the host's gate holds its decisions for.
const APPROVER: &str = "team:owners";
/// The JSON-RPC code of a method with no route.
const METHOD_NOT_FOUND: i64 = -32601;
/// The JSON-RPC code of a refused decision.
const INVALID_PARAMS: i64 = -32602;

/// Calls the gated tool once, then ends the turn after its result.
struct GatedToolProvider {
    /// The tool results each request carried, in order.
    results: Mutex<Vec<Vec<meerkat_core::ToolResult>>>,
}

#[async_trait]
impl LlmClient for GatedToolProvider {
    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
    ) -> Pin<Box<dyn futures::Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>> {
        let results: Vec<_> = request
            .messages
            .iter()
            .flat_map(|message| match message {
                Message::ToolResults { results, .. } => results.clone(),
                _ => vec![],
            })
            .collect();
        let call_tool = results.is_empty();
        self.results.lock().unwrap().push(results);
        let mut events = Vec::new();
        if call_tool {
            events.push(Ok(LlmEvent::ToolCallComplete {
                id: GATED_CALL.into(),
                name: GATED_TOOL.into(),
                args: json!({}),
                meta: None,
            }));
        } else {
            events.push(Ok(LlmEvent::TextDelta {
                delta: "done".into(),
                meta: None,
            }));
        }
        events.push(Ok(LlmEvent::UsageUpdate {
            usage: meerkat_core::TurnUsage::host_declared(
                meerkat_core::Provider::Anthropic,
                &request.model,
                meerkat_core::Usage::default(),
            ),
        }));
        events.push(Ok(LlmEvent::Done {
            outcome: LlmDoneOutcome::success(if call_tool {
                StopReason::ToolUse
            } else {
                StopReason::EndTurn
            }),
        }));
        Box::pin(futures::stream::iter(events))
    }

    fn provider(&self) -> meerkat_core::Provider {
        meerkat_core::Provider::Other
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }
}

/// The Connect-style gate on a plain approval: a call to [`GATED_TOOL`]
/// raises an approval request and waits until it is decided; every other
/// call passes.
struct ApprovalGate {
    approvals: ApprovalService,
    /// The id of each approval the gate raised, sent when it is raised.
    raised: mpsc::UnboundedSender<String>,
}

#[async_trait]
impl meerkat_core::ToolDispatchAdmission for ApprovalGate {
    async fn await_dispatch_admission(
        &self,
        call: ToolCallView<'_>,
        _context: Option<&meerkat_core::ToolDispatchContext>,
        _effect_kind: meerkat_core::LiveBridgeEffectKind,
    ) -> Result<(), ToolError> {
        if call.name != GATED_TOOL {
            return Ok(());
        }
        let record = self
            .approvals
            .request(ApprovalRequest {
                requester: ApprovalPrincipalId::new("member-tool-gate").unwrap(),
                owner: ApprovalOwnerRef::Runtime,
                resource: ApprovalResourceRef {
                    kind: ApprovalResourceKind::ToolCall,
                    id: ApprovalResourceId::new(call.id),
                },
                proposed_action: ApprovalProposedAction {
                    kind: ApprovalActionKind::ToolCall,
                    summary: SUMMARY.into(),
                    body: None,
                },
                risk: ApprovalRisk::Medium,
                request_body: None,
                allowed_decisions: BTreeSet::from([
                    ApprovalDecision::Approve,
                    ApprovalDecision::Deny,
                ]),
                expires_at: None,
                metadata: meerkat_core::SurfaceMetadata::default(),
                request_provenance: None,
            })
            .map_err(|error| ToolError::other(error.to_string()))?;
        let _ = self.raised.send(record.approval_id.to_string());
        loop {
            let current = self
                .approvals
                .get(&record.approval_id)
                .map_err(|error| ToolError::other(error.to_string()))?;
            match current.status {
                ApprovalStatus::Pending => tokio::time::sleep(Duration::from_millis(20)).await,
                ApprovalStatus::Approved => return Ok(()),
                _ => return Err(ToolError::access_denied(call.name)),
            }
        }
    }
}

/// The host's gate on a durable decision: a call to [`GATED_TOOL`] holds a
/// decision for [`APPROVER`] through the runtime's decision owner and waits
/// until it is decided; every other call passes.
struct DecisionGate {
    decisions: DurableDecisionService,
    /// The id of each decision the gate held, sent once it is held.
    raised: mpsc::UnboundedSender<String>,
}

#[async_trait]
impl meerkat_core::ToolDispatchAdmission for DecisionGate {
    async fn await_dispatch_admission(
        &self,
        call: ToolCallView<'_>,
        _context: Option<&meerkat_core::ToolDispatchContext>,
        _effect_kind: meerkat_core::LiveBridgeEffectKind,
    ) -> Result<(), ToolError> {
        if call.name != GATED_TOOL {
            return Ok(());
        }
        let held = self
            .decisions
            .hold(decision_hold(call.id), false)
            .map_err(|error| ToolError::other(error.to_string()))?;
        let _ = self.raised.send(held.decision_id.to_string());
        loop {
            let current = self
                .decisions
                .get(&held.decision_id)
                .map_err(|error| ToolError::other(error.to_string()))?;
            match current.state.phase {
                DecisionPhase::Open => tokio::time::sleep(Duration::from_millis(20)).await,
                DecisionPhase::Approved => return Ok(()),
                _ => return Err(ToolError::access_denied(call.name)),
            }
        }
    }
}

/// The host wall clock in UNIX milliseconds, the clock the decision owner
/// reads.
fn now_ms() -> u64 {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("a clock after the epoch");
    u64::try_from(elapsed.as_millis()).expect("milliseconds")
}

fn principal(kind: PrincipalKind, id: &str) -> PrincipalRef {
    PrincipalRef::in_domain(
        kind,
        id,
        TrustDomainId::new("example.org").expect("trust domain"),
    )
    .expect("principal")
}

/// The decision the host's gate holds for the tool call `call_id`: the call
/// as the held request, [`SUMMARY`] as its rendering, for [`APPROVER`],
/// requested by the gate, open for ten minutes.
fn decision_hold(call_id: &str) -> DecisionHoldRequest {
    let request = json!({"tool": GATED_TOOL, "arguments": {}});
    DecisionHoldRequest::new(
        DecisionKind::new("tool_call").expect("kind"),
        DecisionSubjectKey::new(call_id).expect("subject key"),
        DecisionOperationId::new(format!("tool_call:{call_id}")).expect("operation id"),
        Sha256Digest::of(request.to_string().as_bytes()),
        DecisionRequestBody::from_value(&request).expect("request body"),
        DecisionPrincipalKey::of(&principal(PrincipalKind::RuntimeHost, "member-tool-gate"))
            .expect("principal key"),
        ApproverId::new(APPROVER).expect("approver id"),
        now_ms() + 600_000,
        DecisionRendering::new(SUMMARY).expect("rendering"),
        DecisionRenderer::new("summary.v1").expect("renderer"),
    )
}

/// The host's current answer for a person: whether its policy lets them act
/// for the decision's approver id.
struct HostEligibility(bool);

impl ConsentEligibility for HostEligibility {
    fn check_current(&self) -> Result<(), ConsentIneligible> {
        if self.0 {
            Ok(())
        } else {
            Err(ConsentIneligible)
        }
    }
}

/// The host's trusted adapter: it shows a person what an open decision
/// holds, authenticates their answer and relays it through the router's
/// host-only entry, [`MethodRouter::dispatch_with_decision_event`].
struct HostAdapter {
    router: MethodRouter,
}

impl HostAdapter {
    /// `person`, authenticated in a chat login of their own, answers
    /// `approve` on `opened` after the adapter showed them its rendering;
    /// `eligible` is the host's answer for them. Returns the JSON-RPC
    /// response.
    async fn decide(
        &self,
        opened: &StoredDecision,
        person: &str,
        eligible: bool,
        approve: bool,
    ) -> Value {
        let descriptor = &opened.descriptor;
        let shown_at_ms = now_ms();
        let shown = PresentationRecord::complete(
            PresentationAudience::Approver {
                approver_id: descriptor.approver_id.clone(),
            },
            &descriptor.rendering,
            None,
            DecisionReference::new("chat:approvals").expect("channel ref"),
            DecisionReference::new(format!("chat-message:{person}")).expect("message ref"),
            shown_at_ms,
        );
        let approver = AuthenticatedApprover::new(
            principal(PrincipalKind::Human, person),
            ApprovalAssurance::PersonAuthenticated,
        )
        .expect("an authenticated person")
        .with_authentication_scope(HostAuthenticationScope::new(format!("chat-login:{person}")));
        let decision = AttestedDecision::new(
            approver,
            DecisionIngress::new("chat").expect("ingress"),
            DecisionReference::new(format!("chat-event:{person}:{approve}")).expect("event ref"),
            now_ms().max(shown_at_ms),
            descriptor.approver_id.clone(),
            descriptor.subject_digest.clone(),
            approve,
        )
        .expect("an attested decision")
        .with_presentation(shown);
        let event = AuthenticatedDecisionEvent::new(
            opened.decision_id.clone(),
            decision,
            Arc::new(HostEligibility(eligible)),
        );
        let params = json!({
            "decision_id": opened.decision_id.to_string(),
            "approve": approve,
            "presented_digest": descriptor.subject_digest.to_string(),
        });
        let request = RpcRequest {
            jsonrpc: "2.0".into(),
            id: Some(RpcId::Num(1)),
            method: DECISION_DECIDE_METHOD.into(),
            params: Some(serde_json::value::to_raw_value(&params).expect("raw params")),
        };
        let response = self
            .router
            .dispatch_with_decision_event(request, event)
            .await;
        serde_json::to_value(&response).expect("a JSON-RPC response")
    }
}

/// The host adapter's subscription to the decisions its runtime's decision
/// owner opens. Stand-in until the owner offers one.
struct HostOpenings;

impl HostOpenings {
    fn subscribe(_decisions: &DurableDecisionService) -> Self {
        panic!(
            "no subscription to the decisions a runtime's decision owner opens exists \
             (proposed: DurableDecisionService::subscribe_openings)"
        )
    }

    /// The next decision the owner opened, waiting at most [`LIMIT`].
    async fn next_opened(&mut self) -> Arc<StoredDecision> {
        std::future::pending().await
    }
}

/// Installs a gate on every member the mob spawns.
struct GateCustomizer(Arc<dyn meerkat_core::ToolDispatchAdmission>);

impl meerkat_mob::SpawnMemberCustomizer for GateCustomizer {
    fn customize_spawn(
        &self,
        _ctx: &meerkat_mob::SpawnCustomizationContext,
        spec: &mut meerkat_mob::SpawnMemberSpec,
    ) -> Result<(), meerkat_mob::MobError> {
        spec.tool_dispatch_admission = Some(self.0.clone());
        Ok(())
    }
}

/// One JSON-RPC client over a real TCP connection.
struct Connection {
    writer: OwnedWriteHalf,
    frames: mpsc::UnboundedReceiver<Value>,
    reader_task: JoinHandle<()>,
    next_id: u64,
    /// Notifications (frames without an id) received so far, in order.
    notifications: Vec<Value>,
    /// Responses that arrived while waiting for another one, by id. The
    /// server writes long-running and inline replies on independent
    /// channels, so replies can arrive out of request order.
    responses: HashMap<u64, Value>,
}

impl Drop for Connection {
    fn drop(&mut self) {
        self.reader_task.abort();
    }
}

impl Connection {
    async fn send(&mut self, method: &str, params: Value) -> u64 {
        self.next_id += 1;
        let id = self.next_id;
        let frame = json!({"jsonrpc":"2.0","id":id,"method":method,"params":params});
        self.writer
            .write_all(format!("{frame}\n").as_bytes())
            .await
            .unwrap();
        self.writer.flush().await.unwrap();
        id
    }

    /// File one incoming frame: a response under its id, anything else as
    /// a notification.
    fn route(&mut self, frame: Value) {
        match frame["id"].as_u64() {
            Some(id) if frame.get("method").is_none() => {
                let previous = self.responses.insert(id, frame);
                assert!(previous.is_none(), "response {id} arrived twice");
            }
            _ => self.notifications.push(frame),
        }
    }

    /// The whole answer to request `id`, error or not, buffering every
    /// other frame that arrives first (other responses by id,
    /// notifications in order).
    async fn answer(&mut self, id: u64) -> Value {
        tokio::time::timeout(LIMIT, async {
            loop {
                if let Some(frame) = self.responses.remove(&id) {
                    return frame;
                }
                let frame = self.frames.recv().await.expect("connection open");
                self.route(frame);
            }
        })
        .await
        .unwrap_or_else(|_| panic!("bounded wait for response {id}"))
    }

    /// The result of request `id`, which must have succeeded.
    async fn response(&mut self, id: u64) -> Value {
        let frame = self.answer(id).await;
        assert!(frame["error"].is_null(), "request {id} failed: {frame}");
        frame["result"].clone()
    }

    async fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.send(method, params).await;
        self.response(id).await
    }

    /// Keeps every frame that arrives within `window`.
    async fn collect_for(&mut self, window: Duration) {
        let deadline = tokio::time::Instant::now() + window;
        while let Ok(Some(frame)) = tokio::time::timeout_at(deadline, self.frames.recv()).await {
            self.route(frame);
        }
    }

    /// Takes in every frame the server had produced before now: a
    /// sentinel request is answered, then frames are read until
    /// [`QUIET`] passes with nothing new. The server's writer selects
    /// among its response and notification channels without priority, so
    /// the sentinel's reply alone does not order an already queued
    /// notification before it; the quiet period after it does.
    async fn settle(&mut self) {
        self.request("approval/list", json!({})).await;
        while let Ok(Some(frame)) = tokio::time::timeout(QUIET, self.frames.recv()).await {
            self.route(frame);
        }
    }

    /// The notifications received so far whose method starts with
    /// `family` (`approval/`, `decision/`).
    fn notifications_of(&self, family: &str) -> Vec<&Value> {
        self.notifications
            .iter()
            .filter(|frame| {
                frame["id"].is_null()
                    && frame["method"]
                        .as_str()
                        .is_some_and(|method| method.starts_with(family))
            })
            .collect()
    }

    /// The `approval/*` notifications received so far.
    fn approval_notifications(&self) -> Vec<&Value> {
        self.notifications_of("approval/")
    }

    /// The `decision/*` notifications received so far.
    fn decision_notifications(&self) -> Vec<&Value> {
        self.notifications_of("decision/")
    }
}

/// The decision a host gate holds before it lets its call through.
enum Gate {
    /// A plain approval record of the runtime's approval service.
    PlainApproval,
    /// A durable decision of the runtime's decision owner.
    DurableDecision,
}

struct Harness {
    _temp: tempfile::TempDir,
    runtime: Arc<SessionRuntime>,
    config: Arc<dyn ConfigStore>,
    provider: Arc<GatedToolProvider>,
    raised: mpsc::UnboundedReceiver<String>,
    /// The host's trusted adapter, composed before any connection.
    adapter: HostAdapter,
    servers: Vec<JoinHandle<Result<(), meerkat_rpc::server::ServerError>>>,
}

impl Harness {
    /// A runtime whose host-built mob state gates every member's
    /// [`GATED_TOOL`] calls on a plain approval of the runtime's approval
    /// service.
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let blobs: Arc<dyn BlobStore> = Arc::new(meerkat_store::MemoryBlobStore::new());
        let bundle = meerkat::PersistenceBundle::new(
            Arc::new(meerkat::MemoryStore::new()),
            Arc::new(meerkat_runtime::InMemoryRuntimeStore::new()),
            blobs,
        )
        .expect("construct runtime authority");
        Self::compose(temp, bundle, &Gate::PlainApproval)
    }

    /// A runtime over a realm that keeps durable decisions, whose host-built
    /// mob state gates every member's [`GATED_TOOL`] calls on a durable
    /// decision of the runtime's decision owner.
    async fn with_durable_decisions() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let (_, bundle) = meerkat::open_realm_persistence_in(
            temp.path(),
            REALM,
            Some(meerkat_store::RealmBackend::Sqlite),
            None,
        )
        .await
        .expect("open the realm");
        Self::compose(temp, bundle, &Gate::DurableDecision)
    }

    fn compose(temp: tempfile::TempDir, bundle: meerkat::PersistenceBundle, gate: &Gate) -> Self {
        let config: Arc<dyn ConfigStore> = Arc::new(MemoryConfigStore::new(
            Config::default(),
            meerkat_models::canonical(),
        ));
        let runtime = SessionRuntime::new(
            AgentFactory::new(temp.path().join("sessions")),
            Config::default(),
            10,
            bundle,
            NotificationSink::noop(),
        )
        .expect("compose the RPC session runtime");
        let provider = Arc::new(GatedToolProvider {
            results: Mutex::new(Vec::new()),
        });
        runtime.set_default_llm_client(Some(provider.clone()));
        runtime.set_config_runtime(Arc::new(ConfigRuntime::new(
            config.clone(),
            temp.path().join("config_state.json"),
        )));
        let runtime = Arc::new(runtime);

        let (raised_tx, raised) = mpsc::unbounded_channel();
        let gate: Arc<dyn meerkat_core::ToolDispatchAdmission> = match gate {
            Gate::PlainApproval => Arc::new(ApprovalGate {
                approvals: runtime.approval_service(),
                raised: raised_tx,
            }),
            Gate::DurableDecision => Arc::new(DecisionGate {
                decisions: runtime.decision_service(),
                raised: raised_tx,
            }),
        };
        let llm: Arc<dyn LlmClient> = provider.clone();
        let mob_state = meerkat_mob_mcp::MobMcpState::new_with_runtime_adapter(
            runtime.session_service(),
            Some(runtime.runtime_adapter()),
            meerkat_mob::MobControlPrincipal::Owner,
        )
        .expect("host mob state")
        .with_default_llm_client_provider(Some(Arc::new(move || Some(llm.clone()))))
        .with_child_spawn_member_customizer(Arc::new(GateCustomizer(gate)))
        .into_shared();
        runtime.set_mob_state(mob_state);
        let adapter = HostAdapter {
            router: MethodRouter::new(
                Arc::clone(&runtime),
                config.clone(),
                NotificationSink::noop(),
            )
            .expect("the host adapter's router"),
        };
        Self {
            _temp: temp,
            runtime,
            config,
            provider,
            raised,
            adapter,
            servers: Vec::new(),
        }
    }

    /// A new client connection on the shared runtime, initialized.
    async fn connect(&mut self) -> Connection {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let stream = TcpStream::connect(listener.local_addr().unwrap())
            .await
            .unwrap();
        let (accepted, _) = listener.accept().await.unwrap();
        self.servers.push(tokio::spawn(serve_tcp_connection(
            accepted,
            self.runtime.clone(),
            self.config.clone(),
            None,
        )));
        let (reader, writer) = stream.into_split();
        let (tx, frames) = mpsc::unbounded_channel();
        let reader_task = tokio::spawn(async move {
            let mut lines = BufReader::new(reader).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if tx.send(serde_json::from_str(&line).unwrap()).is_err() {
                    break;
                }
            }
        });
        let mut connection = Connection {
            writer,
            frames,
            reader_task,
            next_id: 0,
            notifications: Vec::new(),
            responses: HashMap::new(),
        };
        connection.request("initialize", json!({})).await;
        connection
    }

    /// The id of the approval or decision the gate raised for the member's
    /// tool call.
    async fn raised(&mut self) -> String {
        tokio::time::timeout(LIMIT, self.raised.recv())
            .await
            .expect("the member's tool call reaches the gate")
            .unwrap()
    }

    /// The gated tool's result reached the model as a success.
    fn assert_tool_ran(&self) {
        let results = self.provider.results.lock().unwrap();
        let last = results.last().expect("the model was asked");
        assert!(
            last.iter()
                .any(|result| result.tool_use_id == GATED_CALL && !result.is_error),
            "the approved call ran: {results:?}"
        );
    }

    async fn clean(self) {
        for server in self.servers {
            server.abort();
            let _ = server.await;
        }
        tokio::time::timeout(LIMIT, self.runtime.try_shutdown())
            .await
            .expect("bounded runtime cleanup")
            .unwrap();
    }
}

/// Creates the mob and its member and starts a member turn that calls the
/// gated tool; returns the turn's request id.
async fn start_gated_turn(connection: &mut Connection) -> u64 {
    connection
        .request(
            "mob/create",
            json!({"definition":{"id":MOB,"profiles":{"worker":{"model":"claude-sonnet-4-6","tools":{"comms":true}}}}}),
        )
        .await;
    connection
        .request(
            "mob/spawn",
            json!({"mob_id":MOB,"profile":"worker","agent_identity":MEMBER,"runtime_mode":"turn_driven"}),
        )
        .await;
    connection
        .send(
            "mob/turn_start",
            json!({"mob_id":MOB,"agent_identity":MEMBER,"prompt":"list your peers"}),
        )
        .await
}

/// When the member's gated tool call holds a durable decision, the host is
/// told without polling: its adapter's subscription to the decisions the
/// runtime's owner opens yields the open decision, with the rendering to
/// show. The adapter shows it to the person, relays their authenticated
/// approval through `MethodRouter::dispatch_with_decision_event`, and the
/// turn completes.
#[tokio::test]
async fn a_durable_decision_a_gated_call_holds_is_announced_to_the_host() {
    let mut h = Harness::with_durable_decisions().await;
    let mut openings = HostOpenings::subscribe(&h.runtime.decision_service());
    let mut client = h.connect().await;
    let turn = start_gated_turn(&mut client).await;
    let decision_id = h.raised().await;

    let opened = openings.next_opened().await;
    assert_eq!(opened.decision_id.to_string(), decision_id);
    assert_eq!(opened.state.phase, DecisionPhase::Open);
    assert_eq!(opened.descriptor.approver_id.as_str(), APPROVER);
    assert_eq!(opened.descriptor.rendering.as_str(), SUMMARY);
    // Setup took effect: the member's call waits on the open decision.
    let open = client
        .request("decision/get", json!({"decision_id": decision_id}))
        .await;
    assert_eq!(open["decision"]["phase"], "open", "{open}");

    let decided = h.adapter.decide(&opened, "alice", true, true).await;
    assert_eq!(
        decided["result"]["decision"]["phase"], "approved",
        "{decided}"
    );
    client.response(turn).await;
    h.assert_tool_ran();
    h.clean().await;
}

/// The host's adapter decides only for an identity its policy admits. An
/// authenticated person the host does not let act for the approver id is
/// refused with the decision's typed refusal, `not_eligible`, and the
/// decision stays open; an eligible person's approval decides it, and a
/// second answer is refused `not_open`.
#[tokio::test]
async fn an_identity_the_host_does_not_admit_is_refused_typed() {
    let h = Harness::with_durable_decisions().await;
    let held = h
        .runtime
        .decision_service()
        .hold(decision_hold("call-ineligible"), false)
        .expect("hold");

    let refused = h.adapter.decide(&held, "mallory", false, true).await;
    assert_eq!(refused["error"]["code"], INVALID_PARAMS, "{refused}");
    assert_eq!(
        refused["error"]["data"],
        json!({"kind": "decision_refused", "decision_refusal": {"kind": "not_eligible"}}),
        "{refused}"
    );
    let still_open = h
        .runtime
        .decision_service()
        .get(&held.decision_id)
        .expect("read the decision");
    assert_eq!(still_open.state.phase, DecisionPhase::Open);
    assert!(still_open.outcome.is_none(), "{still_open:?}");

    let decided = h.adapter.decide(&held, "alice", true, true).await;
    assert_eq!(
        decided["result"]["decision"]["phase"], "approved",
        "{decided}"
    );
    let again = h.adapter.decide(&held, "alice", true, false).await;
    assert_eq!(
        again["error"]["data"],
        json!({"kind": "decision_refused", "decision_refusal": {"kind": "not_open"}}),
        "{again}"
    );
    h.clean().await;
}

/// The unauthenticated TCP connection is no decider. A decision that opens
/// while it is connected is not announced to it, and `decision/decide` has
/// no route on it; the decision stays open and unpresented.
#[tokio::test]
async fn an_unauthenticated_connection_is_not_told_and_cannot_decide() {
    let mut h = Harness::with_durable_decisions().await;
    let mut client = h.connect().await;
    let held = h
        .runtime
        .decision_service()
        .hold(decision_hold("call-unannounced"), false)
        .expect("hold");

    client.settle().await;
    assert!(
        client.decision_notifications().is_empty(),
        "an unauthenticated connection is told nothing: {:?}",
        client.decision_notifications()
    );
    let id = client
        .send(
            "decision/decide",
            json!({
                "decision_id": held.decision_id.to_string(),
                "approve": true,
                "presented_digest": held.descriptor.subject_digest.to_string(),
            }),
        )
        .await;
    let refused = client.answer(id).await;
    assert_eq!(refused["error"]["code"], METHOD_NOT_FOUND, "{refused}");
    let row = h
        .runtime
        .decision_service()
        .get(&held.decision_id)
        .expect("read the decision");
    assert_eq!(row.state.phase, DecisionPhase::Open);
    assert!(row.presentations.is_empty(), "{row:?}");
    assert_eq!(row.state.first_presented_to_approver_at_ms, None);
    h.clean().await;
}

/// The contract for plain approval records: while the member's call waits
/// on a pending plain approval, no notification reaches the client, neither
/// `approval/*` nor `decision/*`; `approval/list` (polling) shows the request
/// with its summary, and deciding it through `approval/decide` lets the call
/// run and the turn complete, again with no notification.
#[tokio::test]
async fn a_pending_plain_approval_is_only_visible_by_polling() {
    let mut h = Harness::new();
    let mut client = h.connect().await;
    let turn = start_gated_turn(&mut client).await;
    let approval_id = h.raised().await;

    client.collect_for(Duration::from_secs(2)).await;
    assert!(
        client.approval_notifications().is_empty(),
        "no approval notification arrives while {approval_id} is pending: {:?}",
        client.approval_notifications()
    );
    assert!(
        client.decision_notifications().is_empty(),
        "a plain approval opens no durable decision: {:?}",
        client.decision_notifications()
    );

    let listed = client
        .request("approval/list", json!({"filter": {"status": "pending"}}))
        .await;
    let approvals = listed["approvals"].as_array().unwrap();
    let pending = approvals
        .iter()
        .find(|record| record["approval_id"] == json!(approval_id))
        .unwrap_or_else(|| panic!("polling finds the pending approval: {listed}"));
    assert_eq!(pending["status"], "pending", "{pending}");
    assert_eq!(pending["proposed_action"]["summary"], SUMMARY, "{pending}");

    let decided = client
        .request(
            "approval/decide",
            json!({"approval_id": approval_id, "decision": "approve", "actor": "operator"}),
        )
        .await;
    assert_eq!(decided["status"], "approved", "{decided}");
    client.response(turn).await;
    h.assert_tool_ran();
    // A decision notification could be written after either reply; drain
    // the connection before asserting it never came.
    client.settle().await;
    assert!(
        client.approval_notifications().is_empty(),
        "no approval notification arrives for the decision either: {:?}",
        client.approval_notifications()
    );
    assert!(
        client.decision_notifications().is_empty(),
        "nor a decision notification: {:?}",
        client.decision_notifications()
    );
    h.clean().await;
}
