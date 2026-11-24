//! Rows of the WorkGraph of a mob the launch's mob set built for the host
//! beside the default mob.
//!
//! The fixture is a launch that composes its WorkGraph as a gateway does (a
//! store on disk scoped to the launch mob, the session builder's WorkGraph
//! tools, the admission's cross-process lock beside the store) and declares
//! its mob to its mob set; the set then builds a second mob of the host, and
//! each row opens its own runtime to requests for that mob.

#![allow(clippy::expect_used, clippy::panic)]

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use meerkat::{AgentFactory, Config, CreateWorkItemRequest, FactoryAgentBuilder, WorkGraphService};
use meerkat_client::types::LlmStream;
use meerkat_client::{LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest};
use meerkat_core::{Message, Provider, StopReason};
use meerkat_mob::{MobDefinition, MobId, SpawnMemberSpec};
use serde_json::{Value, json};

use super::UnifiedRuntime;
use super::mob_entries::lazy_start_tests::{StoredHost, known};
use super::mob_entries_tests::{second_mob, set_of, shut_down};
use crate::blob_store::{Base64BlobStoreAdapter, ObjectStoreBlobStore};
use crate::mob_handle_runtime::{MobBootstrapOptions, MobBootstrapSpec};
use crate::runtime::{InMemoryMetadataStore, RuntimeOptions};
use crate::{DiscoverySpec, MetaConfig, MobRuntimeDelivery};

/// The text of the message that has [`ItemWriter`] create a work item.
const WRITE_AN_ITEM: &str = "write the item of the desk";
/// The title of the work item [`ItemWriter`] creates.
const WRITTEN_BY_THE_MEMBER: &str = "an item the member of the second mob wrote";
const IN_THE_LAUNCH: &str = "an item of the launch mob";
const IN_THE_SECOND: &str = "an item of the second mob";

/// A model that answers every turn with text, except in a session that was
/// sent [`WRITE_AN_ITEM`]: there it calls `workgraph_create` once (the
/// request then carries the call and its title), and answers every request
/// after that with text.
struct ItemWriter;

#[async_trait]
impl LlmClient for ItemWriter {
    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(&'a self, request: &'a LlmRequest) -> LlmStream<'a> {
        let sent = format!("{:?}", request.messages);
        let asked = sent.contains(WRITE_AN_ITEM) && !sent.contains(WRITTEN_BY_THE_MEMBER);
        let (lead, stop_reason) = if asked {
            (
                LlmEvent::ToolCallComplete {
                    id: "call-write-an-item".to_string(),
                    name: "workgraph_create".to_string(),
                    args: json!({ "title": WRITTEN_BY_THE_MEMBER }),
                    meta: None,
                },
                StopReason::ToolUse,
            )
        } else {
            (
                LlmEvent::TextDelta {
                    delta: "ok".to_string(),
                    meta: None,
                },
                StopReason::EndTurn,
            )
        };
        let events = vec![
            Ok(lead),
            Ok(crate::mob_handle_runtime::test_llm_usage::usage_event(
                request,
                Provider::OpenAI,
            )),
            Ok(LlmEvent::Done {
                outcome: LlmDoneOutcome::success(stop_reason),
            }),
        ];
        Box::pin(futures::stream::iter(events))
    }

    fn provider(&self) -> Provider {
        Provider::OpenAI
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }
}

/// The text of the message that has [`CallMaker`] make the call its row
/// queued.
const MAKE_THE_CALL: &str = "make the call that is queued for the desk";
/// The id of the call [`CallMaker`] makes.
const QUEUED_CALL: &str = "call-queued-for-the-desk";

/// A model that makes the one tool call a row queued for it, in a session
/// that was sent [`MAKE_THE_CALL`], and keeps what it is shown next in that
/// session: the request that carries the call's result. It answers every
/// other request with text.
#[derive(Default)]
struct CallMaker {
    queued: std::sync::Mutex<Option<(String, Value)>>,
    shown: std::sync::Mutex<Option<String>>,
    /// The id of the call this model makes, when it is not [`QUEUED_CALL`].
    call_id: Option<&'static str>,
}

impl CallMaker {
    /// A model whose call has the id `call_id`. A row that builds a second
    /// model over sessions an earlier model made its call in tells the
    /// second model's call from the first one's by it.
    fn making(call_id: &'static str) -> Self {
        Self {
            call_id: Some(call_id),
            ..Self::default()
        }
    }

    /// The id of the call this model makes.
    fn call_id(&self) -> &'static str {
        self.call_id.unwrap_or(QUEUED_CALL)
    }

    /// Queue the call of `tool` with `args`.
    fn queue(&self, tool: &str, args: Value) {
        *self
            .queued
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some((tool.to_string(), args));
    }

    /// What the model was shown after its call, once the call was answered.
    fn shown(&self) -> Option<String> {
        self.shown
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

#[async_trait]
impl LlmClient for CallMaker {
    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(&'a self, request: &'a LlmRequest) -> LlmStream<'a> {
        let sent = format!("{:?}", request.messages);
        let call = if sent.contains(self.call_id()) {
            let mut shown = self
                .shown
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if shown.is_none() {
                *shown = Some(sent);
            }
            None
        } else if sent.contains(MAKE_THE_CALL) {
            self.queued
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take()
        } else {
            None
        };
        let (lead, stop_reason) = match call {
            Some((name, args)) => (
                LlmEvent::ToolCallComplete {
                    id: self.call_id().to_string(),
                    name,
                    args,
                    meta: None,
                },
                StopReason::ToolUse,
            ),
            None => (
                LlmEvent::TextDelta {
                    delta: "ok".to_string(),
                    meta: None,
                },
                StopReason::EndTurn,
            ),
        };
        let events = vec![
            Ok(lead),
            Ok(crate::mob_handle_runtime::test_llm_usage::usage_event(
                request,
                Provider::OpenAI,
            )),
            Ok(LlmEvent::Done {
                outcome: LlmDoneOutcome::success(stop_reason),
            }),
        ];
        Box::pin(futures::stream::iter(events))
    }

    fn provider(&self) -> Provider {
        Provider::OpenAI
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }
}

/// A definition with one profile whose members have the WorkGraph tools,
/// under an id of its own.
fn definition(tag: &str) -> MobDefinition {
    MobDefinition::from_toml(&format!(
        "[mob]\nid = \"mob-workgraph-{tag}-{}\"\n\n[profiles.worker]\nmodel = \"gpt-5.5\"\n\n[profiles.worker.tools]\ncomms = true\nworkgraph = true\n",
        uuid::Uuid::new_v4().simple()
    ))
    .expect("a definition with one profile")
}

/// A runtime whose launch declares the mob of `definition` to its mob set,
/// with a WorkGraph composed as a gateway composes it over the state
/// directory `temp`: the store on disk scoped to the launch mob, the
/// WorkGraph tools of the launch's session builder, and the admission's
/// cross-process lock beside the store. Its members' model is
/// [`ItemWriter`].
async fn launch(temp: &tempfile::TempDir, definition: MobDefinition) -> UnifiedRuntime {
    launch_with(temp, definition, Arc::new(ItemWriter)).await
}

/// [`launch`], with `model` as its members' model.
async fn launch_with(
    temp: &tempfile::TempDir,
    definition: MobDefinition,
    model: Arc<dyn LlmClient>,
) -> UnifiedRuntime {
    let store: Arc<dyn meerkat_runtime::RuntimeStore> =
        Arc::new(meerkat_runtime::InMemoryRuntimeStore::new());
    let blobs: Arc<dyn meerkat_core::BlobStore> = Arc::new(Base64BlobStoreAdapter::new(Arc::new(
        ObjectStoreBlobStore::memory(),
    )));
    let machine = Arc::new(
        meerkat_runtime::MeerkatMachine::persistent(store.clone(), blobs.clone())
            .expect("the runtime machine"),
    );
    let jobs: Arc<dyn meerkat::DetachedJobStore> = Arc::new(meerkat::MemoryDetachedJobStore::new());
    let mut builder = FactoryAgentBuilder::new(
        AgentFactory::new(temp.path()).builtins(false).comms(true),
        Config::default(),
    );
    builder.default_detached_job_store = Some(Arc::clone(&jobs));
    let workgraph =
        crate::workgraph_wiring::open_workgraph_service(temp.path(), definition.id.as_str())
            .expect("the WorkGraph store opens under the state directory");
    let admission_slot = crate::workgraph_wiring::install_workgraph_tools(&builder, &workgraph);
    let service = Arc::new(meerkat_session::PersistentSessionService::new(
        builder,
        8,
        Arc::new(meerkat_store::MemoryStore::new()),
        store.clone(),
        blobs,
    ));
    let namespace = definition.id.to_string();
    let spec = MobBootstrapSpec::declared(
        definition,
        crate::mob_composition_manifest::DeclaredMobStore::in_memory(),
        service,
    )
    .with_session_runtime_adapter(Arc::clone(&machine))
    .expect("the launch adopts the runtime machine")
    .with_runtime_delivery(MobRuntimeDelivery::new(store, jobs))
    .with_agent_mob_tools(Arc::new(std::sync::RwLock::new(None)))
    .expect("the agent mob tools")
    .with_child_mob_storage(crate::ChildMobStorage::ProcessBound)
    .with_workgraph_service(Some(workgraph))
    .with_workgraph_admission_slot(admission_slot)
    .with_workgraph_admission_sidecar(temp.path())
    .with_options(MobBootstrapOptions {
        allow_ephemeral_sessions: false,
        notify_orchestrator_on_resume: true,
        default_llm_client: Some(model),
    });
    Box::pin(UnifiedRuntime::bootstrap_with_options(
        spec,
        MetaConfig {
            modules: Vec::new(),
            discovery: DiscoverySpec {
                namespace,
                modules: Vec::new(),
            },
            pre_spawn: Vec::new(),
        },
        Vec::new(),
        Duration::from_secs(2),
        RuntimeOptions::default(),
        Arc::new(InMemoryMetadataStore::new()),
    ))
    .await
    .expect("a runtime over a declared store")
}

/// A host that serves the mobs its set builds, and a second mob of it.
async fn host_and_second_mob(temp: &tempfile::TempDir) -> (UnifiedRuntime, MobId, MobId) {
    let host = launch(temp, definition("host")).await;
    let launch_id = host.mob_handle().mob_id().clone();
    let second_id = second_mob(&host, definition("second")).await;
    (host, launch_id, second_id)
}

/// The WorkGraph realm of the mob `mob_id`.
fn realm_of(mob_id: &MobId) -> String {
    meerkat_core::mob_realm_id(mob_id.as_str())
        .expect("the mob's realm")
        .as_str()
        .to_string()
}

