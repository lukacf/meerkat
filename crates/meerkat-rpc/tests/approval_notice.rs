#![cfg(feature = "mob")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::large_futures
)]

//! No notification announces a pending plain approval on an ungoverned
//! connection.
//!
//! A host that gates a member's tool calls the way Meerkat Connect does (a
//! `SpawnMemberCustomizer` installs a `ToolDispatchAdmission` that raises an
//! `ApprovalService` request and waits for its decision) leaves the turn
//! waiting on a decision only a client can make. The client can see and
//! decide the request through `approval/list`, `approval/get` and
//! `approval/decide`, but nothing is pushed to it: `approval/consent_notice`
//! exists only for the governed R3 consent route. The client has to poll.
//!
//! The provider is scripted; the RPC router, TCP connection, mob, member
//! session, tool dispatch, admission and approval service are real.

use async_trait::async_trait;
use meerkat::AgentFactory;
use meerkat_client::{LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest};
use meerkat_core::{
    ApprovalActionKind, ApprovalDecision, ApprovalOwnerRef, ApprovalPrincipalId,
    ApprovalProposedAction, ApprovalRequest, ApprovalResourceId, ApprovalResourceKind,
    ApprovalResourceRef, ApprovalRisk, ApprovalService, ApprovalStatus, BlobStore, Config,
    ConfigRuntime, ConfigStore, MemoryConfigStore, Message, StopReason, ToolCallView, ToolError,
};
use meerkat_rpc::server::serve_tcp_connection;
use meerkat_rpc::session_runtime::SessionRuntime;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::tcp::OwnedWriteHalf;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

const LIMIT: Duration = Duration::from_secs(10);
const MOB: &str = "approval_notice_mob";
const MEMBER: &str = "w1";
/// The member tool the host gates; a comms tool every member has.
const GATED_TOOL: &str = "peers";
const GATED_CALL: &str = "call-peers";
const SUMMARY: &str = "list the mob's peers";

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

/// The Connect-style gate: a call to [`GATED_TOOL`] raises an approval
/// request and waits until it is decided; every other call passes.
struct ApprovalGate {
    approvals: ApprovalService,
    /// Each approval the gate raised, sent when it is raised.
    raised: mpsc::UnboundedSender<meerkat_core::ApprovalId>,
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
        let _ = self.raised.send(record.approval_id.clone());
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

/// Installs the gate on every member the mob spawns.
struct GateCustomizer(Arc<ApprovalGate>);

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
    /// Notifications that arrived while waiting for a response.
    notifications: Vec<Value>,
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

    /// The result of request `id`, keeping the notifications before it.
    async fn response(&mut self, id: u64) -> Value {
        let frame = tokio::time::timeout(LIMIT, async {
            loop {
                let frame = self.frames.recv().await.expect("connection open");
                if frame["id"] == json!(id) {
                    return frame;
                }
                self.notifications.push(frame);
            }
        })
        .await
        .unwrap_or_else(|_| panic!("bounded wait for response {id}"));
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
            self.notifications.push(frame);
        }
    }

    /// The `approval/*` notifications received so far.
    fn approval_notifications(&self) -> Vec<&Value> {
        self.notifications
            .iter()
            .filter(|frame| {
                frame["id"].is_null()
                    && frame["method"]
                        .as_str()
                        .is_some_and(|method| method.starts_with("approval/"))
            })
            .collect()
    }
}

struct Harness {
    _temp: tempfile::TempDir,
    runtime: Arc<SessionRuntime>,
    config: Arc<dyn ConfigStore>,
    provider: Arc<GatedToolProvider>,
    raised: mpsc::UnboundedReceiver<meerkat_core::ApprovalId>,
    servers: Vec<JoinHandle<Result<(), meerkat_rpc::server::ServerError>>>,
}

impl Harness {
    /// A runtime whose host-built mob state gates every member's
    /// [`GATED_TOOL`] calls through the runtime's approval service.
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let config: Arc<dyn ConfigStore> = Arc::new(MemoryConfigStore::new(
            Config::default(),
            meerkat_models::canonical(),
        ));
        let blobs: Arc<dyn BlobStore> = Arc::new(meerkat_store::MemoryBlobStore::new());
        let runtime = SessionRuntime::new(
            AgentFactory::new(temp.path().join("sessions")),
            Config::default(),
            10,
            meerkat::PersistenceBundle::new(
                Arc::new(meerkat::MemoryStore::new()),
                Arc::new(meerkat_runtime::InMemoryRuntimeStore::new()),
                blobs,
            )
            .expect("construct runtime authority"),
            meerkat_rpc::router::NotificationSink::noop(),
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
        let gate = Arc::new(ApprovalGate {
            approvals: runtime.approval_service(),
            raised: raised_tx,
        });
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
        Self {
            _temp: temp,
            runtime,
            config,
            provider,
            raised,
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
        };
        connection.request("initialize", json!({})).await;
        connection
    }

    /// The approval the gate raised for the member's tool call.
    async fn raised(&mut self) -> meerkat_core::ApprovalId {
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

/// Intended behaviour: when the member's gated tool call raises a plain
/// approval, the connected client is told without polling. An
/// `approval/notice` notification names the pending approval and its
/// summary; the client decides it and the turn completes.
///
/// Ignored until the RPC host pushes plain approval transitions; its
/// `today_` sibling below pins the current behaviour and is deleted with
/// the fix.
#[tokio::test]
#[ignore = "no approval/* notification announces a pending plain approval; see the PR"]
async fn pending_plain_approval_is_pushed_to_the_connected_client() {
    let mut h = Harness::new();
    let mut client = h.connect().await;
    let turn = start_gated_turn(&mut client).await;
    let approval_id = h.raised().await;
    // Setup took effect: the member's call is waiting on a pending approval.
    let pending = client
        .request("approval/get", json!({"approval_id": approval_id}))
        .await;
    assert_eq!(pending["status"], "pending", "{pending}");

    client.collect_for(Duration::from_secs(2)).await;
    let notice = client
        .approval_notifications()
        .into_iter()
        .find(|frame| frame["params"]["approval_id"] == json!(approval_id))
        .cloned()
        .unwrap_or_else(|| {
            panic!(
                "no approval/* notification named pending approval {approval_id}; got {:?}",
                client.notifications
            )
        });
    assert_eq!(notice["method"], "approval/notice", "{notice}");
    assert_eq!(notice["params"]["status"], "pending", "{notice}");
    assert_eq!(
        notice["params"]["proposed_action"]["summary"], SUMMARY,
        "{notice}"
    );

    client
        .request(
            "approval/decide",
            json!({"approval_id": approval_id, "decision": "approve", "actor": "operator"}),
        )
        .await;
    client.response(turn).await;
    h.assert_tool_ran();
    h.clean().await;
}

/// Current behaviour, to be deleted with the fix: while the member's call
/// waits on a pending approval, no `approval/*` notification reaches the
/// client; `approval/list` (polling) shows the request with its summary, and
/// deciding it through `approval/decide` lets the call run and the turn
/// complete.
#[tokio::test]
async fn today_a_pending_plain_approval_is_only_visible_by_polling() {
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
    assert!(
        client.approval_notifications().is_empty(),
        "no approval notification arrives for the decision either: {:?}",
        client.approval_notifications()
    );
    h.clean().await;
}
