//! The production caller enters native from an empty keeper. Only model IO
//! and installed-package readback use mechanical fixtures. The stdio MCP
//! process, native Apply, accepted receipt and operation observation are real.
use super::super::super::admission_tests::{approve_b_configuration, approve_b_configuration_with_tool_policy};
use super::*;
use futures::StreamExt;
use meerkat_authorization_contracts::work_association::InputAuthorityAssociation;
use meerkat_core::{
    ApprovalDecision, ApprovalRecord, ApprovalStatus, ControllerModelSelection, Message,
    Provider, StopReason, ToolIdentity, ToolResult, lifecycle::InputId, types::ToolSourceKind,
};
use meerkat_llm_core::{
    LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest, LlmStream, PlainModelRoute,
    PreparedLlmRequest,
};
use meerkat_runtime::completion::CompletionOutcome;
use meerkat_runtime::terminal_status::{InputTerminalReceiptRead, InteractionSelector};
use meerkat_runtime::input_state::{InputLifecycleState, InputTerminalOutcome, StoredInputState};
use meerkat_runtime::{LogicalRuntimeId, RuntimeStore, SessionServiceRuntimeExt};
use std::{path::{Path, PathBuf}, sync::atomic::AtomicBool};

type PersistentFixture =
    ContinuationFixture<meerkat::PersistentSessionService<meerkat::FactoryAgentBuilder>>;

struct Endpoint {
    _directory: tempfile::TempDir,
    started: PathBuf,
    listed: PathBuf,
    released: PathBuf,
    called: PathBuf,
    result: String,
    fail_before_initialize: bool,
}
impl Endpoint {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        Self {
            started: directory.path().join("started"),
            listed: directory.path().join("listed"),
            released: directory.path().join("released"),
            called: directory.path().join("called"),
            result: format!("public-read-{}", uuid::Uuid::new_v4()),
            fail_before_initialize: false,
            _directory: directory,
        }
    }
    fn failing_initialize() -> Self {
        let mut endpoint = Self::new();
        endpoint.fail_before_initialize = true;
        endpoint
    }
    fn config(&self, realm: &str, deferred: bool) -> meerkat_core::McpServerConfig {
        // The existing native fixture uses this same line-oriented protocol.
        // Positional arguments keep temporary paths out of shell source.
        let script = r#"printf 'start\n' >> "$1"
while IFS= read -r line; do
  id=$(printf '%s\n' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"2025-03-26","capabilities":{"tools":{}},"serverInfo":{"name":"toolkit-test","version":"0"}}}\n' "$id" ;;
    *'"method":"tools/list"'*)
      printf 'list\n' >> "$2"
      while [ ! -e "$3" ]; do sleep 0.02; done
      printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"read","inputSchema":%s}]}}\n' "$id" "$6" ;;
    *'"method":"tools/call"'*)
      printf '%s\n' "$line" >> "$4"
      case "$line" in
        *'"name":"read"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"%s"}],"isError":false}}\n' "$id" "$5" ;;
        *) printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"unexpected fixture operation"}],"isError":true}}\n' "$id" ;;
      esac ;;
  esac