/// One request on the stdio dispatch, with its answer.
async fn stdio(runtime: &UnifiedRuntime, method: &str, params: Value) -> Value {
    let request =
        json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).to_string();
    let answer = tokio::time::timeout(
        Duration::from_secs(40),
        crate::rpc::handle_unified_rpc_json(runtime, &request, Duration::from_secs(1), None, None),
    )
    .await
    .expect("the stdio dispatch answers in time");
    serde_json::from_str(&answer).expect("an answer in JSON")
}

/// `params` naming the mob `mob`, or no mob.
fn naming(params: Value, mob: Option<&MobId>) -> Value {
    let mut params = params;
    if let (Some(mob), Some(object)) = (mob, params.as_object_mut()) {
        object.insert("mob_id".to_string(), json!(mob.as_str()));
    }
    params
}

/// Whether `answer` is the answer of a method that ran: it carries a result
/// and no error.
fn served(answer: &Value) -> bool {
    answer.get("error").is_none() && answer.get("result").is_some()
}

/// A work item titled `title` created through `service`, with its id.
async fn item(service: &WorkGraphService, title: &str) -> String {
    service
        .create(CreateWorkItemRequest {
            title: title.to_string(),
            ..Default::default()
        })
        .await
        .expect("the item is created")
        .id
        .to_string()
}

/// Every work item `service` lists, rendered.
async fn listed(service: &WorkGraphService) -> String {
    let page = service
        .list_page(serde_json::from_value(json!({})).expect("a filter"))
        .await
        .expect("the service lists its items");
    serde_json::to_string(&page).expect("the page renders")
}

/// The request of a goal titled `title` for the member `desk` of the mob
/// the request is for.
fn goal_for_the_desk(title: &str) -> Value {
    json!({"title": title, "target": {"kind": "identity", "identity": "desk"}})
}

/// The resolved second mob has a WorkGraph service of its own realm, in its
/// entry and in the form a door holds: an item written through it is in no
/// list of the launch mob, and an item of the launch mob in none of its
/// own.
#[tokio::test]
async fn a_hosted_mobs_entry_serves_its_own_workgraph_realm() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (host, launch_id, second_id) = host_and_second_mob(&temp).await;
    let of_the_launch = host
        .workgraph_service()
        .expect("the launch has a WorkGraph service");
    assert_eq!(of_the_launch.default_realm_id(), realm_of(&launch_id));

    let of_the_second = host
        .resolve_mob(Some(&second_id))
        .await
        .expect("the second mob of the host resolves")
        .workgraph_service();
    assert!(
        of_the_second.is_some(),
        "the second mob's entry has a WorkGraph service"
    );
    if let Some(of_the_second) = of_the_second {
        assert_eq!(of_the_second.default_realm_id(), realm_of(&second_id));
        item(&of_the_second, IN_THE_SECOND).await;
        item(&of_the_launch, IN_THE_LAUNCH).await;
        let (in_the_launch, in_the_second) =
            (listed(&of_the_launch).await, listed(&of_the_second).await);
        assert!(
            in_the_second.contains(IN_THE_SECOND) && !in_the_second.contains(IN_THE_LAUNCH),
            "the second mob lists its own item alone: {in_the_second}"
        );
        assert!(
            in_the_launch.contains(IN_THE_LAUNCH) && !in_the_launch.contains(IN_THE_SECOND),
            "the launch mob lists its own item alone: {in_the_launch}"
        );
    }
    let through_a_door = host
        .mob_resolver()
        .resolve(Some(&second_id))
        .await
        .expect("a door's resolver answers the second mob")
        .workgraph_service()
        .map(|service| service.default_realm_id().to_string());
    assert_eq!(
        through_a_door,
        Some(realm_of(&second_id)),
        "the form a door holds carries the second mob's own service"
    );
    shut_down(&host, &[&second_id]).await;
}

/// The stdio WorkGraph methods answer for the mob a request names: an item,
/// a goal and a paused binding made with the second mob's id are the second
/// mob's, the same requests without an id are the launch mob's, each mob's
/// list, snapshot and attention list show its own alone, and the launch
/// mob's id is the request without an id.
#[tokio::test]
async fn the_workgraph_methods_answer_for_the_mob_the_request_names() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (host, launch_id, second_id) = host_and_second_mob(&temp).await;
    let mobs = [
        (Some(&second_id), &second_id, IN_THE_SECOND, IN_THE_LAUNCH),
        (None, &launch_id, IN_THE_LAUNCH, IN_THE_SECOND),
    ];

    let mut bindings = Vec::new();
    for (named, mob_id, own, _) in mobs {
        let created = stdio(
            &host,
            "meta/workgraph/create",
            naming(json!({ "title": own }), named),
        )
        .await;
        assert!(served(&created), "a create for {mob_id}: {created}");
        let goal = stdio(
            &host,
            "meta/workgraph/goal/create",
            naming(goal_for_the_desk(&format!("a goal: {own}")), named),
        )
        .await;
        assert!(served(&goal), "a goal for {mob_id}: {goal}");
        let attention = &goal["result"]["attention"];
        let binding = attention["binding_id"]
            .as_str()
            .expect("the goal's binding")
            .to_string();
        let paused = stdio(
            &host,
            "meta/workgraph/attention/pause",
            naming(
                json!({
                    "binding_id": binding,
                    "expected_revision": attention["machine_state"]["revision"],
                }),
                named,
            ),
        )
        .await;
        assert!(served(&paused), "a pause for {mob_id}: {paused}");
        bindings.push(binding);
    }

    for (index, (named, mob_id, own, of_the_other)) in mobs.into_iter().enumerate() {
        let list = stdio(&host, "meta/workgraph/list", naming(json!({}), named)).await;
        let snapshot = stdio(&host, "meta/workgraph/snapshot", naming(json!({}), named)).await;
        let attention = stdio(
            &host,
            "meta/workgraph/attention/list",
            naming(json!({}), named),
        )
        .await;
        for (method, answer) in [("list", &list), ("snapshot", &snapshot)] {
            let shown = answer.to_string();
            assert!(
                served(answer) && shown.contains(own) && !shown.contains(of_the_other),
                "the {method} of {mob_id} shows its own items alone: {shown}"
            );
        }
        assert_eq!(
            snapshot["result"]["realm_id"],
            json!(realm_of(mob_id)),
            "the snapshot of {mob_id} is of its own realm"
        );
        let shown = attention.to_string();
        assert!(
            served(&attention)
                && shown.contains(&bindings[index])
                && !shown.contains(&bindings[1 - index]),
            "the attention list of {mob_id} shows its own binding alone: {shown}"
        );
    }

    let by_the_launch_mobs_id = stdio(
        &host,
        "meta/workgraph/list",
        naming(json!({}), Some(&launch_id)),
    )
    .await
    .to_string();
    assert!(
        by_the_launch_mobs_id.contains(IN_THE_LAUNCH)
            && !by_the_launch_mobs_id.contains(IN_THE_SECOND),
        "the launch mob's id is the request without an id: {by_the_launch_mobs_id}"
    );
    shut_down(&host, &[&second_id]).await;
}

/// A goal made for the second mob names that mob's own member, and a goal
/// whose target names a member of the other mob is refused with both
/// realms, for the second mob as for the launch mob.
#[tokio::test]
async fn a_goal_for_a_hosted_mob_names_its_own_member_and_refuses_the_launch_mobs() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (host, launch_id, second_id) = host_and_second_mob(&temp).await;
    let member_of = |mob: &MobId| format!("mob/{mob}/agent/desk");
    let for_the_member_of = |mob: &MobId| {
        json!({
            "title": "a goal for the other mob's member",
            "target": {"kind": "owner", "owner_key": {"kind": "agent", "id": member_of(mob)}},
        })
    };

    let goal = stdio(
        &host,
        "meta/workgraph/goal/create",
        naming(
            goal_for_the_desk("a goal of the second mob"),
            Some(&second_id),
        ),
    )
    .await;
    assert!(served(&goal), "a goal for the second mob's member: {goal}");
    let target = goal["result"]["attention"]["target"].to_string();
    assert!(
        target.contains(&member_of(&second_id)) && !target.contains(&member_of(&launch_id)),
        "the goal's target is the second mob's own member: {target}"
    );

    for (named, requested, of_the_other) in [
        (Some(&second_id), &second_id, &launch_id),
        (None, &launch_id, &second_id),
    ] {
        let refused = stdio(
            &host,
            "meta/workgraph/goal/create",
            naming(for_the_member_of(of_the_other), named),
        )
        .await;
        let data = &refused["error"]["data"];
        assert!(
            refused["error"]["code"] == json!(-32602)
                && data["kind"] == json!("attention_target_realm_mismatch")
                && data["required_realm_id"] == json!(realm_of(of_the_other))
                && data["realm_id"] == json!(realm_of(requested)),
            "a goal through {requested} for a member of {of_the_other} is refused with both realms: {refused}"
        );
    }
    shut_down(&host, &[&second_id]).await;
}

/// `meta/capabilities` for the second mob lists the WorkGraph methods and
/// says that the mob has a WorkGraph, as it does for the launch mob.
#[tokio::test]
async fn meta_capabilities_of_a_hosted_mob_list_the_workgraph_methods() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (host, launch_id, second_id) = host_and_second_mob(&temp).await;
    for (named, mob_id) in [(Some(&second_id), &second_id), (None, &launch_id)] {
        let capabilities = stdio(&host, "meta/capabilities", naming(json!({}), named)).await;
        assert!(served(&capabilities), "{capabilities}");
        let methods = capabilities["result"]["methods"].to_string();
        let missing: Vec<&str> = crate::rpc::workgraph_methods::WORKGRAPH_READ_METHODS
            .iter()
            .chain(crate::rpc::workgraph_methods::WORKGRAPH_MUTATE_METHODS)
            .copied()
            .filter(|method| !methods.contains(&format!("\"{method}\"")))
            .collect();
        assert!(
            missing.is_empty(),
            "the capabilities of {mob_id} list every WorkGraph method; missing: {missing:?}"
        );
        assert_eq!(
            capabilities["result"]["workgraph"],
            json!(true),
            "the capabilities of {mob_id} say that it has a WorkGraph"
        );
    }
    shut_down(&host, &[&second_id]).await;
}

/// The second mob's admission is its own, over its own mob, and holds what
/// the launch mob's holds against another process on the state directory:
/// the same lock file, and the read of a member's session where the roster
/// misses.
#[tokio::test]
async fn a_hosted_mobs_admission_holds_the_hosts_sidecar_and_session_fallback() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (host, launch_id, second_id) = host_and_second_mob(&temp).await;
    let of_the_launch = host.mob_runtime().workgraph_admission();
    let of_the_second = host
        .resolve_mob(Some(&second_id))
        .await
        .expect("the second mob of the host resolves")
        .workgraph_admission();
    assert!(
        !Arc::ptr_eq(&of_the_launch, &of_the_second),
        "the second mob's admission is its own"
    );
    assert_eq!(of_the_launch.mob_handle().mob_id(), &launch_id);
    assert_eq!(of_the_second.mob_handle().mob_id(), &second_id);
    let sidecar = crate::workgraph_admission::workgraph_admission_sidecar_path(temp.path());
    assert_eq!(
        of_the_launch.cross_process_guards_in_tests(),
        (Some(sidecar.clone()), true),
        "the launch mob's admission takes the lock beside the store and reads member sessions"
    );
    assert_eq!(
        of_the_second.cross_process_guards_in_tests(),
        (Some(sidecar), true),
        "the second mob's admission holds the same two"
    );
    shut_down(&host, &[&second_id]).await;
}

