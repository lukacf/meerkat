//! A governed native run reaches a connector-style MCP call through the
//! router's call-context seam: the call's prepared metadata reaches the
//! server only after the preparation's final check passed at the call's
//! native entry, and a refused final check sends nothing.

use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

use meerkat_core::McpServerConfig;
use meerkat_core::mcp_attachment::{ConfiguredMcpAttachmentAuthority, McpAttachmentRefusal};
use meerkat_core::service::SessionBuildOptions;
use meerkat_mcp::{McpCallContext, McpCallContextError, McpCallContextProvider, McpCallTarget};
use meerkat_runtime::meerkat_machine::ConfiguredMcpAttachmentProvider;

const CREDENTIAL_KEY: &str = "meerkat/connector_credential";

/// A stdio MCP server listing the fixture's record operations. It answers
/// every `tools/call` with the read result and appends each request line to
/// `log`.
fn record_server(log: &std::path::Path) -> McpServerConfig {
    let script = format!(
        r#"while IFS= read -r line; do
  printf '%s\n' "$line" >> '{log}'
  id=$(printf '%s\n' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*) printf '{{"jsonrpc":"2.0","id":%s,"result":{{"protocolVersion":"2025-03-26","capabilities":{{"tools":{{}}}},"serverInfo":{{"name":"records","version":"0"}}}}}}\n' "$id" ;;
    *'"method":"tools/list"'*) printf '{{"jsonrpc":"2.0","id":%s,"result":{{"tools":[{{"name":"read_record","inputSchema":{{"type":"object","properties":{{"record":{{"type":"string"}}}},"required":["record"],"additionalProperties":false}}}},{{"name":"delete_record","inputSchema":{{"type":"object","properties":{{"record":{{"type":"string"}}}},"required":["record"],"additionalProperties":false}}}}]}}}}\n' "$id" ;;
    *'"method":"tools/call"'*) printf '{{"jsonrpc":"2.0","id":%s,"result":{{"content":[{{"type":"text","text":"record-7 value"}}]}}}}\n' "$id" ;;
  esac
done
"#,
        log = log.display()
    );
    McpServerConfig::stdio(
        "records",
        "/bin/sh",
        vec!["-c".to_owned(), script],
        HashMap::new(),
    )
}

/// A stdio MCP server that records its own spawn in `marker` before it reads
/// anything, so a refused attachment is observable as an absent marker.
fn marker_server(name: &str, marker: &std::path::Path) -> McpServerConfig {
    McpServerConfig::stdio(
        name,
        "/bin/sh",
        vec![
            "-c".to_owned(),
            format!("printf spawned > '{}'; cat > /dev/null", marker.display()),
        ],
        HashMap::new(),
    )
}

/// Test fixture only, not a stock policy: the host's configured-server
/// authority for the sessions it is provided for, allowing exactly the
/// `records` server. It records each session it was provided for and each
/// server it decided.
#[derive(Default)]
struct RecordsOnlyConfiguredServers {
    sessions: Mutex<Vec<SessionId>>,
    decided: Mutex<Vec<(String, bool)>>,
}

impl RecordsOnlyConfiguredServers {
    fn provider(self: &Arc<Self>) -> Arc<dyn ConfiguredMcpAttachmentProvider> {
        let fixture = Arc::clone(self);
        // This fixture records which configured servers are decided; it does
        // not decide per agent, so the registration's binding is unused.
        Arc::new(
            move |registration: &meerkat_runtime::RuntimeSessionRegistrationWitness,
                  _agent: meerkat_runtime::meerkat_machine::SessionAgentBinding| {
                fixture
                    .sessions
                    .lock()
                    .expect("sessions")
                    .push(registration.session_id().clone());
                Some(Arc::clone(&fixture) as Arc<dyn ConfiguredMcpAttachmentAuthority>)
            },
        )
    }

    fn decided(&self, name: &str) -> Option<bool> {
        self.decided
            .lock()
            .expect("decisions")
            .iter()
            .find(|(server, _)| server == name)
            .map(|(_, allowed)| *allowed)
    }
}

impl ConfiguredMcpAttachmentAuthority for RecordsOnlyConfiguredServers {
    fn authorize_configured(&self, server: &McpServerConfig) -> Result<(), McpAttachmentRefusal> {
        let allowed = server.name == "records";
        self.decided
            .lock()
            .expect("decisions")
            .push((server.name.clone(), allowed));
        if allowed {
            Ok(())
        } else {
            Err(McpAttachmentRefusal::Unavailable)
        }
    }
}

