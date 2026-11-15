//! A mob member's composed tool dispatcher reports an exact catalog, so a
//! member with many tools sends a short tool list and loads the rest on
//! demand.
//!
//! Every row builds a real member: a real agent factory, a real persistent
//! session service and the mob runtime's own composition. Nothing here
//! composes a member's dispatcher by hand. The rows read two things: the
//! session's typed catalog view (how the next model request projects the
//! catalog) and the tool definitions the model requests actually carried.

#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use meerkat::{AgentFactory, Config, FactoryAgentBuilder, PersistentSessionService};
use meerkat_client::{LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest};
use meerkat_core::types::{
    HandlingMode, ToolCallView, ToolDef, ToolProvenance, ToolResult, ToolSourceKind,
};
use meerkat_core::{
    AgentEvent, AgentToolDispatcher, Message, SessionId, ToolCatalogCapabilities, ToolCatalogEntry,
    ToolCatalogMode, ToolCatalogView, ToolCatalogVisibility, ToolError,
};
use meerkat_mob::{
    AgentIdentity, ExternalToolsProvider, MemberTurnOptions, MobBuilder, MobDefinition, MobHandle,
    MobId, MobMemberStatus, MobRuntimeMode, MobStorage, Profile, ProfileBinding, ProfileName,
    SpawnMemberSpec, ToolConfig,
};
use meerkat_store::{MemoryBlobStore, SqliteSessionStore, StoreAdapter};

const STEP: Duration = Duration::from_mins(2);
const SEARCH: &str = "tool_catalog_search";
const LOAD: &str = "tool_catalog_load";
const MEMBER: &str = "worker-1";

/// One model reply of a scripted turn.
enum Reply {
    Call {
        name: &'static str,
        args: serde_json::Value,
    },
    Done,
}

/// What one model request carried.
#[derive(Clone)]
struct Sent {
    tools: Vec<String>,
    /// Bytes of the request's tool definitions, each serialized as JSON.
    tool_bytes: usize,
    /// Whether the request's messages carry the notice that names the tools
    /// waiting behind the deferred catalog.
    deferred_notice: bool,
    /// Whether the request's messages carry the system prompt guidance that
    /// explains the catalog control tools.
    catalog_guidance: bool,
    /// The request's messages, serialized: everything the model reads beside
    /// its tool definitions.
    messages: String,
}

impl Sent {
    fn has(&self, name: &str) -> bool {
        self.tools.iter().any(|tool| tool == name)
    }

    fn host_tools(&self) -> usize {
        self.tools
            .iter()
            .filter(|tool| tool.starts_with(HOST_TOOL_PREFIX))
            .count()
    }
}

/// A model that answers from a script and records every request.
#[derive(Default)]
struct ScriptedClient {
    script: Mutex<VecDeque<Reply>>,
    sent: Mutex<Vec<Sent>>,
}

impl ScriptedClient {
    fn push(&self, replies: Vec<Reply>) {
        self.script.lock().unwrap().extend(replies);
    }

    /// The requests recorded since the last call.
    fn take_sent(&self) -> Vec<Sent> {
        std::mem::take(&mut *self.sent.lock().unwrap())
    }
}

#[async_trait::async_trait]
impl LlmClient for ScriptedClient {
    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
    ) -> std::pin::Pin<Box<dyn futures::Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>>
    {
        let call_index = {
            let messages = serde_json::to_string(&request.messages).unwrap();
            let mut sent = self.sent.lock().unwrap();
            sent.push(Sent {
                tools: request
                    .tools
                    .iter()
                    .map(|tool| tool.name.to_string())
                    .collect(),
                tool_bytes: request
                    .tools
                    .iter()
                    .map(|tool| serde_json::to_vec(tool.as_ref()).unwrap().len())
                    .sum(),
                deferred_notice: messages.contains("new deferred tools available"),
                catalog_guidance: messages
                    .contains("Additional tools may be available in a deferred catalog"),
                messages,
            });
            sent.len()
        };
        let reply = self
            .script
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(Reply::Done);
        let (mut events, stop) = match reply {
            Reply::Call { name, args } => (
                vec![LlmEvent::ToolCallComplete {
                    id: format!("call-{call_index}-{name}"),
                    name: name.to_string(),
                    args,
                    meta: None,
                }],
                meerkat_core::StopReason::ToolUse,
            ),
            Reply::Done => (
                vec![LlmEvent::TextDelta {
                    delta: "done".to_string(),
                    meta: None,
                }],
                meerkat_core::StopReason::EndTurn,
            ),
        };
        events.push(LlmEvent::UsageUpdate {
            usage: meerkat_core::TurnUsage::host_declared(
                meerkat_core::Provider::OpenAI,
                &request.model,
                meerkat_core::Usage::default(),
            ),
        });
        events.push(LlmEvent::Done {
            outcome: LlmDoneOutcome::success(stop),
        });
        Box::pin(futures::stream::iter(events.into_iter().map(Ok)))
    }

    fn provider(&self) -> meerkat_core::Provider {
        meerkat_core::Provider::OpenAI
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }
}