/// While another process on the state directory holds the admission lock, a
/// goal request waits for it, for the second mob as for the launch mob, and
/// both are served once the lock is free.
#[tokio::test]
async fn a_host_method_on_a_hosted_mob_waits_for_the_cross_process_lock_as_on_the_launch_mob() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (host, launch_id, second_id) = host_and_second_mob(&temp).await;
    // The lock file exists from the first admission on.
    let first = stdio(
        &host,
        "meta/workgraph/goal/create",
        naming(goal_for_the_desk("the first goal"), None),
    )
    .await;
    assert!(served(&first), "the launch mob's first goal: {first}");
    let sidecar = crate::workgraph_admission::workgraph_admission_sidecar_path(temp.path());
    let another_process = rusqlite::Connection::open(&sidecar).expect("the lock file opens");
    another_process
        .execute_batch("BEGIN IMMEDIATE")
        .expect("another process takes the admission lock");

    let goal = |title: &'static str, named: Option<&MobId>| {
        stdio(
            &host,
            "meta/workgraph/goal/create",
            naming(
                json!({"title": title, "target": {"kind": "identity", "identity": "back"}}),
                named,
            ),
        )
    };
    let wait = Duration::from_secs(3);
    let (of_the_launch, of_the_second) = tokio::join!(
        tokio::time::timeout(wait, goal("held: the launch mob", None)),
        tokio::time::timeout(wait, goal("held: the second mob", Some(&second_id))),
    );
    assert!(
        of_the_launch.is_err(),
        "a goal request for {launch_id} waits while another process holds the lock: {of_the_launch:?}"
    );
    assert!(
        of_the_second.is_err(),
        "a goal request for {second_id} waits while another process holds the lock: {of_the_second:?}"
    );

    drop(another_process);
    for (title, named) in [
        ("free: the launch mob", None),
        ("free: the second mob", Some(&second_id)),
    ] {
        let answer = goal(title, named).await;
        assert!(
            served(&answer),
            "{title}: served once the lock is free: {answer}"
        );
    }
    shut_down(&host, &[&second_id]).await;
}

/// The second mob has a hub of its WorkGraph facts, in its entry and in the
/// form a door holds, fed from its own realm alone: its subscriber gets the
/// fact of the second mob's item and no fact of the launch mob's, and the
/// launch mob's subscriber the reverse.
#[tokio::test]
async fn a_hosted_mobs_fact_hub_is_fed_from_its_own_realm_alone() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (host, _launch_id, second_id) = host_and_second_mob(&temp).await;
    let of_the_launch = host
        .workgraph_service()
        .expect("the launch has a WorkGraph service");
    let launch_hub = host
        .workgraph_fact_hub()
        .expect("the launch mob has a hub of its facts");
    let (second_hub, second_service) = {
        let mob = host
            .resolve_mob(Some(&second_id))
            .await
            .expect("the second mob of the host resolves");
        (mob.workgraph_fact_hub(), mob.workgraph_service())
    };
    assert!(
        second_hub.is_some(),
        "the second mob's entry has a hub of its WorkGraph facts"
    );
    let through_a_door = host
        .mob_resolver()
        .resolve(Some(&second_id))
        .await
        .expect("a door's resolver answers the second mob")
        .workgraph_fact_hub();
    assert!(
        through_a_door.is_some(),
        "the form a door holds carries the second mob's hub"
    );
    if let (Some(second_hub), Some(of_the_second)) = (second_hub, second_service) {
        let mut to_the_second = second_hub.subscribe();
        let mut to_the_launch = launch_hub.subscribe();
        for hub in [&second_hub, &launch_hub] {
            tokio::time::timeout(Duration::from_secs(20), hub.wait_for_tail_ready())
                .await
                .expect("the hub's tail reads its realm once it has a subscriber");
        }
        let in_the_launch = item(&of_the_launch, IN_THE_LAUNCH).await;
        let in_the_second = item(&of_the_second, IN_THE_SECOND).await;

        // Each subscriber's facts until the one of its own mob's item came,
        // rendered: the launch mob's item was written first, so a fact of
        // it that reached the second mob's hub would be among them.
        let mut got = Vec::new();
        for (receiver, own) in [
            (&mut to_the_second, &in_the_second),
            (&mut to_the_launch, &in_the_launch),
        ] {
            let mut facts = String::new();
            let own_fact_came = tokio::time::timeout(Duration::from_secs(30), async {
                while !facts.contains(own.as_str()) {
                    match receiver.recv().await {
                        Ok(event) => {
                            facts.push_str(&serde_json::to_string(&event).unwrap_or_default());
                        }
                        Err(_) => break,
                    }
                }
            })
            .await;
            assert!(
                own_fact_came.is_ok() && facts.contains(own.as_str()),
                "the fact of a mob's own item reaches its subscriber: {facts}"
            );
            got.push(facts);
        }
        assert!(
            !got[0].contains(&in_the_launch),
            "no fact of the launch mob's item reaches the second mob's subscriber: {}",
            got[0]
        );
        assert!(
            !got[1].contains(&in_the_second),
            "no fact of the second mob's item reaches the launch mob's subscriber: {}",
            got[1]
        );
    }
    shut_down(&host, &[&second_id]).await;
}

/// The one task that feeds the second mob's fact hub runs while the mob set
/// manages the mob and has ended once the entry went.
#[tokio::test]
async fn a_hosted_mobs_fact_tail_ends_with_its_entry() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (host, _launch_id, second_id) = host_and_second_mob(&temp).await;
    assert!(
        host.resolve_mob(Some(&second_id)).await.is_ok(),
        "the second mob of the host resolves"
    );
    let entry = host
        .mob_entry_in_tests(&second_id)
        .expect("the runtime holds the second mob's entry");
    let tail = entry
        .workgraph_fact_tail_task
        .lock()
        .await
        .0
        .as_ref()
        .map(tokio::task::JoinHandle::abort_handle);
    assert!(
        tail.as_ref().is_some_and(|tail| !tail.is_finished()),
        "the tail of the second mob's facts runs while the mob set manages the mob"
    );

    let destroyed = Box::pin(set_of(&host).mob_destroy(&second_id)).await;
    assert!(
        destroyed.is_ok(),
        "the set destroys the second mob: {:?}",
        destroyed.as_ref().err()
    );
    assert!(
        host.resolve_mob(Some(&second_id)).await.is_err(),
        "the second mob resolves no more"
    );
    assert!(
        tail.as_ref()
            .is_some_and(tokio::task::AbortHandle::is_finished),
        "the tail ended with the entry"
    );
    shut_down(&host, &[]).await;
}

/// A member of the second mob whose profile has the WorkGraph tools is
/// built with them over its own mob's realm: the item its tool call creates
/// is in the second mob's realm of the launch's store, and the host's list
/// for the second mob shows it while the launch mob's does not.
#[tokio::test]
async fn a_hosted_mobs_member_with_workgraph_tools_writes_where_its_host_reads() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (host, _launch_id, second_id) = host_and_second_mob(&temp).await;
    let seated = {
        let mob = host
            .resolve_mob(Some(&second_id))
            .await
            .expect("the second mob of the host resolves");
        Box::pin(
            mob.spawn(SpawnMemberSpec::new("worker", "desk").with_initial_message(WRITE_AN_ITEM)),
        )
        .await
    };
    assert!(
        seated.is_ok(),
        "a member with the WorkGraph tools is built in the second mob: {:?}",
        seated.as_ref().err()
    );

    // The realm the member's tools write to, read through the launch's own
    // service scoped as the mob builder scopes it.
    let of_the_launch = host
        .workgraph_service()
        .expect("the launch has a WorkGraph service");
    let where_the_member_writes =
        meerkat_mob::mob_scoped_workgraph_service(&of_the_launch, &second_id)
            .expect("the second mob has a realm")
            .expect("the launch's store has a WorkGraph");
    let mut written = String::new();
    for _ in 0..300 {
        written = listed(&where_the_member_writes).await;
        if written.contains(WRITTEN_BY_THE_MEMBER) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        written.contains(WRITTEN_BY_THE_MEMBER),
        "the member's tool call creates its item in the second mob's realm: {written}"
    );
    assert!(
        !listed(&of_the_launch).await.contains(WRITTEN_BY_THE_MEMBER),
        "and not in the launch mob's"
    );

    let of_the_second = stdio(
        &host,
        "meta/workgraph/list",
        naming(json!({}), Some(&second_id)),
    )
    .await;
    assert!(
        served(&of_the_second) && of_the_second.to_string().contains(WRITTEN_BY_THE_MEMBER),
        "the host's list for the second mob shows the item its member wrote: {of_the_second}"
    );
    let of_the_launch_mob = stdio(&host, "meta/workgraph/list", json!({})).await;
    assert!(
        served(&of_the_launch_mob)
            && !of_the_launch_mob
                .to_string()
                .contains(WRITTEN_BY_THE_MEMBER),
        "the host's list for the launch mob does not: {of_the_launch_mob}"
    );
    shut_down(&host, &[&second_id]).await;
}

/// Beside a second mob the launch mob's WorkGraph is what it was: the
/// service the launch scoped to its own mob, the admission and the fact hub
/// of the launch's own wrapper, for a request without an id and for one
/// with the launch mob's id, and an item of the launch mob in the launch
/// mob's list.
#[tokio::test]
async fn the_launch_mobs_workgraph_is_unchanged_beside_a_hosted_mob() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (host, launch_id, second_id) = host_and_second_mob(&temp).await;
    let of_the_launch = host
        .workgraph_service()
        .expect("the launch has a WorkGraph service");
    assert_eq!(of_the_launch.default_realm_id(), realm_of(&launch_id));
    for named in [None, Some(&launch_id)] {
        let mob = host
            .resolve_mob(named)
            .await
            .expect("the launch mob resolves");
        assert!(mob.is_default());
        assert_eq!(
            mob.workgraph_service()
                .map(|service| service.default_realm_id().to_string()),
            Some(realm_of(&launch_id))
        );
        assert!(
            Arc::ptr_eq(
                &mob.workgraph_admission(),
                &host.mob_runtime().workgraph_admission()
            ),
            "the launch mob's admission is its wrapper's"
        );
        assert!(mob.workgraph_fact_hub().is_some());
    }
    let created = stdio(
        &host,
        "meta/workgraph/create",
        json!({ "title": IN_THE_LAUNCH }),
    )
    .await;
    assert!(served(&created), "{created}");
    assert!(
        listed(&of_the_launch).await.contains(IN_THE_LAUNCH),
        "an item made without an id is in the launch mob's realm"
    );
    let capabilities = stdio(&host, "meta/capabilities", json!({})).await;
    assert_eq!(capabilities["result"]["workgraph"], json!(true));
    shut_down(&host, &[&second_id]).await;
}

