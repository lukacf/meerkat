//! End-to-end stock governed prompt on the stock rkat composition.
//!
//! A realm is materialized, governed by the local operator
//! (`govern_stock_realm`), given a records MCP installation in its own
//! config, and issued per-agent grants through the stock issuance path
//! (`grant_stock_agent`, which records the selection). It is opened as one
//! agent with `open_stock_realm_persistence_as` (`RealmGovernance::Stock`).
//! Each session is prepared like the stock CLI does it: `prepare_bindings`,
//! `StockGovernance::bind_session_agent`, the session built on those
//! bindings, then its executor registered. Each prompt goes through
//! `StockGovernance::bind_prompt`, the ordinary runtime accept and
//! `PendingOriginal::admit`. The stock policy object is the invocation
//! policy and the operation owner. A scripted model checks its native
//! controller authorization before every answer.
//!
//! Every run's live completion waiter must release the completed result to
//! the work's own current destination, equal to the durable terminal
//! completion read for the same input.
//!
//! The same realm opened for a trusted host
//! (`open_stock_realm_persistence_for_host`) admits the host's authenticated
//! callers instead, through `StockGovernance::bind_caller_prompt`, each only
//! while the host's exact-target decision allows it.
#![cfg(all(
    feature = "session-store",
    feature = "local-authorization",
    feature = "mcp",
    unix
))]
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use meerkat::governed_realm::stock::caller::{
    StockAuthenticatedCaller, StockCallerAccess, StockCallerRequest, StockLoginLifetime,
};
use meerkat::governed_realm::stock::issuance::{
    DEFAULT_STOCK_AGENT, McpOperations, StockGrantRequest, StockGrantTarget, grant_stock_agent,
};
use meerkat::governed_realm::stock::{
    StockGovernance, StockGovernanceError, StockPrincipals, govern_stock_realm,
};
use meerkat::governed_realm::{
    RealmGovernance, open_governed_realm_persistence_with_provider,
    open_stock_realm_persistence_as, open_stock_realm_persistence_for_host,
};
use meerkat::surface::{
    PersistentRuntimeExecutor, build_runtime_backed_service_with_default_reconfigure_host,
};
use meerkat::{AgentFactory, DiskStorageProvider, FactoryAgentBuilder, PersistentSessionService};
use meerkat_authorization::publication::LocalAuthorizationPublication;
use meerkat_authorization_contracts::audit::{
    AuditObservation, AuditTarget, StoredAuthorizationAuditObservation,
};
use meerkat_core::authorization::{
    ModelAuthorizationFacts, ModelAuthorizationUse, OperationObservedOutcome, OperationRefusalKind,
};
use meerkat_core::connection::{AuthCredentialIdentity, RealmId};
use meerkat_core::mcp_config::McpServerConfig;
use meerkat_core::service::{
    CreateSessionRequest, DeferredPromptPolicy, InitialTurnPolicy, SessionBuildOptions,
    SessionService,
};
use meerkat_core::tool_catalog::{TOOL_CATALOG_LOAD_NAME, TOOL_CATALOG_SEARCH_NAME};
use meerkat_core::{
    Config, ControllerModelClient, ControllerModelSelection, Message, OperationAuthorizationError,
    OperationRefused, PrincipalKind, PrincipalRef, Provider, Session, SessionId,
    SessionLlmIdentity, StopReason, ToolResult, TrustDomainId,
};
use meerkat_llm_core::{
    LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest, LlmStream, PreparedLlmRequest,
};
use meerkat_runtime::completion::CompletionOutcome;
use meerkat_runtime::input::{
    ExternalEventInput, Input, InputDurability, InputHeader, InputOrigin, InputVisibility,
    PromptInput,
};
use meerkat_runtime::input_state::StoredInputState;
use meerkat_runtime::meerkat_machine::MeerkatMachine;
use meerkat_runtime::service_ext::SessionServiceRuntimeExt;
use meerkat_store::{RealmBackend, RealmPaths};
use serde_json::{Value, json};

const REALM: &str = "stock-prompt";
const MODEL: &str = "claude-sonnet-4-5";
const ENDPOINT: &str = "https://stock-prompt.invalid/messages";
const RECORDS: &str = "records";
const READ: &str = "read_record";
const LIST: &str = "list_records";
const DELETE: &str = "delete_record";
const ANSWER: &str = "record-7 value";
const FINAL: &str = "read record-7; the delete was refused";
const OTHER_AGENT: &str = "other-agent";
const UNGRANTED_AGENT: &str = "ungranted-agent";
const WAIT: Duration = Duration::from_secs(30);
/// The stable name a trusted host registers its ledger tool bundle under.
const LEDGER: &str = "ledger";
const READ_ENTRY: &str = "read_entry";
const DELETE_ENTRY: &str = "delete_entry";
const ENTRY: &str = "entry-3 value";

fn realm() -> RealmId {
    RealmId::parse(REALM).expect("realm")
}

fn identity() -> SessionLlmIdentity {
    SessionLlmIdentity {
        model: MODEL.into(),
        provider: Provider::Anthropic,
        self_hosted_server_id: None,
        provider_params: None,
        auth_binding: None,
    }
}

/// The scripted controller's credential identity of one fixture. Each
/// independent fixture uses its own `account`.
fn controller_credential(account: &str) -> AuthCredentialIdentity {
    serde_json::from_value(json!({"realm": REALM, "account": account}))
        .expect("nonsecret controller credential identity")
}

fn selection(account: &str) -> ControllerModelSelection {
    ControllerModelSelection::new(
        identity(),
        controller_credential(account),
        "scripted-profile".into(),
        "scripted-transport".into(),
    )
}

fn model_facts(account: &str) -> ModelAuthorizationFacts {
    ModelAuthorizationFacts {
        identity: Arc::new(identity()),
        wire_model: MODEL.into(),
        hosted_capabilities: Arc::from([]),
        backend_profile_id: Some("scripted-profile".into()),
        backend_kind: "scripted-transport".into(),
        endpoint: ENDPOINT.into(),
        credential: Some(controller_credential(account)),
        usage: ModelAuthorizationUse::ControllerInference,
        live_channel: None,
    }
}

// ---------------------------------------------------------------------------
// The records installation and the realm config
// ---------------------------------------------------------------------------

/// A stdio MCP server listing two read operations and a delete. It answers every
/// `tools/call` with the record and appends each request line to `log`.
fn records_server(log: &Path) -> McpServerConfig {
    let script = format!(
        r#"while IFS= read -r line; do
  printf '%s\n' "$line" >> '{log}'
  id=$(printf '%s\n' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*) printf '{{"jsonrpc":"2.0","id":%s,"result":{{"protocolVersion":"2025-03-26","capabilities":{{"tools":{{}}}},"serverInfo":{{"name":"records","version":"0"}}}}}}\n' "$id" ;;
    *'"method":"tools/list"'*) printf '{{"jsonrpc":"2.0","id":%s,"result":{{"tools":[{{"name":"read_record","description":"Read one record","inputSchema":{{"type":"object","properties":{{"record":{{"type":"string"}}}},"required":["record"],"additionalProperties":false}}}},{{"name":"list_records","description":"List record ids","inputSchema":{{"type":"object","properties":{{}},"additionalProperties":false}}}},{{"name":"delete_record","description":"Delete one record","inputSchema":{{"type":"object","properties":{{"record":{{"type":"string"}}}},"required":["record"],"additionalProperties":false}}}}]}}}}\n' "$id" ;;
    *'"method":"tools/call"'*) printf '{{"jsonrpc":"2.0","id":%s,"result":{{"content":[{{"type":"text","text":"record-7 value"}}]}}}}\n' "$id" ;;
  esac
done
"#,
        log = log.display()
    );
    let mut server = McpServerConfig::stdio(
        RECORDS,
        "/bin/sh",
        vec!["-c".to_owned(), script],
        HashMap::new(),
    );
    // The installation's access ceiling admits all three; the agents' grants
    // decide which of them each agent may invoke.
    server.access = Some(meerkat_core::McpServerAccess::allowing([
        READ, LIST, DELETE,
    ]));
    server
}

/// The realm's own config document, as `rkat mcp add --scope realm` writes it.
fn realm_config(paths: &RealmPaths) -> meerkat_core::ConfigRuntime {
    meerkat_core::ConfigRuntime::new(
        Arc::new(meerkat_core::FileConfigStore::new(
            paths.config_path.clone(),
            meerkat_models::canonical(),
        )),
        paths.config_path.with_file_name("config_state.json"),
    )
}

async fn install(paths: &RealmPaths, server: McpServerConfig) {
    meerkat_core::McpRealmPersistTarget::new(&realm_config(paths))
        .add(server, None)
        .await
        .expect("realm installation");
}