done"#;
        let script = if self.fail_before_initialize {
            // Retain the actual native attempt before releasing its initialize
            // failure. No tool discovery or business operation is performed.
            r#"printf 'start\n' >> "$1"
while [ ! -e "$3" ]; do sleep 0.02; done
printf '%s\n' 'toolkit-native-setup-diagnostic-sentinel' >&2
exit 3"#
        } else {
            script
        };
        let mut config = reviewed_config_operations(realm, &[Id::new("read").unwrap()].into());
        let connector = config.connector.take();
        let mut config = meerkat_core::McpServerConfig::stdio(
            "mail",
            "/bin/sh",
            vec![
                "-c".into(),
                script.into(),
                "toolkit-attachment-fixture".into(),
                self.started.to_str().unwrap().into(),
                self.listed.to_str().unwrap().into(),
                self.released.to_str().unwrap().into(),
                self.called.to_str().unwrap().into(),
                self.result.clone(),
                if deferred {
                    // Real schema volume selects native deferred catalog mode.
                    serde_json::json!({"type":"object","description":"A harmless read whose exact schema is intentionally large enough for native deferred discovery. The native catalog must load this tool before it can be offered to the Original model."}).to_string()
                } else { serde_json::json!({"type":"object"}).to_string() },
            ],
            Default::default(),
        );
        config.connector = connector;
        config
    }
    fn release(&self) {
        std::fs::write(&self.released, b"release\n").unwrap();
    }
    fn starts(&self) -> usize {
        match std::fs::read_to_string(&self.started) {
            Ok(text) => text.lines().count(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
            Err(error) => panic!("fixture process observation failed: {error}"),
        }
    }
    fn calls(&self) -> Vec<serde_json::Value> {
        Self::calls_at(&self.called)
    }
    fn calls_at(path: &Path) -> Vec<serde_json::Value> {
        match std::fs::read_to_string(path) {
            Ok(text) => text.lines().map(|line| serde_json::from_str(line).unwrap()).collect(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => vec![],
            Err(error) => panic!("fixture invocation observation failed: {error}"),
        }
    }
    async fn wait_for(&self, path: &Path) {
        tokio::time::timeout(WAIT, async {
            while !path.exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("actual local MCP phase was not observed");
    }
}
impl Drop for Endpoint {
    fn drop(&mut self) {
        // Panic cleanup releases a tools/list barrier before native shutdown.
        // This is a fixture transport signal, never an authorization decision.
        let _ = std::fs::write(&self.released, b"release\n");
    }
}

const PUBLIC_READ_CALL: &str = "actual-public-attachment-read";
const PUBLIC_CATALOG_CALL: &str = "actual-public-attachment-catalog";
// A fixture bound on fresh model choices, never an authorization allowance.
const MAX_PUBLIC_READ_CHOICES: usize = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PublicReadFailure { ReprepareRequired, AuthorizationUnavailable }

struct PublicReadChoice {
    call_id: String,
    controller: meerkat_core::OperationId,
    // None after a prior choice means its feedback was absent from the current
    // request. The final audit must then prove a native same-input run restart.
    prior_feedback: Option<PublicReadFailure>,
    endpoint_calls_before: usize,
}

#[derive(Default)]
struct PublicReadScript {
    armed: AtomicBool,
    responses: AtomicUsize,
    choices: Mutex<Vec<PublicReadChoice>>,
    catalog_choices: Mutex<Vec<PublicReadChoice>>,
    catalog_feedback: Mutex<Option<ToolResult>>,
    answer_controller: Mutex<Option<meerkat_core::OperationId>>,
    endpoint_calls: Mutex<Option<PathBuf>>,
    expected_instruction: Mutex<Option<String>>,
    offered: Mutex<Option<ToolIdentity>>,
    feedback: Mutex<Option<ToolResult>>,
    injected_contexts: Mutex<Vec<String>>,
}
impl PublicReadScript {
    fn provider(self: &Arc<Self>, inner: Arc<dyn LlmClient>) -> Arc<dyn LlmClient> {
        Arc::new(PublicReadProvider { inner, script: self.clone() })
    }
    fn arm(&self, instruction: String, endpoint_calls: PathBuf) {
        assert!(!instruction.is_empty());
        assert!(self.expected_instruction.lock().unwrap().replace(instruction).is_none());
        assert!(self.endpoint_calls.lock().unwrap().replace(endpoint_calls).is_none());
        assert!(!self.armed.swap(true, Ordering::SeqCst));
    }
    fn endpoint_call_count(&self) -> usize {
        Endpoint::calls_at(self.endpoint_calls.lock().unwrap().as_ref().unwrap()).len()
    }
    fn choice_count(&self) -> usize {
        self.choices.lock().unwrap().len() + self.catalog_choices.lock().unwrap().len()
    }
}


fn public_failure(result: &ToolResult) -> PublicReadFailure {
    [PublicReadFailure::ReprepareRequired, PublicReadFailure::AuthorizationUnavailable]
        .into_iter().find(|kind| {
            let error = match kind {
                PublicReadFailure::ReprepareRequired => meerkat_core::ToolError::AuthorizationRefused {
                    refusal: meerkat_core::OperationRefused::new(meerkat_core::OperationRefusalKind::ReprepareRequired),
                },
                PublicReadFailure::AuthorizationUnavailable => meerkat_core::ToolError::OperationAuthorizationUnavailable,
            };
            let expected = meerkat_core::ops::terminal_tool_outcome_for_error(result.tool_use_id.clone(), error).result;
            serde_json::to_value(result).unwrap() == serde_json::to_value(expected).unwrap()
        }).expect("only exact native operation refusal or unavailable feedback permits a fresh choice")
}

struct PublicReadProvider {
    inner: Arc<dyn LlmClient>,
    script: Arc<PublicReadScript>,
}
#[async_trait::async_trait]
impl LlmClient for PublicReadProvider {
    async fn prepare_controller_credential(&self) -> Result<(), meerkat_core::auth::AuthError> {
        self.inner.prepare_controller_credential().await
    }
    fn plain_model_route(
        &self, model: &str,
    ) -> Result<PlainModelRoute, meerkat_core::ControllerFactsUnavailable> {
        self.inner.plain_model_route(model)
    }
    fn controller_model_selection(&self) -> Option<ControllerModelSelection> {
        self.inner.controller_model_selection()
    }
    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        self.inner.project_replay_messages(messages)
    }
    fn stream<'a>(&'a self, request: &'a LlmRequest) -> LlmStream<'a> {
        self.inner.stream(request)
    }
    fn stream_prepared<'a>(&'a self, request: &'a PreparedLlmRequest) -> LlmStream<'a> {
        // This retains the existing provider's native model authorization,
        // physical-entry observation and request count for every response.
        let stream = self.inner.stream_prepared(request);
        if !self.script.armed.load(Ordering::SeqCst) {
            return stream;
        }
        // A refused prepared operation is not a scripted model response.
        // Advance only after the inner provider yields authorized output so
        // Core recovery cannot consume or replay the one public read.
        let mut response = None;
        let mut tool_response = false;
        Box::pin(stream.map(move |event| event.map(|event| match event {
            LlmEvent::TextDelta { meta, .. } => {
                assert!(response.is_none(), "one authorized response must have one text delta");
                self.script.responses.fetch_add(1, Ordering::SeqCst);
                response = Some(());
                // The current native request, not a response ordinal, decides
                // whether to choose a call or answer from its actual feedback.
                let expected = self.script.expected_instruction.lock().unwrap();
                let actual = request.request().messages.iter().rev().find_map(|message| match message {
                    Message::User(user) => Some(user.text_content()),
                    _ => None,
                });
                assert_eq!(actual.as_deref(), expected.as_deref(),
                    "the current request must retain exact approved B");
                let user_index = request.request().messages.iter().rposition(|message|
                    matches!(message, Message::User(user) if !user.transcript_role.is_injected_context()))
                    .expect("the reviewed user instruction remains a separate message");
                let context_index = user_index.checked_sub(1).expect("host context precedes B");
                let Message::User(context) = &request.request().messages[context_index] else {
                    panic!("host context must be a separate typed user-channel message");
                };
                assert!(context.transcript_role.is_injected_context());
                self.script.injected_contexts.lock().unwrap().push(context.text_content());
                let offered = request.request().tools.iter().filter(|tool|
                    tool.provenance.as_ref().is_some_and(|source|
                        source.kind == ToolSourceKind::Mcp && source.source_id.as_str() == "mail"))
                    .collect::<Vec<_>>();
                // A real deferred catalog withholds the MCP tool until the
                // actual catalog-load call succeeds under this same B input.
                let catalog = self.script.catalog_choices.lock().unwrap();
                let catalog_feedback = catalog.last().and_then(|choice| request.request().messages.iter()
                    .flat_map(|message| match message { Message::ToolResults { results, .. } => results.as_slice(), _ => &[] })
                    .find(|result| result.tool_use_id == choice.call_id)).cloned();
                if offered.is_empty() {
                    assert!(request.request().tools.iter().any(|tool| tool.name.as_str() == "tool_catalog_load"),
                        "B must receive the real catalog control before its deferred MCP tool");
                    let prior_feedback = catalog_feedback.as_ref().map(public_failure);
                    assert!(catalog.len() < MAX_PUBLIC_READ_CHOICES, "bounded catalog choices exhausted");
                    let call_id = format!("{PUBLIC_CATALOG_CALL}-{}", catalog.len() + 1);
                    let controller = request.authorization().unwrap().operation_id().clone();
                    assert!(catalog.iter().all(|choice| choice.controller != controller));
                    drop(catalog);
                    assert_eq!(self.script.endpoint_call_count(), 0);
                    self.script.catalog_choices.lock().unwrap().push(PublicReadChoice {
                        call_id: call_id.clone(), controller, prior_feedback, endpoint_calls_before: 0,
                    });
                    tool_response = true;
                    return LlmEvent::ToolCallComplete { id: call_id, name: "tool_catalog_load".into(),
                        args: serde_json::json!({"names":["attachment_public_read"]}), meta: None };
                }
                assert!(!catalog.is_empty(), "the real schema must defer this MCP tool until catalog load");
                if self.script.catalog_feedback.lock().unwrap().is_none() {
                    let result = catalog_feedback.expect("actual catalog result must precede the exposed MCP tool");
                    assert!(!result.is_error && result.settlement_failures.is_empty(), "{result:?}");
                    let loaded: serde_json::Value = serde_json::from_str(&result.text_content()).unwrap();
                    assert_eq!(loaded["catalog_exact"], true);
                    assert_eq!(loaded["accepted_names"], serde_json::json!(["attachment_public_read"]));
                    assert_eq!(loaded["noop_names"], serde_json::json!([]));
                    *self.script.catalog_feedback.lock().unwrap() = Some(result);
                }
                drop(catalog);
                assert_eq!(offered.len(), 1, "B must see the newly attached public tool");
                let identity = offered[0].identity();
                let mut retained = self.script.offered.lock().unwrap();
                if let Some(previous) = retained.as_ref() { assert_eq!(previous, &identity); }
                else { *retained = Some(identity.clone()); }
                drop(retained);
                let mut choices = self.script.choices.lock().unwrap();
                let feedback = choices.last().and_then(|choice| {
                    let found = request.request().messages.iter().flat_map(|message| match message {
                        Message::ToolResults { results, .. } => results.as_slice(),
                        _ => &[],
                    }).filter(|result| result.tool_use_id == choice.call_id).collect::<Vec<_>>();
                    assert!(found.len() <= 1, "the current request cannot duplicate exact call feedback");
                    found.first().map(|result| (**result).clone())
                });
                let controller = request.authorization().unwrap().operation_id().clone();
                if let Some(result) = feedback.as_ref().filter(|result| !result.is_error) {
                    assert!(result.settlement_failures.is_empty(), "{result:?}");
                    assert_eq!(self.script.endpoint_call_count(), 1,
                        "answer requires exactly one actual endpoint read");
                    assert!(self.script.answer_controller.lock().unwrap().replace(controller).is_none());
                    assert!(self.script.feedback.lock().unwrap().replace(result.clone()).is_none());
                    LlmEvent::TextDelta {
                        delta: format!("Public read completed: {}", result.text_content()), meta,
                    }
                } else {
                    let prior_feedback = feedback.map(|result| {
                        public_failure(&result)
                    });
                    assert!(choices.len() < MAX_PUBLIC_READ_CHOICES, "bounded public read choices exhausted");
                    let endpoint_calls_before = self.script.endpoint_call_count();
                    assert_eq!(endpoint_calls_before, 0, "a fresh choice must not repeat an endpoint read");
                    let call_id = if choices.is_empty() { PUBLIC_READ_CALL.to_owned() }
                        else { format!("{PUBLIC_READ_CALL}-choice-{}", choices.len() + 1) };
                    assert!(choices.iter().all(|choice| choice.controller != controller));
                    choices.push(PublicReadChoice { call_id: call_id.clone(), controller,
                        prior_feedback, endpoint_calls_before });
                    tool_response = true;
                    LlmEvent::ToolCallComplete {
                        id: call_id, name: identity.name.as_str().to_owned(),
                        args: serde_json::json!({}), meta: None,
                    }
                }
            }
            LlmEvent::Done { outcome: LlmDoneOutcome::Success { .. } }
                if tool_response => LlmEvent::Done {
                    outcome: LlmDoneOutcome::success(StopReason::ToolUse),
                },
            other => other,
        })))
    }

    fn provider(&self) -> Provider { self.inner.provider() }
    async fn health_check(&self) -> Result<(), LlmError> { self.inner.health_check().await }
}

/// This replaces only the installed-package subprocess. Its answer must match
/// the actual canonical approved record and full configuration digest. No
/// readiness, actor, native authority or attachment receipt is supplied here.
struct ExactPackageReadback {
    evidence: PrivateSourceEvidence,
    config: meerkat_core::McpServerConfig,
    calls: AtomicUsize,
    barrier: std::sync::Mutex<Option<Arc<ReadbackBarrier>>>,
}
struct ReadbackBarrier {
    entered: tokio::sync::Notify,
    release: tokio::sync::Semaphore,
}
#[async_trait::async_trait]
impl ServiceInstallationReadback for ExactPackageReadback {
    async fn readback(
        &self,
        review: &ApprovalRecord,
    ) -> Result<meerkat_core::McpServerConfig, PrivateOriginalError> {
        let body = review
            .proposed_action
            .body
            .as_ref()
            .ok_or(PrivateOriginalError::Denied)?;
        if review.approval_id.to_string() != self.evidence.resource_id.as_str()
            || digest(&canonical_json(review).map_err(|_| PrivateOriginalError::Conflict)?)
                != self.evidence.sha256
            || review.status != ApprovalStatus::Approved
            || !review
                .decision
                .as_ref()
                .is_some_and(|d| d.decision == ApprovalDecision::Approve)
            || body["service_access"]["connection_revision"]
                != digest(&canonical_json(&self.config).unwrap())
        {
            return Err(PrivateOriginalError::Denied);
        }
        self.calls.fetch_add(1, Ordering::SeqCst);
        let barrier = self.barrier.lock().unwrap().clone();
        if let Some(barrier) = barrier {
            barrier.entered.notify_one();
            barrier.release.acquire().await
                .map_err(|_| PrivateOriginalError::Unavailable)?.forget();
        }
        Ok(self.config.clone())
    }
}

async fn approved_native(
    fixture: &PersistentFixture,
    endpoint: &Endpoint,
    public: bool,
) -> (
    PrivateOriginalRef,
    StoredRecord,
    ApprovedTaskLink,
    Arc<ExactPackageReadback>,
    String,
    String,
) {
    let a = fixture.submit().await;
    let a_input = match &a {
        Ok(PrivateOriginalSubmission::Admitted {
            session_id,
            input_id,
            ..
        }) => {
            assert_eq!(session_id, &fixture.session.to_string());
            input_id.clone()
        }
        _ => panic!("actual A submission did not return its native admission"),
    };
    fixture.finish(&a).await.unwrap();
    let dispatch = fixture.probe.as_ref().unwrap().captured();
    let run = dispatch.retained_work().unwrap().run_id().clone();
    tokio::time::timeout(
        WAIT,
        fixture.machine.wait_run_settled(&fixture.session, &run),
    )
    .await
    .unwrap();
    assert_eq!(
        fixture
            .machine
            .run_turn_terminal(&fixture.session, &run)
            .await,
        Some(TurnTerminalOutcome::Completed)
    );
    let snapshot = acquisition(fixture, &dispatch).await;
    let instruction = snapshot.request().request().request.clone();
    let mut config = endpoint.config(&fixture.scope.realm, public);
    if public {
        // Select the existing native NoConnector review branch explicitly.
        // The exact read ceiling remains in the reviewed MCP configuration.
        config.connector = None;
        config.access = Some(meerkat_core::McpServerAccess::allowing(["read".to_owned()]));
        config.tool_names.insert("read".into(), "attachment_public_read".into());
        assert!(meerkat::ConnectorMcpInstallation::from_config(&config).is_none());
    }
    let b = if public {
        // Raw MCP access and model-facing application invocation are separate.
        // Preserve find_and_install from bootstrap and explicitly review both
        // catalog controls and the mapped invocation. Native task catalog
        // authority is separate from the durable ordinary member policy.
        approve_b_configuration_with_tool_policy(
            fixture, snapshot, [Id::new("read").unwrap()].into(), config.clone(),
            ServiceToolPolicySelection {
                tools: vec![crate::installed_host::tool_policy::ServiceToolInvocation {
                    name: config.tool_names.get("read").unwrap().clone(),
                    consequence: meerkat_core::CompiledToolConsequence::R0,
                }],
                catalog: Some(meerkat_core::CompiledToolConsequence::R0),
            },
        ).await
    } else {
        approve_b_configuration(
            fixture, snapshot, [Id::new("read").unwrap()].into(), Some(config.clone()),
        ).await
    };
    let stored = fixture
        .store
        .get(&fixture.source.key(&b).unwrap())
        .await
        .unwrap()
        .unwrap();
    let record = fixture.source.decode_record(&stored, &b).unwrap();
    assert_eq!(record.event.content_sha256, digest(instruction.as_bytes()),
        "the retained B source must bind the selected acquisition instruction");
    let task = record.approved_task.unwrap();
    let readback = Arc::new(ExactPackageReadback {
        evidence: record.event.original_authentication,
        config,
        calls: AtomicUsize::new(0),
        barrier: std::sync::Mutex::new(None),
    });
    fixture
        .source
        .bind_service_installation_readback(readback.clone())
        .unwrap();
    assert!(
        fixture
            .source
            .held_service_attachment(&b, &task.attachments()[0])
            .unwrap()
            .is_none()
    );
    assert!(
        fixture
            .source
            .attachment_parent(&task.attachments()[0])
            .unwrap()
            .service_attachments
            .lock()
            .unwrap()
            .is_empty()
    );
    (b, stored, task, readback, a_input, instruction)
}

async fn installation_publication_case(change_owner: bool) {
    let (fixture, _) =
        PersistentFixture::for_live_attachment_with_model_and_human_policy(|inner| inner).await;
    let endpoint = Endpoint::new();
    let outcome = AssertUnwindSafe(Box::pin(tokio::time::timeout(WAIT * 4, async {
        fixture.source.bind_controller_pin_source(fixture.service.clone()).unwrap();
        let (b, stored, task, readback, _, _) =
            approved_native(&fixture, &endpoint, true).await;
        let unrelated = fixture.store.admit(toolkit_store::AdmissionRequest {
            key: toolkit_store::StoreKey {
                realm: fixture.scope.realm.clone(),
                owner: "test/attachment-publication".into(),
                id: "unrelated".into(),
            },
            admission_key: "unrelated".into(),
            data: toolkit_store::RecordData {
                schema: "attachment-publication-control".into(),
                version: 1,
                body: serde_json::json!({"value": 0}),
            },
            events: vec![], effects: vec![],
        }).await.unwrap().record().clone();
        let barrier = Arc::new(ReadbackBarrier {
            entered: tokio::sync::Notify::new(),
            release: tokio::sync::Semaphore::new(0),
        });
        *readback.barrier.lock().unwrap() = Some(barrier.clone());
        let model_requests = fixture.model_requests();
        let grant = task.attachments()[0].clone();
        let prepare = fixture.source.prepare_service_mcp_installation_member(
            &fixture.store, &b, &grant.grant,
        );
        tokio::pin!(prepare);
        tokio::select! {
            _ = barrier.entered.notified() => {},
            _ = &mut prepare => panic!("package readback did not hold"),
        }
        if change_owner {
            let mut changed = unrelated.data.clone();
            changed.body["value"] = serde_json::json!(1);
            fixture.store.compare_and_swap(toolkit_store::Transition {
                key: unrelated.key, expected_revision: unrelated.revision,
                expected_digest: Some(unrelated.content_digest), claim: None,
                data: changed, events: vec![], effects: vec![],
            }).await.unwrap();
        }
        barrier.release.add_permits(1);
        let prepared = prepare.await;
        if change_owner {
            assert!(matches!(prepared, Err(PrivateOriginalError::Unavailable)),
                "the old captured publication must fail before native entry");
        } else {
            assert!(prepared.is_ok(), "unchanged canonical owners must prepare");
        }
        *readback.barrier.lock().unwrap() = None;
        assert_eq!(readback.calls.load(Ordering::SeqCst), 1);
        if change_owner {
            // A separate preparation may capture current facts. The refused
            // preparation above never refreshes its retained stamp.
            assert!(fixture.source.prepare_service_mcp_installation_member(
                &fixture.store, &b, &grant.grant,
            ).await.is_ok());
            assert_eq!(readback.calls.load(Ordering::SeqCst), 2);
        }
        assert_eq!(fixture.store.get(&stored.key).await.unwrap().unwrap(), stored);
        assert!(fixture.source.held_service_attachment(&b, &grant).unwrap().is_none());
        assert_eq!(fixture.model_requests(), model_requests);
        assert_eq!(endpoint.starts(), 0);
        assert!(endpoint.calls().is_empty());
    }))).catch_unwind().await;
    endpoint.release();
    fixture.source.join_service_attachments().await;
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    match outcome {
        Ok(Ok(())) => (),
        Ok(Err(_)) => panic!("bounded publication preparation control timed out"),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn installation_readback_preserves_unchanged_publication_before_native_entry() {
    installation_publication_case(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn installation_readback_refuses_changed_publication_before_native_entry() {
    installation_publication_case(true).await;
}

async fn assert_native_inputs(
    fixture: &PersistentFixture,
    runtime_store: &dyn RuntimeStore,
    expected: &[(&str, &InputAuthorityAssociation)],
) -> Vec<StoredInputState> {
    let rows = tokio::time::timeout(
        WAIT,
        runtime_store.load_input_states_strict(&LogicalRuntimeId::for_session(&fixture.session)),
    )
    .await
    .expect("canonical input history timed out")
    .expect("canonical input history must decode completely");
    let actual_ids = rows
        .iter()
        .map(|row| row.state.input_id.to_string())
        .collect::<std::collections::BTreeSet<_>>();
    let expected_ids = expected
        .iter()
        .map(|(id, _)| (*id).to_owned())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        rows.len(),
        expected.len(),
        "no extra canonical native input rows"
    );
    assert_eq!(
        actual_ids, expected_ids,
        "exact native admission identities"
    );
    for (id, association) in expected {
        let row = rows
            .iter()
            .find(|row| row.state.input_id.to_string() == *id)
            .unwrap();
        assert!(
            row.state
                .authority_contributors
                .iter()
                .any(|authority| authority.input_id() == &row.state.input_id
                    && authority.association() == *association),
            "canonical input must retain the exact source association"
        );
    }
    rows
}

/// A controller choice is accepted by this test only with the actual B audit.
/// Historical observations grant nothing; every fresh call still uses native
/// preparation, currentness, entry and outcome checks in the real dispatcher.
fn assert_public_tool_audit(row: &StoredInputState, session: &meerkat_core::SessionId, script: &PublicReadScript) -> u32 {
    use meerkat_authorization_contracts::audit::{AuditObservation, AuditTarget, AuditModelUse,
        StoredAuthorizationAuditObservation};
    use meerkat_core::{OperationEntryBoundary, OperationRefusalKind, authorization::OperationObservedOutcome,
        exact_operation::OperationExecutionScope, ops::ToolDispatchTerminalErrorKind};

    // Root, policy wrapper and MCP leaf can observe the same operation. Check
    // their ordered typed observations, not a count of adapter layers. Core's
    // observe_tool_entry explicitly does not count physical effects.
    fn operation_span(
        records: &[StoredAuthorizationAuditObservation],
        operation: &meerkat_core::OperationId,
        target: &AuditTarget,
        failure: Option<PublicReadFailure>,
    ) -> (usize, usize) {
        let steps = records.iter().enumerate().filter(|(_, record)|
            &record.observation.operation_id == operation).collect::<Vec<_>>();
        assert!(!steps.is_empty());
        let (mut prepared, mut entered, mut saw_refusal, mut returned) = (false, false, false, false);
        for (_, record) in &steps {
            assert_eq!(record.observation.run_id, steps[0].1.observation.run_id,
                "one operation cannot migrate between runs");
            match &record.observation.observation {
                AuditObservation::Prepared { target: actual, .. } => {
                    assert!(!prepared && !entered && !saw_refusal && !returned);
                    assert!(actual.as_ref() == target, "one operation must retain its exact target");
                    prepared = true;
                }
                AuditObservation::Entry { boundary } => {
                    assert!(matches!(boundary, Some(OperationEntryBoundary::ToolDispatch
                        | OperationEntryBoundary::ToolExecutionPolicy | OperationEntryBoundary::ToolEffect)),
                        "tool entries must identify their actual dispatch, policy or effect boundary");
                    assert!(prepared && !saw_refusal && !returned,
                        "adapter entry must precede refusal or returned outcome");
                    entered = true;
                }
                AuditObservation::Refused { target: actual, reason, .. } => {
                    assert_eq!(failure, Some(PublicReadFailure::ReprepareRequired));
                    assert!(!returned);
                    assert!(actual.as_ref() == target, "refusal must bind the exact operation target");
                    assert_eq!(*reason, OperationRefusalKind::ReprepareRequired);
                    saw_refusal = true;
                }
                AuditObservation::AuthorizationUnavailable { target: actual } => {
                    assert_eq!(failure, Some(PublicReadFailure::AuthorizationUnavailable));
                    assert!(!returned);
                    assert!(actual.as_ref() == target, "unavailability must bind the exact operation target");
                    saw_refusal = true;
                }
                AuditObservation::Outcome { outcome } => {
                    if let Some(failure) = failure {
                        assert!(saw_refusal);
                        let expected = match failure {
                            PublicReadFailure::ReprepareRequired => ToolDispatchTerminalErrorKind::AuthorizationRefused,
                            PublicReadFailure::AuthorizationUnavailable => ToolDispatchTerminalErrorKind::OperationAuthorizationUnavailable,
                        };
                        assert!(matches!(outcome, OperationObservedOutcome::ToolDispatchError { error }
                            if *error == expected), "every failed adapter outcome must retain its exact authorization error");
                    } else {
                        assert!(entered && !saw_refusal);
                        assert!(matches!(outcome, OperationObservedOutcome::ToolDispatchReturned {
                            asynchronous_operations, result_is_error: false, terminal_error: None,
                        } if asynchronous_operations.is_empty()),
                            "every successful adapter outcome must retain the synchronous read result");
                    }
                    returned = true;
                }
                _ => panic!("unexpected observation in the exact public tool operation"),
            }
        }
        if failure.is_some() {
            // A prepare-time refusal can have no Entry/Outcome. A later
            // currentness refusal can follow adapter entries and unwind with
            // error outcomes. Neither pattern proves an endpoint effect.
            assert!(saw_refusal);
        } else {
            assert!(prepared && entered && returned && !saw_refusal);
        }
        (steps.first().unwrap().0, steps.last().unwrap().0)
    }

    let value = serde_json::to_value(row).unwrap();
    let records: Vec<StoredAuthorizationAuditObservation> =
        serde_json::from_value(value["authorization_audit"].clone()).unwrap();
    let run = row.seed.last_run_id.as_ref().unwrap();
    assert!(!records.is_empty());
    // This is separate actual endpoint evidence, not an interpretation of
    // native stage observations. The endpoint records tools/call before reply.
    assert_eq!(script.endpoint_call_count(), 1, "completed B has exactly one physical endpoint read");
    for record in &records {
        assert!(record.observation.run_id.is_some(), "every B observation must retain its actual run");
        assert_eq!(record.contributors.len(), 1);
        assert_eq!(record.contributors[0].input_id, row.state.input_id);
        assert!(matches!(&record.observation.execution_scope, OperationExecutionScope::RuntimeInput {
            owner_session_id, canonical_input_id, submitted_input_id, ..
        } if owner_session_id == session && canonical_input_id == &row.state.input_id
            && submitted_input_id == &row.state.input_id));
    }
    let mut tools: Vec<(&meerkat_core::OperationId, &AuditTarget)> = Vec::new();
    for record in &records {
        let target = match &record.observation.observation {
            AuditObservation::Prepared { target, .. } | AuditObservation::Refused { target, .. }
                | AuditObservation::AuthorizationUnavailable { target } => target,
            _ => continue,
        };
        if matches!(target.as_ref(), AuditTarget::Tool { .. }) && !tools.iter().any(
            |(operation, _)| *operation == &record.observation.operation_id) {
            tools.push((&record.observation.operation_id, target.as_ref()));
        }
    }
    let catalog_choices = script.catalog_choices.lock().unwrap();
    assert!(!catalog_choices.is_empty() && catalog_choices.len() <= MAX_PUBLIC_READ_CHOICES);
    let catalog_tools = tools.iter().filter(|(_, target)| matches!(target,
        AuditTarget::Tool { tool_name, .. } if tool_name == "tool_catalog_load")).copied().collect::<Vec<_>>();
    assert_eq!(catalog_tools.len(), catalog_choices.len());
    let mut catalog_final_run_calls = 0;
    let mut catalog_end = 0;
    for (index, ((operation, target), choice)) in catalog_tools.iter().zip(catalog_choices.iter()).enumerate() {
        let AuditTarget::Tool { call_id, arguments_digest, owners, .. } = target else { unreachable!() };
        assert_eq!(call_id, &choice.call_id);
        assert_eq!(choice.endpoint_calls_before, 0);
        if let AuditTarget::Tool { arguments_digest: first, owners: first_owners, .. } = catalog_tools[0].1 {
            assert!(arguments_digest == first && owners == first_owners);
        }
        let failure = if index + 1 == catalog_choices.len() { None } else {
            let mut failures = records.iter().filter(|record| &record.observation.operation_id == *operation)
                .filter_map(|record| match &record.observation.observation {
                    AuditObservation::Refused { reason: OperationRefusalKind::ReprepareRequired, .. } => Some(PublicReadFailure::ReprepareRequired),
                    AuditObservation::AuthorizationUnavailable { .. } => Some(PublicReadFailure::AuthorizationUnavailable),
                    _ => None,
                });
            let first = failures.next().expect("an earlier catalog choice needs an exact typed native failure");
            assert!(failures.all(|kind| kind == first));
            if let Some(feedback) = catalog_choices[index + 1].prior_feedback { assert_eq!(feedback, first); }
            Some(first)
        };
        let (start, end) = operation_span(&records, operation, target, failure);
        let operation_run = records[start].observation.run_id.as_ref().unwrap();
        let controller = records.iter().enumerate().filter(|(_, record)| record.observation.operation_id == choice.controller)
            .collect::<Vec<_>>();
        assert_eq!(controller.len(), 3);
        assert!(controller.iter().all(|(i, record)| *i < start && *i >= catalog_end
            && record.observation.run_id.as_ref() == Some(operation_run)));
        assert!(matches!(&controller[0].1.observation.observation, AuditObservation::Prepared { target, .. }
            if matches!(target.as_ref(), AuditTarget::Model(model) if model.usage == AuditModelUse::ControllerInference)));
        assert!(matches!(&controller[1].1.observation.observation, AuditObservation::Entry {
            boundary: Some(OperationEntryBoundary::ModelRequest)
        }));
        assert!(matches!(&controller[2].1.observation.observation, AuditObservation::Outcome {
            outcome: OperationObservedOutcome::HttpResponse { status: 200 }
        }));
        if operation_run == run { catalog_final_run_calls += 1; }
        catalog_end = end;
    }
    assert_eq!(script.catalog_feedback.lock().unwrap().as_ref().unwrap().tool_use_id,
        catalog_choices.last().unwrap().call_id);
    tools.retain(|(_, target)| !matches!(target, AuditTarget::Tool { tool_name, .. } if tool_name == "tool_catalog_load"));
    let choices = script.choices.lock().unwrap();
    assert!(!choices.is_empty() && choices.len() <= MAX_PUBLIC_READ_CHOICES);
    assert_eq!(tools.len(), choices.len(), "every actual scripted choice has one exact native tool operation");
    let expected_identity = script.offered.lock().unwrap();
    let mut previous: Option<(usize, &meerkat_core::RunId, PublicReadFailure)> = None;
    let mut final_run_calls = catalog_final_run_calls;
    for (index, ((operation, target), choice)) in tools.iter().zip(choices.iter()).enumerate() {
        let AuditTarget::Tool { call_id, tool_name, arguments_digest, owners } = target else { unreachable!() };
        assert_eq!(call_id, &choice.call_id);
        assert_eq!(tool_name, expected_identity.as_ref().unwrap().name.as_str());
        assert_eq!(choice.endpoint_calls_before, 0);
        if let AuditTarget::Tool { arguments_digest: first_arguments, owners: first_owners, .. } = tools[0].1 {
            assert!(arguments_digest == first_arguments && owners == first_owners,
                "every choice must retain the same exact arguments and selected owners");
        }
        let operation_records = records.iter().filter(|record|
            &record.observation.operation_id == *operation).collect::<Vec<_>>();
        let operation_run = operation_records[0].observation.run_id.as_ref().unwrap();
        let failure = if index + 1 == choices.len() { None } else {
            let mut failures = operation_records.iter().filter_map(|record| match &record.observation.observation {
                AuditObservation::Refused { reason: OperationRefusalKind::ReprepareRequired, .. } => Some(PublicReadFailure::ReprepareRequired),
                AuditObservation::AuthorizationUnavailable { .. } => Some(PublicReadFailure::AuthorizationUnavailable),
                _ => None,
            });
            let first = failures.next().expect("an earlier choice needs an exact typed native failure");
            assert!(failures.all(|kind| kind == first));
            Some(first)
        };
        let (tool_start, tool_end) = operation_span(&records, operation, target, failure);
        assert!(tool_start > catalog_end, "mapped invocation follows actual catalog admission");
        let controller = records.iter().enumerate().filter(|(_, record)|
            record.observation.operation_id == choice.controller).collect::<Vec<_>>();
        assert_eq!(controller.len(), 3, "the actual fixture controller observes prepare, entry and HTTP return");
        assert!(controller.iter().all(|(i, record)| *i < tool_start
            && record.observation.run_id.as_ref() == Some(operation_run)));
        assert!(matches!(&controller[0].1.observation.observation, AuditObservation::Prepared { target, .. }
            if matches!(target.as_ref(), AuditTarget::Model(model)
                if model.usage == AuditModelUse::ControllerInference && model.hosted_capabilities.is_empty())));
        assert!(matches!(&controller[1].1.observation.observation, AuditObservation::Entry {
            boundary: Some(OperationEntryBoundary::ModelRequest)
        }));
        assert!(matches!(&controller[2].1.observation.observation, AuditObservation::Outcome {
            outcome: OperationObservedOutcome::HttpResponse { status: 200 }
        }));
        if let Some((prior_end, prior_run, prior_failure)) = previous {
            assert!(controller.iter().all(|(i, _)| *i > prior_end));
            if let Some(feedback) = choice.prior_feedback { assert_eq!(feedback, prior_failure); }
            else { assert_ne!(operation_run, prior_run, "missing feedback requires a native same-B run restart"); }
        } else { assert!(choice.prior_feedback.is_none()); }
        if operation_run == run { final_run_calls += 1; }
        if let Some(failure) = failure { previous = Some((tool_end, operation_run, failure)); }
        else {
            assert_eq!(operation_run, run, "the one successful read must belong to final B's run");
            assert_eq!(script.feedback.lock().unwrap().as_ref().unwrap().tool_use_id, choice.call_id);
            let answer_id = script.answer_controller.lock().unwrap().clone().unwrap();
            assert!(choices.iter().all(|choice| choice.controller != answer_id));
            let answer = records.iter().enumerate().filter(|(_, record)|
                record.observation.operation_id == answer_id).collect::<Vec<_>>();
            assert_eq!(answer.len(), 3);
            assert!(answer.iter().all(|(i, record)| *i > tool_end && record.observation.run_id.as_ref() == Some(run)));
            assert!(matches!(&answer[0].1.observation.observation, AuditObservation::Prepared { target, .. }
                if matches!(target.as_ref(), AuditTarget::Model(model)
                    if model.usage == AuditModelUse::ControllerInference && model.hosted_capabilities.is_empty())));
            assert!(matches!(&answer[1].1.observation.observation, AuditObservation::Entry {
            boundary: Some(OperationEntryBoundary::ModelRequest)
        }));
            assert!(matches!(&answer[2].1.observation.observation, AuditObservation::Outcome {
                outcome: OperationObservedOutcome::HttpResponse { status: 200 }
            }));
        }
    }
    final_run_calls
}

/// Persistent input history is authoritative after consumed inputs leave the
/// live ledger. Observe the exact B row and its run, not an input count or a
/// provider response alone. This only reads owners and never wakes the run.
async fn assert_public_input_completed(
    fixture: &PersistentFixture,
    runtime_store: &dyn RuntimeStore,
    original: &PrivateOriginalRef,
    input: &str,
    expected: &[(&str, &InputAuthorityAssociation)],
    expected_result: &str,
    script: &PublicReadScript,
) {
    tokio::time::timeout(WAIT, async {
        loop {
            let rows = assert_native_inputs(fixture, runtime_store, expected).await;
            let row = rows
                .iter()
                .find(|row| row.state.input_id.to_string() == input)
                .unwrap();
            if row.seed.phase == InputLifecycleState::Consumed
                && row.seed.terminal_outcome == Some(InputTerminalOutcome::Consumed)
            {
                let run = row
                    .seed
                    .last_run_id
                    .as_ref()
                    .expect("B ran before consumption");
                if fixture.machine.run_turn_terminal(&fixture.session, run).await
                    == Some(TurnTerminalOutcome::Completed)
                    && fixture.machine.current_run(&fixture.session).await.as_ref() != Some(run)
                {
                    let native_input = InputId::from_uuid(input.parse().unwrap());
                    let terminal = fixture.machine.input_terminal_receipt(
                        &fixture.session, InteractionSelector::InputId(native_input.clone()),
                    ).await.unwrap().expect("B has an actual terminal receipt");
                    if let InputTerminalReceiptRead::Finalized(receipt) = terminal.report {
                        assert_eq!(receipt.input_id(), &native_input);
                        assert_eq!(receipt.owner_input_id(), &native_input);
                        assert_eq!(receipt.recipient_input_ids(), std::slice::from_ref(&native_input));
                        assert_eq!(receipt.run_id(), Some(run));
                        let CompletionOutcome::Completed(result) = receipt.outcome()
                            else { panic!("B must complete with its actual result") };
                        assert_eq!(result.session_id, fixture.session);
                        assert_eq!(result.text, format!("Public read completed: {expected_result}"));
                        // Core counts attempted tool calls in this completed run.
                        // Earlier runs remain in B's audit, while physical success
                        // is independently exactly one endpoint call across all runs.
                        assert_eq!(result.tool_calls, assert_public_tool_audit(row, &fixture.session, script));
                        let projected = fixture.source.observe_service_input_receipt(original).await.unwrap();
                        let crate::private_originals::PrivateServiceInputObservation::Admitted {
                            session_id, input_id, receipt: Some(projected),
                        } = projected else { panic!("canonical B must expose its native receipt") };
                        assert_eq!(session_id, fixture.session.to_string());
                        assert_eq!(input_id, input);
                        assert!(matches!((projected.source, terminal.source),
                            (crate::private_originals::PrivateReceiptWitness::LiveRuntime,
                                meerkat_runtime::terminal_status::TerminalWitnessSource::LiveRuntime)
                            | (crate::private_originals::PrivateReceiptWitness::DurableStore,
                                meerkat_runtime::terminal_status::TerminalWitnessSource::DurableStore)));
                        let crate::private_originals::PrivateServiceInputReport::Finalized {
                            input_id, terminal, attempt_count, scope, run_id, owner_input_id,
                            recipient_input_ids, outcome,
                        } = projected.report else { panic!("the read must retain finalized state") };
                        assert_eq!(input_id, input);
                        assert_eq!(terminal, receipt.terminal().clone());
                        assert_eq!(attempt_count, receipt.attempt_count());
                        assert!(matches!(scope, crate::private_originals::PrivateReceiptScope::Run { run_id: scoped }
                            if scoped == run.to_string()));
                        assert_eq!(run_id, Some(run.to_string()));
                        assert_eq!(owner_input_id, input);
                        assert_eq!(recipient_input_ids, vec![input.to_owned()]);
                        assert_eq!(serde_json::to_value(outcome).unwrap(), serde_json::to_value(receipt.outcome()).unwrap());
                        break;
                    }
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the exact public B input must complete and settle");
}

async fn actual_attachment_case(cancel: bool, public: bool) {
    let script = public.then(|| Arc::new(PublicReadScript::default()));
    let (fixture, runtime_store) = match &script {
        Some(script) => {
            let script = script.clone();
            PersistentFixture::for_live_attachment_with_model_and_human_policy(move |inner| script.provider(inner)).await
        }
        None => PersistentFixture::for_live_attachment().await,
    };
    if public {
        // Exercise cold source custody with the same actual native service.
        fixture.source.bind_controller_pin_source(fixture.service.clone()).unwrap();
    }
    let endpoint = Endpoint::new();
    let outcome = AssertUnwindSafe(Box::pin(tokio::time::timeout(WAIT * 4, async {
        let (b, stored, task, readback, a_input, instruction) = approved_native(&fixture, &endpoint, public).await;
        let a_binding = fixture.binding().await.unwrap().unwrap();
        let a_only = [(a_input.as_str(), &a_binding)];
        let a_rows = assert_native_inputs(&fixture, runtime_store.as_ref(), &a_only).await;
        let a_dispatch = fixture.probe.as_ref().unwrap().captured();
        assert_eq!(
            a_rows[0].seed.last_run_id.as_ref(),
            Some(a_dispatch.retained_work().unwrap().run_id())
        );
        assert_eq!(a_rows[0].seed.phase, InputLifecycleState::Consumed);
        assert_eq!(
            a_rows[0].seed.terminal_outcome,
            Some(InputTerminalOutcome::Consumed)
        );
        let source = &fixture.source;
        assert!(matches!(source.observe_service_input_receipt(&b).await.unwrap(),
            crate::private_originals::PrivateServiceInputObservation::AdmissionUnresolved));
        let model_requests = fixture.model_requests();
        if let Some(script) = &script { script.arm(instruction.clone(), endpoint.called.clone()); }
        let actor = fixture
            .service
            .live_session_actor_witness(&fixture.session)
            .await
            .unwrap();

        // A genuine held member turn boundary makes the native try-entry Busy.
        // It must leave no accepted keeper, child process or B binding behind.
        let lease = fixture
            .service
            .acquire_live_session_actor_turn_boundary_lease_exact(&actor)
            .await
            .unwrap()
            .unwrap();
        source
            .step_service_input_with_attachment(&fixture.store, &stored)
            .await
            .unwrap();
        assert!(source.held_service_attachment(&b, &task.attachments()[0]).unwrap().is_none());
        assert_eq!(endpoint.starts(), 0);
        assert_eq!(
            fixture.store.get(&stored.key).await.unwrap().unwrap(),
            stored
        );
        assert_native_inputs(&fixture, runtime_store.as_ref(), &a_only).await;
        drop(lease);
        let readbacks_before_entry = readback.calls.load(Ordering::SeqCst);

        // Enter the production preparation/call/observation path, still with
        // no keeper. Native starts the exact reviewed process and retains the
        // accepted attempt while its real tools/list answer is held.
        source
            .step_service_input_with_attachment(&fixture.store, &stored)
            .await
            .unwrap();
        endpoint.wait_for(&endpoint.listed).await;
        let held = source
            .held_service_attachment(&b, &task.attachments()[0])
            .unwrap()
            .expect("actual accepted attempt retained");
        let receipt = held
            .receipt(|| panic!("entered native call cannot start twice"))
            .await
            .unwrap()
            .unwrap();
        assert!(receipt.outcome.staged && !receipt.outcome.persisted);
        assert_eq!(
            receipt.attempt.observe("read").await,
            McpAttachmentReadiness::Pending
        );
        assert_eq!(
            fixture
                .service
                .live_session_actor_witness(&fixture.session)
                .await
                .as_ref(),
            Some(&actor)
        );
        assert_eq!(
            fixture.store.get(&stored.key).await.unwrap().unwrap(),
            stored
        );
        assert_native_inputs(&fixture, runtime_store.as_ref(), &a_only).await;
        assert_eq!(fixture.model_requests(), model_requests);
        assert_eq!(endpoint.starts(), 1);
        assert_eq!(
            readback.calls.load(Ordering::SeqCst),
            readbacks_before_entry + 1
        );

        source
            .step_service_input_with_attachment(&fixture.store, &stored)
            .await
            .unwrap();
        assert!(Arc::ptr_eq(
            &held,
            &source.held_service_attachment(&b, &task.attachments()[0]).unwrap().unwrap()
        ));
        assert_eq!(
            fixture.store.get(&stored.key).await.unwrap().unwrap(),
            stored
        );
        assert_eq!(endpoint.starts(), 1);
        assert_eq!(
            readback.calls.load(Ordering::SeqCst),
            readbacks_before_entry + 1
        );
        if cancel {
            source
                .record_task_group_cancelled(&task.parent)
                .await
                .unwrap();
        }
        endpoint.release();
        if !cancel {
            assert!(matches!(
                receipt.attempt.wait("read", WAIT).await,
                McpAttachmentReadiness::Usable { .. }
            ));
            assert!(matches!(
                receipt.attempt.observe("not-reviewed-or-listed").await,
                McpAttachmentReadiness::ToolNotRouted { .. }
            ));
        }
        let expected_control = if cancel { None } else {
            Some(source.current_service_mcp_control(&held.control,
                &held.control.effect_binding(McpAttachmentEffect::Apply),
                source.owner.clock().now().unwrap().unix_ms, false).unwrap())
        };
        let current = fixture.store.get(&stored.key).await.unwrap().unwrap();
        source
            .step_service_input_with_attachment(&fixture.store, &current)
            .await
            .unwrap();
        let saved = fixture.store.get(&stored.key).await.unwrap().unwrap();
        let record = source.decode_record(&saved, &b).unwrap();
        if cancel {
            assert!(record.operation_cancelled);
            assert!(record.final_association.is_none() && record.service_admission.is_none());
            assert!(record.bound_service_input.is_none(), "cancelled setup cannot retain success context");
            assert_native_inputs(&fixture, runtime_store.as_ref(), &a_only).await;
            assert_eq!(fixture.model_requests(), model_requests);
        } else {
            assert!(record.final_association.is_some() && record.service_admission.is_some());
            let meerkat_runtime::Input::Prompt(bound) = record.bound_service_input.as_ref().unwrap().decode().unwrap() else {
                panic!("approved B must retain its exact prompt input");
            };
            assert_eq!(bound.content.text_content(), instruction);
            assert_eq!(digest(instruction.as_bytes()), record.event.content_sha256);
            assert_eq!(bound.injected_context.len(), 1);
            let context = bound.injected_context[0].text_content();
            let observed: serde_json::Value = serde_json::from_str(&context).unwrap();
            let canonical = expected_control.as_ref().unwrap();
            assert_eq!(observed["operation"], task.acquisition.id);
            assert_eq!(observed["original"], b.original.as_str());
            assert_eq!(observed["connection"], serde_json::to_value(&canonical.connection).unwrap());
            assert_eq!(observed["resource"], serde_json::to_value(&canonical.resource).unwrap());
            assert_eq!(observed["allowed_operations"], serde_json::to_value(&canonical.operations).unwrap());
            assert_eq!(observed["expires_at_ms"], canonical.expires_at_ms);
            assert_eq!(observed["configuration_sha256"], serde_json::to_value(&canonical.configuration_revision).unwrap());
            // Positional launch values and the private fixture result are not
            // authority facts and must not leak through the host observation.
            assert!(!context.contains(&endpoint.result));
            assert!(!context.contains(endpoint.started.to_str().unwrap()));
            assert!(!context.contains("/bin/sh"));
            let admission = record.service_admission.as_ref().unwrap();
            assert_eq!(admission.session_id, fixture.session.to_string());
            assert_ne!(admission.input_id, a_input);
            let expected = [
                (a_input.as_str(), &a_binding),
                (
                    admission.input_id.as_str(),
                    record.final_association.as_ref().unwrap(),
                ),
            ];
            assert_native_inputs(&fixture, runtime_store.as_ref(), &expected).await;
            if public {
                assert_public_input_completed(
                    &fixture,
                    runtime_store.as_ref(),
                    &b,
                    &admission.input_id,
                    &expected,
                    &endpoint.result,
                    script.as_ref().unwrap(),
                )
                .await;
                assert_eq!(fixture.model_requests(), model_requests + 1 + script.as_ref().unwrap().choice_count());
                let calls = endpoint.calls();
                assert_eq!(calls.len(), 1, "one real tools/call reaches the attached process");
                assert_eq!(calls[0]["params"]["name"], "read");
                assert_eq!(calls[0]["params"]["arguments"], serde_json::json!({}));
                let script = script.as_ref().unwrap();
                assert_ne!(script.offered.lock().unwrap().as_ref().unwrap().name.as_str(), "read",
                    "the actual catalog supplies the exposed identity, while MCP receives raw read");
                assert_eq!(script.responses.load(Ordering::SeqCst), 1 + script.choice_count());
                let contexts = script.injected_contexts.lock().unwrap();
                assert_eq!(contexts.len(), script.responses.load(Ordering::SeqCst));
                assert!(contexts.iter().all(|projected| projected == &context),
                    "the actual provider receives the exact retained host context beside the unchanged instruction");
                drop(contexts);
                let feedback = script.feedback.lock().unwrap();
                assert_eq!(feedback.as_ref().unwrap().text_content(), endpoint.result);
            }
        }
        assert!(source.held_service_attachment(&b, &task.attachments()[0]).unwrap().is_none());
        source
            .step_service_input_with_attachment(&fixture.store, &saved)
            .await
            .unwrap();
        let replay_saved = fixture.store.get(&stored.key).await.unwrap().unwrap();
        let replay = source.decode_record(&replay_saved, &b).unwrap();
        assert!(replay.service_admission == record.service_admission);
        assert!(replay.final_association == record.final_association);
        assert!(replay.bound_service_input == record.bound_service_input,
            "acknowledgement replay cannot refresh the host observation");
        if let Some(admission) = &record.service_admission {
            assert_native_inputs(
                &fixture,
                runtime_store.as_ref(),
                &[
                    (a_input.as_str(), &a_binding),
                    (
                        admission.input_id.as_str(),
                        record.final_association.as_ref().unwrap(),
                    ),
                ],
            )
            .await;
        } else {
            assert_native_inputs(&fixture, runtime_store.as_ref(), &a_only).await;
        }
        if public {
            assert_eq!(
                fixture.model_requests(),
                if cancel { model_requests } else { model_requests + 1 + script.as_ref().unwrap().choice_count() },
                "recovery cannot replay consumed B or submit a cancelled B"
            );
            let script = script.as_ref().unwrap();
            assert_eq!(endpoint.calls().len(), if cancel { 0 } else { 1 });
            assert_eq!(script.responses.load(Ordering::SeqCst), if cancel { 0 } else { 1 + script.choice_count() });
            if cancel { assert_eq!(script.choice_count(), 0); }
            assert_eq!(script.offered.lock().unwrap().is_some(), !cancel);
            assert_eq!(script.feedback.lock().unwrap().is_some(), !cancel);
            assert_eq!(script.catalog_feedback.lock().unwrap().is_some(), !cancel);
        }
        assert_eq!(
            endpoint.starts(),
            1,
            "no reattachment after readiness or canonical admission"
        );
        assert_eq!(
            readback.calls.load(Ordering::SeqCst),
            readbacks_before_entry + 1
        );
        if public && !cancel {
            let admission = record.service_admission.as_ref().unwrap();
            let association = record.final_association.as_ref().unwrap();
            let input = record.bound_service_input.as_ref().unwrap().decode().unwrap();
            let actual = fixture.machine.input_state(&fixture.session, input.id()).await.unwrap().unwrap();
            let retained = source.retained.lock().unwrap().iter()
                .find(|held| held.reference == b).unwrap().clone();
            let mut retry = input.clone();
            let meerkat_runtime::Input::Prompt(prompt) = &mut retry else { panic!("prompt"); };
            prompt.header.id = InputId::new();
            source.retain_bound_service_input(&b, &retained.original, &retry).await.unwrap();
            meerkat_runtime::input_state::verify_exact_replay(&actual.state, &retry).unwrap();
            let meerkat_runtime::Input::Prompt(prompt) = &mut retry else { panic!("prompt"); };
            let mut context: serde_json::Value = serde_json::from_str(&prompt.injected_context[0].text_content()).unwrap();
            context["expires_at_ms"] = serde_json::json!(0);
            prompt.injected_context[0] = meerkat_core::ContentInput::Text(context.to_string());
            assert!(matches!(source.retain_bound_service_input(&b, &retained.original, &retry).await,
                Err(PrivateOriginalError::Conflict)));
            assert!(meerkat_runtime::input_state::verify_exact_replay(&actual.state, &retry).is_err());
            drop(retained);
            // Lose only this settled B's source custody. Native admission and
            // the existing whole-input record survive; no new input is sent.
            source.retained.lock().unwrap().retain(|held| held.reference != b);
            source.owner.forget_task_custody_for_test(association);
            let restored = source.restore_service_admission(&b).await.unwrap().unwrap();
            assert_eq!(restored.admission.input_id, admission.input_id);
            assert_eq!(restored.admission.session_id, admission.session_id);
            let restored_record = source.decode_record(&fixture.store.get(&stored.key).await.unwrap().unwrap(), &b).unwrap();
            assert!(restored_record.bound_service_input == record.bound_service_input);
            assert_eq!(endpoint.calls().len(), 1);
            assert_eq!(fixture.model_requests(), model_requests + 1 + script.as_ref().unwrap().choice_count());
            // Revoking future operation use must not erase the exact completed
            // native fact. This read neither resubmits B nor resolves execution.
            source.cancel_original_operation(&b).await.unwrap();
            let admission = record.service_admission.as_ref().unwrap();
            assert_public_input_completed(&fixture, runtime_store.as_ref(), &b, &admission.input_id,
                &[(a_input.as_str(), &a_binding), (admission.input_id.as_str(), record.final_association.as_ref().unwrap())],
                &endpoint.result, script.as_ref().unwrap()).await;
            assert_eq!(endpoint.calls().len(), 1);
            assert_eq!(fixture.model_requests(), model_requests + 1 + script.as_ref().unwrap().choice_count());
        }
    })))
    .catch_unwind()
    .await;
    // Failure-only observation of the SAME native runtime store. This reads
    // existing protected audit records before normal cleanup; it neither
    // authorizes an operation nor replaces the original failure or assertion.
    if public && !matches!(&outcome, Ok(Ok(()))) {
        let endpoint_call_count = match std::fs::read_to_string(&endpoint.called) {
            Ok(text) => Some(text.lines().count()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Some(0),
            Err(_) => None,
        };
        // A provider assertion may poison its test-only lock. Preserve the
        // original failure and still retain these safe mechanical observations.
        let choices = script.as_ref().unwrap().choices.lock().unwrap_or_else(|error| error.into_inner());
        let choice_observations = choices.iter().map(|choice| serde_json::json!({
            "call_id": choice.call_id, "controller_operation_id": choice.controller,
            "endpoint_calls_before": choice.endpoint_calls_before,
            "prior_feedback": choice.prior_feedback.map(|kind| match kind {
                PublicReadFailure::ReprepareRequired => "operation_refused",
                PublicReadFailure::AuthorizationUnavailable => "authorization_unavailable",
            }),
        })).collect::<Vec<_>>();
        drop(choices);
        eprintln!("{}", serde_json::json!({
            "event": "attachment_control_endpoint_observation",
            "endpoint_call_count": endpoint_call_count,
            "choices": choice_observations,
        }));
        let observed = tokio::time::timeout(WAIT,
            runtime_store.load_input_states_strict(&LogicalRuntimeId::for_session(&fixture.session))).await;
        match observed {
            Ok(Ok(rows)) => for row in rows {
                let Ok(encoded) = serde_json::to_value(&row) else { continue; };
                let Some(audit) = encoded.get("authorization_audit") else { continue; };
                let Ok(records) = serde_json::from_value::<Vec<
                    meerkat_authorization_contracts::audit::StoredAuthorizationAuditObservation
                >>(audit.clone()) else { continue; };
                for (index, record) in records.iter().enumerate() {
                    use meerkat_authorization_contracts::audit::{AuditObservation, AuditTarget};
                    let observation = &record.observation;
                    let (kind, target, reason, publication, observed_at) = match &observation.observation {
                        AuditObservation::Prepared { target, policy } =>
                            ("prepared", Some(target.as_ref()), None, Some(policy.publication_sequence), Some(policy.observed_at_ms)),
                        AuditObservation::Refused { target, reason, .. } =>
                            ("refused", Some(target.as_ref()), Some(*reason), None, None),
                        AuditObservation::AuthorizationUnavailable { target } =>
                            ("authorization_unavailable", Some(target.as_ref()), None, None, None),
                        AuditObservation::Entry { .. } => ("entry", None, None, None, None),
                        AuditObservation::Outcome { .. } => ("outcome", None, None, None, None),
                        AuditObservation::ReviewAttemptStarted { .. } => ("review_attempt_started", None, None, None, None),
                        AuditObservation::MandatoryHookWithheld { .. } => ("mandatory_hook_withheld", None, None, None, None),
                        _ => ("unknown", None, None, None, None),
                    };
                    // Do not print prompts, tool arguments/digests, credentials,
                    // accounts, endpoints, grant lineages or output payloads.
                    let target = match target {
                        Some(AuditTarget::Model(model)) => serde_json::json!({
                            "kind": "model", "usage": model.usage,
                            "hosted_capability_count": model.hosted_capabilities.len(),
                        }),
                        Some(AuditTarget::Tool { call_id, tool_name, .. }) =>
                            serde_json::json!({"kind": "tool", "call_id": call_id, "tool_name": tool_name}),
                        Some(_) => serde_json::json!({"kind": "other"}),
                        None => serde_json::Value::Null,
                    };
                    let returned = match &observation.observation {
                        AuditObservation::Outcome { outcome } => {
                            use meerkat_core::authorization::OperationObservedOutcome;
                            match outcome {
                                OperationObservedOutcome::ToolDispatchReturned {
                                    asynchronous_operations, result_is_error, terminal_error,
                                } => serde_json::json!({
                                    "kind": "tool_dispatch_returned",
                                    "asynchronous_operation_count": asynchronous_operations.len(),
                                    "result_is_error": result_is_error, "terminal_error": terminal_error,
                                }),
                                OperationObservedOutcome::ToolDispatchError { error } =>
                                    serde_json::json!({"kind": "tool_dispatch_error", "error": error}),
                                OperationObservedOutcome::HttpResponse { status } =>
                                    serde_json::json!({"kind": "http_response", "status": status}),
                                _ => serde_json::json!({"kind": "other"}),
                            }
                        }
                        _ => serde_json::Value::Null,
                    };
                    eprintln!("{}", serde_json::json!({
                        "event": "attachment_control_native_authorization_audit",
                        "input_id": row.state.input_id, "index": index,
                        "operation_id": observation.operation_id, "run_id": observation.run_id,
                        "kind": kind, "target": target, "reason": reason, "outcome": returned,
                        "publication_sequence": publication, "observed_at_ms": observed_at,
                    }));
                }
            },
            _ => eprintln!("attachment control native audit observation unavailable"),
        }
    }
    // Release before shutdown on both success and panic. Native owns process
    // termination; this test does not kill a provider or detached OS process.
    endpoint.release();
    fixture.source.join_service_attachments().await;
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    match outcome {
        Ok(Ok(())) => (),
        Ok(Err(_)) => panic!("actual persistent attachment control timed out"),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actual_persistent_attachment_from_empty_keeper_gates_and_admits_b_once() {
    actual_attachment_case(false, false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actual_pending_attachment_cannot_admit_b_after_parent_cancellation() {
    actual_attachment_case(true, false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actual_public_attachment_loads_catalog_calls_mapped_read_and_completes_exact_b_once_with_cancellation() {
    // Real captured A with find_and_install only, reviewed complete successor,
    // native catalog scope, Apply, catalog load, mapped invocation and exact B
    // completion. Controlled MCP/model IO is not live provider acceptance.
    for cancel in [false, true] {
        actual_attachment_case(cancel, true).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actual_initialize_failure_is_captured_and_delivered_only_to_original() {
    use meerkat::surface::SessionServiceDeliveryHost;
    use meerkat::{
        ContinuationHostBindings, ContinuationKey, ContinuationOwner, ContinuationStatus,
        DetachedJobService, MemoryDetachedJobStore, RuntimeDeliveryOwner,
    };
    use meerkat_runtime::{InMemoryRuntimeStore, RuntimeDeliveryInbox, SessionServiceRuntimeExt};
    use meerkat_toolkit::continuation::{ContinuationStep, ContinuationStepOptions};
    use meerkat_toolkit::setup::{
        ApprovedServiceAttachmentClass, ApprovedServiceAttachmentFailure,
        ApprovedServiceAttachmentPhase,
    };

    const SENTINEL: &str = "toolkit-native-setup-diagnostic-sentinel";
    let (mut fixture, runtime_store) = PersistentFixture::for_live_attachment().await;
    let native_handle = fixture.native_for_console_mut().mob_handle();
    let endpoint = Endpoint::failing_initialize();
    let mut owner_handle = None;
    let mut owner_passes = None;
    let outcome = AssertUnwindSafe(Box::pin(tokio::time::timeout(WAIT * 4, async {
        let (b, stored, task, readback, a_input, _) =
            approved_native(&fixture, &endpoint, false).await;
        let source = &fixture.source;
        let a_binding = fixture.binding().await.unwrap().unwrap();
        assert_native_inputs(&fixture, runtime_store.as_ref(), &[(a_input.as_str(), &a_binding)])
            .await;
        let model_requests = fixture.model_requests();
        let actor = fixture.service.live_session_actor_witness(&fixture.session).await.unwrap();

        // This is the ordinary empty-keeper path. The real child holds its
        // initialize reply until the test can retain the exact native receipt.
        source.step_service_input_with_attachment(&fixture.store, &stored).await.unwrap();
        endpoint.wait_for(&endpoint.started).await;
        let held = source.held_service_attachment(&b, &task.attachments()[0]).unwrap().unwrap();
        let receipt = held.receipt(|| panic!("one accepted native attempt only"))
            .await.unwrap().unwrap();
        assert!(receipt.outcome.staged && !receipt.outcome.persisted);
        assert_eq!(receipt.attempt.server(), "mail");
        assert_eq!(receipt.attempt.observe("read").await, McpAttachmentReadiness::Pending);
        assert_eq!(fixture.service.live_session_actor_witness(&fixture.session).await.as_ref(),
            Some(&actor));
        assert_eq!(endpoint.starts(), 1);
        assert!(!endpoint.listed.exists());
        assert!(endpoint.calls().is_empty());
        assert_eq!(readback.calls.load(Ordering::SeqCst), 1);
        assert_eq!(fixture.store.get(&stored.key).await.unwrap().unwrap(), stored);
        assert_eq!(fixture.model_requests(), model_requests);

        endpoint.release();
        assert!(matches!(receipt.attempt.wait("read", WAIT).await,
            McpAttachmentReadiness::Failed { .. }));
        assert!(Arc::ptr_eq(&held, &source.held_service_attachment(&b, &task.attachments()[0]).unwrap().unwrap()));
        // Production reopens currentness, reads the real Meta-wrapped observer,
        // and commits its finite projection in the existing cancellation CAS.
        source.step_service_input_with_attachment(&fixture.store, &stored).await.unwrap();
        let saved = fixture.store.get(&stored.key).await.unwrap().unwrap();
        let record = source.decode_record(&saved, &b).unwrap();
        assert!(record.operation_cancelled);
        assert!(record.final_association.is_none() && record.service_admission.is_none());
        assert!(record.bound_service_input.is_none());
        assert!(record.service_attachment_failure == Some(ApprovedServiceAttachmentFailure::Failed));
        let observation = record.service_attachment_observation
            .expect("actual native diagnostics must survive production capture");
        assert_eq!(observation.phase, ApprovedServiceAttachmentPhase::Initialize);
        let ApprovedServiceAttachmentClass::Closed { exit } = observation.class else {
            panic!("actual initialize close must retain its finite class");
        };
        // The exact selected Core contract asserts the process status on Linux.
        #[cfg(target_os = "linux")]
        assert_eq!(exit, Some(meerkat_toolkit::setup::ApprovedServiceAttachmentExit::Code(3)));
        #[cfg(not(target_os = "linux"))]
        let _ = exit;
        assert!(!serde_json::to_string(&saved.data).unwrap().contains(SENTINEL));
        assert!(source.held_service_attachment(&b, &task.attachments()[0]).unwrap().is_none());
        assert_native_inputs(&fixture, runtime_store.as_ref(), &[(a_input.as_str(), &a_binding)])
            .await;
        assert_eq!(fixture.model_requests(), model_requests);

        let setup = source.service_setup_store().unwrap();
        let obligation = setup.committed_continuation(&Id::new(task.acquisition.id.clone()).unwrap())
            .await.unwrap().unwrap();
        assert_eq!(obligation.service_attachment_observation(), Some(observation));
        assert!(obligation.service_attachment_failure() == Some(ApprovedServiceAttachmentFailure::Failed));
        assert!(source.failed_service_allows_original_result(&obligation).await.unwrap());
        let exact_digest = obligation.digest().unwrap();
        let setup_record = fixture.store.get(&task.acquisition).await.unwrap().unwrap();
        assert!(!serde_json::to_string(&setup_record.data).unwrap().contains(SENTINEL));
        assert!(!serde_json::to_string(&obligation).unwrap().contains(SENTINEL));
        source.step_service_input_with_attachment(&fixture.store, &saved).await.unwrap();
        assert_eq!(fixture.store.get(&stored.key).await.unwrap().unwrap(), saved);
        assert_eq!(setup.committed_continuation(obligation.operation()).await.unwrap().unwrap()
            .digest().unwrap(), exact_digest);
        assert_eq!(endpoint.starts(), 1);
        assert_eq!(readback.calls.load(Ordering::SeqCst), 1);
        assert!(endpoint.calls().is_empty());

        // Reuse the actual native continuation host, inbox and retained A owner.
        // No attachment receipt or delivered result is fabricated here.
        let adapter = crate::installed_host::acquisition::InstalledNativeContinuation::for_service_continuation_test(
            fixture.store.clone(), fixture.scope.clone(), source.clone());
        let state = meerkat_mob_mcp::MobMcpState::new_with_runtime_adapter(fixture.service.clone(),
            Some(fixture.machine.clone()), meerkat_mob::MobControlPrincipal::Owner).unwrap()
            .with_host_job_source(meerkat_mob_mcp::HostJobSource::new([adapter.namespace()], adapter.clone()).unwrap())
            .into_shared();
        let binding = obligation.continuation().native_original.as_ref().unwrap();
        let member = binding.member.as_ref().unwrap();
        state.try_mob_insert_handle(meerkat_mob::MobId::from(member.mob_id.as_str()), native_handle)
            .await.unwrap();
        let inbox = RuntimeDeliveryInbox::new(Arc::new(InMemoryRuntimeStore::new()));
        let bindings = Arc::new(ContinuationHostBindings::default());
        state.bind_continuations(inbox.clone(), &bindings).unwrap();
        let service = state.continuation_service().unwrap();
        adapter.bind_fixture_native(service.clone(), state.clone()).unwrap();
        let options = ContinuationStepOptions { worker: Id::new("actual-native-failure-result").unwrap(),
            lease_ms: 30_000, owner_timeout_ms: 1_000, settlement_budget_ms: 1_000 };
        assert!(setup.step_continuation(obligation.operation(), adapter.as_ref(), &options)
            .await.is_err());
        let owner = ContinuationOwner::Member {
            mob_id: member.mob_id.clone(), identity: member.member.clone(),
        };
        let key = ContinuationKey::new(obligation.continuation().delivery_key.to_string()).unwrap();
        let ContinuationStatus::Pending { receipt, .. } = service.continuation_status(&owner, &key)
            .await.unwrap() else { panic!("canonical failure must precede native A admission"); };
        let pending = inbox.list_pending(&LogicalRuntimeId(receipt.address), 2).await.unwrap();
        assert_eq!(pending.len(), 1);
        let payload: serde_json::Value = serde_json::from_slice(pending[0].submission.payload()).unwrap();
        let delivery: meerkat::ContinuationDelivery = serde_json::from_value(payload).unwrap();
        let expected = serde_json::json!({
            "outcome": obligation.outcome(),
            "service_attachment_failure": "failed",
            "service_attachment_observation": observation,
            "explanation": "The selected service attachment attempt failed or was rejected. Its replacement instruction was permanently fenced before admission. This is a setup failure result under your original authority, not service readiness or permission to retry. Research any corrected choice and request a separately reviewed setup.",
        });
        let expected = String::from_utf8(toolkit_package::canonical_json(&expected).unwrap()).unwrap();
        assert!(!expected.contains(SENTINEL));
        assert_eq!(delivery.body, expected.into());
        let jobs = Arc::new(MemoryDetachedJobStore::new());
        let host = SessionServiceDeliveryHost::new(&fixture.service, &fixture.machine,
            DetachedJobService::new(jobs.clone()), None).with_continuation_bindings(bindings.clone());
        let handle = RuntimeDeliveryOwner::new(jobs, inbox)
            .with_source_changes(bindings.subscribe_source_changes()).arm(Arc::new(host)).unwrap();
        owner_passes = Some(handle.subscribe_passes());
        owner_handle = Some(handle);
        let status = tokio::time::timeout(WAIT, async {
            loop {
                let status = service.continuation_status(&owner, &key).await.unwrap();
                if matches!(status, ContinuationStatus::Applied { .. }) { break status; }
                owner_passes.as_mut().unwrap().changed().await.unwrap();
            }
        }).await.expect("actual native failure result must resume retained A");
        let ContinuationStatus::Applied { session, input, .. } = status else { unreachable!() };
        assert_eq!(session, fixture.session);
        assert_ne!(input.to_string(), a_input);
        let delivered = fixture.machine.input_state(&session, &input).await.unwrap().unwrap();
        assert_eq!(delivered.state.retained_resume.as_ref().unwrap().identity, binding.retained_work);
        let rows = runtime_store.load_input_states_strict(&LogicalRuntimeId::for_session(&session))
            .await.unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows.iter().map(|row| row.state.input_id.to_string()).collect::<std::collections::BTreeSet<_>>(),
            [a_input, input.to_string()].into_iter().collect());
        for _ in 0..2 {
            assert!(matches!(setup.step_continuation(obligation.operation(), adapter.as_ref(), &options)
                .await.unwrap(), ContinuationStep::Acknowledged(_)));
        }
        let final_b = fixture.store.get(&stored.key).await.unwrap().unwrap();
        assert_eq!(final_b, saved);
        assert_eq!(setup.committed_continuation(obligation.operation()).await.unwrap().unwrap()
            .digest().unwrap(), exact_digest);
        // Terminal archival may empty the live spine ledger. Check the same
        // canonical native input history again after both replay acknowledgements.
        let after_replay = runtime_store
            .load_input_states_strict(&LogicalRuntimeId::for_session(&session))
            .await
            .unwrap();
        assert_eq!(after_replay.len(), 2);
        assert_eq!(
            after_replay
                .iter()
                .map(|row| row.state.input_id.to_string())
                .collect::<std::collections::BTreeSet<_>>(),
            rows.iter()
                .map(|row| row.state.input_id.to_string())
                .collect::<std::collections::BTreeSet<_>>()
        );
        assert_eq!(endpoint.starts(), 1);
        assert!(!endpoint.listed.exists());
        assert!(endpoint.calls().is_empty());
        assert_eq!(readback.calls.load(Ordering::SeqCst), 1);
    }))).catch_unwind().await;
    endpoint.release();
    // Dropping a handle aborts its task. Use the native retirement owner to
    // finish any entered pass and join it before releasing the handle.
    let owner_joined = match owner_handle.as_ref() {
        Some(owner) => owner.retire().join(WAIT).await.is_ok(),
        None => true,
    };
    drop(owner_handle);
    drop(owner_passes);
    fixture.source.join_service_attachments().await;
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    assert!(
        owner_joined,
        "the actual continuation owner must join before test completion"
    );
    match outcome {
        Ok(Ok(())) => (),
        Ok(Err(_)) => panic!("actual initialize failure control timed out"),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

#[path = "batch_native_tests.rs"]
mod batch;