fn record_grants() -> (Arc<LocalGrantAuthority>, GrantLineageRef, GrantLineageRef) {
    let grants = Arc::new(
        LocalGrantAuthority::new(
            LocalGrantConfiguration {
                root: principal("grant-owner"),
                namespace: id("native-grants"),
                generation: 1,
            },
            LocalAuthorizationPublication::new(),
            Arc::new(HostAuthorizationClock),
        )
        .expect("actual generated grant owner"),
    );
    let controller = grants
        .issue_root(
            &principal("grant-owner"),
            id("controller"),
            principal("executor"),
            None,
            ceiling("infer"),
        )
        .expect("controller lineage");
    let operation = grants
        .issue_root(
            &principal("grant-owner"),
            id("read-only"),
            principal("executor"),
            None,
            ceiling("read"),
        )
        .expect("read-only operation grant");
    (grants, controller, operation)
}

fn record_machine(
    grants: Arc<LocalGrantAuthority>,
    configured: &Arc<RecordsOnlyConfiguredServers>,
) -> Arc<MeerkatMachine> {
    let ingress: Arc<NativeIngressCheck> = Arc::new(move |_, _, _, _| Ok(()));
    Arc::new(
        MeerkatMachine::ephemeral()
            .with_local_grant_authorization(NativeGrantWorkConfiguration {
                retained_resume: None,
                kept_original: None,
                require_scoped_destination: false,
                grants,
                ingress,
                invocation_owner: Arc::new(InvocationOwner),
                operation_owner: Arc::new(CatalogRecordOwner),
                configured_mcp_attachment: Some(configured.provider()),
            })
            .expect("native host setup"),
    )
}

/// Attaches a call-scoped credential to every call to the record server,
/// with a final check whose verdict the test chooses.
struct CredentialProvider {
    passes: bool,
    /// Every classification the router asked for: (operation, read-only).
    classified: Mutex<Vec<(String, bool)>>,
    prepared: AtomicUsize,
    final_checked: Arc<AtomicUsize>,
}

#[async_trait]
impl McpCallContextProvider for CredentialProvider {
    /// The host declares the read operation read-only, as the read-only
    /// grant's execution restriction requires; delete stays undeclared.
    fn tool_mutation_class(
        &self,
        config: &McpServerConfig,
        raw_operation: &str,
    ) -> meerkat_core::ToolMutationClass {
        let read_only = config.name == "records" && raw_operation == "read_record";
        self.classified
            .lock()
            .expect("classification observations")
            .push((raw_operation.to_owned(), read_only));
        if read_only {
            meerkat_core::ToolMutationClass::ReadOnly
        } else {
            meerkat_core::ToolMutationClass::Unknown
        }
    }

    async fn prepare(
        &self,
        target: McpCallTarget<'_>,
        _: ToolCallView<'_>,
        context: &ToolDispatchContext,
    ) -> Result<Option<McpCallContext>, McpCallContextError> {
        if target.config.name != "records" {
            return Ok(None);
        }
        assert!(
            context.work_authorization().is_some(),
            "the governed context reaches the call's preparation"
        );
        self.prepared.fetch_add(1, Ordering::SeqCst);
        let checked = Arc::clone(&self.final_checked);
        let passes = self.passes;
        Ok(Some(
            McpCallContext::new(
                serde_json::Map::from_iter([(
                    CREDENTIAL_KEY.to_owned(),
                    serde_json::json!({"authorization": "Bearer fixture-credential"}),
                )]),
                (),
            )
            .with_final_check(move || {
                checked.fetch_add(1, Ordering::SeqCst);
                if passes {
                    Ok(())
                } else {
                    Err(McpCallContextError::Denied)
                }
            }),
        ))
    }
}

const LOAD_CALL: &str = "attempt-load";

/// The parent's record policy, plus the catalog control a model uses to load
/// the server's deferred tools: loading is a read of the tool catalog.
struct CatalogRecordOwner;
impl OperationPolicyOwner for CatalogRecordOwner {
    fn authorize_controller_admission(
        &self,
        association: &InputAuthorityAssociation,
        facts: &meerkat_core::ControllerModelFacts,
        now_ms: u64,
    ) -> Result<
        meerkat_authorization::grant_policy::ControllerAdmissionAllowance,
        meerkat_core::OperationAuthorizationError,
    > {
        RecordOwner.authorize_controller_admission(association, facts, now_ms)
    }