/// The realm's installations exactly as persisted.
async fn installed(paths: &RealmPaths) -> Vec<McpServerConfig> {
    meerkat_core::McpRealmPersistTarget::new(&realm_config(paths))
        .servers()
        .await
        .expect("realm installations")
        .0
}

// ---------------------------------------------------------------------------
// The realm as the operator governs and grants it
// ---------------------------------------------------------------------------

/// Materialize the realm, govern it as the local operator, install the
/// records server in its own config, and issue through stock issuance:
/// the default agent's worker grant (catalog reads and the records read
/// only) and controller grant, and another agent's own valid pair whose
/// worker grant covers the catalog but not the records server. The
/// ungranted agent gets nothing. Each controller grant names exactly the
/// actual lowered route of the fixture's scripted controller for
/// `controller_account`, as the session's pinned controller reports it.
async fn provision(root: &Path, log: &Path, controller_account: &str) -> RealmPaths {
    drop(
        open_governed_realm_persistence_with_provider(
            &DiskStorageProvider,
            root,
            REALM,
            Some(RealmBackend::Sqlite),
            None,
            None,
            RealmGovernance::Ungoverned,
        )
        .await
        .expect("the realm materializes"),
    );
    let principals = StockPrincipals::for_local_operator(&realm()).expect("local operator");
    govern_stock_realm(root, REALM, &principals)
        .await
        .expect("the operator governs the realm");
    let paths = meerkat_store::realm_paths_in(root, REALM);
    install(&paths, records_server(log)).await;
    let route = model_facts(controller_account)
        .controller_route_key()
        .expect("the scripted controller's route key");
    let controller = || StockGrantTarget::Controller {
        routes: [route.clone()].into(),
    };
    let issue = |agent: &str, target: StockGrantTarget| StockGrantRequest {
        agent: agent.to_string(),
        target,
        expires_in_ms: None,
    };
    for request in [
        issue(DEFAULT_STOCK_AGENT, StockGrantTarget::Catalog),
        issue(
            DEFAULT_STOCK_AGENT,
            StockGrantTarget::Mcp {
                server: RECORDS.to_string(),
                operations: McpOperations::Exact([READ.to_string()].into()),
            },
        ),
        issue(DEFAULT_STOCK_AGENT, controller()),
        issue(OTHER_AGENT, StockGrantTarget::Catalog),
        issue(OTHER_AGENT, controller()),
    ] {
        grant_stock_agent(root, REALM, &principals, &request)
            .await
            .expect("the operator issues and selects the grant");
    }
    paths
}

// ---------------------------------------------------------------------------
// The stock composition, its sessions and prompts
// ---------------------------------------------------------------------------

/// One process's stock composition of the realm, acting as one agent.
struct StockRealm {
    model: Arc<ScriptedModel>,
    machine: Arc<MeerkatMachine>,
    service: Arc<PersistentSessionService<FactoryAgentBuilder>>,
    stock: Arc<StockGovernance>,
}

struct StockSession {
    session_id: SessionId,
    controller: ControllerModelClient,
}

/// Open the governed realm as `agent` through the stock open, with the
/// persistent runtime-backed service and the real factory.
async fn open_as(root: &Path, agent: &str, model: Arc<ScriptedModel>) -> StockRealm {
    let (_, bundle) = open_stock_realm_persistence_as(
        &DiskStorageProvider,
        root,
        REALM,
        Some(RealmBackend::Sqlite),
        None,
        None,
        agent,
    )
    .await
    .expect("the governed realm opens");
    compose(root, agent, model, bundle).await
}

/// Open the governed realm as the default agent for a trusted host whose
/// callers `access` admits, through `ingress`.
async fn open_for_host(
    root: &Path,
    model: Arc<ScriptedModel>,
    ingress: PrincipalRef,
    access: Arc<dyn StockCallerAccess>,
) -> StockRealm {
    let (_, bundle) = open_stock_realm_persistence_for_host(
        &DiskStorageProvider,
        root,
        REALM,
        Some(RealmBackend::Sqlite),
        None,
        None,
        DEFAULT_STOCK_AGENT,
        ingress,
        access,
    )
    .await
    .expect("the governed realm opens for the host");
    compose(root, DEFAULT_STOCK_AGENT, model, bundle).await
}

/// The persistent runtime-backed service and the real factory over an
/// opened stock realm.
async fn compose(
    root: &Path,
    agent: &str,
    model: Arc<ScriptedModel>,
    bundle: meerkat::PersistenceBundle,
) -> StockRealm {
    let stock = Arc::clone(
        bundle
            .stock_governance()
            .expect("a governed realm opens with the stock composition"),
    );
    let mut builder = FactoryAgentBuilder::new(AgentFactory::minimal(), Config::default());
    builder.default_llm_client = Some(Arc::clone(&model) as Arc<dyn LlmClient>);
    // The installation's tools must be routed before the first model call.
    builder.wait_for_mcp = true;
    let (service, machine) = build_runtime_backed_service_with_default_reconfigure_host(
        builder,
        4,
        bundle,
        root.join(format!("config-state-{agent}.json")),
    )
    .expect("compose the runtime-backed service");
    // The realm's one owner holds its scripted controller credential from
    // the open: published once, for this fixture's own identity.
    meerkat_core::publish_token_lifecycle_acquired_for_identity_within(
        &machine.generated_auth_lease_handle(),
        model.selection().credential(),
        &meerkat_core::auth::PersistedTokens::api_key("synthetic-stock-prompt-credential"),
        tokio::time::Instant::now() + WAIT,
    )
    .await
    .expect("controller credential");
    StockRealm {
        model,
        machine,
        service,
        stock,
    }
}

impl StockRealm {
    /// A stock session prepared like the stock CLI prepares it: bindings,
    /// the agent bound to the registration, the session built on those
    /// bindings with the realm's installations, then its executor.
    async fn session(&self, servers: Vec<McpServerConfig>) -> StockSession {
        self.session_with(servers, None).await
    }

    /// [`Self::session`], with the host's own tools composed into the build.
    async fn session_with(
        &self,
        servers: Vec<McpServerConfig>,
        external_tools: Option<Arc<dyn meerkat_core::AgentToolDispatcher>>,
    ) -> StockSession {
        let seed = Session::new();
        let session_id = seed.id().clone();
        let bindings = self
            .machine
            .prepare_bindings(session_id.clone())
            .await
            .expect("runtime bindings");
        self.stock
            .bind_session_agent(
                &self.machine,
                &session_id,
                servers.iter().map(|server| server.name.as_str()),
            )
            .await
            .expect("the agent binds to the session's registration");
        self.service
            .create_session(CreateSessionRequest {
                model: MODEL.into(),
                prompt: "".into(),
                injected_context: Vec::new(),
                system_prompt: meerkat_core::config::SystemPromptOverride::Inherit,
                max_tokens: None,
                event_tx: None,
                initial_turn: InitialTurnPolicy::Defer,
                deferred_prompt_policy: DeferredPromptPolicy::Discard,
                build: Some(SessionBuildOptions {
                    resume_session: Some(seed),
                    resume_session_intent: meerkat_core::SessionBuildIntent::Mint,
                    mcp_servers: servers,
                    external_tools,
                    runtime_build_mode: meerkat_core::RuntimeBuildMode::SessionOwned(bindings),
                    ..Default::default()
                }),
                labels: None,
            })
            .await
            .expect("the session is created");
        let witness = self
            .service
            .live_session_actor_witness(&session_id)
            .await
            .expect("live session actor");
        self.machine
            .register_session_with_executor(
                session_id.clone(),
                Box::new(
                    PersistentRuntimeExecutor::new(
                        Arc::clone(&self.service),
                        Arc::clone(&self.machine),
                        session_id.clone(),
                    )
                    .with_publication_actor_witness(witness.clone())
                    .expect("the executor holds the live actor's publication authority"),
                ),
            )
            .await
            .expect("executor registration");
        let controller = {
            let lease = self
                .service
                .acquire_live_session_actor_turn_boundary_lease_exact(&witness)
                .await
                .expect("turn boundary lease")
                .expect("live actor");
            self.service
                .pin_controller_client_for_actor(&lease)
                .await
                .expect("pinned controller")
        };
        assert!(controller.selection() == &self.model.selection());
        StockSession {
            session_id,
            controller,
        }
    }

    /// The operator's prompt through `bind_prompt`, the ordinary accept and
    /// the original's admission; then its live and durable completions.
    async fn prompt(
        &self,
        session: &StockSession,
        text: &str,
    ) -> Result<(meerkat_core::InputId, String), StockGovernanceError> {
        let (input, pending) = self
            .stock
            .bind_prompt(
                &self.machine,
                &session.session_id,
                Input::Prompt(PromptInput::new(text, None)),
                session.controller.clone(),
            )
            .await?;
        Ok(self.admitted(session, input, pending).await)
    }