/// The words the refusal of a reassign or an escalation holds for a member
/// of a mob the gateway hosts beside its launch mob.
const NOT_FOR_A_HOSTED_MOBS_MEMBER: &str =
    "is not available to a member of a mob this gateway hosts beside its launch mob";

/// A host whose members' model is `model`, serving the mobs its set
/// builds, and a second mob of it.
async fn host_and_second_mob_with(
    temp: &tempfile::TempDir,
    model: &Arc<CallMaker>,
) -> (UnifiedRuntime, MobId) {
    let members_model: Arc<CallMaker> = Arc::clone(model);
    let host = launch_with(temp, definition("host"), members_model).await;
    let second_id = second_mob(&host, definition("second")).await;
    (host, second_id)
}

/// A host of one mob whose members' model is `model`, with its mob's id.
async fn host_of_one_mob_with(
    temp: &tempfile::TempDir,
    model: &Arc<CallMaker>,
) -> (UnifiedRuntime, MobId) {
    let members_model: Arc<CallMaker> = Arc::clone(model);
    let host = launch_with(temp, definition("host"), members_model).await;
    let launch_id = host.mob_handle().mob_id().clone();
    (host, launch_id)
}

/// Seat the member `name` of the mob `mob_id` on the fixture's profile,
/// which has the WorkGraph tools, with `labels`.
async fn seated(
    host: &UnifiedRuntime,
    mob_id: &MobId,
    name: &str,
    labels: std::collections::BTreeMap<String, String>,
) {
    let seated = {
        let mob = host
            .resolve_mob(Some(mob_id))
            .await
            .expect("the mob of the host resolves");
        Box::pin(
            mob.mob_handle()
                .spawn_spec(SpawnMemberSpec::new("worker", name).with_labels(labels)),
        )
        .await
    };
    assert!(
        seated.is_ok(),
        "the member {name} is seated in {mob_id}: {:?}",
        seated.as_ref().err()
    );
}

/// The labels of a member private to a person.
fn private_to_a_person() -> std::collections::BTreeMap<String, String> {
    std::collections::BTreeMap::from([(
        crate::access::PRIVATE_TO_LABEL.to_string(),
        "console-caller:v1:someone".to_string(),
    )])
}

/// The attention bindings of the mob `mob_id` whose target names its
/// member `member`, as the host's attention list for that mob shows them.
async fn bindings_naming(host: &UnifiedRuntime, mob_id: &MobId, member: &str) -> Vec<Value> {
    let listed = stdio(
        host,
        "meta/workgraph/attention/list",
        naming(json!({}), Some(mob_id)),
    )
    .await;
    assert!(served(&listed), "the attention list of {mob_id}: {listed}");
    let named = format!("mob/{mob_id}/agent/{member}");
    listed["result"]["attention"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|binding| binding["target"].to_string().contains(&named))
        .collect()
}

/// A goal of the mob `mob_id` for its member `member`, made on the host's
/// door, and its binding paused: the binding's id.
async fn a_paused_goal_of(host: &UnifiedRuntime, mob_id: &MobId, member: &str) -> Value {
    let held = stdio(
        host,
        "meta/workgraph/goal/create",
        naming(
            json!({
                "title": format!("a goal of the {member}"),
                "target": {"kind": "identity", "identity": member},
            }),
            Some(mob_id),
        ),
    )
    .await;
    assert!(served(&held), "a goal of the {member}: {held}");
    let attention = &held["result"]["attention"];
    let paused = stdio(
        host,
        "meta/workgraph/attention/pause",
        naming(
            json!({
                "binding_id": attention["binding_id"],
                "expected_revision": attention["machine_state"]["revision"],
            }),
            Some(mob_id),
        ),
    )
    .await;
    assert!(
        served(&paused),
        "the {member}'s binding is paused: {paused}"
    );
    attention["binding_id"].clone()
}

/// Have the member `desk` of the mob `mob_id`, under a goal that lets it
/// coordinate, reassign that goal's attention to the member `to` of its own
/// mob with its own WorkGraph tools: the goal is made on the host's door,
/// the model's one call is queued, and the member is seated with the
/// message that has its model make the call, in its own session. Answers
/// what the model was shown after the call, and the goal's binding.
async fn the_desk_reassigns_its_goal_to(
    host: &UnifiedRuntime,
    model: &CallMaker,
    mob_id: &MobId,
    to: &str,
) -> (String, Value) {
    the_member_reassigns_its_goal_to(host, model, mob_id, "desk", to).await
}