    fn authorize_operation(
        &self,
        association: &InputAuthorityAssociation,
        binding: &PreparedAuthorizationBinding,
        purpose: LocalPolicyPurpose,
        now_ms: u64,
    ) -> Result<LocalPolicyAllowance, meerkat_core::OperationAuthorizationError> {
        if let AuthorizationOperation::Tool(facts) = &binding.facts().operation
            && purpose == LocalPolicyPurpose::Operation
            && facts.name.as_str() == meerkat_core::tool_catalog::TOOL_CATALOG_LOAD_NAME
        {
            return Ok(LocalPolicyAllowance {
                operation_values: vec![LocalOperationValues {
                    action: action("read"),
                    resource_domain: domain(),
                    processor: ProcessorRef::Principal {
                        principal: principal("executor"),
                    },
                    audience: AudienceRef::Principal {
                        principal: principal("requester"),
                    },
                }],
                restrictions: ExecutionRestrictions::unrestricted(),
                expires_at_ms: now_ms + 60_000,
                review_tier: meerkat_core::authorization::OperationReviewTier::R1,
            });
        }
        RecordOwner.authorize_operation(association, binding, purpose, now_ms)
    }
}

/// The parent's scripted model with a catalog load first: after loading the
/// server's deferred tools it requires both to be offered, requires the
/// delete to be refused by the operation's read-only grant, and requires the
/// read to return its result.
#[derive(Default)]
struct RecordModel {
    requests: Mutex<Vec<Vec<Message>>>,
    /// The tool names each model request offered.
    offered: Mutex<Vec<Vec<String>>>,
    ready_to_finish: Notify,
    finish: Notify,
}

#[async_trait]
impl LlmClient for RecordModel {
    fn plain_model_route(
        &self,
        logical_model: &str,
    ) -> Result<meerkat_llm_core::PlainModelRoute, meerkat_core::ControllerFactsUnavailable> {
        if logical_model != MODEL {
            return Err(meerkat_core::ControllerFactsUnavailable);
        }
        meerkat_llm_core::PlainModelRoute::new(ENDPOINT, MODEL)
    }
    fn controller_model_selection(&self) -> Option<ControllerModelSelection> {
        Some(selection())
    }
    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        Ok(messages.to_vec())
    }
    fn stream<'a>(&'a self, _: &'a LlmRequest) -> LlmStream<'a> {
        panic!("governed request must reach the prepared provider entry")
    }
    fn stream_prepared<'a>(&'a self, request: &'a PreparedLlmRequest) -> LlmStream<'a> {
        let check = request
            .authorization()
            .expect("native work reaches actual provider")
            .prepare(model_facts())
            .expect("controller authorization")
            .current()
            .expect("final controller entry check");
        check
            .observe_entry(meerkat_core::OperationEntryBoundary::ModelRequest)
            .expect("transport entry");
        check
            .observe_outcome(OperationObservedOutcome::HttpResponse { status: 200 })
            .expect("fixture response");
        self.offered.lock().expect("offered tools").push(
            request
                .request()
                .tools
                .iter()
                .map(|tool| tool.name.to_string())
                .collect(),
        );
        let mut requests = self.requests.lock().expect("request observations");
        let index = requests.len();
        requests.push(request.request().messages.clone());
        match index {
            // The server's two tools are deferred catalog entries: load them.
            0 => Box::pin(futures::stream::iter(vec![
                Ok(LlmEvent::ToolCallComplete {
                    id: LOAD_CALL.into(),
                    name: meerkat_core::tool_catalog::TOOL_CATALOG_LOAD_NAME.into(),
                    args: serde_json::json!({"names": ["delete_record", "read_record"]}),
                    meta: None,
                }),
                done(StopReason::ToolUse),
            ])),
            1 => {
                let feedback = tool_feedback(&requests[index], LOAD_CALL)
                    .expect("catalog load returned to the model");
                assert!(!feedback.is_error, "{}", feedback.text_content());
                let offered: Vec<&str> = request
                    .request()
                    .tools
                    .iter()
                    .map(|tool| tool.name.as_str())
                    .collect();
                assert!(
                    offered.contains(&"delete_record") && offered.contains(&"read_record"),
                    "the load made both record tools visible: {offered:?}"
                );
                Box::pin(futures::stream::iter(tool_events(
                    DENIED_CALL,
                    "delete_record",
                )))
            }
            2 => {
                let feedback = tool_feedback(&requests[index], DENIED_CALL)
                    .expect("denial returned to same model loop");
                assert!(feedback.is_error, "{}", feedback.text_content());
                // The read-only grant's refusal, not a visibility or
                // classification refusal.
                assert!(
                    feedback
                        .text_content()
                        .contains("operation unavailable under current authorization"),
                    "{}",
                    feedback.text_content()
                );
                Box::pin(futures::stream::iter(tool_events(
                    PERMITTED_CALL,
                    "read_record",
                )))
            }
            3 => {
                let feedback = tool_feedback(&requests[index], PERMITTED_CALL)
                    .expect("permitted result returned to model");
                assert!(!feedback.is_error, "{}", feedback.text_content());
                assert_eq!(feedback.text_content(), "record-7 value");
                Box::pin(
                    futures::stream::once(async move {
                        self.ready_to_finish.notify_one();
                        self.finish.notified().await;
                        Ok(LlmEvent::TextDelta {
                            delta: "read completed after refusal".into(),
                            meta: None,
                        })
                    })
                    .chain(futures::stream::iter([done(StopReason::EndTurn)])),
                )
            }
            _ => panic!("unexpected extra model request"),
        }
    }
    fn provider(&self) -> Provider {
        Provider::Anthropic
    }
    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }
}