    /// A host-authenticated caller's input through `bind_caller_prompt`,
    /// the ordinary accept and the original's admission; then its live and
    /// durable completions.
    async fn caller_prompt(
        &self,
        session: &StockSession,
        text: &str,
        caller: &StockAuthenticatedCaller,
    ) -> Result<(meerkat_core::InputId, String), StockGovernanceError> {
        let (input, pending) = self
            .stock
            .bind_caller_prompt(
                &self.machine,
                &session.session_id,
                Input::Prompt(PromptInput::new(text, None)),
                session.controller.clone(),
                caller,
            )
            .await?;
        Ok(self.admitted(session, input, pending).await)
    }

    /// Accept a bound input, admit its original, and read its live and
    /// durable completions.
    async fn admitted(
        &self,
        session: &StockSession,
        input: Input,
        pending: meerkat::governed_realm::stock::originals::PendingOriginal,
    ) -> (meerkat_core::InputId, String) {
        let input_id = input.id().clone();
        let (_, completion) = self
            .machine
            .accept_input_with_completion(&session.session_id, input)
            .await
            .expect("native admission");
        pending.admit().expect("the original is held as admitted");
        let live = tokio::time::timeout(WAIT, completion.expect("completion waiter").wait())
            .await
            .expect("the governed run settles")
            .expect("the live waiter releases the result to the current destination");
        let durable = self
            .machine
            .input_terminal_completion(&session.session_id, &input_id)
            .await
            .expect("durable terminal read")
            .expect("the input's terminal completion");
        let (CompletionOutcome::Completed(live), CompletionOutcome::Completed(read)) =
            (&live, &durable)
        else {
            panic!("live {live:?} and durable {durable:?} completions are both completed")
        };
        assert_eq!(
            live.text, read.text,
            "the live and durable completions carry the same result"
        );
        (input_id, read.text.clone())
    }

    async fn audit(
        &self,
        session: &StockSession,
        input_id: &meerkat_core::InputId,
    ) -> Vec<StoredAuthorizationAuditObservation> {
        let stored: StoredInputState = self
            .machine
            .input_state(&session.session_id, input_id)
            .await
            .expect("native row query")
            .expect("the admitted input's row");
        #[derive(serde::Deserialize)]
        struct AuditProjection {
            #[serde(default)]
            authorization_audit: Vec<StoredAuthorizationAuditObservation>,
        }
        serde_json::from_value::<AuditProjection>(serde_json::to_value(&stored).expect("row"))
            .expect("row audit")
            .authorization_audit
    }