/// [`the_desk_reassigns_its_goal_to`], for the member `member` of the mob,
/// which the mob has not seated yet.
async fn the_member_reassigns_its_goal_to(
    host: &UnifiedRuntime,
    model: &CallMaker,
    mob_id: &MobId,
    member: &str,
    to: &str,
) -> (String, Value) {
    let goal = stdio(
        host,
        "meta/workgraph/goal/create",
        naming(
            json!({
                "title": format!("a goal the {member} coordinates"),
                "target": {"kind": "identity", "identity": member},
                "mode": "coordinate",
            }),
            Some(mob_id),
        ),
    )
    .await;
    assert!(served(&goal), "a goal the {member} coordinates: {goal}");
    let attention = &goal["result"]["attention"];
    model.queue(
        "workgraph_attention_reassign",
        json!({
            "binding_id": attention["binding_id"],
            "expected_revision": attention["machine_state"]["revision"],
            "target": {
                "kind": "owner",
                "owner_key": {"kind": "agent", "id": format!("mob/{mob_id}/agent/{to}")},
            },
        }),
    );
    let seated = {
        let mob = host
            .resolve_mob(Some(mob_id))
            .await
            .expect("the mob of the host resolves");
        Box::pin(
            mob.spawn(SpawnMemberSpec::new("worker", member).with_initial_message(MAKE_THE_CALL)),
        )
        .await
    };
    assert!(
        seated.is_ok(),
        "the {member} is seated with the WorkGraph tools: {:?}",
        seated.as_ref().err()
    );
    for _ in 0..600 {
        if let Some(shown) = model.shown() {
            return (shown, attention["binding_id"].clone());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("the call of the {member} was not answered within a minute");
}

/// On a host of one mob, a member of the launch mob, under a goal that lets
/// it coordinate, moves that goal's attention to a seated free member of
/// its mob with its own WorkGraph tools: the call carries the goal's
/// attention into the member's session, passes the mob's admission and
/// reaches the store. The two rows after it stand on this one: what they
/// show refused is refused by the admission, not for want of the call's
/// standing.
#[tokio::test]
async fn a_launch_mobs_member_moves_its_goal_to_a_free_member_of_its_mob() {
    let temp = tempfile::tempdir().expect("tempdir");
    let model = Arc::new(CallMaker::default());
    let (host, launch_id) = host_of_one_mob_with(&temp, &model).await;
    seated(&host, &launch_id, "bench", Default::default()).await;

    let (shown, _) = the_desk_reassigns_its_goal_to(&host, &model, &launch_id, "bench").await;
    let on_the_bench = bindings_naming(&host, &launch_id, "bench").await;
    assert_eq!(
        on_the_bench.len(),
        1,
        "the member's own call moves its goal's attention to the free member: the bindings \
         that name it are {on_the_bench:?}; the model was shown {shown}"
    );
    shut_down(&host, &[]).await;
}

/// On a host of one mob, a member of the launch mob binds no WorkGraph work
/// to a member of its mob that is private to a person: its own reassign of
/// a goal it coordinates onto such a member is answered the tools' one
/// denial for a key that names no member work may be bound to, and no
/// binding names that member.
#[tokio::test]
async fn a_launch_mobs_member_binds_no_work_to_a_member_private_to_a_person() {
    let temp = tempfile::tempdir().expect("tempdir");
    let model = Arc::new(CallMaker::default());
    let (host, launch_id) = host_of_one_mob_with(&temp, &model).await;
    seated(&host, &launch_id, "private-helper", private_to_a_person()).await;

    let (shown, _) =
        the_desk_reassigns_its_goal_to(&host, &model, &launch_id, "private-helper").await;
    let mut wrong = Vec::new();
    let on_the_private_member = bindings_naming(&host, &launch_id, "private-helper").await;
    if !on_the_private_member.is_empty() {
        wrong.push(format!(
            "a binding names the member private to a person: {on_the_private_member:?}"
        ));
    }
    if !shown.contains("workgraph_member_not_bindable") {
        wrong.push(format!(
            "the member's call is not answered the tools' denial: {shown}"
        ));
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    shut_down(&host, &[]).await;
}

/// On a host of one mob, a member of the launch mob lands no second binding
/// on a seated member of its mob whose binding is paused: its own reassign
/// of a goal it coordinates onto that member is answered the admission's
/// conflict, and the paused binding stays the only one that names the
/// member.
#[tokio::test]
async fn a_launch_mobs_member_lands_no_second_binding_on_a_member_whose_binding_is_paused() {
    let temp = tempfile::tempdir().expect("tempdir");
    let model = Arc::new(CallMaker::default());
    let (host, launch_id) = host_of_one_mob_with(&temp, &model).await;
    seated(&host, &launch_id, "bench", Default::default()).await;
    let paused = a_paused_goal_of(&host, &launch_id, "bench").await;

    let (shown, _) = the_desk_reassigns_its_goal_to(&host, &model, &launch_id, "bench").await;
    let mut wrong = Vec::new();
    let on_the_bench = bindings_naming(&host, &launch_id, "bench").await;
    let only_the_paused_one = on_the_bench.len() == 1
        && on_the_bench
            .first()
            .is_some_and(|binding| binding["binding_id"] == paused);
    if !only_the_paused_one {
        wrong.push(format!(
            "the paused binding is not the only one that names the bench: {on_the_bench:?}"
        ));
    }
    if !shown.contains("workgraph conflict") {
        wrong.push(format!(
            "the member's call is not answered the admission's conflict: {shown}"
        ));
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    shut_down(&host, &[]).await;
}

/// A member of a mob the gateway hosts beside its launch mob is refused the
/// reassign of a goal it coordinates, onto a seated free member of its own
/// mob: the model is shown the refusal, no binding names that member, and
/// the goal's binding is the desk's as it was. The host then makes the same
/// change for that mob with its own method.
#[tokio::test]
async fn a_hosted_mobs_member_is_refused_a_reassign_and_the_host_makes_it() {
    let temp = tempfile::tempdir().expect("tempdir");
    let model = Arc::new(CallMaker::default());
    let (host, second_id) = host_and_second_mob_with(&temp, &model).await;
    seated(&host, &second_id, "bench", Default::default()).await;

    let (shown, binding) = the_desk_reassigns_its_goal_to(&host, &model, &second_id, "bench").await;
    let mut wrong = Vec::new();
    if !shown.contains(NOT_FOR_A_HOSTED_MOBS_MEMBER) {
        wrong.push(format!("the member's call is not refused: {shown}"));
    }
    let on_the_bench = bindings_naming(&host, &second_id, "bench").await;
    if !on_the_bench.is_empty() {
        wrong.push(format!(
            "a binding names the bench after the member's call: {on_the_bench:?}"
        ));
    }
    let of_the_desk = bindings_naming(&host, &second_id, "desk").await;
    if !of_the_desk.iter().any(|held| held["binding_id"] == binding) {
        wrong.push(format!(
            "the goal's binding is not the desk's any more: {of_the_desk:?}"
        ));
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));

    // The host makes the change for the mob: the binding as it stands now,
    // then the reassign on the host's door. The member's turn may still be
    // ending, so the pair is asked again while the door answers a conflict.
    let mut by_the_host = Value::Null;
    for _ in 0..50 {
        let status = stdio(
            &host,
            "meta/workgraph/goal/status",
            naming(json!({ "binding_id": binding }), Some(&second_id)),
        )
        .await;
        assert!(served(&status), "the goal's status: {status}");
        by_the_host = stdio(
            &host,
            "meta/workgraph/attention/reassign",
            naming(
                json!({
                    "binding_id": binding,
                    "expected_revision":
                        status["result"]["attention"]["machine_state"]["revision"],
                    "target": {"kind": "identity", "identity": "bench"},
                }),
                Some(&second_id),
            ),
        )
        .await;
        if served(&by_the_host) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        served(&by_the_host),
        "the host reassigns the goal of the second mob: {by_the_host}"
    );
    assert_eq!(
        bindings_naming(&host, &second_id, "bench").await.len(),
        1,
        "the host's reassign moves the goal's attention to the bench"
    );
    shut_down(&host, &[&second_id]).await;
}

/// A member of a mob the gateway hosts beside its launch mob binds no
/// WorkGraph work to a member of its mob that is private to a person: its
/// own reassign of a goal it coordinates onto such a member is refused, and
/// no binding names that member.
#[tokio::test]
async fn a_hosted_mobs_member_binds_no_work_to_a_member_private_to_a_person() {
    let temp = tempfile::tempdir().expect("tempdir");
    let model = Arc::new(CallMaker::default());
    let (host, second_id) = host_and_second_mob_with(&temp, &model).await;
    seated(&host, &second_id, "private-helper", private_to_a_person()).await;

    let (shown, _) =
        the_desk_reassigns_its_goal_to(&host, &model, &second_id, "private-helper").await;
    let mut wrong = Vec::new();
    let on_the_private_member = bindings_naming(&host, &second_id, "private-helper").await;
    if !on_the_private_member.is_empty() {
        wrong.push(format!(
            "a binding names the member private to a person: {on_the_private_member:?}"
        ));
    }
    if !shown.contains(NOT_FOR_A_HOSTED_MOBS_MEMBER) {
        wrong.push(format!("the member's call is not refused: {shown}"));
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    shut_down(&host, &[&second_id]).await;
}

/// A member of a mob the gateway hosts beside its launch mob lands no
/// second binding on a seated member of its mob whose binding is paused:
/// its own reassign of a goal it coordinates onto that member is refused,
/// and the paused binding stays the only one that names the member.
#[tokio::test]
async fn a_hosted_mobs_member_lands_no_second_binding_on_a_member_whose_binding_is_paused() {
    let temp = tempfile::tempdir().expect("tempdir");
    let model = Arc::new(CallMaker::default());
    let (host, second_id) = host_and_second_mob_with(&temp, &model).await;
    seated(&host, &second_id, "bench", Default::default()).await;
    let paused = a_paused_goal_of(&host, &second_id, "bench").await;

    let (shown, _) = the_desk_reassigns_its_goal_to(&host, &model, &second_id, "bench").await;
    let mut wrong = Vec::new();
    let on_the_bench = bindings_naming(&host, &second_id, "bench").await;
    let only_the_paused_one = on_the_bench.len() == 1
        && on_the_bench
            .first()
            .is_some_and(|binding| binding["binding_id"] == paused);
    if !only_the_paused_one {
        wrong.push(format!(
            "the paused binding is not the only one that names the bench: {on_the_bench:?}"
        ));
    }
    if !shown.contains(NOT_FOR_A_HOSTED_MOBS_MEMBER) {
        wrong.push(format!("the member's call is not refused: {shown}"));
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    shut_down(&host, &[&second_id]).await;
}

/// A launch over the state directory of `stored`, as the fixture of the
/// rows for a host's stored mobs makes it: its mob set unloads an idle mob
/// and loads a stored mob when something addresses it. With a WorkGraph
/// composed as a gateway composes it over that directory for the launch mob
/// `launch_id`, and `model` as its members' model.
async fn stored_launch_with(
    stored: &StoredHost,
    launch_id: &MobId,
    model: &Arc<CallMaker>,
) -> UnifiedRuntime {
    stored_launch_and_machine_with(stored, launch_id, model)
        .await
        .0
}

/// [`stored_launch_with`], with the launch's runtime machine: every mob of
/// the host registers its members' sessions on it.
async fn stored_launch_and_machine_with(
    stored: &StoredHost,
    launch_id: &MobId,
    model: &Arc<CallMaker>,
) -> (UnifiedRuntime, Arc<meerkat_runtime::MeerkatMachine>) {
    let workgraph =
        crate::workgraph_wiring::open_workgraph_service(stored.state_dir(), launch_id.as_str())
            .expect("the WorkGraph store opens under the state directory");
    let (spec, _sessions, machine, admission_slot) = stored
        .spec_over(|builder| crate::workgraph_wiring::install_workgraph_tools(builder, &workgraph));
    let members_model: Arc<CallMaker> = Arc::clone(model);
    let members_model: Arc<dyn LlmClient> = members_model;
    let host = stored
        .bootstrap(
            spec.with_workgraph_service(Some(workgraph))
                .with_workgraph_admission_slot(admission_slot)
                .with_workgraph_admission_sidecar(stored.state_dir())
                .with_options(MobBootstrapOptions {
                    allow_ephemeral_sessions: false,
                    notify_orchestrator_on_resume: true,
                    default_llm_client: Some(members_model),
                }),
        )
        .await;
    (host, machine)
}

/// Have `host`'s mob set unload its idle mobs until the mob `mob_id` is
/// known to it and not loaded. A member that is still being seated keeps its
/// mob loaded for a pass, so the pass is asked again.
async fn put_to_sleep(host: &UnifiedRuntime, mob_id: &MobId) {
    for _ in 0..300 {
        set_of(host).unload_idle_mobs_for_tests().await;
        let now = known(host).await;
        if now.not_loaded.contains(mob_id) && !now.loaded.contains(mob_id) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("the mob set did not unload the idle mob {mob_id} within half a minute");
}

/// What is wrong, `when`, after a member's reassign onto the bench of the
/// mob `mob_id` that showed its model `shown`: the call was not refused, or
/// a binding names the bench.
async fn not_refused(
    host: &UnifiedRuntime,
    mob_id: &MobId,
    shown: &str,
    when: &str,
) -> Vec<String> {
    let mut wrong = Vec::new();
    if !shown.contains(NOT_FOR_A_HOSTED_MOBS_MEMBER) {
        wrong.push(format!("{when}, the member's call is not refused: {shown}"));
    }
    let on_the_bench = bindings_naming(host, mob_id, "bench").await;
    if !on_the_bench.is_empty() {
        wrong.push(format!(
            "{when}, a binding names the bench after the member's call: {on_the_bench:?}"
        ));
    }
    wrong
}

/// A member of a mob the gateway hosts beside its launch mob is refused the
/// reassign of a goal it coordinates also in a mob the gateway loaded from
/// its store. After the mob set unloaded the idle mob, the request that
/// makes the goal loads it again, the member's model is shown the refusal,
/// and no binding names the member the call named. The same holds after a
/// restart of the gateway over the same state directory, which knows the
/// mob from its store alone.
#[tokio::test]
async fn a_hosted_mobs_member_is_refused_a_reassign_after_an_unload_and_after_a_restart() {
    let launch_definition = definition("stored-host");
    let launch_id = launch_definition.id.clone();
    let stored = StoredHost::of(launch_definition);
    let model = Arc::new(CallMaker::default());
    let host = stored_launch_with(&stored, &launch_id, &model).await;
    let second_id = second_mob(&host, definition("second")).await;
    set_of(&host)
        .mob_spawn(
            &second_id,
            meerkat_mob::ProfileName::from("worker"),
            meerkat_mob::AgentIdentity::from("bench"),
            Some(meerkat_mob::MobRuntimeMode::TurnDriven),
            None,
            None,
        )
        .await
        .expect("the second mob seats its bench");

    put_to_sleep(&host, &second_id).await;
    let (shown, _) =
        the_member_reassigns_its_goal_to(&host, &model, &second_id, "desk", "bench").await;
    let wrong = not_refused(&host, &second_id, &shown, "after the unload").await;
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    shut_down(&host, &[]).await;
    drop(host);

    // The gateway starts again over the same state directory. Its second
    // model's call has an id of its own: the sessions of the first launch
    // hold the first model's call.
    let model = Arc::new(CallMaker::making("call-made-after-the-restart"));
    let host = stored_launch_with(&stored, &launch_id, &model).await;
    let at_start = known(&host).await;
    assert!(
        at_start.not_loaded.contains(&second_id) || at_start.loaded.contains(&second_id),
        "the restarted gateway knows the second mob from its store: it manages {:?} and knows \
         as not loaded {:?}",
        at_start.loaded,
        at_start.not_loaded
    );
    let (shown, _) =
        the_member_reassigns_its_goal_to(&host, &model, &second_id, "night-desk", "bench").await;
    let wrong = not_refused(&host, &second_id, &shown, "after the restart").await;
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    shut_down(&host, &[&second_id]).await;
}

/// The item of the goal behind the binding `binding` of the mob `mob_id`,
/// as the host's goal status for that mob shows it.
async fn goal_item_of(host: &UnifiedRuntime, mob_id: &MobId, binding: &Value) -> Value {
    let status = stdio(
        host,
        "meta/workgraph/goal/status",
        naming(json!({ "binding_id": binding }), Some(mob_id)),
    )
    .await;
    assert!(served(&status), "the goal's status in {mob_id}: {status}");
    status["result"]["item"].clone()
}

/// The host tightens the completion policy of a goal of a mob it hosts
/// beside its launch mob. The member the goal is for is refused that
/// escalation with its own WorkGraph tools, and the goal's policy stays
/// what it was. The same request on the host's door without the mob's id is
/// for the launch mob, which has no such goal, and changes nothing. The
/// host's `meta/workgraph/policy/escalate` with the mob's id then makes the
/// change, and the mob's own goal shows it.
#[tokio::test]
async fn the_host_escalates_a_goal_of_a_hosted_mob_after_its_member_was_refused() {
    let temp = tempfile::tempdir().expect("tempdir");
    let model = Arc::new(CallMaker::default());
    let (host, second_id) = host_and_second_mob_with(&temp, &model).await;

    let goal = stdio(
        &host,
        "meta/workgraph/goal/create",
        naming(
            json!({
                "title": "a goal the host tightens",
                "target": {"kind": "identity", "identity": "desk"},
            }),
            Some(&second_id),
        ),
    )
    .await;
    assert!(
        served(&goal),
        "a goal for the desk of the second mob: {goal}"
    );
    let binding = goal["result"]["attention"]["binding_id"].clone();
    let item_id = goal["result"]["item"]["id"].clone();
    let host_confirmed = json!("host_confirmed");
    assert_ne!(
        goal["result"]["item"]["completion_policy"]["kind"], host_confirmed,
        "the goal starts under a policy the host can tighten: {goal}"
    );

    // The member's own escalation, in its own session.
    model.queue(
        "workgraph_policy_escalate",
        json!({
            "id": item_id,
            "expected_revision": goal["result"]["item"]["revision"],
            "completion_policy": {"kind": "host_confirmed"},
        }),
    );
    let seated = {
        let mob = host
            .resolve_mob(Some(&second_id))
            .await
            .expect("the mob of the host resolves");
        Box::pin(
            mob.spawn(SpawnMemberSpec::new("worker", "desk").with_initial_message(MAKE_THE_CALL)),
        )
        .await
    };
    assert!(
        seated.is_ok(),
        "the desk is seated with the WorkGraph tools: {:?}",
        seated.as_ref().err()
    );
    let mut shown = None;
    for _ in 0..600 {
        shown = model.shown();
        if shown.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let shown = shown.expect("the desk's call is answered within a minute");
    let mut wrong = Vec::new();
    if !shown.contains(&format!(
        "workgraph_policy_escalate {NOT_FOR_A_HOSTED_MOBS_MEMBER}"
    )) {
        wrong.push(format!("the member's escalation is not refused: {shown}"));
    }
    let after_the_member = goal_item_of(&host, &second_id, &binding).await;
    if after_the_member["completion_policy"]["kind"] == host_confirmed {
        wrong.push(format!(
            "the member's own call changed the goal's policy: {after_the_member}"
        ));
    }

    // The same request without the mob's id is for the launch mob, whose
    // WorkGraph realm does not hold the second mob's binding.
    let launch_id = host.mob_handle().mob_id().clone();
    let for_the_launch_mob = stdio(
        &host,
        "meta/workgraph/policy/escalate",
        json!({
            "binding_id": binding,
            "id": item_id,
            "expected_revision": after_the_member["revision"],
            "completion_policy": {"kind": "host_confirmed"},
        }),
    )
    .await;
    let no_such_binding_in_the_launch_mob =
        format!("not found in realm '{}'", realm_of(&launch_id));
    if served(&for_the_launch_mob)
        || !for_the_launch_mob["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains(&no_such_binding_in_the_launch_mob))
    {
        wrong.push(format!(
            "the request without the mob's id is not answered that the launch mob's realm has \
             no such binding: {for_the_launch_mob}"
        ));
    }
    let after_the_launch_mobs_request = goal_item_of(&host, &second_id, &binding).await;
    if after_the_launch_mobs_request["completion_policy"]["kind"] == host_confirmed {
        wrong.push(format!(
            "the request without the mob's id changed the second mob's goal: \
             {after_the_launch_mobs_request}"
        ));
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));

    // The host makes the change for the mob. The member's turn may still be
    // ending, so a request that is not served is made again with the item
    // as it stands, whatever the door answered, for at most fifty requests;
    // the last answer is the one the row judges.
    let mut by_the_host = Value::Null;
    for _ in 0..50 {
        let now = goal_item_of(&host, &second_id, &binding).await;
        by_the_host = stdio(
            &host,
            "meta/workgraph/policy/escalate",
            naming(
                json!({
                    "binding_id": binding,
                    "id": item_id,
                    "expected_revision": now["revision"],
                    "completion_policy": {"kind": "host_confirmed"},
                }),
                Some(&second_id),
            ),
        )
        .await;
        if served(&by_the_host) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        served(&by_the_host),
        "the host escalates the goal of the second mob: {by_the_host}"
    );
    assert_eq!(
        by_the_host["result"]["item"]["completion_policy"]["kind"], host_confirmed,
        "the host's answer carries the tightened policy: {by_the_host}"
    );
    let in_the_mob = goal_item_of(&host, &second_id, &binding).await;
    assert_eq!(
        in_the_mob["completion_policy"]["kind"], host_confirmed,
        "the second mob's goal is under the tightened policy: {in_the_mob}"
    );
    shut_down(&host, &[&second_id]).await;
}

/// A definition with a durable profile, `worker`, and an ordinary one,
/// `helper`, under an id of its own. Neither has the WorkGraph tools: the
/// hosts of the row that uses it differ in whether they have a WorkGraph at
/// all, and a member's build is the same on both.
fn durable_definition(tag: &str) -> MobDefinition {
    MobDefinition::from_toml(&format!(
        "[mob]\nid = \"mob-workgraph-durable-{tag}-{}\"\n\n[profiles.worker]\nmodel = \"gpt-5.5\"\ndurable = true\n\n[profiles.worker.tools]\ncomms = true\n\n[profiles.helper]\nmodel = \"gpt-5.5\"\n\n[profiles.helper.tools]\ncomms = true\n",
        uuid::Uuid::new_v4().simple()
    ))
    .expect("a definition with a durable profile and an ordinary one")
}

/// Seat the turn-driven member `name` of the mob `mob_id` on `profile`
/// through the host's mob set, and answer the session it was built on.
async fn seated_on(
    host: &UnifiedRuntime,
    mob_id: &MobId,
    profile: &str,
    name: &str,
) -> meerkat_core::types::SessionId {
    let set = set_of(host);
    let identity = meerkat_mob::AgentIdentity::from(name);
    set.mob_spawn(
        mob_id,
        meerkat_mob::ProfileName::from(profile),
        identity.clone(),
        Some(meerkat_mob::MobRuntimeMode::TurnDriven),
        None,
        None,
    )
    .await
    .expect("the mob seats the member");
    set.handle_for(mob_id)
        .await
        .expect("the mob of the host")
        .resolve_bridge_session_id(&identity)
        .await
        .expect("the member has a session")
}

/// Whether the registration of `session` on the launch's runtime `machine`
/// holds a resumed-run admission: what a mob binds there for a durable
/// member when the mob was built with a resume gate, and what the runtime
/// asks before it resumes that member's interrupted run.
async fn holds_a_resume_admission(
    machine: &meerkat_runtime::MeerkatMachine,
    session: &meerkat_core::types::SessionId,
) -> bool {
    let registration = machine
        .current_session_registration_witness(session)
        .await
        .expect("the member's session is registered on the launch's runtime machine");
    machine
        .resumed_run_admission_bound(&registration)
        .await
        .expect("read the registration's admission")
}

/// A durable member of a mob the gateway hosts beside its launch mob is
/// built with a resume gate: on a host with a WorkGraph, the launch builds
/// each mob it hosts with the gate of that mob, so the registration of the
/// hosted mob's durable member holds a resumed-run admission, and the
/// registration of an ordinary member of the same mob holds none. On a host
/// without a WorkGraph there is no claim to check: the same durable member
/// is built with no admission, and its run would resume unchecked.
#[tokio::test]
async fn a_hosted_mobs_durable_member_is_built_with_its_mobs_resume_gate() {
    let mut wrong = Vec::new();

    let launch_definition = durable_definition("host");
    let launch_id = launch_definition.id.clone();
    let stored = StoredHost::of(launch_definition);
    let model = Arc::new(CallMaker::default());
    let (host, machine) = stored_launch_and_machine_with(&stored, &launch_id, &model).await;
    let second_id = second_mob(&host, durable_definition("second")).await;
    let durable = seated_on(&host, &second_id, "worker", "keeper").await;
    let ordinary = seated_on(&host, &second_id, "helper", "runner").await;
    if !holds_a_resume_admission(&machine, &durable).await {
        wrong.push(
            "on a host with a WorkGraph, the hosted mob's durable member holds no resumed-run \
             admission",
        );
    }
    if holds_a_resume_admission(&machine, &ordinary).await {
        wrong.push(
            "on a host with a WorkGraph, an ordinary member of the hosted mob holds a \
             resumed-run admission",
        );
    }
    shut_down(&host, &[&second_id]).await;
    drop(host);

    let bare = StoredHost::of(durable_definition("bare-host"));
    let (bare_host, _, bare_machine, ()) = bare.launch_over(|_| ()).await;
    let bare_second = second_mob(&bare_host, durable_definition("bare-second")).await;
    let bare_durable = seated_on(&bare_host, &bare_second, "worker", "keeper").await;
    if holds_a_resume_admission(&bare_machine, &bare_durable).await {
        wrong.push(
            "on a host without a WorkGraph, the hosted mob's durable member holds a \
             resumed-run admission",
        );
    }
    shut_down(&bare_host, &[&bare_second]).await;

    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// A goal of the mob `mob_id` made on the host's door for the name `member`,
/// which no member of that mob holds yet: the host may bind such a name.
async fn a_goal_for_the_name(host: &UnifiedRuntime, mob_id: &MobId, member: &str) {
    let made = stdio(
        host,
        "meta/workgraph/goal/create",
        naming(
            json!({
                "title": format!("a goal for the name {member}"),
                "target": {"kind": "identity", "identity": member},
            }),
            Some(mob_id),
        ),
    )
    .await;
    assert!(
        served(&made),
        "a goal for the name {member} in {mob_id}: {made}"
    );
}

/// The one attention binding of the mob `mob_id` that names its member
/// `member`, as the host's attention list for that mob shows it.
async fn the_binding_naming(host: &UnifiedRuntime, mob_id: &MobId, member: &str) -> Value {
    let mut named = bindings_naming(host, mob_id, member).await;
    assert_eq!(
        named.len(),
        1,
        "one binding names {member} in {mob_id}: {named:?}"
    );
    named.remove(0)
}

/// Whether the mob `mob_id` of `host` holds the member `member` with the
/// label of a member private to a person.
async fn holds_as_private(host: &UnifiedRuntime, mob_id: &MobId, member: &str) -> bool {
    let entry = host
        .resolve_mob(Some(mob_id))
        .await
        .expect("the mob of the host resolves")
        .mob_handle()
        .get_member(&meerkat_mob::AgentIdentity::from(member))
        .await
        .expect("the roster is read");
    entry.is_some_and(|entry| crate::access::private_owner(&entry.labels).is_some())
}

/// The host may bind a goal to a name of a mob it hosts beside its launch
/// mob before a member holds that name. When the member then built under
/// that name is private to a person, the binding does not stay active: the
/// member's build holds it paused, with no deadline, as the build of a
/// launch mob's member does.
#[tokio::test]
async fn a_hosted_mobs_private_member_build_holds_a_binding_made_before_it() {
    let temp = tempfile::tempdir().expect("tempdir");
    let model = Arc::new(CallMaker::default());
    let (host, second_id) = host_and_second_mob_with(&temp, &model).await;
    a_goal_for_the_name(&host, &second_id, "later-helper").await;
    let before = the_binding_naming(&host, &second_id, "later-helper").await;
    assert_eq!(
        before["status"]["state"],
        json!("active"),
        "the host binds a name no member of the hosted mob holds yet: {before}"
    );

    seated(&host, &second_id, "later-helper", private_to_a_person()).await;
    assert!(
        holds_as_private(&host, &second_id, "later-helper").await,
        "the hosted mob holds the member, private to a person"
    );

    let held = the_binding_naming(&host, &second_id, "later-helper").await;
    assert_eq!(
        held["status"]["state"],
        json!("paused"),
        "the build of a hosted mob's member private to a person holds a binding that names it: \
         {held}"
    );
    assert!(
        held["status"]["until"].is_null(),
        "the hold has no deadline: {held}"
    );
    shut_down(&host, &[&second_id]).await;
}

/// The same order in the launch mob of the same fixture: the build of a
/// launch mob's member private to a person holds a binding the host made
/// onto its name before, with a second mob hosted beside it or without.
#[tokio::test]
async fn a_launch_mobs_private_member_build_holds_its_own_binding_beside_a_hosted_mob() {
    let temp = tempfile::tempdir().expect("tempdir");
    let model = Arc::new(CallMaker::default());
    let members_model: Arc<CallMaker> = Arc::clone(&model);
    let host = launch_with(&temp, definition("host"), members_model).await;
    let launch_id = host.mob_handle().mob_id().clone();
    let second_id = second_mob(&host, definition("second")).await;
    a_goal_for_the_name(&host, &launch_id, "later-helper").await;
    // A goal of the second mob for the same name: the launch mob's build
    // holds its own mob's binding and leaves this one.
    a_goal_for_the_name(&host, &second_id, "later-helper").await;

    seated(&host, &launch_id, "later-helper", private_to_a_person()).await;

    let held = the_binding_naming(&host, &launch_id, "later-helper").await;
    assert_eq!(
        held["status"]["state"],
        json!("paused"),
        "the build of a launch mob's member private to a person holds a binding that names it: \
         {held}"
    );
    assert!(
        held["status"]["until"].is_null(),
        "the hold has no deadline: {held}"
    );
    let of_the_second = the_binding_naming(&host, &second_id, "later-helper").await;
    assert_eq!(
        of_the_second["status"]["state"],
        json!("active"),
        "the second mob's binding onto the same name, which no member of it holds, is left as \
         it is: {of_the_second}"
    );
    shut_down(&host, &[&second_id]).await;
}

/// A mob hosted beside the launch mob with no member private to a person
/// and no binding makes no admission lock file: making the mob and seating
/// a shared member leave the state directory without one (its entry takes
/// no admission where no lock file exists yet, and with no binding it holds
/// nothing). And a binding the host made onto the name of a member that is
/// then built shared stays active.
#[tokio::test]
async fn a_hosted_mob_without_a_private_member_takes_no_admission_and_holds_no_binding() {
    let temp = tempfile::tempdir().expect("tempdir");
    let model = Arc::new(CallMaker::default());
    let (host, second_id) = host_and_second_mob_with(&temp, &model).await;
    seated(&host, &second_id, "bench", Default::default()).await;
    // The second mob's entry stands and has answered a request.
    assert!(
        bindings_naming(&host, &second_id, "bench").await.is_empty(),
        "no binding names the bench"
    );
    let sidecar = crate::workgraph_admission::workgraph_admission_sidecar_path(temp.path());
    assert!(
        !sidecar.exists(),
        "a hosted mob with no member private to a person and no binding took no admission: the \
         lock file {} exists",
        sidecar.display()
    );

    a_goal_for_the_name(&host, &second_id, "later-helper").await;
    seated(&host, &second_id, "later-helper", Default::default()).await;
    let left = the_binding_naming(&host, &second_id, "later-helper").await;
    assert_eq!(
        left["status"]["state"],
        json!("active"),
        "the build of a shared member holds no binding: {left}"
    );
    shut_down(&host, &[&second_id]).await;
}

/// A store may hold an active binding whose target is a member private to
/// a person of a mob the gateway hosts beside its launch mob. The gateway
/// does not keep such a binding active past a start: when it starts again
/// over the same state directory and the hosted mob is loaded, the binding
/// is paused, with no deadline, as a start pauses one of the launch mob.
#[tokio::test]
async fn a_start_pauses_a_stored_binding_onto_a_hosted_mobs_private_member() {
    let launch_definition = definition("stored-host");
    let launch_id = launch_definition.id.clone();
    let stored = StoredHost::of(launch_definition);
    let model = Arc::new(CallMaker::default());
    let host = stored_launch_with(&stored, &launch_id, &model).await;
    let second_id = second_mob(&host, definition("second")).await;
    seated(&host, &second_id, "private-helper", private_to_a_person()).await;
    // The WorkGraph itself knows no private member: a binding written to
    // the second mob's realm directly names one.
    let service = host
        .resolve_mob(Some(&second_id))
        .await
        .expect("the second mob of the host resolves")
        .workgraph_service()
        .expect("the second mob has a WorkGraph service");
    let request: meerkat::GoalCreateRequest = serde_json::from_value(json!({
        "title": "a goal bound past the host's methods",
        "target": {
            "kind": "owner",
            "owner_key": {
                "kind": "agent",
                "id": format!("mob/{second_id}/agent/private-helper"),
            },
        },
    }))
    .expect("a goal request");
    service
        .create_goal(request)
        .await
        .expect("the goal is made");
    let before = the_binding_naming(&host, &second_id, "private-helper").await;
    assert_eq!(
        before["status"]["state"],
        json!("active"),
        "the stored binding is active before the restart: {before}"
    );
    shut_down(&host, &[]).await;
    drop(host);

    // The gateway starts again over the same state directory; the request
    // that names the second mob loads it.
    let model = Arc::new(CallMaker::making("call-made-after-the-restart"));
    let host = stored_launch_with(&stored, &launch_id, &model).await;
    assert!(
        holds_as_private(&host, &second_id, "private-helper").await,
        "the restarted gateway's second mob holds the member, private to a person"
    );
    let held = the_binding_naming(&host, &second_id, "private-helper").await;
    assert_eq!(
        held["status"]["state"],
        json!("paused"),
        "a start pauses a stored binding onto a hosted mob's member private to a person: {held}"
    );
    assert!(
        held["status"]["until"].is_null(),
        "the hold has no deadline: {held}"
    );
    shut_down(&host, &[&second_id]).await;
}

/// The TOML of a definition as [`definition`] makes it, under the id
/// `mob_id`: what a host's create takes.
fn config_under(mob_id: &str) -> String {
    format!(
        "[mob]\nid = \"{mob_id}\"\n\n[profiles.worker]\nmodel = \"gpt-5.5\"\n\n[profiles.worker.tools]\ncomms = true\nworkgraph = true\n"
    )
}

/// An id of its own for a mob a row's host creates.
fn an_id(tag: &str) -> String {
    format!("mob-workgraph-{tag}-{}", uuid::Uuid::new_v4().simple())
}

/// The host's own create of a mob under the id `mob_id`: its whole answer.
async fn the_host_creates(host: &UnifiedRuntime, mob_id: &str) -> Value {
    stdio(
        host,
        "meta/mobs/create",
        json!({ "mob_config": config_under(mob_id) }),
    )
    .await
}

/// Every work item the launch's store holds in the realm of the mob
/// `mob_id`, rendered: read from the store, whether or not a mob stands
/// under the id.
async fn in_the_realm_of(host: &UnifiedRuntime, mob_id: &MobId) -> String {
    let of_the_launch = host
        .workgraph_service()
        .expect("the launch has a WorkGraph service");
    let of_the_realm = meerkat_mob::mob_scoped_workgraph_service(&of_the_launch, mob_id)
        .expect("the mob has a realm")
        .expect("the launch's store has a WorkGraph");
    listed(&of_the_realm).await
}

/// What is wrong with `answer` as the refusal of a host's create under the
/// id `mob_id` whose WorkGraph realm holds work: the family's code, its
/// kind for an id a mob of the host cannot have, the id, and a sentence
/// that names the WorkGraph.
fn not_the_refusal_of_a_used_realm(answer: &Value, mob_id: &str) -> Option<String> {
    let error = &answer["error"];
    let refused = (
        error["code"].as_i64(),
        error["data"]["kind"].as_str(),
        error["data"]["mob_id"].as_str(),
    ) == (
        Some(crate::rpc::mob_hosting::MOB_HOSTING_REFUSED_CODE),
        Some("invalid_mob_id"),
        Some(mob_id),
    );
    let says_why = error["message"]
        .as_str()
        .is_some_and(|sentence| sentence.contains("WorkGraph"));
    (!(refused && says_why)).then(|| answer.to_string())
}

/// A host's create under the id of a mob it destroyed is refused while the
/// launch's WorkGraph store holds work under that id. The refusal is the
/// family's own for an id a mob of the host cannot have; no mob is created,
/// the rows in the store are as they were, and a mob under another id is
/// created.
#[tokio::test]
async fn a_hosts_create_under_an_id_whose_workgraph_realm_holds_work_is_refused() {
    let temp = tempfile::tempdir().expect("tempdir");
    let host = launch(&temp, definition("host")).await;
    let second = an_id("second");
    let second_id = MobId::from(second.as_str());
    let made = the_host_creates(&host, &second).await;
    assert!(served(&made), "the host creates the mob: {made}");
    let written = stdio(
        &host,
        "meta/workgraph/create",
        naming(json!({ "title": IN_THE_SECOND }), Some(&second_id)),
    )
    .await;
    assert!(served(&written), "the mob's item is written: {written}");
    let destroyed = stdio(&host, "meta/mobs/destroy", json!({ "mob_id": second })).await;
    assert!(served(&destroyed), "the host destroys the mob: {destroyed}");

    let again = the_host_creates(&host, &second).await;
    let wrong = not_the_refusal_of_a_used_realm(&again, &second);
    assert!(
        wrong.is_none(),
        "the create under an id whose realm holds work is refused: it answered {wrong:?}"
    );
    assert!(
        host.resolve_mob(Some(&second_id)).await.is_err(),
        "no mob stands under the id after the refused create"
    );
    assert!(
        in_the_realm_of(&host, &second_id)
            .await
            .contains(IN_THE_SECOND),
        "the item is in the store as it was"
    );

    let other = an_id("other");
    let made = the_host_creates(&host, &other).await;
    assert!(served(&made), "a mob under another id is created: {made}");
    shut_down(&host, &[&MobId::from(other.as_str())]).await;
}

/// The same refusal after a restart of the gateway over the same state
/// directory: the launch's store is a file, and the work under the id is
/// in it when the gateway starts again.
#[tokio::test]
async fn a_hosts_create_after_a_restart_under_an_id_whose_workgraph_realm_holds_work_is_refused() {
    let launch_definition = definition("stored-host");
    let launch_id = launch_definition.id.clone();
    let stored = StoredHost::of(launch_definition);
    let model = Arc::new(CallMaker::default());
    let host = stored_launch_with(&stored, &launch_id, &model).await;
    let second = an_id("second");
    let second_id = MobId::from(second.as_str());
    let made = the_host_creates(&host, &second).await;
    assert!(served(&made), "the host creates the mob: {made}");
    let written = stdio(
        &host,
        "meta/workgraph/create",
        naming(json!({ "title": IN_THE_SECOND }), Some(&second_id)),
    )
    .await;
    assert!(served(&written), "the mob's item is written: {written}");
    let destroyed = stdio(&host, "meta/mobs/destroy", json!({ "mob_id": second })).await;
    assert!(served(&destroyed), "the host destroys the mob: {destroyed}");
    shut_down(&host, &[]).await;
    drop(host);

    let model = Arc::new(CallMaker::default());
    let host = stored_launch_with(&stored, &launch_id, &model).await;
    assert!(
        in_the_realm_of(&host, &second_id)
            .await
            .contains(IN_THE_SECOND),
        "setup: the restarted gateway's store holds the item"
    );
    let again = the_host_creates(&host, &second).await;
    let wrong = not_the_refusal_of_a_used_realm(&again, &second);
    assert!(
        wrong.is_none(),
        "the create under an id whose realm holds work is refused after a restart: it answered \
         {wrong:?}"
    );
    assert!(
        host.resolve_mob(Some(&second_id)).await.is_err(),
        "no mob stands under the id after the refused create"
    );
    shut_down(&host, &[]).await;
}

/// A host's create under the id of a mob it destroyed is made when the
/// launch's WorkGraph store holds nothing under that id, and the new mob's
/// WorkGraph is served. The host keeps its mobs in stores.
#[tokio::test]
async fn a_hosts_create_under_a_reused_id_whose_workgraph_realm_is_empty_is_made() {
    let launch_definition = definition("stored-host");
    let launch_id = launch_definition.id.clone();
    let stored = StoredHost::of(launch_definition);
    let model = Arc::new(CallMaker::default());
    let host = stored_launch_with(&stored, &launch_id, &model).await;
    let second = an_id("second");
    let second_id = MobId::from(second.as_str());
    let made = the_host_creates(&host, &second).await;
    assert!(served(&made), "the host creates the mob: {made}");
    // Work of the launch mob is in another realm and decides nothing here.
    let written = stdio(
        &host,
        "meta/workgraph/create",
        json!({ "title": IN_THE_LAUNCH }),
    )
    .await;
    assert!(
        served(&written),
        "the launch mob's item is written: {written}"
    );
    let destroyed = stdio(&host, "meta/mobs/destroy", json!({ "mob_id": second })).await;
    assert!(served(&destroyed), "the host destroys the mob: {destroyed}");

    let again = the_host_creates(&host, &second).await;
    assert!(
        served(&again),
        "a create under a reused id whose realm is empty is made: {again}"
    );
    assert_eq!(again["result"]["mob"]["mob_id"], json!(second), "{again}");
    let written = stdio(
        &host,
        "meta/workgraph/create",
        naming(json!({ "title": IN_THE_SECOND }), Some(&second_id)),
    )
    .await;
    assert!(served(&written), "the new mob's item is written: {written}");
    assert!(
        in_the_realm_of(&host, &second_id)
            .await
            .contains(IN_THE_SECOND),
        "the new mob's item is in its realm"
    );
    shut_down(&host, &[&second_id]).await;
}

/// An id a mob of the host has today is answered `mob_exists`,
/// though that mob's realm holds its work: the id of a hosted mob and the
/// id of the launch mob.
#[tokio::test]
async fn a_hosts_create_under_an_id_it_has_answers_mob_exists_though_the_realm_holds_work() {
    let temp = tempfile::tempdir().expect("tempdir");
    let host = launch(&temp, definition("host")).await;
    let launch_id = host.mob_handle().mob_id().clone();
    let second = an_id("second");
    let second_id = MobId::from(second.as_str());
    let made = the_host_creates(&host, &second).await;
    assert!(served(&made), "the host creates the mob: {made}");
    for named in [Some(&second_id), None] {
        let written = stdio(
            &host,
            "meta/workgraph/create",
            naming(json!({ "title": IN_THE_SECOND }), named),
        )
        .await;
        assert!(served(&written), "an item is written: {written}");
    }

    for taken in [second.as_str(), launch_id.as_str()] {
        let again = the_host_creates(&host, taken).await;
        let error = &again["error"];
        assert_eq!(
            (
                error["code"].as_i64(),
                error["data"]["kind"].as_str(),
                error["data"]["mob_id"].as_str(),
            ),
            (
                Some(crate::rpc::mob_hosting::MOB_HOSTING_REFUSED_CODE),
                Some("mob_exists"),
                Some(taken),
            ),
            "an id a mob of the host has is answered as taken: {again}"
        );
    }
    shut_down(&host, &[&second_id]).await;
}

/// A host's create whose judgement cannot be made is refused: where the
/// launch's WorkGraph store cannot answer the read, no mob is created, and
/// the caller is answered a fixed sentence with none of the store's words.
#[tokio::test]
async fn a_hosts_create_is_refused_where_the_workgraph_store_cannot_be_read() {
    let temp = tempfile::tempdir().expect("tempdir");
    let host = launch(&temp, definition("host")).await;
    // The store's file loses its event table behind the store's back: the
    // read the create asks then fails in the store.
    let file = temp
        .path()
        .join(crate::workgraph_wiring::WORKGRAPH_STORE_FILE);
    rusqlite::Connection::open(&file)
        .expect("the store's file opens")
        .execute_batch("DROP TABLE workgraph_events;")
        .expect("the event table is dropped");

    let second = an_id("second");
    let second_id = MobId::from(second.as_str());
    let answer = the_host_creates(&host, &second).await;
    let error = &answer["error"];
    assert_eq!(
        error["code"].as_i64(),
        Some(-32603),
        "a create that cannot be judged is refused: {answer}"
    );
    let sentence = error["message"].as_str().unwrap_or_default();
    assert!(
        sentence.contains("could not read its WorkGraph store"),
        "the refusal says the store could not be read: {answer}"
    );
    assert!(
        !sentence.contains("workgraph_events")
            && !sentence.contains("no such table")
            && !answer.to_string().contains("sqlite"),
        "the answer holds none of the store's words: {answer}"
    );
    assert!(
        host.resolve_mob(Some(&second_id)).await.is_err(),
        "no mob stands under the id after the refused create"
    );
    let mobs = stdio(&host, "meta/mobs/list", json!({})).await;
    assert!(
        served(&mobs) && !mobs.to_string().contains(&second),
        "the host lists no mob under the id: {mobs}"
    );
    shut_down(&host, &[]).await;
}

/// What makes no mob is untouched by the refusal: a mob the host created
/// that holds work is unloaded and loaded again, stopped, redefined and
/// resumed, and is served with its work after each.
#[tokio::test]
async fn a_hosted_mob_that_holds_work_is_loaded_again_and_redefined_with_its_work() {
    let launch_definition = definition("stored-host");
    let launch_id = launch_definition.id.clone();
    let stored = StoredHost::of(launch_definition);
    let model = Arc::new(CallMaker::default());
    let host = stored_launch_with(&stored, &launch_id, &model).await;
    let second = an_id("second");
    let second_id = MobId::from(second.as_str());
    let made = the_host_creates(&host, &second).await;
    assert!(served(&made), "the host creates the mob: {made}");
    let written = stdio(
        &host,
        "meta/workgraph/create",
        naming(json!({ "title": IN_THE_SECOND }), Some(&second_id)),
    )
    .await;
    assert!(served(&written), "the mob's item is written: {written}");

    put_to_sleep(&host, &second_id).await;
    let listed_after_the_load = stdio(
        &host,
        "meta/workgraph/list",
        naming(json!({}), Some(&second_id)),
    )
    .await;
    assert!(
        served(&listed_after_the_load) && listed_after_the_load.to_string().contains(IN_THE_SECOND),
        "the mob is loaded again and lists its work: {listed_after_the_load}"
    );

    let stopped = stdio(
        &host,
        "meta/mobs/lifecycle",
        json!({ "mob_id": second, "action": "stop" }),
    )
    .await;
    assert!(served(&stopped), "the host stops the mob: {stopped}");
    let revision = stdio(&host, "meta/mobs/get", json!({ "mob_id": second })).await["result"]
        ["mob"]["revision"]
        .clone();
    let redefined = stdio(
        &host,
        "meta/mobs/redefine",
        json!({
            "mob_id": second,
            "mob_config": format!(
                "{}\n[profiles.reviewer]\nmodel = \"gpt-5.5\"\n\n[profiles.reviewer.tools]\ncomms = true\n",
                config_under(&second)
            ),
            "expected_revision": revision,
        }),
    )
    .await;
    assert!(
        served(&redefined),
        "the host redefines the stopped mob that holds work: {redefined}"
    );
    let resumed = stdio(
        &host,
        "meta/mobs/lifecycle",
        json!({ "mob_id": second, "action": "resume" }),
    )
    .await;
    assert!(served(&resumed), "the host resumes the mob: {resumed}");
    let listed_after_the_redefine = stdio(
        &host,
        "meta/workgraph/list",
        naming(json!({}), Some(&second_id)),
    )
    .await;
    assert!(
        served(&listed_after_the_redefine)
            && listed_after_the_redefine
                .to_string()
                .contains(IN_THE_SECOND),
        "the redefined mob lists its work: {listed_after_the_redefine}"
    );
    shut_down(&host, &[&second_id]).await;
}