const HOST_TOOL_COUNT: usize = 40;
const HOST_TOOL_PREFIX: &str = "host_report_";
const HOST_TOOL_03: &str = "host_report_03";
const HOST_TOOL_07: &str = "host_report_07";

fn host_tool_name(index: usize) -> String {
    format!("{HOST_TOOL_PREFIX}{index:02}")
}

/// A host's own tool source, as a host registers one for its members: an
/// exact catalog of forty tools, each of which may wait behind the deferred
/// catalog.
struct HostTools {
    catalog: Arc<[ToolCatalogEntry]>,
}

impl HostTools {
    fn new() -> Self {
        let provenance = ToolProvenance {
            kind: ToolSourceKind::Callback,
            source_id: "host-reports".into(),
            artifact: None,
        };
        let catalog = (0..HOST_TOOL_COUNT)
            .map(|index| {
                let mut tool = ToolDef::new(
                    host_tool_name(index),
                    format!(
                        "Build report {index} for one account and period. Returns the \
                         report rows as JSON, with totals, and the period's notes."
                    ),
                    serde_json::json!({
                        "type": "object",
                        "properties": {
                            "account": {
                                "type": "string",
                                "description": "Account the report covers."
                            },
                            "period": {
                                "type": "string",
                                "description": "Reporting period, as YYYY-MM."
                            },
                            "include_notes": {
                                "type": "boolean",
                                "description": "Whether the period's notes are included."
                            }
                        },
                        "required": ["account", "period"]
                    }),
                );
                tool.provenance = Some(provenance.clone());
                ToolCatalogEntry::session_deferred(Arc::new(tool), true, provenance.clone())
            })
            .collect::<Vec<_>>()
            .into();
        Self { catalog }
    }

    fn definition_bytes(&self) -> usize {
        self.catalog
            .iter()
            .map(|entry| serde_json::to_vec(entry.tool.as_ref()).unwrap().len())
            .sum()
    }
}

#[async_trait::async_trait]
impl AgentToolDispatcher for HostTools {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        self.catalog
            .iter()
            .map(|entry| Arc::clone(&entry.tool))
            .collect::<Vec<_>>()
            .into()
    }

    fn tool_catalog_capabilities(&self) -> ToolCatalogCapabilities {
        ToolCatalogCapabilities {
            exact_catalog: true,
            may_require_catalog_control_plane: false,
        }
    }

    fn tool_catalog(&self) -> Arc<[ToolCatalogEntry]> {
        Arc::clone(&self.catalog)
    }

    async fn dispatch(
        &self,
        call: ToolCallView<'_>,
    ) -> Result<meerkat_core::ops::ToolDispatchOutcome, ToolError> {
        if !self
            .catalog
            .iter()
            .any(|entry| entry.tool.name == call.name)
        {
            return Err(ToolError::not_found(call.name));
        }
        Ok(ToolResult::new(call.id.to_string(), format!("{} rows: 3", call.name), false).into())
    }
}

/// The same host tools behind a source that reports no exact catalog (the
/// trait default). A member that mounts it sends every tool inline, as a
/// member did while one of its sources reported no exact catalog.
struct InexactHostTools(HostTools);

#[async_trait::async_trait]
impl AgentToolDispatcher for InexactHostTools {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        self.0.tools()
    }

    async fn dispatch(
        &self,
        call: ToolCallView<'_>,
    ) -> Result<meerkat_core::ops::ToolDispatchOutcome, ToolError> {
        self.0.dispatch(call).await
    }
}

#[derive(Clone, Copy)]
enum HostSource {
    None,
    Exact,
    Inexact,
}

impl HostSource {
    fn provider(self) -> Option<ExternalToolsProvider> {
        let source: Arc<dyn AgentToolDispatcher> = match self {
            Self::None => return None,
            Self::Exact => Arc::new(HostTools::new()),
            Self::Inexact => Arc::new(InexactHostTools(HostTools::new())),
        };
        Some(Arc::new(move || Some(Arc::clone(&source))))
    }
}

type Service = Arc<PersistentSessionService<FactoryAgentBuilder>>;

struct Paths {
    root: PathBuf,
    realm_db: PathBuf,
    mob_db: PathBuf,
}

impl Paths {
    fn new(root: &Path) -> Self {
        let paths = Self {
            root: root.to_path_buf(),
            realm_db: root.join("stores").join("realm.db"),
            mob_db: root.join("mob.db"),
        };
        for dir in ["project-root", "context-root", "stores"] {
            std::fs::create_dir_all(root.join(dir)).expect("create the test roots");
        }
        paths
    }
}