/// The scripted model asks for delete, then read, then finishes. With the
/// final check passing, the read is the only server call and carries the
/// prepared credential; with it refusing, nothing reaches the server.
async fn governed_record_run(passes: bool) -> (Arc<CredentialProvider>, Vec<String>) {
    let log_dir = tempfile::tempdir().expect("record server log dir");
    let log = log_dir.path().join("calls.log");
    let (grants, controller, operation) = record_grants();
    let configured = Arc::new(RecordsOnlyConfiguredServers::default());
    let machine = record_machine(grants, &configured);
    let credential = Arc::new(CredentialProvider {
        passes,
        classified: Mutex::default(),
        prepared: AtomicUsize::new(0),
        final_checked: Arc::default(),
    });
    let provider = Arc::new(RecordModel::default());
    let factory = AgentFactory::minimal().mcp_call_context_provider(credential.clone());
    let mut builder = FactoryAgentBuilder::new(factory, Config::default());
    builder.default_llm_client = Some(provider.clone());
    builder.default_session_store = Some(Arc::new(RecordingStore::default()));
    // The record server's tools must be routed before the first model call.
    builder.wait_for_mcp = true;
    let service = Arc::new(EphemeralSessionService::new(builder, 2));
    let created = meerkat::surface::materialize_ephemeral_runtime_session(
        &service,
        &machine,
        CreateSessionRequest {
            model: MODEL.into(),
            prompt: "".into(),
            injected_context: Vec::new(),
            system_prompt: meerkat_core::config::SystemPromptOverride::Inherit,
            max_tokens: None,
            event_tx: None,
            initial_turn: InitialTurnPolicy::Defer,
            deferred_prompt_policy: DeferredPromptPolicy::Discard,
            build: Some(SessionBuildOptions {
                mcp_servers: vec![record_server(&log)],
                ..SessionBuildOptions::default()
            }),
            labels: None,
        },
        false,
    )
    .await
    .expect("native executor with the record server");
    let session_id = created.session_id;
    assert_eq!(
        *configured.sessions.lock().expect("sessions"),
        vec![session_id.clone()],
        "the authority is provided for exactly this session"
    );
    // The server connects in the background: wait until it listed its tools.
    tokio::time::timeout(Duration::from_secs(10), async {
        while !std::fs::read_to_string(&log)
            .unwrap_or_default()
            .contains("tools/list")
        {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("record server listed its tools");
    tokio::time::sleep(Duration::from_millis(200)).await;
    let actor = service
        .live_session_actor_witness(&session_id)
        .await
        .expect("live actor");
    let pin = service
        .pin_controller_client_for_actor(&actor)
        .await
        .expect("selected client");
    meerkat_core::publish_token_lifecycle_acquired_for_identity_within(
        &machine.generated_auth_lease_handle(),
        pin.selection().credential(),
        &meerkat_core::auth::PersistedTokens::api_key("synthetic-native-loop-credential"),
        tokio::time::Instant::now() + CREDENTIAL_PUBLICATION_WAIT,
    )
    .await
    .expect("fixture controller credential");
    let runtime = LogicalRuntimeId::for_session(&session_id);
    let mut prompt = PromptInput::new("Try delete, then read if the operation is refused", None);
    prompt.header.authority_association = Some(association(
        &runtime,
        controller,
        operation,
        pin.selection().clone(),
    ));
    let input = Input::Prompt(prompt);
    let current = NativeIngressContext::from_trusted_ingress(
        &input,
        principal("requester"),
        principal("ingress"),
        RealmId::parse("native-loop").expect("realm"),
        evidence("fresh-transport-authentication"),
    )
    .expect("trusted ingress")
    .with_controller_client(&input, pin)
    .expect("bound pin");
    let input = input
        .with_ingress_context(current)
        .expect("finalized input");
    let (_, completion) = machine
        .accept_input_with_completion(&session_id, input)
        .await
        .expect("native admission");
    let completion = completion.expect("completion waiter");
    if passes {
        let reached =
            tokio::time::timeout(Duration::from_secs(20), provider.ready_to_finish.notified())
                .await;
        assert!(
            reached.is_ok(),
            "the run did not reach its third model request; offered tools per request: {:?}; classifications: {:?}; preparations: {}",
            provider.offered.lock().expect("offered tools"),
            credential.classified.lock().expect("classifications"),
            credential.prepared.load(Ordering::SeqCst),
        );
        provider.finish.notify_one();
    }
    let outcome = tokio::time::timeout(Duration::from_secs(20), completion.wait())
        .await
        .expect("native run completes")
        .expect("native completion observation");
    let CompletionOutcome::Completed(result) = outcome else {
        panic!("the refused delete must not terminate admitted work: {outcome:?}")
    };
    assert_eq!(result.text, "read completed after refusal");
    assert_eq!(result.session_id, session_id);
    assert!(result.terminal_cause_kind.is_none());
    assert_eq!(provider.requests.lock().expect("requests").len(), 4);
    let calls = std::fs::read_to_string(&log)
        .unwrap_or_default()
        .lines()
        .filter(|line| line.contains("\"method\":\"tools/call\""))
        .map(str::to_owned)
        .collect();
    (credential, calls)
}

/// The provider is a test provider on the call-context final-check seam (a
/// fixed credential), not the stock connector provider or a real OAuth
/// account, and the operation owner maps only `tool_catalog_load`.
///
/// The governed allow path: the model loads the server's deferred tools;
/// the refused delete never reaches preparation or the server; the permitted read's preparation sees the governed context,
/// its final check passes at the native entry, and the server receives the
/// read with the prepared credential.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_governed_read_carries_the_prepared_credential_to_the_server() {
    let (credential, calls) = governed_record_run(true).await;

    assert_eq!(credential.prepared.load(Ordering::SeqCst), 1);
    assert_eq!(credential.final_checked.load(Ordering::SeqCst), 1);
    assert_eq!(calls.len(), 1, "{calls:?}");
    let call: serde_json::Value = serde_json::from_str(&calls[0]).expect("a JSON-RPC request");
    assert_eq!(call["params"]["name"], "read_record");
    assert_eq!(
        call["params"]["_meta"][CREDENTIAL_KEY],
        serde_json::json!({"authorization": "Bearer fixture-credential"}),
        "the server received exactly the prepared credential"
    );
}

/// A configured server the host's authority does not allow is refused at its
/// connection's physical entry and never spawned, while the allowed server
/// of the same session connects.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unauthorized_configured_server_is_refused_and_never_spawned() {
    let dir = tempfile::tempdir().expect("server log dir");
    let log = dir.path().join("calls.log");
    let marker = dir.path().join("unlisted.spawned");
    let (grants, _, _) = record_grants();
    let configured = Arc::new(RecordsOnlyConfiguredServers::default());
    let machine = record_machine(grants, &configured);
    let mut builder = FactoryAgentBuilder::new(AgentFactory::minimal(), Config::default());
    builder.default_llm_client = Some(Arc::new(RecordModel::default()));
    builder.default_session_store = Some(Arc::new(RecordingStore::default()));
    let service = Arc::new(EphemeralSessionService::new(builder, 2));
    let created = meerkat::surface::materialize_ephemeral_runtime_session(
        &service,
        &machine,
        CreateSessionRequest {
            model: MODEL.into(),
            prompt: "".into(),
            injected_context: Vec::new(),
            system_prompt: meerkat_core::config::SystemPromptOverride::Inherit,
            max_tokens: None,
            event_tx: None,
            initial_turn: InitialTurnPolicy::Defer,
            deferred_prompt_policy: DeferredPromptPolicy::Discard,
            build: Some(SessionBuildOptions {
                mcp_servers: vec![record_server(&log), marker_server("unlisted", &marker)],
                ..SessionBuildOptions::default()
            }),
            labels: None,
        },
        false,
    )
    .await
    .expect("the session continues without the refused server");
    assert_eq!(
        *configured.sessions.lock().expect("sessions"),
        vec![created.session_id.clone()]
    );
    tokio::time::timeout(Duration::from_secs(10), async {
        while configured.decided("unlisted").is_none()
            || !std::fs::read_to_string(&log)
                .unwrap_or_default()
                .contains("tools/list")
        {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("both servers reached their physical entry");
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(configured.decided("records"), Some(true));
    assert_eq!(configured.decided("unlisted"), Some(false));
    assert!(
        !marker.exists(),
        "a refused configured server must never be spawned"
    );
}