    /// End this process's composition: tear each session's registration down
    /// to terminal, discard its live actor and wait until the machine (with
    /// the controller credential's lifecycle and the grant owner's lease) is
    /// released, as a process exit would.
    async fn close(self, sessions: Vec<StockSession>) {
        for session in &sessions {
            self.machine
                .unregister_current_session_registration_until_terminal(&session.session_id)
                .await
                .expect("the session's registration reaches terminal teardown");
            self.service
                .discard_live_session(&session.session_id)
                .await
                .expect("the session's live actor is discarded");
        }
        let machine = Arc::downgrade(&self.machine);
        drop(sessions);
        drop(self);
        tokio::time::timeout(WAIT, async {
            while machine.upgrade().is_some() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the realm's machine is released");
    }
}

// ---------------------------------------------------------------------------
// The scripted model
// ---------------------------------------------------------------------------

/// One model step: what the request before it must show, and the answer.
enum Step {
    Call {
        id: &'static str,
        name: &'static str,
        args: Value,
    },
    /// Several sibling tool calls in one model response.
    Calls(Vec<(&'static str, &'static str, Value)>),
    Finish(&'static str),
}

/// A scripted physical transport that only answers governed requests: every
/// request passes its native controller authorization first. It records the
/// tools each request offered and every request's messages.
struct ScriptedModel {
    /// The fixture's own controller credential account.
    account: &'static str,
    script: Vec<Step>,
    requests: Mutex<Vec<Vec<Message>>>,
    offered: Mutex<Vec<BTreeSet<String>>>,
}

impl ScriptedModel {
    fn new(account: &'static str, script: Vec<Step>) -> Arc<Self> {
        Arc::new(Self {
            account,
            script,
            requests: Mutex::default(),
            offered: Mutex::default(),
        })
    }

    fn selection(&self) -> ControllerModelSelection {
        selection(self.account)
    }

    fn requests(&self) -> Vec<Vec<Message>> {
        self.requests.lock().expect("requests").clone()
    }

    fn offered(&self) -> Vec<BTreeSet<String>> {
        self.offered.lock().expect("offered").clone()
    }

    /// The tool result for `call_id` the model received in request `index`.
    fn feedback(&self, index: usize, call_id: &str) -> ToolResult {
        self.requests()[index]
            .iter()
            .find_map(|message| match message {
                Message::ToolResults { results, .. } => results
                    .iter()
                    .find(|result| result.tool_use_id == call_id)
                    .cloned(),
                _ => None,
            })
            .unwrap_or_else(|| panic!("request {index} carries the result of {call_id}"))
    }
}

fn done(stop_reason: StopReason) -> Result<LlmEvent, LlmError> {
    Ok(LlmEvent::Done {
        outcome: LlmDoneOutcome::success(stop_reason),
    })
}

#[async_trait]
impl LlmClient for ScriptedModel {
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
        Some(selection(self.account))
    }
    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        Ok(messages.to_vec())
    }
    fn stream<'a>(&'a self, _: &'a LlmRequest) -> LlmStream<'a> {
        panic!("governed work must reach the prepared provider entry")
    }
    fn stream_prepared<'a>(&'a self, request: &'a PreparedLlmRequest) -> LlmStream<'a> {
        let check = request
            .authorization()
            .expect("native work reaches the provider")
            .prepare(model_facts(self.account))
            .expect("native controller authorization")
            .current()
            .expect("final controller entry check");
        check
            .observe_entry(meerkat_core::OperationEntryBoundary::ModelRequest)
            .expect("transport entry");
        check
            .observe_outcome(OperationObservedOutcome::HttpResponse { status: 200 })
            .expect("fixture response");
        self.offered.lock().expect("offered").push(
            request
                .request()
                .tools
                .iter()
                .map(|tool| tool.name.to_string())
                .collect(),
        );
        let index = {
            let mut requests = self.requests.lock().expect("requests");
            requests.push(request.request().messages.clone());
            requests.len() - 1
        };
        match self.script.get(index) {
            Some(Step::Call { id, name, args }) => Box::pin(futures::stream::iter(vec![
                Ok(LlmEvent::ToolCallComplete {
                    id: (*id).into(),
                    name: (*name).into(),
                    args: args.clone(),
                    meta: None,
                }),
                done(StopReason::ToolUse),
            ])),
            Some(Step::Calls(calls)) => Box::pin(futures::stream::iter(
                calls
                    .iter()
                    .map(|(id, name, args)| {
                        Ok(LlmEvent::ToolCallComplete {
                            id: (*id).into(),
                            name: (*name).into(),
                            args: args.clone(),
                            meta: None,
                        })
                    })
                    .chain([done(StopReason::ToolUse)])
                    .collect::<Vec<_>>(),
            )),
            Some(Step::Finish(text)) => Box::pin(futures::stream::iter(vec![
                Ok(LlmEvent::TextDelta {
                    delta: (*text).into(),
                    meta: None,
                }),
                done(StopReason::EndTurn),
            ])),
            None => panic!("unexpected model request {index}"),
        }
    }
    fn provider(&self) -> Provider {
        Provider::Anthropic
    }
    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Observations
// ---------------------------------------------------------------------------

fn server_calls(log: &Path) -> Vec<String> {
    std::fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .filter(|line| line.contains("\"method\":\"tools/call\""))
        .map(str::to_owned)
        .collect()
}

async fn wait_listed(log: &Path) {
    tokio::time::timeout(WAIT, async {
        while !std::fs::read_to_string(log)
            .unwrap_or_default()
            .contains("tools/list")
        {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the installation listed its tools");
}

/// The prepared/entry/outcome observations of the tool call `call_id`.
fn tool_audit<'a>(
    audit: &'a [StoredAuthorizationAuditObservation],
    call_id: &str,
) -> Vec<&'a AuditObservation> {
    let operation = audit
        .iter()
        .find_map(|record| match &record.observation.observation {
            AuditObservation::Prepared { target, .. }
            | AuditObservation::Refused { target, .. } => match target.as_ref() {
                AuditTarget::Tool {
                    call_id: observed, ..
                } if observed == call_id => Some(record.observation.operation_id.clone()),
                _ => None,
            },
            _ => None,
        });
    let Some(operation) = operation else {
        return Vec::new();
    };
    audit
        .iter()
        .filter(|record| record.observation.operation_id == operation)
        .map(|record| &record.observation.observation)
        .collect()
}

fn server_call_params(log: &Path) -> Vec<(String, Value)> {
    server_calls(log)
        .iter()
        .map(|line| {
            let request: Value = serde_json::from_str(line).expect("a JSON-RPC request line");
            (
                request["params"]["name"]
                    .as_str()
                    .expect("the called operation")
                    .to_owned(),
                request["params"]["arguments"].clone(),
            )
        })
        .collect()
}

/// Fixture isolation only: the cases take turns and each ends its
/// composition before the next starts. Each fixture has its own controller
/// credential identity, published once by its realm's owner at open. An
/// earlier variant that published after the session was created was
/// intermittently refused with "controller credential authority is
/// unavailable"; the captured cause was the realm's SQLite runtime store
/// refusing its non-waiting controller-custody open while busy ("current
/// schema catalog is not ready without waiting", once "database is
/// locked"). That is neither a global credential lifecycle rule nor a rule
/// about pinned controllers. Publishing at open meets the same busy custody
/// under load, so the open retries only that typed Busy refusal, until WAIT.
static CONTROLLER_CREDENTIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn catalog() -> BTreeSet<String> {
    [TOOL_CATALOG_SEARCH_NAME, TOOL_CATALOG_LOAD_NAME]
        .map(str::to_owned)
        .into()
}

// ---------------------------------------------------------------------------
// The governed prompt
// ---------------------------------------------------------------------------

/// The operator's prompt through the stock composition: the installation's
/// operations are deferred catalog entries; the agent loads the read and the
/// delete; in one model response it calls both. The stock owner refuses the
/// delete (its grant names only the read) as that call's typed feedback,
/// the read reaches the server with its exact arguments, and the original
/// continues to its final answer.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stock_prompt_loads_reads_and_has_its_write_refused() {
    let _turn = CONTROLLER_CREDENTIAL.lock().await;
    let root = tempfile::tempdir().expect("state root");
    let log = root.path().join("records.log");
    let paths = provision(root.path(), &log, "controller-operator-prompt").await;
    let model = ScriptedModel::new(
        "controller-operator-prompt",
        vec![
            Step::Call {
                id: "load",
                name: TOOL_CATALOG_LOAD_NAME,
                args: json!({"names": [READ, DELETE]}),
            },
            Step::Calls(vec![
                ("delete", DELETE, json!({"record": "record-7"})),
                ("read", READ, json!({"record": "record-7"})),
            ]),
            Step::Finish(FINAL),
        ],
    );
    let realm = open_as(root.path(), DEFAULT_STOCK_AGENT, model.clone()).await;
    let session = realm.session(installed(&paths).await).await;
    wait_listed(&log).await;

    let (input_id, text) = realm
        .prompt(
            &session,
            "Load the records tools, then delete and read record-7",
        )
        .await
        .expect("the operator's prompt is bound");
    assert_eq!(text, FINAL);

    let with_loaded: BTreeSet<String> = [
        TOOL_CATALOG_SEARCH_NAME,
        TOOL_CATALOG_LOAD_NAME,
        READ,
        DELETE,
    ]
    .map(str::to_owned)
    .into();
    assert_eq!(
        model.offered(),
        vec![catalog(), with_loaded.clone(), with_loaded],
        "the operations are deferred until loaded, then exactly the loaded ones are offered"
    );
    let load = model.feedback(1, "load");
    assert!(!load.is_error, "{}", load.text_content());
    let loaded: Value = serde_json::from_str(&load.text_content()).expect("load response");
    assert_eq!(loaded["accepted_names"], json!([DELETE, READ]), "{loaded}");
    let delete = model.feedback(2, "delete");
    assert!(delete.is_error, "{}", delete.text_content());
    assert_eq!(delete.text_content(), DELETE_REFUSAL);
    let read = model.feedback(2, "read");
    assert!(!read.is_error, "{}", read.text_content());
    assert_eq!(read.text_content(), ANSWER);
    assert_eq!(
        server_call_params(&log),
        vec![(READ.to_owned(), json!({"record": "record-7"}))],
        "only the permitted read reaches the server"
    );

    let audit = realm.audit(&session, &input_id).await;
    assert!(
        matches!(
            tool_audit(&audit, "delete").first(),
            Some(AuditObservation::Refused {
                reason: OperationRefusalKind::Denied,
                ..
            })
        ),
        "the stock owner refuses the delete operation-locally"
    );
    for call in ["load", "read"] {
        let observed = tool_audit(&audit, call);
        assert!(
            matches!(observed.first(), Some(AuditObservation::Prepared { .. })),
            "{call} was prepared under the stock policy"
        );
        assert!(
            observed
                .iter()
                .any(|observation| matches!(observation, AuditObservation::Entry { .. })),
            "{call} reached its native entry"
        );
    }
    realm.close(vec![session]).await;
}

/// The exact typed feedback the model receives for the refused delete: the
/// owner denied it before the tool ran, which the payload says.
const DELETE_REFUSAL: &str = r#"{"data":{"kind":"denied","tool_invoked":false},"error":"operation_refused","message":"operation unavailable under current authorization"}"#;

/// Another agent with its own valid controller and worker grants, whose
/// worker grant does not cover the records installation, is refused that
/// installation: its session is never offered the records operations,
/// loading the read from its catalog selects nothing, nothing reaches the
/// server, and its original still completes.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn another_agent_is_refused_the_records_installation() {
    let _turn = CONTROLLER_CREDENTIAL.lock().await;
    let root = tempfile::tempdir().expect("state root");
    let log = root.path().join("records.log");
    let paths = provision(root.path(), &log, "controller-other-agent").await;
    let model = ScriptedModel::new(
        "controller-other-agent",
        vec![
            Step::Call {
                id: "load",
                name: TOOL_CATALOG_LOAD_NAME,
                args: json!({"names": [READ]}),
            },
            Step::Finish(FINAL),
        ],
    );
    let realm = open_as(root.path(), OTHER_AGENT, model.clone()).await;
    let session = realm.session(installed(&paths).await).await;
    let (_, text) = realm
        .prompt(&session, "Read record-7")
        .await
        .expect("the other agent's own prompt is bound");
    assert_eq!(text, FINAL);
    let offered = model.offered();
    assert!(
        offered
            .iter()
            .all(|tools| !tools.contains(READ) && !tools.contains(LIST) && !tools.contains(DELETE)),
        "the other agent is never offered the records operations: {offered:?}"
    );
    let load = model.feedback(1, "load");
    assert!(!load.is_error, "{}", load.text_content());
    let loaded: Value = serde_json::from_str(&load.text_content()).expect("load response");
    assert_eq!(
        loaded,
        json!({
            "catalog_exact": true,
            "accepted_names": [],
            "noop_names": [],
            "resolutions": [{"name": READ, "accepted": false, "rejected_reason": "unknown_key"}],
        }),
        "the records read is not in the other agent's catalog"
    );
    assert!(
        !log.exists(),
        "no MCP request was observed for the records installation"
    );
    realm.close(vec![session]).await;
}

/// An agent the operator issued no grants to is refused locally and typed
/// at `bind_prompt`: no input is submitted, no model request runs and
/// nothing reaches the server.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_agent_without_a_grant_is_refused_before_any_model() {
    let _turn = CONTROLLER_CREDENTIAL.lock().await;
    let root = tempfile::tempdir().expect("state root");
    let log = root.path().join("records.log");
    let paths = provision(root.path(), &log, "controller-ungranted-agent").await;
    let model = ScriptedModel::new("controller-ungranted-agent", vec![Step::Finish(FINAL)]);
    let realm = open_as(root.path(), UNGRANTED_AGENT, model.clone()).await;
    assert!(!realm.stock.has_grant_selection());
    let session = realm.session(installed(&paths).await).await;
    let refused = realm.prompt(&session, "Read record-7").await;
    assert!(
        matches!(refused, Err(StockGovernanceError::GrantsUnavailable)),
        "{:?}",
        refused.map(drop)
    );
    assert!(model.requests().is_empty(), "no model request ran");
    assert!(server_calls(&log).is_empty(), "nothing reaches the server");
    realm.close(vec![session]).await;
}

// ---------------------------------------------------------------------------
// A host-authenticated caller
// ---------------------------------------------------------------------------

/// A trusted host's exact-target decision for its callers: it answers with
/// its current answer and records every question it is asked. Once joined
/// to the composition's publication, every change of its answer is an
/// owner change published there.
struct HostAccess {
    answer: Mutex<Result<(), OperationAuthorizationError>>,
    /// The lifetime of the login the host admits.
    login: Mutex<StockLoginLifetime>,
    asked: Mutex<Vec<(PrincipalRef, PrincipalRef, SessionId)>>,
    publication: Mutex<Option<LocalAuthorizationPublication>>,
}

impl HostAccess {
    fn answering(answer: Result<(), OperationAuthorizationError>) -> Arc<Self> {
        Arc::new(Self {
            answer: Mutex::new(answer),
            login: Mutex::new(StockLoginLifetime::UntilRetired),
            asked: Mutex::default(),
            publication: Mutex::default(),
        })
    }