/// A persistent service in the production realm shape: SQLite session rows
/// and the runtime authority share one database, so a second service over
/// the same paths resumes what the first one stored.
fn persistent_service(paths: &Paths, client: &Arc<ScriptedClient>) -> Service {
    let factory = AgentFactory::new(paths.root.join("runtime-root").join("factory-store"))
        .user_config_root(paths.root.join("user-config"))
        .runtime_root(paths.root.join("runtime-root"))
        .project_root(paths.root.join("project-root"))
        .context_root(paths.root.join("context-root"))
        .comms(true);
    let mut builder = FactoryAgentBuilder::new(factory, Config::default());
    builder.default_llm_client = Some(client.clone());
    // The schedule tools a host gives its sessions, for profiles that switch
    // the schedule family on.
    meerkat::surface::set_default_schedule_tools(
        &builder,
        Some(Arc::new(meerkat::ScheduleToolDispatcher::new(
            meerkat::ScheduleService::new(Arc::new(meerkat::MemoryScheduleStore::new())),
        ))),
    );
    let store =
        Arc::new(SqliteSessionStore::open(&paths.realm_db).expect("open the session store"));
    builder.default_session_store = Some(Arc::new(StoreAdapter::new(store.clone())));
    let store_dyn: Arc<dyn meerkat::SessionStore> = store;
    let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> = Arc::new(
        meerkat_runtime::SqliteRuntimeStore::new_head_canonical(&paths.realm_db)
            .expect("open the runtime store"),
    );
    let blob_store: Arc<dyn meerkat_core::BlobStore> = Arc::new(MemoryBlobStore::default());
    Arc::new(PersistentSessionService::new(
        builder,
        8,
        store_dyn,
        runtime_store,
        blob_store,
    ))
}

fn mob_definition(tools: ToolConfig) -> MobDefinition {
    let mut profiles = BTreeMap::new();
    profiles.insert(
        ProfileName::from("worker"),
        ProfileBinding::Inline(Box::new(Profile {
            model_fallback: None,
            model: "gpt-5.5".to_string(),
            provider: None,
            self_hosted_server_id: None,
            auth_binding: None,
            image_generation_provider: None,
            auto_compact_threshold: None,
            resume_overrides: Vec::new(),
            skills: vec![],
            tools,
            peer_description: "team worker".to_string(),
            external_addressable: true,
            backend: None,
            runtime_mode: MobRuntimeMode::TurnDriven,
            max_inline_peer_notifications: None,
            output_schema: None,
            provider_params: None,
            durable: false,
        })),
    );
    let mut definition = MobDefinition::explicit(MobId::from(format!(
        "member-catalog-{}",
        uuid::Uuid::new_v4().simple()
    )));
    definition.profiles = profiles;
    definition
}

/// The families the rows' member mounts: builtins, comms, memory search and
/// the mob operator tools.
fn member_families() -> ToolConfig {
    ToolConfig {
        builtins: true,
        comms: true,
        memory: true,
        mob: true,
        ..ToolConfig::default()
    }
}

async fn member_session(handle: &MobHandle) -> SessionId {
    handle
        .roster()
        .await
        .list()
        .find(|entry| entry.agent_identity == MEMBER)
        .and_then(|entry| entry.bridge_session_id().cloned())
        .expect("the member has a session")
}

async fn wait_member_active(handle: &MobHandle) {
    let deadline = tokio::time::Instant::now() + STEP;
    let mut changes = handle.machine_state_changes();
    loop {
        let members = handle.list_members().await;
        if members
            .iter()
            .any(|entry| entry.status == MobMemberStatus::Active)
        {
            return;
        }
        tokio::select! {
            changed = changes.changed() => changed.unwrap_or_else(|_| {
                panic!("the mob actor stopped before the member was active: {members:?}")
            }),
            () = tokio::time::sleep_until(deadline) => {
                panic!("timed out waiting for the member to be active: {members:?}")
            }
        }
    }
}

/// One real member of a real mob, with the model that answers it.
struct Member {
    temp: tempfile::TempDir,
    client: Arc<ScriptedClient>,
    service: Service,
    handle: MobHandle,
    session: SessionId,
}

impl Member {
    async fn spawn(tools: ToolConfig, host: HostSource) -> Self {
        Self::spawn_with_launch_policy(tools, host, None).await
    }

    /// [`Self::spawn`], with a tool access policy given for this one spawn.
    async fn spawn_with_launch_policy(
        tools: ToolConfig,
        host: HostSource,
        launch: Option<meerkat_core::ops::ToolAccessPolicy>,
    ) -> Self {
        let temp = tempfile::tempdir().expect("temp dir");
        let paths = Paths::new(temp.path());
        let client = Arc::new(ScriptedClient::default());
        let service = persistent_service(&paths, &client);
        let definition = mob_definition(tools);
        // The mob's own WorkGraph service, scoped to the realm its members
        // build in, for profiles that switch the WorkGraph family on.
        let workgraph = meerkat::WorkGraphService::with_namespace_grant(
            Arc::new(meerkat::MemoryWorkGraphStore::new()),
            meerkat::WorkGraphNamespaceGrant::new(
                meerkat_core::mob_realm_id(definition.id.as_str())
                    .expect("mob realm")
                    .as_str(),
                "default",
            )
            .expect("a WorkGraph grant in the mob realm"),
        )
        .expect("a WorkGraph service for the grant");
        let handle = MobBuilder::new(
            definition,
            MobStorage::persistent(&paths.mob_db).expect("mob storage"),
        )
        .with_session_service(service.clone())
        .with_default_llm_client(client.clone())
        .with_default_external_tools_provider(host.provider())
        .with_workgraph_service(Some(workgraph))
        .create()
        .await
        .expect("create the mob");
        let mut spec = SpawnMemberSpec::new("worker", AgentIdentity::from(MEMBER));
        if let Some(policy) = launch {
            spec = spec.with_tool_access_policy(policy);
        }
        handle.spawn_spec(spec).await.expect("spawn the member");
        let session = member_session(&handle).await;
        Self {
            temp,
            client,
            service,
            handle,
            session,
        }
    }