    /// The admitted login's lifetime, set without any owner change: a
    /// deadline is a fact of the login itself, not a host publication.
    fn login(&self, login: StockLoginLifetime) {
        *self.login.lock().expect("login") = login;
    }

    /// Join the host's access facts to the composition's publication, at
    /// quiescent startup.
    fn join(&self, publication: LocalAuthorizationPublication) {
        *self.publication.lock().expect("publication") = Some(publication);
    }

    fn answer(&self, answer: Result<(), OperationAuthorizationError>) {
        let publication = self.publication.lock().expect("publication").clone();
        // Never waits: a publication held by someone else fails here, bounded,
        // instead of blocking a test that holds a binding.
        let _change = publication.as_ref().map(|publication| {
            publication
                .try_begin_owner_change()
                .expect("the publication is free for the host's owner change")
        });
        *self.answer.lock().expect("answer") = answer;
    }
}

impl StockCallerAccess for HostAccess {
    fn authorize_caller(
        &self,
        request: &StockCallerRequest<'_>,
    ) -> Result<StockLoginLifetime, OperationAuthorizationError> {
        self.asked.lock().expect("asked").push((
            request.caller().clone(),
            request.owner().clone(),
            request.session().clone(),
        ));
        let login = *self.login.lock().expect("login");
        (*self.answer.lock().expect("answer")).map(|()| login)
    }
}

fn denied() -> OperationAuthorizationError {
    OperationRefused::new(OperationRefusalKind::Denied).into()
}

/// A human the host authenticated in its own identity domain, with the
/// host's evidence of that authentication.
fn host_caller() -> StockAuthenticatedCaller {
    let human = PrincipalRef::in_domain(
        PrincipalKind::Human,
        "alice",
        TrustDomainId::new("console-idp").expect("domain"),
    )
    .expect("qualified caller");
    let authentication = meerkat_authorization_contracts::evidence::HistoricalEvidenceRef {
        resource: meerkat_authorization_contracts::resource::ResourceRef {
            domain: meerkat_authorization_contracts::constraints::ResourceDomain {
                authority: host_ingress(),
                namespace: "console-logins".into(),
            },
            resource_id: "login-1".into(),
        },
        revision: meerkat_authorization_contracts::evidence::EvidenceId::new("login-1-r1")
            .expect("revision"),
        digest: meerkat_authorization_contracts::evidence::EvidenceDigest::from_array([3; 32]),
    };
    StockAuthenticatedCaller::new(human, authentication).expect("authenticated caller")
}

/// The trusted host's own ingress actor in the realm's trust domain.
fn host_ingress() -> PrincipalRef {
    PrincipalRef::in_domain(
        PrincipalKind::RuntimeHost,
        "console-host",
        TrustDomainId::new(format!("rkat-realm-{REALM}")).expect("domain"),
    )
    .expect("qualified ingress")
}

/// A caller the trusted host authenticated, distinct from the operator who
/// owns the session, runs only while the host's exact decision for that
/// session allows it. Refused, its input is not bound: the typed refusal
/// returns before any binding and no model request runs. Allowed at the
/// bind but refused before admission, native admission asks again and
/// refuses. Allowed, the same session runs it to completion. The operator's
/// own prompt is refused locally: in this posture it is no requester.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_host_authenticated_caller_runs_only_while_the_host_allows_it() {
    let _turn = CONTROLLER_CREDENTIAL.lock().await;
    let root = tempfile::tempdir().expect("state root");
    let log = root.path().join("records.log");
    provision(root.path(), &log, "controller-host-caller").await;
    let operator = StockPrincipals::for_local_operator(&realm())
        .expect("local operator")
        .operator;
    let model = ScriptedModel::new("controller-host-caller", vec![Step::Finish(FINAL)]);
    let access = HostAccess::answering(Err(denied()));
    let realm = open_for_host(
        root.path(),
        model.clone(),
        host_ingress(),
        Arc::clone(&access) as Arc<dyn StockCallerAccess>,
    )
    .await;
    let session = realm.session(Vec::new()).await;
    let operators = realm.prompt(&session, "hello").await;
    assert!(
        matches!(
            operators,
            Err(StockGovernanceError::RequesterUnsupported(_))
        ),
        "{:?}",
        operators.map(drop)
    );
    let caller = host_caller();
    let refused = realm.caller_prompt(&session, "hello", &caller).await;
    assert!(
        matches!(refused, Err(StockGovernanceError::CallerNotAdmitted(error)) if error == denied()),
        "{:?}",
        refused.map(drop)
    );
    access.answer(Ok(()));
    let (input, pending) = realm
        .stock
        .bind_caller_prompt(
            &realm.machine,
            &session.session_id,
            Input::Prompt(PromptInput::new("hello", None)),
            session.controller.clone(),
            &caller,
        )
        .await
        .expect("the allowed caller's input binds");
    access.answer(Err(denied()));
    assert!(
        realm
            .machine
            .accept_input_with_completion(&session.session_id, input)
            .await
            .is_err(),
        "native admission asks the host again and refuses"
    );
    drop(pending);
    assert!(model.requests().is_empty(), "no model request ran");
    access.answer(Ok(()));
    let (_, text) = realm
        .caller_prompt(&session, "hello again", &caller)
        .await
        .expect("the allowed caller's input binds");
    assert_eq!(text, FINAL);
    assert_eq!(model.requests().len(), 1);
    let asked = access.asked.lock().expect("asked").clone();
    assert!(!asked.is_empty());
    assert!(
        asked.iter().all(|(who, owner, asked_session)| {
            who == caller.requester() && *owner == operator && *asked_session == session.session_id
        }),
        "the host is asked about the actual caller, owner and session"
    );
    realm.close(vec![session]).await;
}

/// A refused caller's bind changes nothing. On an idle scoped session whose
/// earlier legitimate result is disclosed, a denied and an unavailable
/// caller each get the host's typed answer before any binding, and the
/// operator named as an authenticated caller is refused by the stock
/// requester rule while the host admits it, before the host is asked; the
/// earlier result stays disclosed: the session's binding is unchanged. An
/// allowed caller's bound input carries the caller's own identity.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_refused_caller_leaves_the_sessions_binding_and_results() {
    let _turn = CONTROLLER_CREDENTIAL.lock().await;
    let root = tempfile::tempdir().expect("state root");
    let log = root.path().join("records.log");
    provision(root.path(), &log, "controller-host-binding").await;
    let operator = StockPrincipals::for_local_operator(&realm())
        .expect("local operator")
        .operator;
    let model = ScriptedModel::new("controller-host-binding", vec![Step::Finish(FINAL)]);
    let access = HostAccess::answering(Ok(()));
    let realm = open_for_host(
        root.path(),
        model.clone(),
        host_ingress(),
        Arc::clone(&access) as Arc<dyn StockCallerAccess>,
    )
    .await;
    let session = realm.session(Vec::new()).await;
    let caller = host_caller();
    let (earlier, text) = realm
        .caller_prompt(&session, "hello", &caller)
        .await
        .expect("the allowed caller's input binds");
    assert_eq!(text, FINAL);
    let disclosed = |outcome: &Result<
        Option<CompletionOutcome>,
        meerkat_runtime::RuntimeDriverError,
    >| {
        matches!(outcome, Ok(Some(CompletionOutcome::Completed(result))) if result.text == FINAL)
    };
    assert!(disclosed(
        &realm
            .machine
            .input_terminal_completion(&session.session_id, &earlier)
            .await
    ));
    for (expected, label) in [
        (denied(), "denied"),
        (OperationAuthorizationError::Unavailable, "unavailable"),
    ] {
        access.answer(Err(expected));
        let refused = realm
            .stock
            .bind_caller_prompt(
                &realm.machine,
                &session.session_id,
                Input::Prompt(PromptInput::new("again", None)),
                session.controller.clone(),
                &caller,
            )
            .await;
        assert!(
            matches!(refused, Err(StockGovernanceError::CallerNotAdmitted(error)) if error == expected),
            "{label}: {:?}",
            refused.map(drop)
        );
        assert!(
            disclosed(
                &realm
                    .machine
                    .input_terminal_completion(&session.session_id, &earlier)
                    .await
            ),
            "{label}: the earlier result stays disclosed, its binding unchanged"
        );
    }
    access.answer(Ok(()));
    // The operator named as an authenticated caller, while the host admits
    // every caller: the stock requester rule refuses it before the host is
    // asked and before any binding.
    let asked = access.asked.lock().expect("asked").len();
    let operator_caller =
        StockAuthenticatedCaller::new(operator.clone(), caller.authentication().clone())
            .expect("a qualified principal");
    let refused = realm
        .stock
        .bind_caller_prompt(
            &realm.machine,
            &session.session_id,
            Input::Prompt(PromptInput::new("again", None)),
            session.controller.clone(),
            &operator_caller,
        )
        .await;
    assert!(
        matches!(refused, Err(StockGovernanceError::CallerNotAdmitted(error)) if error == denied()),
        "operator-named: {:?}",
        refused.map(drop)
    );
    assert_eq!(
        access.asked.lock().expect("asked").len(),
        asked,
        "the host is not asked about the operator"
    );
    assert!(
        disclosed(
            &realm
                .machine
                .input_terminal_completion(&session.session_id, &earlier)
                .await
        ),
        "operator-named: the earlier result stays disclosed, its binding unchanged"
    );
    // A host's administrative outcome is a local operator's report: a
    // host-authenticated composition refuses it before any binding.
    let refused = realm
        .stock
        .bind_host_outcome(
            &realm.machine,
            &session.session_id,
            host_outcome(external()),
            session.controller.clone(),
        )
        .await;
    assert!(
        matches!(refused, Err(StockGovernanceError::RequesterUnsupported(_))),
        "a host outcome here: {:?}",
        refused.map(drop)
    );
    let (input, pending) = realm
        .stock
        .bind_caller_prompt(
            &realm.machine,
            &session.session_id,
            Input::Prompt(PromptInput::new("again", None)),
            session.controller.clone(),
            &caller,
        )
        .await
        .expect("the allowed caller's input binds");
    let Input::Prompt(prompt) = &input else {
        panic!("a bound prompt");
    };
    let requester = &prompt
        .header
        .authority_association
        .as_ref()
        .expect("the bound association")
        .candidate()
        .requester;
    assert!(requester == caller.requester() && *requester != operator);
    drop(pending);
    drop(input);
    assert_eq!(model.requests().len(), 1);
    realm.close(vec![session]).await;
}