    /// Stop this process's mob and service and resume both over the same
    /// stores, as a restarted host does. `host` is the tool source the
    /// resumed mob gives its members.
    async fn restart(self, host: HostSource) -> Self {
        let Self {
            temp,
            client,
            service,
            handle,
            session,
        } = self;
        handle.shutdown().await.expect("shutdown before restart");
        service
            .discard_live_session(&session)
            .await
            .expect("discard the live session before restart");
        drop(handle);
        drop(service);

        let paths = Paths::new(temp.path());
        let service = persistent_service(&paths, &client);
        let handle =
            MobBuilder::for_resume(MobStorage::persistent(&paths.mob_db).expect("reopen storage"))
                .with_session_service(service.clone())
                .with_default_llm_client(client.clone())
                .with_default_external_tools_provider(host.provider())
                .notify_orchestrator_on_resume(false)
                .with_member_restore_policy(meerkat_mob::MemberRestorePolicy::Eager)
                .resume()
                .await
                .expect("resume the mob after the restart");
        wait_member_active(&handle).await;
        assert_eq!(
            member_session(&handle).await,
            session,
            "the member keeps its session across the restart"
        );
        Self {
            temp,
            client,
            service,
            handle,
            session,
        }
    }

    async fn view(&self) -> ToolCatalogView {
        self.service
            .tool_catalog_view(&self.session)
            .await
            .expect("the catalog view query succeeds")
            .expect("a live member reports its catalog view")
    }

    /// Run one turn to its end with `replies` as the model's answers.
    /// Returns its tool results as `(tool, is_error, text)` and the requests
    /// the model received.
    async fn turn(
        &self,
        prompt: &str,
        replies: Vec<Reply>,
    ) -> (Vec<(String, bool, String)>, Vec<Sent>) {
        self.client.take_sent();
        self.client.push(replies);
        let member = self
            .handle
            .member(&AgentIdentity::from(MEMBER))
            .await
            .expect("member handle");
        let (events_tx, mut events_rx) = tokio::sync::mpsc::channel(4096);
        let turn = member
            .start_turn(
                prompt,
                HandlingMode::Queue,
                MemberTurnOptions::default(),
                Some(events_tx),
            )
            .await
            .expect("start the turn");
        tokio::time::timeout(STEP, turn.wait())
            .await
            .expect("the turn completes")
            .expect("the turn succeeds");
        let mut results = Vec::new();
        while let Ok(envelope) = events_rx.try_recv() {
            if let AgentEvent::ToolResultReceived {
                name,
                is_error,
                content,
                ..
            } = envelope.payload
            {
                results.push((
                    name.clone(),
                    is_error,
                    meerkat_core::types::text_content(&content),
                ));
            }
        }
        (results, self.client.take_sent())
    }
}

fn visibility(view: &ToolCatalogView, name: &str) -> Option<ToolCatalogVisibility> {
    view.entries
        .iter()
        .find(|entry| entry.name.as_str() == name)
        .map(|entry| entry.visibility)
}

fn result<'a>(results: &'a [(String, bool, String)], tool: &str) -> &'a (String, bool, String) {
    results
        .iter()
        .find(|(name, _, _)| name == tool)
        .unwrap_or_else(|| panic!("{tool} returned a result; results: {results:?}"))
}

/// The families a profile can switch on, one at a time and together. Every
/// mob member has comms.
fn family_cases() -> Vec<(&'static str, ToolConfig)> {
    let comms = || ToolConfig {
        comms: true,
        ..ToolConfig::default()
    };
    vec![
        ("comms", comms()),
        (
            "comms+builtins",
            ToolConfig {
                builtins: true,
                ..comms()
            },
        ),
        (
            "comms+memory",
            ToolConfig {
                memory: true,
                ..comms()
            },
        ),
        (
            "comms+builtins+memory",
            ToolConfig {
                builtins: true,
                memory: true,
                ..comms()
            },
        ),
        (
            "comms+mob",
            ToolConfig {
                mob: true,
                ..comms()
            },
        ),
        (
            "comms+schedule",
            ToolConfig {
                schedule: true,
                ..comms()
            },
        ),
        (WORKGRAPH_CASE, workgraph_member()),
        (
            "comms+builtins+memory+mob+schedule",
            ToolConfig {
                schedule: true,
                ..member_families()
            },
        ),
    ]
}

const WORKGRAPH_CASE: &str = "comms+builtins+mob+workgraph";

/// A member with the mob operator tools and WorkGraph, whose only
/// deferred-eligible tools are WorkGraph's own two.
fn workgraph_member() -> ToolConfig {
    ToolConfig {
        builtins: true,
        comms: true,
        mob: true,
        workgraph: true,
        ..ToolConfig::default()
    }
}