/// A publication that completes while a caller's binding is held before its
/// commit refuses that binding at the commit. The host admits the caller
/// when the binding observes it; while the binding is held immediately
/// before its driver takes the session's machine authority, the host revokes
/// the caller and publishes that change on the composition's publication.
/// The observation is no longer current at the commit, so nothing is bound;
/// observed again, the caller is refused typed, and the session's binding
/// and the disclosure of its earlier result are unchanged. (That the
/// binder's decision runs with the machine authority held is the driver's
/// own fixture.)
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_caller_revoked_while_its_binding_waits_is_refused_at_the_commit() {
    let _turn = CONTROLLER_CREDENTIAL.lock().await;
    let root = tempfile::tempdir().expect("state root");
    let log = root.path().join("records.log");
    provision(root.path(), &log, "controller-host-commit").await;
    let model = ScriptedModel::new("controller-host-commit", vec![Step::Finish(FINAL)]);
    let access = HostAccess::answering(Ok(()));
    let realm = open_for_host(
        root.path(),
        model.clone(),
        host_ingress(),
        Arc::clone(&access) as Arc<dyn StockCallerAccess>,
    )
    .await;
    // The host joins its access facts at quiescent startup.
    access.join(realm.stock.authorization_publication());
    let session = realm.session(Vec::new()).await;
    let caller = host_caller();
    let (earlier, text) = realm
        .caller_prompt(&session, "hello", &caller)
        .await
        .expect("the allowed caller's input binds");
    assert_eq!(text, FINAL);
    let (arrived, release) = realm
        .machine
        .arm_scoped_binding_before_machine_authority_test_hook(session.session_id.clone());
    let binding = {
        let stock = Arc::clone(&realm.stock);
        let machine = Arc::clone(&realm.machine);
        let session_id = session.session_id.clone();
        let controller = session.controller.clone();
        let caller = caller.clone();
        tokio::spawn(async move {
            stock
                .bind_caller_prompt(
                    &machine,
                    &session_id,
                    Input::Prompt(PromptInput::new("again", None)),
                    controller,
                    &caller,
                )
                .await
                .map(drop)
        })
    };
    tokio::time::timeout(WAIT, arrived)
        .await
        .expect("the binding reaches its commit")
        .expect("the binding waits at its commit");
    // The host revokes the caller while the binding is held.
    access.answer(Err(denied()));
    drop(release);
    let refused = tokio::time::timeout(WAIT, binding)
        .await
        .expect("the binding settles")
        .expect("the binding task");
    assert!(
        matches!(refused, Err(StockGovernanceError::CallerNotAdmitted(error)) if error == denied()),
        "{refused:?}"
    );
    assert!(
        matches!(
            realm
                .machine
                .input_terminal_completion(&session.session_id, &earlier)
                .await,
            Ok(Some(CompletionOutcome::Completed(result))) if result.text == FINAL
        ),
        "the earlier result stays disclosed, its binding unchanged"
    );

    // The host admits the caller again, for a login with its own deadline.
    // The deadline passes while the binding waits at its commit, and the
    // host publishes nothing: the commit still refuses, by the deadline.
    access.answer(Ok(()));
    let now_ms = u64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_millis(),
    )
    .expect("milliseconds");
    access.login(StockLoginLifetime::ExpiresAt {
        unix_ms: now_ms + 1_000,
    });
    let held_bind = |stock: Arc<StockGovernance>| {
        let machine = Arc::clone(&realm.machine);
        let session_id = session.session_id.clone();
        let controller = session.controller.clone();
        let caller = caller.clone();
        tokio::spawn(async move {
            stock
                .bind_caller_prompt(
                    &machine,
                    &session_id,
                    Input::Prompt(PromptInput::new("again", None)),
                    controller,
                    &caller,
                )
                .await
                .map(drop)
        })
    };
    let (arrived, release) = realm
        .machine
        .arm_scoped_binding_before_machine_authority_test_hook(session.session_id.clone());
    let binding = held_bind(Arc::clone(&realm.stock));
    tokio::time::timeout(WAIT, arrived)
        .await
        .expect("the binding reaches its commit")
        .expect("the binding waits at its commit");
    tokio::time::sleep(std::time::Duration::from_millis(1_500)).await;
    drop(release);
    let expired = tokio::time::timeout(WAIT, binding)
        .await
        .expect("the binding settles")
        .expect("the binding task");
    assert!(
        matches!(expired, Err(StockGovernanceError::CallerNotAdmitted(error)) if error == denied()),
        "{expired:?}"
    );
    assert!(
        matches!(
            realm
                .machine
                .input_terminal_completion(&session.session_id, &earlier)
                .await,
            Ok(Some(CompletionOutcome::Completed(result))) if result.text == FINAL
        ),
        "the expired login's refusal keeps the earlier result disclosed, its binding unchanged"
    );

    // A login whose deadline is still ahead commits, held the same way.
    access.login(StockLoginLifetime::ExpiresAt {
        unix_ms: now_ms + 3_600_000,
    });
    let (arrived, release) = realm
        .machine
        .arm_scoped_binding_before_machine_authority_test_hook(session.session_id.clone());
    let binding = held_bind(Arc::clone(&realm.stock));
    tokio::time::timeout(WAIT, arrived)
        .await
        .expect("the binding reaches its commit")
        .expect("the binding waits at its commit");
    drop(release);
    let committed = tokio::time::timeout(WAIT, binding)
        .await
        .expect("the binding settles")
        .expect("the binding task");
    assert!(committed.is_ok(), "{committed:?}");
    assert_eq!(model.requests().len(), 1);
    realm.close(vec![session]).await;
}

// ---------------------------------------------------------------------------
// A host's registered tool bundle
// ---------------------------------------------------------------------------

/// A host's Rust ledger tool bundle: reads and deletes entries and records
/// every call it runs.
#[derive(Default)]
struct Ledger {
    calls: Mutex<Vec<String>>,
}

impl Ledger {
    fn calls(&self) -> Vec<String> {
        self.calls.lock().expect("calls").clone()
    }
}

#[async_trait]
impl meerkat_core::AgentToolDispatcher for Ledger {
    fn tools(&self) -> Arc<[Arc<meerkat_core::ToolDef>]> {
        [READ_ENTRY, DELETE_ENTRY]
            .map(|name| {
                Arc::new(meerkat_core::ToolDef::new(
                    name,
                    format!("{name} of the ledger"),
                    json!({
                        "type": "object",
                        "properties": {"entry": {"type": "string"}},
                        "required": ["entry"],
                        "additionalProperties": false,
                    }),
                ))
            })
            .into()
    }

    async fn dispatch(
        &self,
        call: meerkat_core::ToolCallView<'_>,
    ) -> Result<meerkat_core::ToolDispatchOutcome, meerkat_core::ToolError> {
        self.calls
            .lock()
            .expect("calls")
            .push(call.name.to_string());
        Ok(meerkat_core::ToolDispatchOutcome::sync_result(
            ToolResult::new(call.id.to_string(), ENTRY.to_string(), false),
        ))
    }
}