/// A member that mounts builtin or external tools (the mob operator tools, a
/// host tool source) composes an exact catalog, whatever other families its
/// profile switches on. The row prints every composition, so a family that
/// keeps a member inexact is named.
///
/// A member with neither composes on the empty base dispatcher, which
/// reports no exact catalog. Such a member is printed and not asserted: it
/// mounts nothing here that could wait behind the deferred catalog.
///
/// A member whose only deferred-eligible tools are WorkGraph's two attention
/// tools keeps them in its requests, as every other session with an exact
/// catalog and WorkGraph does: the two are under the deferred catalog's byte
/// budget, so the member's catalog is exact and inline.
#[tokio::test(flavor = "multi_thread")]
async fn a_member_with_builtin_or_external_tools_composes_an_exact_catalog() {
    let mut inexact = Vec::new();
    for (host_label, host) in [("none", HostSource::None), ("exact", HostSource::Exact)] {
        for (label, tools) in family_cases() {
            let expect_exact = tools.builtins || tools.mob || matches!(host, HostSource::Exact);
            let member = Member::spawn(tools, host).await;
            let view = member.view().await;
            let mut deferred = view
                .entries
                .iter()
                .filter(|entry| entry.visibility == ToolCatalogVisibility::Deferred)
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>();
            deferred.sort_unstable();
            println!(
                "member families={label} host_source={host_label}: exact_catalog={} mode={:?} \
                 catalog_entries={} deferred={}{}",
                view.exact_catalog,
                view.mode,
                view.entries.len(),
                deferred.len(),
                if deferred.is_empty() || deferred.len() > 4 {
                    String::new()
                } else {
                    format!(" {deferred:?}")
                }
            );
            if expect_exact && !view.exact_catalog {
                inexact.push(format!("families={label} host_source={host_label}"));
            }
            if view.exact_catalog && label == WORKGRAPH_CASE && matches!(host, HostSource::None) {
                assert_eq!(
                    view.mode,
                    ToolCatalogMode::Inline,
                    "WorkGraph's two attention tools alone are under the byte budget"
                );
                assert!(
                    deferred.is_empty(),
                    "nothing waits behind the catalog for this member: {deferred:?}"
                );
                for name in ["workgraph_attention_reassign", "workgraph_policy_escalate"] {
                    assert!(
                        view.entries.iter().any(|entry| entry.name.as_str() == name),
                        "{name} is in the member's catalog"
                    );
                }
            }
        }
    }
    assert!(
        inexact.is_empty(),
        "members with builtin or external tools whose composed catalog is not exact: {inexact:?}"
    );
}

/// A member with builtins, comms, memory search, the mob operator tools and
/// a host source of forty tools: its requests carry the families' tools and
/// the two catalog control tools, the forty wait behind the deferred
/// catalog, and the model finds, loads and calls one of them.
#[tokio::test(flavor = "multi_thread")]
async fn a_member_with_many_tools_sends_a_short_tool_list_and_loads_the_rest_on_demand() {
    let member = Member::spawn(member_families(), HostSource::Exact).await;

    // What the request carries, before any assertion on the mode.
    let (_, sent) = member.turn("say hello", vec![Reply::Done]).await;
    let first = sent.first().expect("the turn made a model request");
    println!(
        "member request: {} tool definitions, {} bytes ({} host tools inline; the host \
         source's {HOST_TOOL_COUNT} definitions are {} bytes)",
        first.tools.len(),
        first.tool_bytes,
        first.host_tools(),
        HostTools::new().definition_bytes()
    );

    let view = member.view().await;
    assert!(
        view.exact_catalog,
        "the member's composed dispatcher reports an exact catalog"
    );
    assert_eq!(
        view.mode,
        ToolCatalogMode::Deferred,
        "a member with forty deferred-eligible tools uses the deferred catalog"
    );
    for index in 0..HOST_TOOL_COUNT {
        assert_eq!(
            visibility(&view, &host_tool_name(index)),
            Some(ToolCatalogVisibility::Deferred),
            "{} waits behind the deferred catalog",
            host_tool_name(index)
        );
    }
    assert_eq!(
        first.host_tools(),
        0,
        "the request carries none of the host's tools: {:?}",
        first.tools
    );
    // The member can still act: the families stay inline, and the control
    // tools are there to load the rest.
    for name in [
        SEARCH,
        LOAD,
        "spawn_member",
        "list_members",
        "memory_search",
        "send_message",
        "task_create",
    ] {
        assert!(
            first.has(name),
            "{name} is in the request: {:?}",
            first.tools
        );
        assert_eq!(
            visibility(&view, name),
            Some(ToolCatalogVisibility::Visible),
            "{name} is visible"
        );
    }
    assert!(
        first.deferred_notice,
        "the request tells the model which tools wait behind the deferred catalog"
    );
    assert!(
        first.catalog_guidance,
        "the system prompt explains the catalog control tools"
    );

    // The model finds one host tool, loads it and calls it.
    let (results, sent) = member
        .turn(
            "build report 7",
            vec![
                Reply::Call {
                    name: SEARCH,
                    args: serde_json::json!({ "query": HOST_TOOL_07 }),
                },
                Reply::Call {
                    name: LOAD,
                    args: serde_json::json!({ "names": [HOST_TOOL_07] }),
                },
                Reply::Call {
                    name: HOST_TOOL_07,
                    args: serde_json::json!({ "account": "a-1", "period": "2026-10" }),
                },
                Reply::Done,
            ],
        )
        .await;
    assert_eq!(sent.len(), 4, "search, load, call and the final answer");

    let (_, search_error, search_text) = result(&results, SEARCH);
    assert!(!search_error, "the search succeeds: {search_text}");
    let search: serde_json::Value = serde_json::from_str(search_text).expect("search json");
    assert_eq!(search["catalog_exact"], true);
    assert!(
        search["results"]
            .as_array()
            .expect("search results")
            .iter()
            .any(|item| item["name"] == HOST_TOOL_07 && item["visibility_status"] == "deferred"),
        "the search finds the deferred tool: {search_text}"
    );

    let (_, load_error, load_text) = result(&results, LOAD);
    assert!(!load_error, "the load succeeds: {load_text}");
    let load: serde_json::Value = serde_json::from_str(load_text).expect("load json");
    assert_eq!(load["accepted_names"], serde_json::json!([HOST_TOOL_07]));

    assert_eq!(sent[0].host_tools(), 0, "before the load: no host tool");
    assert_eq!(
        sent[1].host_tools(),
        0,
        "the load applies at the next boundary"
    );
    assert!(
        sent[2].has(HOST_TOOL_07) && sent[2].host_tools() == 1,
        "after the load the request carries exactly the loaded tool: {:?}",
        sent[2].tools
    );

    let (_, call_error, call_text) = result(&results, HOST_TOOL_07);
    assert!(!call_error, "the loaded tool runs: {call_text}");
    assert!(call_text.contains("host_report_07 rows: 3"), "{call_text}");

    // The loaded tool stays loaded for the member's next turn.
    let (_, sent) = member.turn("anything else", vec![Reply::Done]).await;
    assert!(
        sent[0].has(HOST_TOOL_07) && sent[0].host_tools() == 1,
        "the loaded tool is still in the next turn's request: {:?}",
        sent[0].tools
    );
    let view = member.view().await;
    assert_eq!(
        visibility(&view, HOST_TOOL_07),
        Some(ToolCatalogVisibility::Visible)
    );
    assert_eq!(
        visibility(&view, HOST_TOOL_03),
        Some(ToolCatalogVisibility::Deferred)
    );
}

/// A member whose earlier turns saw every tool inline (one of its sources
/// reported no exact catalog then) is resumed by a restarted host whose
/// sources all report one. Its session resumes, its requests now carry the
/// short list, a direct call of a tool it used before is refused until the
/// tool is loaded, and the load makes it callable again.
#[tokio::test(flavor = "multi_thread")]
async fn a_resumed_member_that_saw_its_tools_inline_loads_them_on_demand() {
    let member = Member::spawn(member_families(), HostSource::Inexact).await;
    let view = member.view().await;
    assert!(
        !view.exact_catalog,
        "an inexact source keeps the member inline"
    );
    assert_eq!(view.mode, ToolCatalogMode::Inline);
    let (results, sent) = member
        .turn(
            "build report 3",
            vec![
                Reply::Call {
                    name: HOST_TOOL_03,
                    args: serde_json::json!({ "account": "a-1", "period": "2026-10" }),
                },
                Reply::Done,
            ],
        )
        .await;
    assert_eq!(
        sent[0].host_tools(),
        HOST_TOOL_COUNT,
        "the inline member's request carries every host tool"
    );
    assert!(!sent[0].has(SEARCH) && !sent[0].has(LOAD));
    assert!(!result(&results, HOST_TOOL_03).1, "the inline tool runs");
    let inline_bytes = sent[0].tool_bytes;

    let member = member.restart(HostSource::Exact).await;
    let view = member.view().await;
    assert!(view.exact_catalog, "the resumed member's catalog is exact");
    assert_eq!(view.mode, ToolCatalogMode::Deferred);
    assert_eq!(
        visibility(&view, HOST_TOOL_03),
        Some(ToolCatalogVisibility::Deferred),
        "a tool the member used inline waits behind the catalog after the resume"
    );

    let (results, sent) = member
        .turn(
            "build report 3 again",
            vec![
                // The model remembers the tool from its transcript and calls
                // it directly.
                Reply::Call {
                    name: HOST_TOOL_03,
                    args: serde_json::json!({ "account": "a-1", "period": "2026-11" }),
                },
                Reply::Call {
                    name: LOAD,
                    args: serde_json::json!({ "names": [HOST_TOOL_03] }),
                },
                Reply::Call {
                    name: HOST_TOOL_03,
                    args: serde_json::json!({ "account": "a-1", "period": "2026-11" }),
                },
                Reply::Done,
            ],
        )
        .await;
    println!(
        "resumed member request: {} tool definitions, {} bytes (inline before the restart: \
         {inline_bytes} bytes); deferred notice={} system prompt guidance={}",
        sent[0].tools.len(),
        sent[0].tool_bytes,
        sent[0].deferred_notice,
        sent[0].catalog_guidance
    );
    assert_eq!(
        sent[0].host_tools(),
        0,
        "the resumed request is the short list"
    );
    assert!(sent[0].has(SEARCH) && sent[0].has(LOAD));
    assert!(
        sent[0].deferred_notice,
        "the resumed request tells the model which tools now wait behind the catalog"
    );

    let host_results = results
        .iter()
        .filter(|(name, _, _)| name == HOST_TOOL_03)
        .collect::<Vec<_>>();
    assert_eq!(host_results.len(), 2, "{results:?}");
    let (_, refused, refused_text) = host_results[0];
    println!("direct call of a deferred tool: is_error={refused} {refused_text}");
    assert!(
        *refused,
        "a direct call of a tool behind the catalog is an error result: {refused_text}"
    );
    assert!(
        !result(&results, LOAD).1,
        "the load succeeds: {}",
        result(&results, LOAD).2
    );
    let (_, loaded_error, loaded_text) = host_results[1];
    assert!(!*loaded_error, "the loaded tool runs: {loaded_text}");
    assert!(loaded_text.contains("host_report_03 rows: 3"));

    // A loaded tool stays loaded across another restart.
    let member = member.restart(HostSource::Exact).await;
    let view = member.view().await;
    assert_eq!(
        visibility(&view, HOST_TOOL_03),
        Some(ToolCatalogVisibility::Visible),
        "the loaded tool is still visible after a restart"
    );
    let (_, sent) = member.turn("once more", vec![Reply::Done]).await;
    assert!(
        sent[0].has(HOST_TOOL_03) && sent[0].host_tools() == 1,
        "the restarted member's request carries the loaded tool: {:?}",
        sent[0].tools
    );
}