/// The ledger, registered by the trusted host under its stable name: a new
/// registration instance each time the host composes it.
fn registered_ledger(ledger: &Arc<Ledger>) -> Arc<dyn meerkat_core::AgentToolDispatcher> {
    Arc::new(meerkat_core::RegisteredToolBundleDispatcher::new(
        LEDGER,
        Arc::clone(ledger) as Arc<dyn meerkat_core::AgentToolDispatcher>,
    ))
}

/// The operator grants the default agent exactly the ledger's read. In one
/// model response the agent calls the delete and the read: the delete, with
/// no grant of its own (the agent's catalog grant never covers it), is
/// refused as that call's typed feedback, the read runs on the bundle, and
/// the original completes. The realm's composition is then torn down and
/// the realm reopened in this same process; with the host's fresh
/// registration under the same stable name, the retained grant admits the
/// read again.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_bundle_operation_runs_beside_its_refused_sibling_and_after_reopen() {
    let _turn = CONTROLLER_CREDENTIAL.lock().await;
    let root = tempfile::tempdir().expect("state root");
    let log = root.path().join("records.log");
    provision(root.path(), &log, "controller-bundle-operation").await;
    let principals = StockPrincipals::for_local_operator(&realm()).expect("local operator");
    grant_stock_agent(
        root.path(),
        REALM,
        &principals,
        &StockGrantRequest {
            agent: DEFAULT_STOCK_AGENT.to_string(),
            target: StockGrantTarget::ToolBundleOperation {
                bundle: LEDGER.to_string(),
                operation: READ_ENTRY.to_string(),
            },
            expires_in_ms: None,
        },
    )
    .await
    .expect("the operator grants the ledger read");

    let ledger = Arc::new(Ledger::default());
    let model = ScriptedModel::new(
        "controller-bundle-operation",
        vec![
            Step::Calls(vec![
                ("delete", DELETE_ENTRY, json!({"entry": "entry-3"})),
                ("read", READ_ENTRY, json!({"entry": "entry-3"})),
            ]),
            Step::Finish(FINAL),
        ],
    );
    let realm = open_as(root.path(), DEFAULT_STOCK_AGENT, model.clone()).await;
    let session = realm
        .session_with(Vec::new(), Some(registered_ledger(&ledger)))
        .await;
    let (input_id, text) = realm
        .prompt(&session, "Delete and read entry-3")
        .await
        .expect("the operator's prompt is bound");
    assert_eq!(text, FINAL);
    let offered = model.offered();
    assert!(
        offered[0].contains(READ_ENTRY) && offered[0].contains(DELETE_ENTRY),
        "the bundle's operations are offered: {offered:?}"
    );
    let delete = model.feedback(1, "delete");
    assert!(delete.is_error, "{}", delete.text_content());
    assert_eq!(delete.text_content(), DELETE_REFUSAL);
    let read = model.feedback(1, "read");
    assert!(!read.is_error, "{}", read.text_content());
    assert_eq!(read.text_content(), ENTRY);
    assert_eq!(
        ledger.calls(),
        vec![READ_ENTRY.to_string()],
        "only the granted read reaches the bundle"
    );
    let audit = realm.audit(&session, &input_id).await;
    assert!(
        matches!(
            tool_audit(&audit, "delete").first(),
            Some(AuditObservation::Refused {
                reason: OperationRefusalKind::Denied,
                ..
            })
        ),
        "the delete is refused operation-locally"
    );
    assert!(
        tool_audit(&audit, "read")
            .iter()
            .any(|observation| matches!(observation, AuditObservation::Entry { .. })),
        "the read reached its native entry"
    );
    realm.close(vec![session]).await;

    // The same process reopens the realm after that teardown: the host
    // composes a fresh registration of the ledger under the same stable
    // name, and the retained grant admits it.
    let reopened_ledger = Arc::new(Ledger::default());
    let model = ScriptedModel::new(
        "controller-bundle-operation",
        vec![
            Step::Call {
                id: "read-again",
                name: READ_ENTRY,
                args: json!({"entry": "entry-3"}),
            },
            Step::Finish(FINAL),
        ],
    );
    let realm = open_as(root.path(), DEFAULT_STOCK_AGENT, model.clone()).await;
    let session = realm
        .session_with(Vec::new(), Some(registered_ledger(&reopened_ledger)))
        .await;
    let (_, text) = realm
        .prompt(&session, "Read entry-3 again")
        .await
        .expect("the reopened realm binds the prompt");
    assert_eq!(text, FINAL);
    let read = model.feedback(1, "read-again");
    assert!(!read.is_error, "{}", read.text_content());
    assert_eq!(read.text_content(), ENTRY);
    assert_eq!(reopened_ledger.calls(), vec![READ_ENTRY.to_string()]);
    assert_eq!(
        ledger.calls(),
        vec![READ_ENTRY.to_string()],
        "the old registration is not used"
    );
    realm.close(vec![session]).await;
}

/// The origin of a host's administrative outcome: an external event source.
fn external() -> InputOrigin {
    InputOrigin::External {
        source_name: "connector-finalization".to_owned(),
    }
}

/// An administrative outcome the operator's own process reports, as an
/// external event of `source`.
fn host_outcome(source: InputOrigin) -> ExternalEventInput {
    ExternalEventInput {
        header: InputHeader {
            ingress_context: None,
            retained_resume: None,
            authority_association: None,
            id: meerkat_core::lifecycle::InputId::new(),
            timestamp: chrono::Utc::now(),
            source,
            durability: InputDurability::Durable,
            visibility: InputVisibility::default(),
            idempotency_key: None,
            supersession_key: None,
            correlation_id: None,
        },
        event_type: "connector_finalization".to_owned(),
        payload: json!({"outcome": "persisted", "generation": 2}),
        blocks: None,
        handling_mode: meerkat_core::types::HandlingMode::Queue,
        render_metadata: None,
        objective_id: None,
    }
}

/// An administrative outcome the operator's own process reports binds as
/// the stock ingress's own original: the requester is the ingress, a
/// runtime host, under the host-outcome labels and this process's own
/// observation, with no represented subject; it is admitted under the
/// operator's grants and its turn runs.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_host_outcome_is_the_stock_ingress_own_original() {
    let _turn = CONTROLLER_CREDENTIAL.lock().await;
    let root = tempfile::tempdir().expect("state root");
    let log = root.path().join("records.log");
    provision(root.path(), &log, "controller-host-outcome").await;
    let principals = StockPrincipals::for_local_operator(&realm()).expect("local operator");
    let model = ScriptedModel::new("controller-host-outcome", vec![Step::Finish(FINAL)]);
    let realm = open_as(root.path(), DEFAULT_STOCK_AGENT, model.clone()).await;
    let session = realm.session(Vec::new()).await;
    let (input, pending) = realm
        .stock
        .bind_host_outcome(
            &realm.machine,
            &session.session_id,
            host_outcome(external()),
            session.controller.clone(),
        )
        .await
        .expect("the host's outcome binds");
    let Input::ExternalEvent(event) = &input else {
        panic!("a bound external event");
    };
    let association = event
        .header
        .authority_association
        .as_ref()
        .expect("the bound association")
        .candidate();
    assert_eq!(
        association.requester, principals.ingress_actor,
        "the stock ingress itself is the requester"
    );
    assert_eq!(association.requester.kind, PrincipalKind::RuntimeHost);
    assert!(association.represented_subject.is_none());
    assert!(
        association
            .original_work
            .work
            .as_str()
            .starts_with("host-outcome-"),
        "{}",
        association.original_work.work.as_str()
    );
    assert_eq!(
        association.root_event.resource.domain.namespace,
        "host-outcome-events"
    );
    assert_eq!(
        association
            .original_authentication
            .resource
            .domain
            .namespace,
        "host-outcome-authentication"
    );
    let key = event
        .header
        .idempotency_key
        .as_ref()
        .expect("the event key");
    assert!(key.0.starts_with("rkat-host-outcome-"), "{}", key.0);
    let (_, text) = realm.admitted(&session, input, pending).await;
    assert_eq!(text, FINAL);
    assert_eq!(model.requests().len(), 1);
    realm.close(vec![session]).await;
}

/// An external event is no prompt of the operator: `bind_prompt` refuses
/// it, typed and before any write, and a host's outcome binds only as a
/// durable external event of external origin. The refusals come before the
/// session's destination binding: on a session with no registration they
/// are the same, never the destination's.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_outcome_binds_only_as_the_hosts_external_event() {
    let _turn = CONTROLLER_CREDENTIAL.lock().await;
    let root = tempfile::tempdir().expect("state root");
    let log = root.path().join("records.log");
    provision(root.path(), &log, "controller-outcome-refusals").await;
    let model = ScriptedModel::new("controller-outcome-refusals", vec![Step::Finish(FINAL)]);
    let realm = open_as(root.path(), DEFAULT_STOCK_AGENT, model.clone()).await;
    let session = realm.session(Vec::new()).await;
    let unregistered = SessionId::new();
    for target in [&session.session_id, &unregistered] {
        let refused = realm
            .stock
            .bind_prompt(
                &realm.machine,
                target,
                Input::ExternalEvent(host_outcome(external())),
                session.controller.clone(),
            )
            .await;
        assert!(
            matches!(refused, Err(StockGovernanceError::RequesterUnsupported(_))),
            "an event as a prompt: {:?}",
            refused.map(drop)
        );
        for (label, origin) in [
            ("operator", InputOrigin::Operator),
            ("system", InputOrigin::System),
        ] {
            let refused = realm
                .stock
                .bind_host_outcome(
                    &realm.machine,
                    target,
                    host_outcome(origin),
                    session.controller.clone(),
                )
                .await;
            assert!(
                matches!(refused, Err(StockGovernanceError::RequesterUnsupported(_))),
                "{label} origin: {:?}",
                refused.map(drop)
            );
        }
        for (label, durability) in [
            ("derived", InputDurability::Derived),
            ("ephemeral", InputDurability::Ephemeral),
        ] {
            let mut outcome = host_outcome(external());
            outcome.header.durability = durability;
            let refused = realm
                .stock
                .bind_host_outcome(&realm.machine, target, outcome, session.controller.clone())
                .await;
            assert!(
                matches!(refused, Err(StockGovernanceError::RequesterUnsupported(_))),
                "{label} durability: {:?}",
                refused.map(drop)
            );
        }
    }
    assert!(model.requests().is_empty(), "nothing was submitted");
    let (_, text) = realm
        .prompt(&session, "hello")
        .await
        .expect("the operator's prompt still binds");
    assert_eq!(text, FINAL);
    realm.close(vec![session]).await;
}

/// A stock governed realm never admits a host original. The record is a
/// consistency check at submit, so a source holding the observation exactly
/// lets it commit. Drained before the host binds its continuation services
/// (the stock delivery host composes consent notices), the observation stays
/// pending, as the setup signal (`Unbound`) says. Once the host binds a
/// source that holds it but offers no binder (the stock composition offers
/// none), it is refused `NoAdmissibleWorkBinding` and the signal reads
/// `NotOffered`. It is never bound or admitted, and nothing runs.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stock_governed_realm_never_admits_a_host_original() {
    use meerkat::{
        ContinuationDelivery, ContinuationHandling, ContinuationKey, ContinuationOwner,
        ContinuationOwnerService, ContinuationProducer, ContinuationResultRef, ContinuationStatus,
        HostOriginalFacts, HostOriginalLookup, HostOriginalReadiness, HostOriginalRecord,
        RetainedJobLookup, RetainedJobSource,
    };

    /// Holds the observation exactly, for the submit-time consistency check.
    struct Holds(HostOriginalFacts);

    #[async_trait]
    impl RetainedJobSource for Holds {
        async fn retained_job(
            &self,
            _producer: &ContinuationProducer,
            _job_id: &str,
        ) -> RetainedJobLookup {
            RetainedJobLookup::Absent
        }

        async fn host_original(&self, _namespace: &str, _producer_id: &str) -> HostOriginalLookup {
            HostOriginalLookup::Found(self.0.clone())
        }
    }

    let _turn = CONTROLLER_CREDENTIAL.lock().await;
    let root = tempfile::tempdir().expect("state root");
    let log = root.path().join("records.log");
    provision(root.path(), &log, "controller-host-original").await;
    let model = ScriptedModel::new("controller-host-original", Vec::new());
    let realm = open_as(root.path(), DEFAULT_STOCK_AGENT, model.clone()).await;
    let session = realm.session(Vec::new()).await;
    let owner = ContinuationOwner::Session {
        session_id: session.session_id.clone(),
    };
    let delivery = ContinuationDelivery {
        key: ContinuationKey::new("ingest:observation-1").expect("key"),
        result: ContinuationResultRef {
            producer: ContinuationProducer::Host {
                namespace: "app.ingest".into(),
            },
            producer_id: "observation-1".into(),
            result_digest: "sha256:observation-1".into(),
            summary: None,
        },
        body: "a new item arrived".into(),
        handling: ContinuationHandling::Queue,
    };
    let source = Holds(HostOriginalFacts::new(
        owner.clone(),
        delivery.clone(),
        meerkat_runtime::OriginalWorkRef {
            authority: PrincipalRef::in_domain(
                PrincipalKind::ServiceAccount,
                "source-owner",
                TrustDomainId::new("stock-prompt").expect("domain"),
            )
            .expect("qualified"),
            work: meerkat_authorization_contracts::evidence::EvidenceId::new("observation-1")
                .expect("id"),
        },
    ));
    let record = HostOriginalRecord::from_owner(&source, "app.ingest", "observation-1")
        .await
        .expect("read the source")
        .expect("the source holds the observation");
    let inbox = meerkat_runtime::RuntimeDeliveryInbox::new(Arc::new(
        meerkat_runtime::InMemoryRuntimeStore::new(),
    ));
    let continuations = ContinuationOwnerService::new(
        inbox.clone(),
        Arc::new(meerkat::SessionAddressResolver),
        Arc::clone(&realm.machine),
    );
    continuations
        .submit_host_original(&owner, delivery, &record, 1)
        .await
        .expect("the record is a consistency check at submit");

    let bindings = Arc::new(meerkat::ContinuationHostBindings::default());
    assert_eq!(
        bindings
            .host_original_readiness(&realm.machine, "app.ingest", &owner)
            .await,
        HostOriginalReadiness::Unbound,
        "the stock composition binds no continuation services"
    );
    let lock_dir = root.path().join("hosting");
    let domain = meerkat_core::session_hosting::HostingPaths {
        hosting_lock_dir: lock_dir.clone(),
        cold_delivery_lock: lock_dir.join("cold-delivery.lock"),
        database: None,
    }
    .domain_ref();
    let host = meerkat::surface::SessionServiceDeliveryHost::new(
        &realm.service,
        &realm.machine,
        meerkat::DetachedJobService::new(Arc::new(meerkat::MemoryDetachedJobStore::new())),
        None,
    )
    .with_continuation_bindings(Arc::clone(&bindings))
    .with_consent_notices(Arc::new(meerkat::ConsentNoticeProjector::new(
        Arc::new(meerkat_core::ApprovalService::new()),
        domain,
        inbox.clone(),
        &realm.machine,
    )));
    let handle =
        meerkat::RuntimeDeliveryOwner::new(Arc::new(meerkat::MemoryDetachedJobStore::new()), inbox)
            .with_source_changes(bindings.subscribe_source_changes())
            .arm(Arc::new(host))
            .expect("arm the stock delivery owner");
    let mut passes = handle.subscribe_passes();
    let key = ContinuationKey::new("ingest:observation-1").expect("key");
    let drained = tokio::time::timeout(
        WAIT,
        passes.wait_for(|pass| pass.refused > 0 || !pass.blocked_runtimes.is_empty()),
    )
    .await
    .expect("the arming reconcile drained the observation")
    .expect("owner pass channel")
    .clone();
    assert_eq!(
        drained.refused, 0,
        "pending before the host binds, never refused: {:?}",
        drained.failures
    );
    assert!(matches!(
        continuations
            .continuation_status(&owner, &key)
            .await
            .expect("status"),
        ContinuationStatus::Pending { .. }
    ));

    // The host binds its services: a source that holds the observation
    // exactly but, as the stock composition, offers no binder.
    let _binding = bindings
        .bind(Arc::new(meerkat::SessionAddressResolver), Arc::new(source))
        .expect("bind the host's continuation services");
    assert_eq!(
        bindings
            .host_original_readiness(&realm.machine, "app.ingest", &owner)
            .await,
        HostOriginalReadiness::NotOffered
    );
    let status = tokio::time::timeout(WAIT, async {
        loop {
            let status = continuations
                .continuation_status(&owner, &key)
                .await
                .expect("status");
            if matches!(
                status,
                ContinuationStatus::Applied { .. } | ContinuationStatus::Refused { .. }
            ) {
                return status;
            }
            passes.changed().await.expect("owner pass channel");
        }
    })
    .await
    .unwrap_or_else(|_| panic!("settled; last pass {:?}", passes.borrow().clone()));
    assert!(
        matches!(
            status,
            ContinuationStatus::Refused {
                reason: meerkat_runtime::RuntimeDeliveryRefusalReason::NoAdmissibleWorkBinding,
                ..
            }
        ),
        "{status:?}"
    );
    assert!(
        realm
            .machine
            .list_active_inputs(&session.session_id)
            .await
            .expect("active inputs")
            .is_empty(),
        "nothing was admitted"
    );
    assert!(model.requests().is_empty(), "nothing ran");
    drop(continuations);
    drop(handle);
    realm.close(vec![session]).await;
}