/// A name no tool source of the rows' members ever registered.
const UNREGISTERED_TOOL: &str = "host_report_never_registered";

/// The deferred catalog notice of a request: the text from its opening
/// words to the end of the message that carries it.
fn deferred_catalog_notice(sent: &Sent) -> &str {
    let start = sent
        .messages
        .find("Deferred catalog changed at turn boundary:")
        .unwrap_or_else(|| panic!("the request carries the notice: {}", sent.messages));
    let notice = &sent.messages[start..];
    &notice[..notice.find('"').unwrap_or(notice.len())]
}

/// What a member's model is told about `blocked`, a deferred-eligible tool
/// the member's filters block, in one fresh member: the first request of
/// its first turn (tool definitions and messages, the deferred catalog
/// notice among them), a search for the tool, a load of it and a load of a
/// name nobody registered.
struct BlockedToolProbe {
    first: Sent,
    search: serde_json::Value,
    blocked_load: serde_json::Value,
    unregistered_load: serde_json::Value,
}

impl BlockedToolProbe {
    async fn run(member: &Member, blocked: &'static str) -> Self {
        let (_, sent) = member.turn("say hello", vec![Reply::Done]).await;
        let first = sent.into_iter().next().expect("a model request");
        assert_eq!(
            member.view().await.mode,
            ToolCatalogMode::Deferred,
            "precondition: the member uses the deferred catalog"
        );
        assert!(
            first.deferred_notice && first.has(SEARCH) && first.has(LOAD),
            "precondition: the request offers the deferred catalog: {:?}",
            first.tools
        );
        let (results, _) = member
            .turn(
                "look for the tool",
                vec![
                    Reply::Call {
                        name: SEARCH,
                        args: serde_json::json!({ "query": blocked }),
                    },
                    Reply::Call {
                        name: LOAD,
                        args: serde_json::json!({ "names": [blocked] }),
                    },
                    Reply::Call {
                        name: LOAD,
                        args: serde_json::json!({ "names": [UNREGISTERED_TOOL] }),
                    },
                    Reply::Done,
                ],
            )
            .await;
        let json = |tool: &str, nth: usize| -> serde_json::Value {
            let (_, is_error, text) = results
                .iter()
                .filter(|(name, _, _)| name == tool)
                .nth(nth)
                .unwrap_or_else(|| panic!("{tool} result {nth}; results: {results:?}"));
            assert!(!is_error, "{tool} answers: {text}");
            serde_json::from_str(text)
                .unwrap_or_else(|error| panic!("{tool} json: {error}: {text}"))
        };
        Self {
            first,
            search: json(SEARCH, 0),
            blocked_load: json(LOAD, 0),
            unregistered_load: json(LOAD, 1),
        }
    }

    /// The model is told nothing about `blocked`: it is in no tool
    /// definition of the first request and not in that request's deferred
    /// catalog notice, a search does not return it, and a load of it answers
    /// exactly as a load of a name nobody registered.
    fn assert_not_disclosed(&self, blocked: &str) {
        assert!(
            !self.first.has(blocked),
            "{blocked} is not in the request's tool definitions: {:?}",
            self.first.tools
        );
        let notice = deferred_catalog_notice(&self.first);
        assert!(
            !notice.contains(blocked),
            "the deferred catalog notice does not name {blocked}: {notice}"
        );
        assert!(
            !self.search["results"]
                .as_array()
                .expect("search results")
                .iter()
                .any(|item| item["name"] == blocked),
            "a search does not return {blocked}: {}",
            self.search
        );
        let renamed = |load: &serde_json::Value, name: &str| {
            serde_json::to_string(load)
                .expect("load json")
                .replace(name, "NAME")
        };
        assert_eq!(
            renamed(&self.blocked_load, blocked),
            renamed(&self.unregistered_load, UNREGISTERED_TOOL),
            "a load of {blocked} answers as a load of a name nobody registered"
        );
    }
}

/// A deferred-eligible tool the member's profile denies (`tools.deny`) is
/// not disclosed by the member's deferred catalog. A profile's deny list
/// cannot name a host tool: it names tools the member composes itself, and
/// the spawn refuses the name of a tool from the host's external source
/// (`DeclaredToolUnknown`). So the row denies one of WorkGraph's two
/// deferred-eligible tools; the host's forty tools put the member over the
/// budget.
#[tokio::test(flavor = "multi_thread")]
async fn a_member_tool_its_profile_denies_is_not_disclosed_by_the_deferred_catalog() {
    const DENIED: &str = "workgraph_attention_reassign";
    let member = Member::spawn(
        ToolConfig {
            workgraph: true,
            deny: vec![DENIED.to_string()],
            ..member_families()
        },
        HostSource::Exact,
    )
    .await;
    assert_eq!(
        visibility(&member.view().await, "workgraph_policy_escalate"),
        Some(ToolCatalogVisibility::Deferred),
        "precondition: WorkGraph's other deferred-eligible tool waits behind the catalog"
    );
    BlockedToolProbe::run(&member, DENIED)
        .await
        .assert_not_disclosed(DENIED);
}

/// A deferred-eligible host tool the member's launch denies (the spawn's
/// tool access policy) is not disclosed by the member's deferred catalog.
#[tokio::test(flavor = "multi_thread")]
async fn a_member_tool_its_launch_policy_denies_is_not_disclosed_by_the_deferred_catalog() {
    let member = Member::spawn_with_launch_policy(
        member_families(),
        HostSource::Exact,
        Some(meerkat_core::ops::ToolAccessPolicy::DenyList(
            [HOST_TOOL_03].into_iter().collect(),
        )),
    )
    .await;
    BlockedToolProbe::run(&member, HOST_TOOL_03)
        .await
        .assert_not_disclosed(HOST_TOOL_03);
}

/// A deferred-eligible host tool behind the member's closed grant gate
/// (`tools.grant_gated`) is not disclosed by the member's deferred catalog.
/// Once a grant opens the gate the tool is a deferred tool like the others:
/// a search finds it, and the member loads and calls it.
#[tokio::test(flavor = "multi_thread")]
async fn a_member_tool_behind_a_closed_grant_gate_is_not_disclosed_until_the_gate_opens() {
    let member = Member::spawn(
        ToolConfig {
            grant_gated: meerkat_core::tool_grant::GrantGatedTools::default()
                .with_tool(HOST_TOOL_03),
            ..member_families()
        },
        HostSource::Exact,
    )
    .await;
    BlockedToolProbe::run(&member, HOST_TOOL_03)
        .await
        .assert_not_disclosed(HOST_TOOL_03);

    member
        .handle
        .grant_member_tool(
            &AgentIdentity::from(MEMBER),
            meerkat_mob::MemberToolGrant {
                target: meerkat_core::tool_grant::ToolGrantTarget::Tool {
                    name: meerkat_core::types::ToolName::from(HOST_TOOL_03),
                },
                issuer: meerkat_core::tool_grant::ToolGrantIssuer::operator(),
                reason: Some("the row opens the gate".to_string()),
                window: meerkat_runtime::tool_grant::ToolGrantWindow::For(Duration::from_mins(10)),
                run_scope: None,
            },
            Some(&member.session),
        )
        .await
        .expect("the grant opens the member's gate");

    let (results, sent) = member
        .turn(
            "build report 3",
            vec![
                Reply::Call {
                    name: SEARCH,
                    args: serde_json::json!({ "query": HOST_TOOL_03 }),
                },
                Reply::Call {
                    name: LOAD,
                    args: serde_json::json!({ "names": [HOST_TOOL_03] }),
                },
                Reply::Call {
                    name: HOST_TOOL_03,
                    args: serde_json::json!({ "account": "a-1", "period": "2026-10" }),
                },
                Reply::Done,
            ],
        )
        .await;
    let (_, search_error, search_text) = result(&results, SEARCH);
    assert!(!search_error, "the search succeeds: {search_text}");
    let search: serde_json::Value = serde_json::from_str(search_text).expect("search json");
    assert!(
        search["results"]
            .as_array()
            .expect("search results")
            .iter()
            .any(|item| item["name"] == HOST_TOOL_03 && item["visibility_status"] == "deferred"),
        "with the gate open a search finds the tool as a deferred one: {search_text}"
    );
    let (_, load_error, load_text) = result(&results, LOAD);
    assert!(!load_error, "the load succeeds: {load_text}");
    let load: serde_json::Value = serde_json::from_str(load_text).expect("load json");
    assert_eq!(load["accepted_names"], serde_json::json!([HOST_TOOL_03]));
    assert!(
        sent[2].has(HOST_TOOL_03),
        "after the load the request carries the tool: {:?}",
        sent[2].tools
    );
    let (_, call_error, call_text) = result(&results, HOST_TOOL_03);
    assert!(!call_error, "the loaded tool runs: {call_text}");
}
