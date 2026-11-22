//! Rows of the methods over the mobs of a host: `meta/mobs/*`.
//!
//! The fixture is a launch that declares its mob to its mob set, as the
//! gateways do, in memory. Every request goes through the stdio dispatch,
//! as a host's own request does. Every launch lets a request resolve the
//! mobs its mob set built for the host, which is what makes the runtime
//! name the mobs it keeps an entry for and serve the family.

#![allow(clippy::expect_used, clippy::panic)]

use std::sync::Arc;
use std::time::Duration;

use meerkat::{AgentFactory, Config, FactoryAgentBuilder};
use meerkat_mob::{MobDefinition, MobId, SpawnMemberSpec};
use serde_json::{Value, json};

use crate::blob_store::{Base64BlobStoreAdapter, ObjectStoreBlobStore};
use crate::mob_handle_runtime::{MobBootstrapOptions, MobBootstrapSpec};
use crate::runtime::{InMemoryMetadataStore, RuntimeOptions};
use crate::{DiscoverySpec, MetaConfig, MobRuntimeDelivery, UnifiedRuntime};

/// The code of the family's own refusals.
const REFUSED: i64 = -32059;

/// The TOML of a definition with one profile under the id `mob_id`.
pub(crate) fn config(mob_id: &str) -> String {
    format!(
        "[mob]\nid = \"{mob_id}\"\n\n[profiles.worker]\nmodel = \"gpt-5.5\"\n\n[profiles.worker.tools]\ncomms = true\n"
    )
}

/// An id of its own that sorts by `lead`.
fn id(lead: &str) -> String {
    format!("{lead}-mob-hosting-{}", uuid::Uuid::new_v4().simple())
}

fn definition(mob_id: &str) -> MobDefinition {
    MobDefinition::from_toml(&config(mob_id)).expect("a definition with one profile")
}

/// A runtime whose launch declares the mob `mob_id` to its mob set, in
/// memory: its requests resolve the mobs that set builds for the host.
pub(crate) async fn host(temp: &tempfile::TempDir, mob_id: &str) -> UnifiedRuntime {
    launched(temp, mob_id, false).await
}

/// [`host`] whose launch mob and the mobs the host creates beside it are
/// kept in stores under `temp`: the mob set can load each of them again.
pub(crate) async fn stored_host(temp: &tempfile::TempDir, mob_id: &str) -> UnifiedRuntime {
    launched(temp, mob_id, true).await
}

/// The launch of [`host`] and [`stored_host`]: it declares its mob to its
/// mob set.
async fn launched(temp: &tempfile::TempDir, mob_id: &str, stored: bool) -> UnifiedRuntime {
    launched_keeping(temp, mob_id, stored, Arc::new(InMemoryMetadataStore::new())).await
}

/// [`launched`] whose host keeps its records in `metadata`.
async fn launched_keeping(
    temp: &tempfile::TempDir,
    mob_id: &str,
    stored: bool,
    metadata: Arc<dyn crate::runtime::PersistentMetadataStore>,
) -> UnifiedRuntime {
    let definition = definition(mob_id);
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
    let service = Arc::new(meerkat_session::PersistentSessionService::new(
        builder,
        8,
        Arc::new(meerkat_store::MemoryStore::new()),
        store.clone(),
        blobs,
    ));
    let namespace = definition.id.to_string();
    let (launch_store, child_mobs) = if stored {
        (
            crate::mob_composition_manifest::declared_mob_store(
                temp.path().join("launch_mob.sqlite3"),
            ),
            crate::ChildMobStorage::Durable(
                crate::storage_layout::MetaStorageLayout::with_injected_roots(
                    temp.path().to_path_buf(),
                    None,
                ),
            ),
        )
    } else {
        (
            crate::mob_composition_manifest::DeclaredMobStore::in_memory(),
            crate::ChildMobStorage::ProcessBound,
        )
    };
    let spec = MobBootstrapSpec::declared(definition, launch_store, service)
        .with_session_runtime_adapter(machine)
        .expect("the launch adopts the runtime machine")
        .with_runtime_delivery(MobRuntimeDelivery::new(store, jobs))
        .with_agent_mob_tools(Arc::new(std::sync::RwLock::new(None)))
        .expect("the agent mob tools")
        .with_child_mob_storage(child_mobs)
        .with_options(MobBootstrapOptions {
            allow_ephemeral_sessions: false,
            notify_orchestrator_on_resume: true,
            default_llm_client: Some(Arc::new(meerkat_client::TestClient::default())),
        });
    let mut runtime = Box::pin(UnifiedRuntime::bootstrap_with_options(
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
        metadata,
    ))
    .await
    .expect("a runtime over a declared store");
    // What a gateway without an identity plane does after its bootstrap: a
    // launch mob an earlier host of the same state stopped at its shutdown
    // is run again.
    runtime
        .activate_without_identity_context()
        .await
        .expect("the launch runs its mob");
    runtime
}

/// The launch's mob set.
fn set_of(host: &UnifiedRuntime) -> Arc<meerkat_mob_mcp::MobMcpState> {
    host.mob_runtime()
        .agent_mob_mcp_state()
        .expect("the launch's mob set")
}

/// Shut down the mobs `others` through the set, then the runtime.
async fn shut_down(host: &UnifiedRuntime, others: &[&str]) {
    let set = set_of(host);
    for mob_id in others {
        if let Ok(handle) = set.handle_for(&MobId::from(*mob_id)).await {
            let _ = handle.shutdown().await;
        }
    }
    tokio::time::timeout(Duration::from_secs(30), host.shutdown())
        .await
        .expect("the runtime shuts down");
}

/// One request on the stdio dispatch: its whole answer.
async fn ask(host: &UnifiedRuntime, method: &str, params: Value) -> Value {
    let request =
        json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).to_string();
    let answer = tokio::time::timeout(
        Duration::from_mins(1),
        crate::rpc::handle_unified_rpc_json(host, &request, Duration::from_secs(1), None, None),
    )
    .await
    .expect("the dispatch answers in time");
    serde_json::from_str(&answer).expect("a JSON answer")
}

/// The result of a request that must succeed.
pub(crate) async fn result(host: &UnifiedRuntime, method: &str, params: Value) -> Value {
    let answer = ask(host, method, params).await;
    assert!(
        answer.get("error").is_none(),
        "{method} succeeds: {}",
        answer["error"]
    );
    answer["result"].clone()
}

/// The error of a request that must be refused.
pub(crate) async fn refusal(host: &UnifiedRuntime, method: &str, params: Value) -> Value {
    let answer = ask(host, method, params).await;
    assert!(
        answer.get("result").is_none() && answer.get("error").is_some(),
        "{method} is refused: {answer}"
    );
    answer["error"].clone()
}

/// The entries of `meta/mobs/list`.
async fn listed(host: &UnifiedRuntime) -> Vec<Value> {
    result(host, "meta/mobs/list", json!({})).await["mobs"]
        .as_array()
        .expect("a list of mobs")
        .clone()
}

/// The entry of `meta/mobs/get` for `mob_id`.
async fn got(host: &UnifiedRuntime, mob_id: &str) -> Value {
    result(host, "meta/mobs/get", json!({"mob_id": mob_id})).await["mob"].clone()
}

/// Whether `error` is the stdio dispatch's refusal of an id the host does
/// not have, naming `requested` and the host's launch mob.
fn is_not_a_mob_of_the_host(error: &Value, requested: &str, default_mob: &str) -> bool {
    error["code"] == json!(-32602)
        && error["data"]["kind"] == json!("foreign_mob_target")
        && error["data"]["requested_mob_id"] == json!(requested)
        && error["data"]["gateway_mob_id"] == json!(default_mob)
}

/// The names of an entry's members, sorted.
fn members(entry: &Value) -> Vec<&str> {
    let mut names: Vec<&str> = entry
        .as_object()
        .expect("an entry is an object")
        .keys()
        .map(String::as_str)
        .collect();
    names.sort_unstable();
    names
}

/// A mob a member of the launch mob created: its id.
pub(crate) async fn member_created_mob(host: &UnifiedRuntime, mob_id: &str) -> MobId {
    let owner = meerkat_mob::AgentIdentity::from("owner");
    Box::pin(host.spawn(SpawnMemberSpec::new("worker", "owner")))
        .await
        .expect("the owner member");
    let owner_session = host
        .mob_handle()
        .resolve_bridge_session_id(&owner)
        .await
        .expect("the owner member's session");
    Box::pin(
        set_of(host).mob_create_definition_with_owner_bridge_session(
            definition(mob_id),
            owner_session,
            true,
            false,
        ),
    )
    .await
    .expect("a mob the member owns")
}

/// A mob a member of the mob `parent`, a mob the host created beside its
/// launch mob, created: its id.
pub(crate) async fn member_created_mob_in(
    host: &UnifiedRuntime,
    parent: &MobId,
    mob_id: &str,
) -> MobId {
    let owner = meerkat_mob::AgentIdentity::from("owner");
    let parent = set_of(host)
        .handle_for(parent)
        .await
        .expect("the mob set serves the parent mob");
    Box::pin(parent.spawn_spec(SpawnMemberSpec::new("worker", "owner")))
        .await
        .expect("the owner member of the parent mob");
    let owner_session = parent
        .resolve_bridge_session_id(&owner)
        .await
        .expect("the owner member's session");
    Box::pin(
        set_of(host).mob_create_definition_with_owner_bridge_session(
            definition(mob_id),
            owner_session,
            true,
            false,
        ),
    )
    .await
    .expect("a mob the parent mob's member owns")
}

/// The mobs of a host are listed with the launch mob first and the others
/// by id, each with its id, whether it is the launch mob, its phase and
/// who created it. A host of one mob lists that mob.
#[tokio::test]
async fn the_mobs_of_a_host_are_listed_the_launch_mob_first_then_by_id() {
    let temp = tempfile::tempdir().expect("tempdir");
    let default_mob = id("m");
    let host = host(&temp, &default_mob).await;

    let alone = listed(&host).await;
    assert_eq!(
        alone,
        vec![json!({
            "mob_id": default_mob,
            "default": true,
            "state": "running",
            "provenance": "host",
        })],
        "a host of one mob lists its launch mob"
    );

    let (late, early) = (id("z"), id("a"));
    for mob_id in [&late, &early] {
        Box::pin(set_of(&host).mob_create_definition(definition(mob_id)))
            .await
            .expect("the mob set creates a mob of the host");
    }
    let mobs = listed(&host).await;
    let ids: Vec<&str> = mobs
        .iter()
        .map(|mob| mob["mob_id"].as_str().expect("an id"))
        .collect();
    assert_eq!(
        ids,
        vec![default_mob.as_str(), early.as_str(), late.as_str()]
    );
    let defaults: Vec<bool> = mobs
        .iter()
        .map(|mob| mob["default"].as_bool().expect("a flag"))
        .collect();
    assert_eq!(defaults, vec![true, false, false]);
    for mob in &mobs {
        assert_eq!(mob["state"], json!("running"), "{mob}");
        assert_eq!(mob["provenance"], json!("host"), "{mob}");
        assert_eq!(
            members(mob),
            vec!["default", "mob_id", "provenance", "state"],
            "an entry of a list carries what the mob set's records keep and nothing else"
        );
    }
    shut_down(&host, &[&late, &early]).await;
}

/// A listed mob carries its own phase: a mob that was stopped reads
/// `stopped` on the list and on `get`, and the other mobs keep theirs.
#[tokio::test]
async fn a_mob_of_a_host_is_read_with_its_own_phase() {
    let temp = tempfile::tempdir().expect("tempdir");
    let default_mob = id("m");
    let host = host(&temp, &default_mob).await;
    let second = id("s");
    Box::pin(set_of(&host).mob_create_definition(definition(&second)))
        .await
        .expect("a second mob of the host");
    set_of(&host)
        .mob_stop(&MobId::from(second.as_str()))
        .await
        .expect("the second mob stops");

    let states: Vec<(String, String)> = listed(&host)
        .await
        .iter()
        .map(|mob| {
            (
                mob["mob_id"].as_str().expect("an id").to_string(),
                mob["state"].as_str().expect("a state").to_string(),
            )
        })
        .collect();
    assert_eq!(
        states,
        vec![
            (default_mob.clone(), "running".to_string()),
            (second.clone(), "stopped".to_string()),
        ]
    );
    assert_eq!(
        got(&host, &second).await,
        json!({
            "mob_id": second,
            "default": false,
            "state": "stopped",
            "provenance": "host",
            "revision": 1,
        })
    );
    assert_eq!(
        got(&host, &default_mob).await,
        json!({
            "mob_id": default_mob,
            "default": true,
            "state": "running",
            "provenance": "host",
            "revision": 1,
        })
    );
    shut_down(&host, &[&second]).await;
}

/// The methods that act on one mob name it: without a `mob_id` each is
/// invalid params, never a request for the launch mob; an id that is no
/// string is refused as on every method; an id the host does not have is
/// the dispatch's refusal of such an id, before anything else of the
/// request is judged.
#[tokio::test]
async fn a_method_that_acts_on_one_mob_never_takes_the_launch_mob_for_want_of_an_id() {
    let temp = tempfile::tempdir().expect("tempdir");
    let default_mob = id("m");
    let host = host(&temp, &default_mob).await;

    for method in [
        "meta/mobs/get",
        "meta/mobs/lifecycle",
        "meta/mobs/redefine",
        "meta/mobs/destroy",
    ] {
        for params in [json!({}), json!({"mob_id": null}), Value::Null] {
            let error = refusal(&host, method, params.clone()).await;
            assert_eq!(error["code"], json!(-32602), "{method} {params}: {error}");
            assert_eq!(
                error["message"],
                json!("Invalid params: mob_id is required"),
                "{method} {params}"
            );
        }
        let error = refusal(&host, method, json!({"mob_id": 7})).await;
        assert_eq!(
            error["message"],
            json!("Invalid params: mob_id must be a string"),
            "{method}"
        );
        // Nothing else of the request is right either: the id is judged
        // first.
        let error = refusal(&host, method, json!({"mob_id": "no-such-mob", "extra": 1})).await;
        assert!(
            is_not_a_mob_of_the_host(&error, "no-such-mob", &default_mob),
            "{method}: {error}"
        );
    }
    let state = got(&host, &default_mob).await["state"].clone();
    assert_eq!(state, json!("running"), "the launch mob was not stopped");

    let error = refusal(
        &host,
        "meta/mobs/lifecycle",
        json!({"mob_id": default_mob, "action": "pause"}),
    )
    .await;
    assert_eq!(
        error["code"],
        json!(-32602),
        "an action it does not know: {error}"
    );
    let error = refusal(
        &host,
        "meta/mobs/get",
        json!({"mob_id": default_mob, "extra": 1}),
    )
    .await;
    assert_eq!(
        error["code"],
        json!(-32602),
        "a member it does not know: {error}"
    );
    shut_down(&host, &[]).await;
}

/// `create` makes a mob of the host from a definition: the mob set creates
/// it under the definition's id with the host as its creator, it runs, and
/// the answer is its entry. It is listed from then on.
#[tokio::test]
async fn create_makes_a_mob_of_the_host_and_answers_its_entry() {
    let temp = tempfile::tempdir().expect("tempdir");
    let default_mob = id("m");
    let host = host(&temp, &default_mob).await;
    let second = id("s");

    let created = result(
        &host,
        "meta/mobs/create",
        json!({"mob_config": config(&second)}),
    )
    .await;
    assert_eq!(
        created,
        json!({"mob": {
            "mob_id": second,
            "default": false,
            "state": "running",
            "provenance": "host",
            "revision": 1,
        }})
    );
    let handle = set_of(&host)
        .handle_for(&MobId::from(second.as_str()))
        .await
        .expect("the mob set manages the created mob");
    assert_eq!(
        handle.provenance().await.ok(),
        Some(meerkat_mob::MobProvenance::Host),
        "the mob's creation record names the host"
    );
    let ids: Vec<String> = listed(&host)
        .await
        .iter()
        .map(|mob| mob["mob_id"].as_str().expect("an id").to_string())
        .collect();
    assert_eq!(ids, vec![default_mob.clone(), second.clone()]);
    assert!(
        host.resolve_mob(Some(&MobId::from(second.as_str())))
            .await
            .is_ok(),
        "the created mob is one this host serves requests for"
    );
    shut_down(&host, &[&second]).await;
}

/// The refusals of `create`. An id a mob of the host has, the launch mob's
/// among them, is `mob_exists`; a definition without an id is
/// `invalid_mob_id`: both under the family's one code, with the mob id,
/// `retryable: false` and the sentence again as `detail`. A definition that
/// does not parse is invalid params with the launch's text, and so are
/// params that are not the method's.
#[tokio::test]
async fn create_refuses_a_taken_id_a_blank_id_and_a_definition_it_cannot_read() {
    let temp = tempfile::tempdir().expect("tempdir");
    let default_mob = id("m");
    let host = host(&temp, &default_mob).await;
    let second = id("s");
    result(
        &host,
        "meta/mobs/create",
        json!({"mob_config": config(&second)}),
    )
    .await;

    for taken in [&second, &default_mob] {
        let error = refusal(
            &host,
            "meta/mobs/create",
            json!({"mob_config": config(taken)}),
        )
        .await;
        assert_eq!(error["code"], json!(REFUSED), "{taken}: {error}");
        assert_eq!(error["data"]["kind"], json!("mob_exists"), "{error}");
        assert_eq!(error["data"]["mob_id"], json!(taken), "{error}");
        assert_eq!(error["data"]["retryable"], json!(false), "{error}");
        assert_eq!(error["data"]["detail"], error["message"], "{error}");
        assert_eq!(
            members(&error["data"]),
            vec!["detail", "kind", "mob_id", "retryable"]
        );
    }

    for blank in ["", "   "] {
        let error = refusal(
            &host,
            "meta/mobs/create",
            json!({"mob_config": config(blank)}),
        )
        .await;
        assert_eq!(error["code"], json!(REFUSED), "{blank:?}: {error}");
        assert_eq!(error["data"]["kind"], json!("invalid_mob_id"), "{error}");
        assert_eq!(error["data"]["mob_id"], json!(blank), "{error}");
        assert_eq!(error["data"]["retryable"], json!(false), "{error}");
    }

    let error = refusal(
        &host,
        "meta/mobs/create",
        json!({"mob_config": "[mob\nid ="}),
    )
    .await;
    assert_eq!(error["code"], json!(-32602), "{error}");
    assert!(
        error["message"]
            .as_str()
            .is_some_and(|message| message.starts_with("Invalid mob_config TOML: ")),
        "a definition that does not parse answers as the launch does: {error}"
    );
    for params in [
        json!({}),
        json!({"mob_config": 7}),
        json!({"mob_config": config(&id("x")), "extra": 1}),
    ] {
        let error = refusal(&host, "meta/mobs/create", params.clone()).await;
        assert_eq!(error["code"], json!(-32602), "{params}: {error}");
    }
    assert_eq!(listed(&host).await.len(), 2, "no refused create made a mob");
    shut_down(&host, &[&second]).await;
}

/// A `create` the host's create admission refuses is answered as Meerkat
/// answers a refused create, unchanged: the code, the sentence and the data
/// (`kind` `mob_create_refused`, the `mob_id`, `retryable: false` and the
/// `reason`) are those the mob set's own create of that id answers on the
/// host, and not the family's own code. No mob is made. Shown on a host
/// whose metadata store has no release of a mob's records: what an earlier
/// mob of the id left there could not be released, so the admission
/// refuses every create.
#[tokio::test]
async fn a_create_the_hosts_admission_refuses_is_answered_as_a_refused_create_and_makes_no_mob() {
    let temp = tempfile::tempdir().expect("tempdir");
    let default_mob = id("m");
    let host = launched_keeping(
        &temp,
        &default_mob,
        false,
        Arc::new(crate::console_audit::test_support::ListingStore::default()),
    )
    .await;
    let second = id("s");

    let error = refusal(
        &host,
        "meta/mobs/create",
        json!({"mob_config": config(&second)}),
    )
    .await;

    assert_eq!(
        error["data"]["kind"],
        json!("mob_create_refused"),
        "{error}"
    );
    assert_eq!(error["data"]["mob_id"], json!(second), "{error}");
    assert_eq!(error["data"]["retryable"], json!(false), "{error}");
    assert_eq!(
        error["data"]["reason"],
        json!(crate::private_owners::CREATE_RELEASE_FAILED),
        "{error}"
    );
    assert_eq!(
        members(&error["data"]),
        vec!["kind", "mob_id", "reason", "retryable"]
    );
    assert_ne!(
        error["code"],
        json!(REFUSED),
        "not one of the family's own refusals: {error}"
    );
    let of_the_set = Box::pin(set_of(&host).mob_create_definition(definition(&second)))
        .await
        .expect_err("the mob set's own create of the id is refused on this host");
    assert!(
        meerkat_mob_mcp::MobMcpState::is_create_refusal(&of_the_set),
        "the mob set answers a refused create: {of_the_set}"
    );
    let (code, data) = crate::identity_first::runtime::mob_error_wire(&of_the_set)
        .expect("the refusal's wire form");
    assert_eq!(error["code"], json!(code), "{error}");
    assert_eq!(error["message"], json!(of_the_set.to_string()), "{error}");
    assert_eq!(Some(error["data"].clone()), data, "{error}");

    let ids: Vec<String> = listed(&host)
        .await
        .iter()
        .map(|mob| mob["mob_id"].as_str().expect("an id").to_string())
        .collect();
    assert_eq!(
        ids,
        vec![default_mob.clone()],
        "the refused create made no mob"
    );
    let error = refusal(&host, "meta/mobs/get", json!({"mob_id": second})).await;
    assert!(
        is_not_a_mob_of_the_host(&error, &second, &default_mob),
        "the id is one the host does not have: {error}"
    );
    shut_down(&host, &[]).await;
}

/// `lifecycle` stops and resumes the mob it names and no other: the answer
/// carries the mob's entry after the action and, for a stop, the mob's own
/// stop report. The launch mob is not stopped or resumed through the
/// family: both are refused with the kind `default_mob`, and the launch
/// mob runs on. The host's own way to stop its mob is unchanged.
#[tokio::test]
async fn lifecycle_stops_and_resumes_the_named_mob_alone() {
    let temp = tempfile::tempdir().expect("tempdir");
    let default_mob = id("m");
    let host = host(&temp, &default_mob).await;
    let second = id("s");
    result(
        &host,
        "meta/mobs/create",
        json!({"mob_config": config(&second)}),
    )
    .await;

    let stopped = result(
        &host,
        "meta/mobs/lifecycle",
        json!({"mob_id": second, "action": "stop"}),
    )
    .await;
    assert_eq!(stopped["mob"]["mob_id"], json!(second));
    assert_eq!(stopped["mob"]["state"], json!("stopped"));
    assert!(
        stopped["stop_report"].is_object(),
        "a stop answers the mob's stop report: {stopped}"
    );
    assert_eq!(got(&host, &default_mob).await["state"], json!("running"));

    let resumed = result(
        &host,
        "meta/mobs/lifecycle",
        json!({"mob_id": second, "action": "resume"}),
    )
    .await;
    assert_eq!(resumed["mob"]["state"], json!("running"));
    assert!(
        resumed.get("stop_report").is_none(),
        "a resume answers no stop report: {resumed}"
    );

    for action in ["stop", "resume"] {
        let error = refusal(
            &host,
            "meta/mobs/lifecycle",
            json!({"mob_id": default_mob, "action": action}),
        )
        .await;
        assert_eq!(error["code"], json!(REFUSED), "{action}: {error}");
        assert_eq!(
            error["data"]["kind"],
            json!("default_mob"),
            "{action}: {error}"
        );
        assert_eq!(
            error["data"]["mob_id"],
            json!(default_mob),
            "{action}: {error}"
        );
        assert_eq!(
            got(&host, &default_mob).await["state"],
            json!("running"),
            "{action}: the launch mob runs on"
        );
    }
    assert_eq!(got(&host, &second).await["state"], json!("running"));
    shut_down(&host, &[&second]).await;
}

/// `destroy` removes a mob the host created: the answer names the mob and
/// carries the mob set's destroy report, the mob is listed no more and
/// read as an id the host does not have, the runtime keeps no entry for it,
/// and its id is free for a new mob.
#[tokio::test]
async fn destroy_removes_a_mob_of_the_host_and_frees_its_id() {
    let temp = tempfile::tempdir().expect("tempdir");
    let default_mob = id("m");
    let host = host(&temp, &default_mob).await;
    let second = id("s");
    result(
        &host,
        "meta/mobs/create",
        json!({"mob_config": config(&second)}),
    )
    .await;
    assert!(
        host.resolve_mob(Some(&MobId::from(second.as_str())))
            .await
            .is_ok()
    );

    let destroyed = result(&host, "meta/mobs/destroy", json!({"mob_id": second})).await;
    assert_eq!(destroyed["mob_id"], json!(second));
    assert!(
        destroyed["destroy_report"].is_object(),
        "a destroy answers the mob set's report: {destroyed}"
    );
    assert_eq!(members(&destroyed), vec!["destroy_report", "mob_id"]);

    let ids: Vec<String> = listed(&host)
        .await
        .iter()
        .map(|mob| mob["mob_id"].as_str().expect("an id").to_string())
        .collect();
    assert_eq!(ids, vec![default_mob.clone()]);
    for method in ["meta/mobs/get", "meta/mobs/destroy"] {
        let error = refusal(&host, method, json!({"mob_id": second})).await;
        assert!(
            is_not_a_mob_of_the_host(&error, &second, &default_mob),
            "{method} of a destroyed mob: {error}"
        );
    }
    assert!(
        host.resolve_mob(Some(&MobId::from(second.as_str())))
            .await
            .is_err(),
        "a destroyed mob resolves on no request"
    );
    assert_eq!(
        host.mob_entries_in_tests(),
        Vec::<MobId>::new(),
        "the runtime keeps no entry of a destroyed mob"
    );

    let again = result(
        &host,
        "meta/mobs/create",
        json!({"mob_config": config(&second)}),
    )
    .await;
    assert_eq!(again["mob"]["state"], json!("running"));
    shut_down(&host, &[&second]).await;
}

/// The launch mob stays its launch's: a destroy of it is refused
/// `default_mob` under the family's code, and the mob runs on.
#[tokio::test]
async fn destroy_of_the_launch_mob_is_refused() {
    let temp = tempfile::tempdir().expect("tempdir");
    let default_mob = id("m");
    let host = host(&temp, &default_mob).await;

    let error = refusal(&host, "meta/mobs/destroy", json!({"mob_id": default_mob})).await;
    assert_eq!(error["code"], json!(REFUSED), "{error}");
    assert_eq!(
        error["data"],
        json!({
            "kind": "default_mob",
            "mob_id": default_mob,
            "retryable": false,
            "detail": error["message"],
        })
    );
    assert_eq!(got(&host, &default_mob).await["state"], json!("running"));
    shut_down(&host, &[]).await;
}

/// A mob a member created is a mob of the host's mob set: it is listed, and
/// `get` reads it with who created it. It is its owner's to stop, resume
/// and destroy: `lifecycle` and `destroy` answer for it as for an id the
/// host does not have, and it runs on.
#[tokio::test]
async fn a_mob_a_member_created_is_read_and_never_changed_by_the_hosts_methods() {
    let temp = tempfile::tempdir().expect("tempdir");
    let default_mob = id("m");
    let host = host(&temp, &default_mob).await;
    let child = member_created_mob(&host, &id("c")).await;

    let mobs = listed(&host).await;
    let entry = mobs
        .iter()
        .find(|mob| mob["mob_id"] == json!(child.as_str()))
        .expect("a mob a member created is listed");
    assert_eq!(entry["default"], json!(false));
    assert_eq!(entry["state"], json!("running"));
    assert_eq!(
        entry["provenance"],
        json!("member"),
        "the mob set's records name who created the mob, without a read: {entry}"
    );
    assert!(
        entry.get("revision").is_none(),
        "a list reads no mob's event log: {entry}"
    );
    let read = got(&host, child.as_str()).await;
    assert_eq!(read["provenance"], json!("member"), "{read}");

    for (method, params) in [
        (
            "meta/mobs/lifecycle",
            json!({"mob_id": child.as_str(), "action": "stop"}),
        ),
        (
            "meta/mobs/redefine",
            redefines(child.as_str(), config(child.as_str()), 1),
        ),
        ("meta/mobs/destroy", json!({"mob_id": child.as_str()})),
    ] {
        let error = refusal(&host, method, params).await;
        assert!(
            is_not_a_mob_of_the_host(&error, child.as_str(), &default_mob),
            "{method} of a mob a member created: {error}"
        );
    }
    assert_eq!(got(&host, child.as_str()).await["state"], json!("running"));
    shut_down(&host, &[child.as_str()]).await;
}

/// The host's records judge a mob before a request's own parameters are
/// read. A mob one of the host's agents created is answered as an id the
/// host does not have by `lifecycle`, `redefine` and `destroy` also when
/// the request for it is malformed. The same malformed requests for a mob
/// the host created are refused for their parameters. Neither mob is
/// changed.
#[tokio::test]
async fn a_mob_a_member_created_is_refused_before_a_requests_parameters_are_read() {
    let temp = tempfile::tempdir().expect("tempdir");
    let default_mob = id("m");
    let host = host(&temp, &default_mob).await;
    let child = member_created_mob(&host, &id("c")).await;
    let second = id("s");
    result(
        &host,
        "meta/mobs/create",
        json!({"mob_config": config(&second)}),
    )
    .await;
    let malformed = |mob_id: &str| {
        [
            (
                "meta/mobs/lifecycle",
                json!({"mob_id": mob_id, "action": "sideways"}),
            ),
            ("meta/mobs/redefine", json!({"mob_id": mob_id})),
            (
                "meta/mobs/destroy",
                json!({"mob_id": mob_id, "and": "more"}),
            ),
        ]
    };

    for (method, params) in malformed(child.as_str()) {
        let error = refusal(&host, method, params).await;
        assert!(
            is_not_a_mob_of_the_host(&error, child.as_str(), &default_mob),
            "{method} of a mob a member created, malformed: {error}"
        );
    }
    for (method, params) in malformed(second.as_str()) {
        let error = refusal(&host, method, params).await;
        assert!(
            error["code"] == json!(-32602) && error["data"]["kind"] != json!("foreign_mob_target"),
            "{method} of a mob the host created, malformed, is refused for its parameters: \
             {error}"
        );
    }
    assert_eq!(got(&host, child.as_str()).await["state"], json!("running"));
    assert_eq!(got(&host, &second).await["state"], json!("running"));
    shut_down(&host, &[child.as_str(), second.as_str()]).await;
}

/// The family's methods are the host's: a request that names a mob the host
/// does not have with the addressing id is refused on `list` and `create`
/// as on every method, and the launch mob's id there changes nothing.
#[tokio::test]
async fn list_and_create_take_the_addressing_id_like_a_method_of_the_host() {
    let temp = tempfile::tempdir().expect("tempdir");
    let default_mob = id("m");
    let host = host(&temp, &default_mob).await;

    let error = refusal(&host, "meta/mobs/list", json!({"mob_id": "no-such-mob"})).await;
    assert!(
        is_not_a_mob_of_the_host(&error, "no-such-mob", &default_mob),
        "{error}"
    );
    let named = result(&host, "meta/mobs/list", json!({"mob_id": default_mob})).await;
    assert_eq!(named["mobs"].as_array().map(Vec::len), Some(1));
    let second = id("s");
    let created = result(
        &host,
        "meta/mobs/create",
        json!({"mob_id": default_mob, "mob_config": config(&second)}),
    )
    .await;
    assert_eq!(created["mob"]["mob_id"], json!(second));
    shut_down(&host, &[&second]).await;
}

/// A launch that must act nowhere refuses the methods that change the mobs
/// of the host, and answers the reads.
#[test]
fn a_candidate_launch_refuses_the_changes_and_answers_the_reads() {
    for (method, refused) in [
        ("meta/mobs/create", true),
        ("meta/mobs/lifecycle", true),
        ("meta/mobs/redefine", true),
        ("meta/mobs/destroy", true),
        ("meta/mobs/list", false),
        ("meta/mobs/get", false),
    ] {
        assert_eq!(
            crate::rpc::candidate_launch_refuses(method, &json!({}), None),
            refused,
            "{method}"
        );
    }
}

/// A host whose launch declared its mob to no mob set serves none of the
/// family and announces nothing of it, on the stdio dispatch and on the
/// console, though its requests resolve the mobs of its mob set. That a
/// request resolves a second mob is one of the two things a host needs;
/// the other is its own: its launch declared its mob to its mob set. The
/// set of this host exists and holds no launch mob, so its mobs are not
/// this host's to list, create or destroy. Each of the six is answered as
/// a method the gateway does not know, on both doors, and nothing is
/// stopped or destroyed.
#[tokio::test]
async fn a_host_whose_launch_declared_its_mob_to_no_mob_set_serves_nothing_though_its_requests_resolve_a_second_mob()
 {
    let runtime = UnifiedRuntime::builder()
        .definition(definition(&id("m")))
        .default_llm_client(Arc::new(meerkat_client::TestClient::default()))
        .build()
        .await
        .expect("runtime builds");
    assert!(
        runtime.mob_resolver().resolves_host_mobs(),
        "the fixture of this row is a host whose requests resolve the mobs of its mob set"
    );
    assert!(
        runtime
            .mob_runtime()
            .agent_mob_mcp_state()
            .is_some_and(|set| set.default_mob_id().is_none()),
        "the fixture of this row is a launch whose mob set holds no launch mob"
    );
    let default_mob = runtime.mob_handle().mob_id().to_string();
    let requests = || {
        [
            ("meta/mobs/list", json!({})),
            ("meta/mobs/create", json!({"mob_config": config(&id("s"))})),
            ("meta/mobs/get", json!({"mob_id": default_mob})),
            (
                "meta/mobs/lifecycle",
                json!({"mob_id": default_mob, "action": "stop"}),
            ),
            (
                "meta/mobs/redefine",
                json!({"mob_id": default_mob, "mob_config": config(&default_mob), "expected_revision": 1}),
            ),
            ("meta/mobs/destroy", json!({"mob_id": default_mob})),
        ]
    };

    let (features, methods) = announced(&result(&runtime, "meta/capabilities", json!({})).await);
    assert!(
        !features.iter().any(|name| name == "hosted_mobs"),
        "the host names no feature: {features:?}"
    );
    assert!(
        !methods
            .iter()
            .any(|method| method.starts_with("meta/mobs/")),
        "and lists none of the methods: {methods:?}"
    );
    for (method, params) in requests() {
        let error = refusal(&runtime, method, params).await;
        assert_eq!(error["code"], json!(-32601), "{method}: {error}");
        assert_eq!(error["message"], json!("Method not found"), "{method}");
    }

    // The console of that host: the same answers, and the same silence.
    let app = console_of(&runtime, false);
    let (features, methods) =
        announced(&console_result(&app, "meta/capabilities", json!({})).await);
    assert!(
        !features.iter().any(|name| name == "hosted_mobs"),
        "the console names no feature: {features:?}"
    );
    assert!(
        !methods
            .iter()
            .any(|method| method.starts_with("meta/mobs/")),
        "and lists none of the methods: {methods:?}"
    );
    for (method, params) in requests() {
        let error = console_refusal(&app, method, params).await;
        assert_eq!(
            error["code"],
            json!(-32601),
            "{method} on the console: {error}"
        );
        assert_eq!(
            error["message"],
            json!("Method not found"),
            "{method} on the console"
        );
    }
    drop(app);

    assert_eq!(
        runtime.mob_handle().lifecycle_observation().phase,
        meerkat_mob::MobState::Running,
        "nothing was stopped or destroyed"
    );
    let _ = tokio::time::timeout(Duration::from_secs(30), runtime.shutdown()).await;
}
/// The reference application of `host`, as an open console: every caller
/// may call every method. A read-only console refuses changes.
fn console_of(host: &UnifiedRuntime, read_only: bool) -> axum::Router {
    host.build_reference_app_router(crate::runtime::RuntimeDecisionState::local_console(
        crate::ConsolePolicy {
            require_app_auth: false,
            read_only,
            ..Default::default()
        },
        None,
    ))
}

/// One request on the console RPC: its whole answer.
async fn ask_console(app: &axum::Router, method: &str, params: Value) -> Value {
    use tower::ServiceExt;
    let request = axum::http::Request::builder()
        .method("POST")
        .uri("/console/rpc")
        .header(axum::http::header::CONTENT_TYPE, "application/json")
        .body(axum::body::Body::from(
            json!({"jsonrpc": "2.0", "id": "row", "method": method, "params": params}).to_string(),
        ))
        .expect("console request");
    let response = tokio::time::timeout(Duration::from_mins(1), app.clone().oneshot(request))
        .await
        .expect("the console answers in time")
        .expect("the console answers");
    let body = axum::body::to_bytes(response.into_body(), 1 << 22)
        .await
        .expect("the answer's body");
    serde_json::from_slice(&body).expect("a JSON answer")
}

/// The result of a console request that must succeed.
async fn console_result(app: &axum::Router, method: &str, params: Value) -> Value {
    let answer = ask_console(app, method, params).await;
    assert!(
        answer.get("error").is_none(),
        "{method} succeeds on the console: {}",
        answer["error"]
    );
    answer["result"].clone()
}

/// The error of a console request that must be refused.
async fn console_refusal(app: &axum::Router, method: &str, params: Value) -> Value {
    let answer = ask_console(app, method, params).await;
    assert!(
        answer.get("result").is_none() && answer.get("error").is_some(),
        "{method} is refused on the console: {answer}"
    );
    answer["error"].clone()
}

/// The console serves the methods as the stdio dispatch does: it lists the
/// mobs of the host, creates one, reads it, stops it and destroys it, and
/// each answer is the one the host itself gets. An id the host does not
/// have is refused as a door that has callers refuses it: by the id the
/// request named and no other mob.
#[tokio::test]
async fn the_console_serves_the_methods_over_the_mobs_of_the_host() {
    let temp = tempfile::tempdir().expect("tempdir");
    let launch_mob = id("m");
    let host = host(&temp, &launch_mob).await;
    let app = console_of(&host, false);

    assert_eq!(
        console_result(&app, "meta/mobs/list", json!({})).await["mobs"],
        json!(listed(&host).await)
    );
    let second = id("s");
    let created = console_result(
        &app,
        "meta/mobs/create",
        json!({"mob_config": config(&second)}),
    )
    .await;
    assert_eq!(
        created,
        json!({"mob": {
            "mob_id": second,
            "default": false,
            "state": "running",
            "provenance": "host",
            "revision": 1,
        }})
    );
    assert_eq!(
        console_result(&app, "meta/mobs/get", json!({"mob_id": second})).await["mob"],
        got(&host, &second).await
    );
    assert_eq!(
        console_result(&app, "meta/mobs/list", json!({})).await["mobs"],
        json!(listed(&host).await),
        "the console and the host list the same mobs"
    );
    let stopped = console_result(
        &app,
        "meta/mobs/lifecycle",
        json!({"mob_id": second, "action": "stop"}),
    )
    .await;
    assert_eq!(stopped["mob"]["state"], json!("stopped"));
    assert!(stopped["stop_report"].is_object(), "{stopped}");
    assert_eq!(got(&host, &launch_mob).await["state"], json!("running"));

    let own = console_refusal(&app, "meta/mobs/destroy", json!({"mob_id": launch_mob})).await;
    assert_eq!(own["code"], json!(REFUSED), "{own}");
    assert_eq!(own["data"]["kind"], json!("default_mob"), "{own}");

    let destroyed = console_result(&app, "meta/mobs/destroy", json!({"mob_id": second})).await;
    assert_eq!(destroyed["mob_id"], json!(second));
    assert_eq!(listed(&host).await.len(), 1, "the host has one mob again");

    for method in ["meta/mobs/get", "meta/mobs/destroy"] {
        let error = console_refusal(&app, method, json!({"mob_id": second})).await;
        assert_eq!(error["code"], json!(-32602), "{method}: {error}");
        assert_eq!(
            error["data"],
            json!({"kind": "foreign_mob_target", "requested_mob_id": second}),
            "{method}: a door that has callers names the requested id and no other mob"
        );
    }
    drop(app);
    shut_down(&host, &[]).await;
}

/// The handle of the mob `mob_id` of `host`, from the launch's mob set.
async fn handle_of(host: &UnifiedRuntime, mob_id: &MobId) -> meerkat_mob::MobHandle {
    set_of(host)
        .handle_for(mob_id)
        .await
        .expect("the set serves the mob")
}

/// Seat the agent `name` in `mob` through the mob's own handle, private to
/// the person `private_to` or to no one. The handle is let go before this
/// returns: a mob whose handle is held is not destroyed to its end.
async fn seat(mob: meerkat_mob::MobHandle, name: &str, private_to: Option<&str>) {
    let spec = SpawnMemberSpec::new("worker", name);
    let spec = match private_to {
        Some(person) => spec.with_labels(std::collections::BTreeMap::from([(
            crate::access::PRIVATE_TO_LABEL.to_string(),
            person.to_string(),
        )])),
        None => spec,
    };
    Box::pin(mob.spawn_spec(spec))
        .await
        .expect("the agent is seated");
}

/// A mob the Console creates under the id of a mob it destroyed is judged
/// by none of the attributes the Console read of the earlier mob's agents.
///
/// A host that keeps its mobs in stores, with the access controller its
/// Console judges with (the runtime's own, given before the Console is
/// built), a second and a third mob created on the Console, and agents
/// seated in each. A Console request served for a mob reads that mob's
/// roster into the controller. The row reads the controller as the
/// announcements' audience reads it for a member of another mob, for a
/// person on a console that identifies its people: whether the name is
/// known, and whether the member is hidden (it is private to someone
/// else, or the name is not known). It also reads whether an alias
/// recorded for an agent of the earlier mob still counts as that name's
/// current one.
///
/// The second mob's `desk` is private to no one, and so is its `courier`.
/// After the destroy on the Console the entries are kept. After the create
/// on the Console under the id, where `desk` is private to a person and no
/// `courier` exists, and before any request is served for the new mob:
/// neither name is known, and both are hidden. After a request served for
/// the new mob: `desk` is known and hidden as the new agent is private,
/// `courier` is not known, and the earlier alias is no current one. The
/// third mob's agent and the launch mob's are read as before throughout.
#[tokio::test]
async fn a_mob_the_console_creates_under_a_destroyed_mobs_id_is_judged_by_none_of_the_earlier_mobs_agents()
 {
    const OWNER: &str = "person:owner@example.test";
    const OTHER: &str = "other@example.test";
    /// What a view answers for a name: (known, hidden).
    type Answers = (bool, bool);
    /// One reading of the row.
    type Reading = (String, Answers, Answers);
    let temp = tempfile::tempdir().expect("tempdir");
    let launch_mob = id("m");
    let mut host = stored_host(&temp, &launch_mob).await;
    let access = crate::AccessController::new(crate::access::AccessControlConfig::default())
        .expect("an access controller with no rules");
    host.set_access_controller(access.clone());
    let app = console_of(&host, false);
    let (second, third) = (id("s"), id("t"));
    let (second_id, third_id) = (MobId::from(second.as_str()), MobId::from(third.as_str()));
    for mob in [&second, &third] {
        console_result(&app, "meta/mobs/create", json!({"mob_config": config(mob)})).await;
    }
    seat(host.mob_handle(), "desk", None).await;
    seat(handle_of(&host, &second_id).await, "desk", None).await;
    seat(handle_of(&host, &second_id).await, "courier", None).await;
    seat(handle_of(&host, &third_id).await, "scout", Some(OWNER)).await;
    // A request served for each mob: the Console reads that mob's roster.
    for served in [
        json!({}),
        json!({"mob_id": second}),
        json!({"mob_id": third}),
    ] {
        console_result(&app, "meta/list_members", served).await;
    }
    // What a read of an identity plane records beside an agent: its
    // current runtime alias.
    access.record_current_runtime_aliases_in(
        &second_id,
        [("desk".to_string(), Some("rt:desk:1".to_string()))],
    );

    // (known, hidden) of `name` in `mob` (none: the launch mob) for the
    // other person, and whether the earlier alias of the second mob's
    // `desk` names a current agent.
    let view_of = |mob: Option<&MobId>| {
        access
            .view_for_caller(Some(OTHER), Some("person:other@example.test"))
            .on_console(true)
            .in_stored_mob(mob)
    };
    let answers = |mob: Option<&MobId>, name: &str| {
        let view = view_of(mob);
        (
            view.knows_agent(name),
            view.hides_agent(name) || view.withholds_unknown_member(name),
        )
    };
    let alias_is_current = || view_of(Some(&second_id)).lineage_links("rt:desk:1") > 0;
    let shown = (true, false);
    let private = (true, true);
    let not_known = (false, true);
    // Each reading: what was read, what was found, what is wanted.
    let mut readings: Vec<Reading> = Vec::new();
    let others = |when: &str| {
        [
            (
                format!("{when}: the third mob's `scout`"),
                answers(Some(&third_id), "scout"),
                private,
            ),
            (
                format!("{when}: the launch mob's `desk`"),
                answers(None, "desk"),
                shown,
            ),
        ]
    };

    // The scene: without these entries the row shows nothing of a create.
    for name in ["desk", "courier"] {
        readings.push((
            format!("before the destroy: the second mob's `{name}`"),
            answers(Some(&second_id), name),
            shown,
        ));
    }
    readings.extend(others("before the destroy"));
    let alias_before = alias_is_current();

    console_result(&app, "meta/mobs/destroy", json!({"mob_id": second})).await;
    for _ in 0..200 {
        if !host.mob_entries_in_tests().contains(&second_id) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    // The readings after the destroy.
    for name in ["desk", "courier"] {
        readings.push((
            format!("after the destroy: the destroyed mob's `{name}`"),
            answers(Some(&second_id), name),
            shown,
        ));
    }

    console_result(
        &app,
        "meta/mobs/create",
        json!({"mob_config": config(&second)}),
    )
    .await;
    seat(handle_of(&host, &second_id).await, "desk", Some(OWNER)).await;
    for name in ["desk", "courier"] {
        readings.push((
            format!("after the create, before a request is served for the new mob: `{name}`"),
            answers(Some(&second_id), name),
            not_known,
        ));
    }
    readings.extend(others("after the create"));

    console_result(&app, "meta/list_members", json!({"mob_id": second})).await;
    readings.push((
        "after a request served for the new mob: the new mob's `desk`, private to a person"
            .to_string(),
        answers(Some(&second_id), "desk"),
        private,
    ));
    readings.push((
        "after a request served for the new mob: `courier`, a name only the earlier mob held"
            .to_string(),
        answers(Some(&second_id), "courier"),
        not_known,
    ));
    readings.extend(others("after a request served for the new mob"));
    let alias_after = alias_is_current();

    let mut wrong: Vec<String> = readings
        .into_iter()
        .filter(|(_, found, wanted)| found != wanted)
        .map(|(what, found, wanted)| {
            format!("{what}: (known, hidden) is {found:?}, not {wanted:?}")
        })
        .collect();
    if (alias_before, alias_after) != (true, false) {
        wrong.push(format!(
            "the earlier `desk`'s alias: current (before the destroy, after the new mob was \
             read) is ({alias_before}, {alias_after}), not (true, false)"
        ));
    }

    drop(app);
    shut_down(&host, &[&second, &third]).await;
    assert!(
        wrong.is_empty(),
        "a mob the Console created under a destroyed mob's id met the earlier mob's \
         attributes:\n{}",
        wrong.join("\n")
    );
}

/// A member build's request, as the pre-build hook of the base session
/// layer is handed it: for the member `member` of the mob `mob_id`, private
/// to the person `private_to` or to no one, built in the mob at
/// `incarnation` (none: a build that names no incarnation).
fn member_build(
    mob_id: &MobId,
    member: &str,
    private_to: Option<&str>,
    incarnation: Option<meerkat_core::service::LiveMobIncarnation>,
) -> meerkat_core::service::CreateSessionRequest {
    let labels = private_to
        .map(|person| {
            std::collections::BTreeMap::from([(
                crate::access::PRIVATE_TO_LABEL.to_string(),
                person.to_string(),
            )])
        })
        .unwrap_or_default();
    meerkat_core::service::CreateSessionRequest {
        model: "gpt-5.5".to_string(),
        prompt: meerkat_core::types::ContentInput::Text("hello".to_string()),
        system_prompt: meerkat_core::config::SystemPromptOverride::Inherit,
        max_tokens: None,
        event_tx: None,
        initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
        build: Some(meerkat_core::service::SessionBuildOptions {
            mob_member_binding: Some(meerkat_core::MobMemberBinding {
                mob_id: mob_id.to_string(),
                role: "worker".to_string(),
                member: member.to_string(),
            }),
            mob_incarnation: incarnation,
            ..Default::default()
        }),
        labels: Some(labels),
        deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::default(),
        injected_context: Vec::new(),
    }
}

/// Hand `request` to the launch's record of whom each member is private
/// to, as the pre-build hook of a member build does.
async fn built(host: &UnifiedRuntime, mut request: meerkat_core::service::CreateSessionRequest) {
    let nothing_before: crate::mob_handle_runtime::PreBuildHook = Arc::new(
        |_request: &mut meerkat_core::service::CreateSessionRequest| Box::pin(async { Ok(()) }),
    );
    let hook = host
        .mob_runtime()
        .private_owners()
        .after_hook(nothing_before);
    hook(&mut request).await.expect("the hook runs");
}

/// What the launch's record holds of the member `member` of the mob
/// `mob_id`: whom it is private to, that it is private to no one, or that
/// the record does not hold it.
fn held(
    host: &UnifiedRuntime,
    mob_id: &MobId,
    member: &str,
) -> crate::mob_announcements::MemberPerson {
    host.mob_runtime()
        .private_owners()
        .person_of(mob_id.as_str(), member)
}

/// The record of whom each member is private to takes no write from a
/// member build of a mob that has ended. A mob of the host with a private
/// member is destroyed by the mob set and a mob is created under its id,
/// with a member of the same name that is private to no one. A build of
/// the earlier mob that reaches the record after the create writes
/// nothing: the standing member of that name is still private to no one,
/// and a name only the earlier mob held is not held. A build that names no
/// incarnation is recorded, and so is a build of the standing mob. The
/// members of another mob and of the launch mob are held as before.
#[tokio::test]
async fn a_member_build_of_an_ended_mob_writes_nothing_in_the_owner_record() {
    use crate::mob_announcements::MemberPerson;
    const OWNER: &str = "person:owner@example.test";
    let temp = tempfile::tempdir().expect("tempdir");
    let launch_mob = id("m");
    let host = host(&temp, &launch_mob).await;
    let (second, third) = (id("s"), id("t"));
    let (launch_id, second_id, third_id) = (
        MobId::from(launch_mob.as_str()),
        MobId::from(second.as_str()),
        MobId::from(third.as_str()),
    );
    for mob in [&second, &third] {
        result(
            &host,
            "meta/mobs/create",
            json!({"mob_config": config(mob)}),
        )
        .await;
    }
    seat(host.mob_handle(), "ledger", Some(OWNER)).await;
    seat(handle_of(&host, &second_id).await, "desk", Some(OWNER)).await;
    seat(handle_of(&host, &third_id).await, "scout", Some(OWNER)).await;
    let earlier = handle_of(&host, &second_id).await.live_incarnation();
    let private = MemberPerson::Private(OWNER.to_string());
    assert_eq!(
        [
            held(&host, &second_id, "desk"),
            held(&host, &third_id, "scout"),
            held(&host, &launch_id, "ledger"),
        ],
        [private.clone(), private.clone(), private.clone()],
        "the scene: each member's own build recorded whom it is private to"
    );

    // The mob set destroys the second mob, and a mob is created under its
    // id with a `desk` that is private to no one.
    let destroyed = Box::pin(set_of(&host).mob_destroy(&second_id)).await;
    assert!(
        destroyed.is_ok(),
        "the set destroys the second mob: {:?}",
        destroyed.as_ref().err()
    );
    for _ in 0..200 {
        if !host.mob_entries_in_tests().contains(&second_id) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    result(
        &host,
        "meta/mobs/create",
        json!({"mob_config": config(&second)}),
    )
    .await;
    seat(handle_of(&host, &second_id).await, "desk", None).await;
    let standing = handle_of(&host, &second_id).await.live_incarnation();
    assert_ne!(
        earlier.incarnation(),
        standing.incarnation(),
        "the scene: the mob created under the id is another incarnation"
    );
    assert_eq!(
        held(&host, &second_id, "desk"),
        MemberPerson::Shared,
        "the scene: the standing mob's `desk` is private to no one"
    );

    // Builds of the earlier mob reach the record now.
    built(
        &host,
        member_build(&second_id, "desk", Some(OWNER), Some(earlier)),
    )
    .await;
    built(
        &host,
        member_build(&second_id, "courier", Some(OWNER), Some(earlier)),
    )
    .await;
    // A build that names no incarnation, and a build of the standing mob.
    built(&host, member_build(&second_id, "porter", Some(OWNER), None)).await;
    built(
        &host,
        member_build(&second_id, "clerk", Some(OWNER), Some(standing)),
    )
    .await;

    assert_eq!(
        [
            held(&host, &second_id, "desk"),
            held(&host, &second_id, "courier"),
        ],
        [MemberPerson::Shared, MemberPerson::NotKnown],
        "a build of the ended mob writes nothing: (the standing member of the name, a name \
         only the ended mob held)"
    );
    assert_eq!(
        [
            held(&host, &second_id, "porter"),
            held(&host, &second_id, "clerk"),
            held(&host, &third_id, "scout"),
            held(&host, &launch_id, "ledger"),
        ],
        [private.clone(), private.clone(), private.clone(), private,],
        "(a build that names no incarnation, a build of the standing mob, another mob's \
         member, the launch mob's member)"
    );
    shut_down(&host, &[&second, &third]).await;
}

/// A mob the host's own destroy ended: a build of it that reaches the
/// record of whom each member is private to afterwards writes nothing,
/// with no mob created under the id since. A build under the id that names
/// no incarnation is recorded.
#[tokio::test]
async fn a_member_build_of_a_mob_the_host_destroyed_writes_nothing_in_the_owner_record() {
    use crate::mob_announcements::MemberPerson;
    const OWNER: &str = "person:owner@example.test";
    let temp = tempfile::tempdir().expect("tempdir");
    let launch_mob = id("m");
    let host = host(&temp, &launch_mob).await;
    let second = id("s");
    let second_id = MobId::from(second.as_str());
    result(
        &host,
        "meta/mobs/create",
        json!({"mob_config": config(&second)}),
    )
    .await;
    let earlier = handle_of(&host, &second_id).await.live_incarnation();
    let destroyed = result(&host, "meta/mobs/destroy", json!({"mob_id": second})).await;
    assert_eq!(destroyed["mob_id"], json!(second), "{destroyed}");

    built(
        &host,
        member_build(&second_id, "desk", Some(OWNER), Some(earlier)),
    )
    .await;
    built(&host, member_build(&second_id, "porter", Some(OWNER), None)).await;
    assert_eq!(
        [
            held(&host, &second_id, "desk"),
            held(&host, &second_id, "porter"),
        ],
        [
            MemberPerson::NotKnown,
            MemberPerson::Private(OWNER.to_string()),
        ],
        "(a build of the mob the host destroyed, a build that names no incarnation)"
    );
    shut_down(&host, &[]).await;
}

/// A destroy by the host that could not finish and that its repeat
/// continued has ended its mob as well: a build of that mob that reaches
/// the record of whom each member is private to after the repeat writes
/// nothing, with no mob created under the id since. A build under the id
/// that names no incarnation is recorded.
#[tokio::test]
async fn a_member_build_of_a_mob_whose_destroy_its_repeat_continued_writes_nothing_in_the_owner_record()
 {
    use crate::mob_announcements::MemberPerson;
    const OWNER: &str = "person:owner@example.test";
    let temp = tempfile::tempdir().expect("tempdir");
    let launch_mob = id("m");
    let host = stored_host(&temp, &launch_mob).await;
    let second = id("s");
    let second_id = MobId::from(second.as_str());
    result(
        &host,
        "meta/mobs/create",
        json!({"mob_config": config(&second)}),
    )
    .await;
    let earlier = handle_of(&host, &second_id).await.live_incarnation();
    // The mob is destroyed while a reader holds its store open: the
    // destroy's storage cleanup stays owed, and its repeat continues it
    // once the reader has gone.
    let reader = meerkat_mob::MobStorage::persistent(store_file_of(&temp, &second))
        .expect("a reader of the mob's store");
    let incomplete = refusal(&host, "meta/mobs/destroy", json!({"mob_id": second})).await;
    assert!(
        incomplete["message"]
            .as_str()
            .is_some_and(|message| message.contains("storage cleanup outstanding")),
        "the scene: the destroy answers what it still owes: {incomplete}"
    );
    drop(reader);
    let repeated = result(&host, "meta/mobs/destroy", json!({"mob_id": second})).await;
    assert_eq!(
        repeated["mob_id"],
        json!(second),
        "the scene: the repeat answers the destroy: {repeated}"
    );

    built(
        &host,
        member_build(&second_id, "desk", Some(OWNER), Some(earlier)),
    )
    .await;
    built(&host, member_build(&second_id, "porter", Some(OWNER), None)).await;
    assert_eq!(
        [
            held(&host, &second_id, "desk"),
            held(&host, &second_id, "porter"),
        ],
        [
            MemberPerson::NotKnown,
            MemberPerson::Private(OWNER.to_string()),
        ],
        "(a build of the mob whose destroy the repeat continued, a build that names no \
         incarnation)"
    );
    shut_down(&host, &[]).await;
}

/// A member build of a mob that stands is recorded, also after a destroy
/// of that mob was asked and answered. A mob of the host is destroyed and
/// a mob is created under its id. The destroy of the later mob judges it
/// and is held before the mob set is asked; the mob is unloaded meanwhile,
/// and the request is then answered while the later mob stands, not
/// loaded, at its incarnation. A build of the later mob that reaches the
/// record of whom each member is private to afterwards is recorded.
#[tokio::test]
async fn a_member_build_of_a_mob_that_stands_after_a_destroy_of_it_was_answered_is_recorded() {
    use crate::mob_announcements::MemberPerson;
    const OWNER: &str = "person:owner@example.test";
    let temp = tempfile::tempdir().expect("tempdir");
    let launch_mob = id("m");
    let host = stored_host(&temp, &launch_mob).await;
    let second = id("s");
    let second_id = MobId::from(second.as_str());
    result(
        &host,
        "meta/mobs/create",
        json!({"mob_config": config(&second)}),
    )
    .await;
    let destroyed = result(&host, "meta/mobs/destroy", json!({"mob_id": second})).await;
    assert_eq!(
        destroyed["mob_id"],
        json!(second),
        "the scene: a destroy under the id that is done: {destroyed}"
    );
    for _ in 0..200 {
        if !host.mob_entries_in_tests().contains(&second_id) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    // The later mob, stopped: a stopped mob is unloaded when it is idle.
    result(
        &host,
        "meta/mobs/create",
        json!({"mob_config": config(&second)}),
    )
    .await;
    result(
        &host,
        "meta/mobs/lifecycle",
        json!({"mob_id": second, "action": "stop"}),
    )
    .await;
    let standing = handle_of(&host, &second_id).await.live_incarnation();

    let hold = crate::rpc::mob_hosting::destroy_seam::hold_the_next_destroy_of(&second);
    let late = ask(&host, "meta/mobs/destroy", json!({"mob_id": second}));
    tokio::pin!(late);
    until_held(late.as_mut(), &hold).await;
    let set = set_of(&host);
    set.unload_idle_mobs_for_tests().await;
    let not_loaded_when_asked = set
        .mob_inventory()
        .await
        .expect("the inventory reads")
        .dormant
        .contains(&second_id);
    hold.release.notify_one();
    let answer = late.await;
    let stands_at_its_incarnation = handle_of(&host, &second_id)
        .await
        .live_incarnation()
        .incarnation()
        == standing.incarnation();

    built(
        &host,
        member_build(&second_id, "desk", Some(OWNER), Some(standing)),
    )
    .await;
    let recorded = held(&host, &second_id, "desk");
    shut_down(&host, &[&second]).await;
    assert!(
        not_loaded_when_asked,
        "the scene: the later mob is not loaded when the mob set is asked"
    );
    assert!(
        stands_at_its_incarnation,
        "the scene: the later mob stands at its incarnation after the request was answered: \
         {answer}"
    );
    assert_eq!(
        recorded,
        MemberPerson::Private(OWNER.to_string()),
        "a build of the mob that stands is recorded"
    );
}

/// A read-only console answers the reads and refuses the three changes
/// with its own refusal, and nothing is created, stopped or destroyed.
#[tokio::test]
async fn a_read_only_console_refuses_the_changes_and_answers_the_reads() {
    let temp = tempfile::tempdir().expect("tempdir");
    let launch_mob = id("m");
    let host = host(&temp, &launch_mob).await;
    let second = id("s");
    result(
        &host,
        "meta/mobs/create",
        json!({"mob_config": config(&second)}),
    )
    .await;
    let app = console_of(&host, true);

    assert_eq!(
        console_result(&app, "meta/mobs/list", json!({})).await["mobs"],
        json!(listed(&host).await)
    );
    assert_eq!(
        console_result(&app, "meta/mobs/get", json!({"mob_id": second})).await["mob"],
        got(&host, &second).await
    );
    for (method, params) in [
        ("meta/mobs/create", json!({"mob_config": config(&id("t"))})),
        (
            "meta/mobs/lifecycle",
            json!({"mob_id": second, "action": "stop"}),
        ),
        (
            "meta/mobs/redefine",
            json!({"mob_id": second, "mob_config": config(&second), "expected_revision": 1}),
        ),
        ("meta/mobs/destroy", json!({"mob_id": second})),
    ] {
        let error = console_refusal(&app, method, params).await;
        assert_eq!(error["code"], json!(-32010), "{method}: {error}");
        assert_eq!(
            error["data"]["kind"],
            json!("read_only"),
            "{method}: {error}"
        );
    }
    assert_eq!(
        listed(&host).await,
        vec![
            json!({"mob_id": launch_mob, "default": true, "state": "running", "provenance": "host"}),
            json!({"mob_id": second, "default": false, "state": "running", "provenance": "host"}),
        ],
        "a refused change changed nothing"
    );
    drop(app);
    shut_down(&host, &[&second]).await;
}

/// The names of `meta/capabilities.feature_capabilities` and `methods` in
/// an answer.
fn announced(capabilities: &Value) -> (Vec<String>, Vec<String>) {
    let names = |member: &str| -> Vec<String> {
        capabilities[member]
            .as_array()
            .map(|names| {
                names
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    };
    (names("feature_capabilities"), names("methods"))
}

/// A gateway that serves the methods says so: `meta/capabilities` names
/// the feature `hosted_mobs` and lists the six methods, on the stdio
/// dispatch and on the console; a read-only console lists the reads alone
/// and still names the feature. A host whose launch declared its mob to no
/// mob set serves none of them and announces nothing of them.
#[tokio::test]
async fn a_gateway_that_serves_the_methods_announces_them() {
    const METHODS: [&str; 6] = [
        "meta/mobs/create",
        "meta/mobs/destroy",
        "meta/mobs/get",
        "meta/mobs/lifecycle",
        "meta/mobs/list",
        "meta/mobs/redefine",
    ];
    let temp = tempfile::tempdir().expect("tempdir");
    let host = host(&temp, &id("m")).await;

    let (features, methods) = announced(&result(&host, "meta/capabilities", json!({})).await);
    assert_eq!(
        features
            .iter()
            .filter(|name| name.as_str() == "hosted_mobs")
            .count(),
        1,
        "the stdio dispatch names the feature once: {features:?}"
    );
    for method in METHODS {
        assert!(
            methods.iter().any(|listed| listed == method),
            "the stdio dispatch lists {method}"
        );
    }

    let app = console_of(&host, false);
    let (features, methods) =
        announced(&console_result(&app, "meta/capabilities", json!({})).await);
    assert!(
        features.iter().any(|name| name == "hosted_mobs"),
        "the console names the feature: {features:?}"
    );
    for method in METHODS {
        assert!(
            methods.iter().any(|listed| listed == method),
            "the console lists {method}"
        );
    }
    drop(app);

    let read_only = console_of(&host, true);
    let (features, methods) =
        announced(&console_result(&read_only, "meta/capabilities", json!({})).await);
    assert!(features.iter().any(|name| name == "hosted_mobs"));
    let listed_of_the_family: Vec<&str> = methods
        .iter()
        .map(String::as_str)
        .filter(|method| method.starts_with("meta/mobs/"))
        .collect();
    assert_eq!(
        listed_of_the_family,
        vec!["meta/mobs/get", "meta/mobs/list"],
        "a read-only console lists the reads alone"
    );
    drop(read_only);
    shut_down(&host, &[]).await;

    let undeclared = UnifiedRuntime::builder()
        .definition(definition(&id("u")))
        .default_llm_client(Arc::new(meerkat_client::TestClient::default()))
        .build()
        .await
        .expect("runtime builds");
    let (features, methods) = announced(&result(&undeclared, "meta/capabilities", json!({})).await);
    assert!(
        !features.iter().any(|name| name == "hosted_mobs"),
        "a host that serves none of the methods names no feature: {features:?}"
    );
    assert!(
        !methods
            .iter()
            .any(|method| method.starts_with("meta/mobs/")),
        "and lists none of them: {methods:?}"
    );
    let _ = tokio::time::timeout(Duration::from_secs(30), undeclared.shutdown()).await;
}

/// [`config`] with a second profile, `reviewer`.
fn config_with_a_reviewer(mob_id: &str) -> String {
    format!(
        "{}\n[profiles.reviewer]\nmodel = \"gpt-5.5\"\n\n[profiles.reviewer.tools]\ncomms = true\n",
        config(mob_id)
    )
}

/// How many profiles the definition of the mob `mob_id` has, as the mob set
/// serves the mob now.
async fn profiles_of(host: &UnifiedRuntime, mob_id: &str) -> usize {
    set_of(host)
        .handle_for(&MobId::from(mob_id))
        .await
        .expect("the mob set serves the mob")
        .definition()
        .profiles
        .len()
}

fn redefines(mob_id: &str, mob_config: String, expected_revision: u64) -> Value {
    json!({"mob_id": mob_id, "mob_config": mob_config, "expected_revision": expected_revision})
}

/// `redefine` replaces the definition of a stopped mob the host created, on
/// the revision `get` reads: the answer is the mob's entry, still stopped
/// and at the next revision, and the receipt of the commit. The mob set
/// serves the mob with the new definition, the mob resumes with it, and the
/// launch mob is as it was. The console serves the method too.
#[tokio::test]
async fn redefine_replaces_a_stopped_mobs_definition_and_leaves_it_stopped() {
    let temp = tempfile::tempdir().expect("tempdir");
    let launch_mob = id("m");
    let host = stored_host(&temp, &launch_mob).await;
    let second = id("s");
    result(
        &host,
        "meta/mobs/create",
        json!({"mob_config": config(&second)}),
    )
    .await;
    assert_eq!(got(&host, &second).await["revision"], json!(1));
    result(
        &host,
        "meta/mobs/lifecycle",
        json!({"mob_id": second, "action": "stop"}),
    )
    .await;

    let redefined = result(
        &host,
        "meta/mobs/redefine",
        redefines(&second, config_with_a_reviewer(&second), 1),
    )
    .await;
    assert_eq!(
        redefined["mob"],
        json!({
            "mob_id": second,
            "default": false,
            "state": "stopped",
            "provenance": "host",
            "revision": 2,
        }),
        "the redefined mob is loaded again, stopped, at the next revision"
    );
    let receipt = &redefined["receipt"];
    assert_eq!(
        members(receipt),
        vec![
            "committed_revision",
            "declared_fields",
            "event_cursor",
            "mob_id",
            "previous_revision",
            "projection_revision",
        ],
        "{receipt}"
    );
    assert_eq!(receipt["mob_id"], json!(second), "{receipt}");
    assert_eq!(receipt["previous_revision"], json!(1), "{receipt}");
    assert_eq!(receipt["committed_revision"], json!(2), "{receipt}");
    assert!(
        receipt["projection_revision"].is_u64() && receipt["event_cursor"].is_u64(),
        "{receipt}"
    );
    assert!(
        receipt["declared_fields"]
            .as_array()
            .is_some_and(|fields| fields.iter().any(|field| field
                .as_str()
                .is_some_and(|field| field.contains("profiles")))),
        "the receipt names what the new definition changed: {receipt}"
    );
    assert_eq!(got(&host, &second).await, redefined["mob"]);
    assert_eq!(profiles_of(&host, &second).await, 2);

    let launch = got(&host, &launch_mob).await;
    assert_eq!(launch["state"], json!("running"), "{launch}");
    assert_eq!(launch["revision"], json!(1), "{launch}");

    let resumed = result(
        &host,
        "meta/mobs/lifecycle",
        json!({"mob_id": second, "action": "resume"}),
    )
    .await;
    assert_eq!(resumed["mob"]["state"], json!("running"), "{resumed}");
    assert_eq!(resumed["mob"]["revision"], json!(2), "{resumed}");
    assert_eq!(profiles_of(&host, &second).await, 2);

    let app = console_of(&host, false);
    console_result(
        &app,
        "meta/mobs/lifecycle",
        json!({"mob_id": second, "action": "stop"}),
    )
    .await;
    let back = console_result(
        &app,
        "meta/mobs/redefine",
        redefines(&second, config(&second), 2),
    )
    .await;
    assert_eq!(back["mob"]["state"], json!("stopped"), "{back}");
    assert_eq!(back["mob"]["revision"], json!(3), "{back}");
    assert_eq!(back["receipt"]["previous_revision"], json!(2), "{back}");
    assert_eq!(profiles_of(&host, &second).await, 1);
    drop(app);
    shut_down(&host, &[&second]).await;
}

/// What `redefine` refuses, and that a refusal changes nothing: a mob that
/// is not stopped and the launch mob under the family's code; a revision
/// the mob is not at, a definition the mob has already and a definition
/// that names another mob with the refusal a launch answers for a
/// definition update it declared; a definition that does not parse and a
/// request without a revision as invalid params; an id the host does not
/// have as on every method of the family.
#[tokio::test]
async fn redefine_refuses_what_it_cannot_replace_and_changes_nothing() {
    let temp = tempfile::tempdir().expect("tempdir");
    let launch_mob = id("m");
    let host = stored_host(&temp, &launch_mob).await;
    let second = id("s");
    result(
        &host,
        "meta/mobs/create",
        json!({"mob_config": config(&second)}),
    )
    .await;

    let running = refusal(
        &host,
        "meta/mobs/redefine",
        redefines(&second, config_with_a_reviewer(&second), 1),
    )
    .await;
    assert_eq!(running["code"], json!(REFUSED), "{running}");
    assert_eq!(running["data"]["kind"], json!("mob_not_stopped"));
    assert_eq!(running["data"]["mob_id"], json!(second), "{running}");
    assert_eq!(running["data"]["retryable"], json!(false), "{running}");
    // The mob's phase is judged before the revision: a request that is
    // stale too is told that the mob is not stopped.
    let running_and_stale = refusal(
        &host,
        "meta/mobs/redefine",
        redefines(&second, config_with_a_reviewer(&second), 7),
    )
    .await;
    assert_eq!(
        running_and_stale["data"]["kind"],
        json!("mob_not_stopped"),
        "{running_and_stale}"
    );

    let own = refusal(
        &host,
        "meta/mobs/redefine",
        redefines(&launch_mob, config_with_a_reviewer(&launch_mob), 1),
    )
    .await;
    assert_eq!(own["code"], json!(REFUSED), "{own}");
    assert_eq!(own["data"]["kind"], json!("default_mob"), "{own}");
    assert_eq!(own["data"]["mob_id"], json!(launch_mob), "{own}");

    result(
        &host,
        "meta/mobs/lifecycle",
        json!({"mob_id": second, "action": "stop"}),
    )
    .await;
    let declared_update_refused = json!(crate::STORAGE_RESOLUTION_CODE);
    for (what, params, says) in [
        (
            "a revision the mob is not at",
            redefines(&second, config_with_a_reviewer(&second), 7),
            "is at revision 1, not the expected revision 7",
        ),
        (
            // The revision is judged before the definition.
            "a revision the mob is not at, with the definition the mob has",
            redefines(&second, config(&second), 7),
            "is at revision 1, not the expected revision 7",
        ),
        (
            "the definition the mob has",
            redefines(&second, config(&second), 1),
            "already matches",
        ),
        (
            "a definition that names another mob",
            redefines(&second, config(&id("x")), 1),
            "the definition it carries names",
        ),
    ] {
        let error = refusal(&host, "meta/mobs/redefine", params).await;
        assert_eq!(error["code"], declared_update_refused, "{what}: {error}");
        let message = error["message"].as_str().expect("a message");
        assert!(
            message.starts_with("declared mob spec update refused: ") && message.contains(says),
            "{what}: {error}"
        );
    }
    for (what, params) in [
        (
            "a definition that does not parse",
            redefines(&second, "[mob".to_string(), 1),
        ),
        (
            "no revision",
            json!({"mob_id": second, "mob_config": config_with_a_reviewer(&second)}),
        ),
    ] {
        let error = refusal(&host, "meta/mobs/redefine", params).await;
        assert_eq!(error["code"], json!(-32602), "{what}: {error}");
    }
    let unknown = id("n");
    let error = refusal(
        &host,
        "meta/mobs/redefine",
        redefines(&unknown, config(&unknown), 1),
    )
    .await;
    assert!(
        is_not_a_mob_of_the_host(&error, &unknown, &launch_mob),
        "{error}"
    );

    assert_eq!(
        got(&host, &second).await,
        json!({
            "mob_id": second,
            "default": false,
            "state": "stopped",
            "provenance": "host",
            "revision": 1,
        }),
        "no refusal changed the mob"
    );
    assert_eq!(profiles_of(&host, &second).await, 1);
    assert_eq!(got(&host, &launch_mob).await["revision"], json!(1));
    shut_down(&host, &[&second]).await;
}

/// A mob that lives in memory has no store the host could load it from
/// again, so its definition is not replaced: `redefine` answers the refusal
/// of a declared definition update, and the mob stays loaded and stopped.
#[tokio::test]
async fn redefine_of_a_mob_that_lives_in_memory_is_refused() {
    let temp = tempfile::tempdir().expect("tempdir");
    let launch_mob = id("m");
    let host = host(&temp, &launch_mob).await;
    let second = id("s");
    result(
        &host,
        "meta/mobs/create",
        json!({"mob_config": config(&second)}),
    )
    .await;
    result(
        &host,
        "meta/mobs/lifecycle",
        json!({"mob_id": second, "action": "stop"}),
    )
    .await;

    let error = refusal(
        &host,
        "meta/mobs/redefine",
        redefines(&second, config_with_a_reviewer(&second), 1),
    )
    .await;
    assert_eq!(
        error["code"],
        json!(crate::STORAGE_RESOLUTION_CODE),
        "{error}"
    );
    let message = error["message"].as_str().expect("a message");
    assert!(
        message.starts_with("declared mob spec update refused: ")
            && message.contains("no store this host can load it from again"),
        "{error}"
    );
    let kept = got(&host, &second).await;
    assert_eq!(kept["state"], json!("stopped"), "{kept}");
    assert_eq!(kept["revision"], json!(1), "{kept}");
    assert_eq!(profiles_of(&host, &second).await, 1);
    shut_down(&host, &[&second]).await;
}

/// A stopped mob that still holds something (here a foreground owner's
/// lease) does not close for its unload: `redefine` is refused under the
/// family's code as a request that can be repeated, the mob stays loaded
/// and stopped with its definition, and the same request succeeds once the
/// lease is released.
#[tokio::test]
async fn redefine_of_a_stopped_mob_that_still_holds_work_can_be_repeated() {
    let temp = tempfile::tempdir().expect("tempdir");
    let launch_mob = id("m");
    let host = stored_host(&temp, &launch_mob).await;
    let second = id("s");
    result(
        &host,
        "meta/mobs/create",
        json!({"mob_config": config(&second)}),
    )
    .await;
    result(
        &host,
        "meta/mobs/lifecycle",
        json!({"mob_id": second, "action": "stop"}),
    )
    .await;
    let lease = set_of(&host)
        .handle_for(&MobId::from(second.as_str()))
        .await
        .expect("the mob set serves the mob")
        .acquire_owner()
        .expect("an owner's lease on the stopped mob");

    let request = redefines(&second, config_with_a_reviewer(&second), 1);
    let busy = refusal(&host, "meta/mobs/redefine", request.clone()).await;
    assert_eq!(busy["code"], json!(REFUSED), "{busy}");
    assert_eq!(busy["data"]["kind"], json!("mob_busy"), "{busy}");
    assert_eq!(busy["data"]["mob_id"], json!(second), "{busy}");
    assert_eq!(busy["data"]["retryable"], json!(true), "{busy}");
    assert_eq!(
        busy["data"]["detail"],
        json!(format!(
            "'{second}' is stopped and does not close for a new definition; held: owner"
        )),
        "the detail ends with one closed word for each thing held: {busy}"
    );
    let kept = got(&host, &second).await;
    assert_eq!(kept["state"], json!("stopped"), "{kept}");
    assert_eq!(kept["revision"], json!(1), "{kept}");
    assert_eq!(profiles_of(&host, &second).await, 1);

    drop(lease);
    let redefined = result(&host, "meta/mobs/redefine", request).await;
    assert_eq!(redefined["mob"]["state"], json!("stopped"), "{redefined}");
    assert_eq!(redefined["mob"]["revision"], json!(2), "{redefined}");
    assert_eq!(profiles_of(&host, &second).await, 2);
    shut_down(&host, &[&second]).await;
}

/// A `redefine` repeated after its commit is answered as the first was: the
/// same receipt, with the fields that commit changed, and the mob's entry
/// at the revision it made. Nothing is committed a second time.
#[tokio::test]
async fn redefine_repeated_after_its_commit_answers_the_same_receipt() {
    let temp = tempfile::tempdir().expect("tempdir");
    let launch_mob = id("m");
    let host = stored_host(&temp, &launch_mob).await;
    let second = id("s");
    result(
        &host,
        "meta/mobs/create",
        json!({"mob_config": config(&second)}),
    )
    .await;
    result(
        &host,
        "meta/mobs/lifecycle",
        json!({"mob_id": second, "action": "stop"}),
    )
    .await;

    let request = redefines(&second, config_with_a_reviewer(&second), 1);
    let first = result(&host, "meta/mobs/redefine", request.clone()).await;
    assert!(
        first["receipt"]["declared_fields"]
            .as_array()
            .is_some_and(|fields| !fields.is_empty()),
        "the first answer names the fields it changed: {first}"
    );
    let again = result(&host, "meta/mobs/redefine", request).await;
    assert_eq!(
        again["receipt"], first["receipt"],
        "the repeated request is answered the receipt of the commit it made"
    );
    assert_eq!(again["mob"], first["mob"], "{again}");
    assert_eq!(got(&host, &second).await["revision"], json!(2));
    assert_eq!(profiles_of(&host, &second).await, 2);
    shut_down(&host, &[&second]).await;
}

/// A `destroy` destroys the mob it judged and no other. The late destroy
/// here found the mob the host created and is held before its change;
/// meanwhile that mob is destroyed and a member creates a mob of its own
/// under the id. The late destroy is then refused as an id the host does
/// not have, and the member's mob is as it was.
#[tokio::test]
async fn destroy_never_destroys_a_mob_created_under_the_id_after_it_judged_the_mob() {
    let temp = tempfile::tempdir().expect("tempdir");
    let launch_mob = id("m");
    let host = host(&temp, &launch_mob).await;
    let second = id("s");
    result(
        &host,
        "meta/mobs/create",
        json!({"mob_config": config(&second)}),
    )
    .await;

    let hold = crate::rpc::mob_hosting::destroy_seam::hold_the_next_destroy_of(&second);
    let late = ask(&host, "meta/mobs/destroy", json!({"mob_id": second}));
    tokio::pin!(late);
    tokio::time::timeout(Duration::from_mins(1), async {
        loop {
            tokio::select! {
                answer = &mut late => {
                    panic!("the late destroy answered before it was held: {answer}")
                }
                () = tokio::time::sleep(Duration::from_millis(5)) => {
                    if hold.reached.load(std::sync::atomic::Ordering::SeqCst) {
                        break;
                    }
                }
            }
        }
    })
    .await
    .expect("the late destroy judges the mob and reaches its change");

    result(&host, "meta/mobs/destroy", json!({"mob_id": second})).await;
    let members = member_created_mob(&host, &second).await;
    hold.release.notify_one();
    let answer = tokio::time::timeout(Duration::from_mins(1), late)
        .await
        .expect("the late destroy answers");

    let mut wrong = Vec::new();
    let inventory = set_of(&host)
        .mob_inventory()
        .await
        .expect("the inventory reads");
    if !inventory.managed.iter().any(|(id, _)| id == &members) {
        wrong.push(format!(
            "the member's mob was destroyed by a destroy that judged the host's mob: {answer}"
        ));
    }
    if answer.get("result").is_some()
        || !is_not_a_mob_of_the_host(&answer["error"], &second, &launch_mob)
    {
        wrong.push(format!(
            "the late destroy is not refused as an id the host does not have: {answer}"
        ));
    }
    shut_down(&host, &[&second]).await;
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// Drive the request `late` until the change it asks for is held at the
/// handler's seam, after the handler judged its mob.
pub(crate) async fn until_held(
    mut late: std::pin::Pin<&mut impl std::future::Future<Output = Value>>,
    hold: &crate::rpc::mob_hosting::destroy_seam::Hold,
) {
    tokio::time::timeout(Duration::from_mins(1), async {
        loop {
            tokio::select! {
                answer = &mut late => {
                    panic!("the request answered before its change was held: {answer}")
                }
                () = tokio::time::sleep(Duration::from_millis(5)) => {
                    if hold.reached.load(std::sync::atomic::Ordering::SeqCst) {
                        break;
                    }
                }
            }
        }
    })
    .await
    .expect("the request judges its mob and reaches its change");
}

/// A `destroy` that could not finish is continued by its repeat. Here the
/// mob is destroyed while a reader holds its store open, so the storage
/// cleanup is left owed and the answer is the destroy's own "incomplete".
/// While it is owed no mob is created under the id. Once the reader is
/// gone the repeated destroy answers the destroy's report, and a create
/// under the id succeeds.
#[tokio::test]
async fn destroy_that_could_not_finish_is_continued_by_its_repeat_and_frees_the_id() {
    let temp = tempfile::tempdir().expect("tempdir");
    let launch_mob = id("m");
    let host = stored_host(&temp, &launch_mob).await;
    let second = id("s");
    result(
        &host,
        "meta/mobs/create",
        json!({"mob_config": config(&second)}),
    )
    .await;
    let reader = meerkat_mob::MobStorage::persistent(store_file_of(&temp, &second))
        .expect("a reader of the mob's store");

    let incomplete = refusal(&host, "meta/mobs/destroy", json!({"mob_id": second})).await;
    assert_eq!(incomplete["code"], json!(-32603), "{incomplete}");
    assert!(
        incomplete["message"]
            .as_str()
            .is_some_and(|message| message.contains("storage cleanup outstanding")),
        "the destroy answers what it still owes: {incomplete}"
    );
    let taken = refusal(
        &host,
        "meta/mobs/create",
        json!({"mob_config": config(&second)}),
    )
    .await;
    assert!(
        taken["message"]
            .as_str()
            .is_some_and(|message| message.contains("retry that destroy")),
        "no mob is created under the id while the cleanup is owed: {taken}"
    );

    drop(reader);
    let repeated = result(&host, "meta/mobs/destroy", json!({"mob_id": second})).await;
    assert_eq!(repeated["mob_id"], json!(second), "{repeated}");
    assert!(
        repeated["destroy_report"].is_object(),
        "the repeat answers the destroy's report: {repeated}"
    );
    let created = result(
        &host,
        "meta/mobs/create",
        json!({"mob_config": config(&second)}),
    )
    .await;
    assert_eq!(created["mob"]["state"], json!("running"), "{created}");
    shut_down(&host, &[&second]).await;
}

/// A `redefine` redefines the mob it judged and no other. The late
/// redefine here found the stopped mob the host created and is held before
/// its change; meanwhile that mob is destroyed and a member creates a mob
/// of its own under the id, stopped at the same revision. The late
/// redefine is then refused as an id the host does not have, and the
/// member's mob keeps its definition.
#[tokio::test]
async fn redefine_never_redefines_a_mob_created_under_the_id_after_it_judged_the_mob() {
    let temp = tempfile::tempdir().expect("tempdir");
    let launch_mob = id("m");
    let host = stored_host(&temp, &launch_mob).await;
    let second = id("s");
    result(
        &host,
        "meta/mobs/create",
        json!({"mob_config": config(&second)}),
    )
    .await;
    result(
        &host,
        "meta/mobs/lifecycle",
        json!({"mob_id": second, "action": "stop"}),
    )
    .await;

    let hold = crate::rpc::mob_hosting::destroy_seam::hold_the_next("redefine", &second);
    let late = ask(
        &host,
        "meta/mobs/redefine",
        redefines(&second, config_with_a_reviewer(&second), 1),
    );
    tokio::pin!(late);
    until_held(late.as_mut(), &hold).await;

    result(&host, "meta/mobs/destroy", json!({"mob_id": second})).await;
    let members = member_created_mob(&host, &second).await;
    set_of(&host)
        .mob_stop(&members)
        .await
        .expect("the member's mob is stopped");
    hold.release.notify_one();
    let answer = tokio::time::timeout(Duration::from_mins(1), late)
        .await
        .expect("the late redefine answers");

    let mut wrong = Vec::new();
    let profiles = set_of(&host)
        .handle_for(&members)
        .await
        .expect("the member's mob is served")
        .definition()
        .profiles
        .len();
    if profiles != 1 {
        wrong.push(format!(
            "the member's mob was redefined by a redefine that judged the host's mob: it has \
             {profiles} profiles: {answer}"
        ));
    }
    if answer.get("result").is_some()
        || !is_not_a_mob_of_the_host(&answer["error"], &second, &launch_mob)
    {
        wrong.push(format!(
            "the late redefine is not refused as an id the host does not have: {answer}"
        ));
    }
    shut_down(&host, &[&second]).await;
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// A `lifecycle` stops the mob it judged and no other. The late stop here
/// found the mob the host created and is held before its change; meanwhile
/// that mob is destroyed and a member creates a mob of its own under the
/// id. The late stop is then answered an error, and the member's mob runs
/// on.
#[tokio::test]
async fn lifecycle_never_stops_a_mob_created_under_the_id_after_it_judged_the_mob() {
    let temp = tempfile::tempdir().expect("tempdir");
    let launch_mob = id("m");
    let host = host(&temp, &launch_mob).await;
    let second = id("s");
    result(
        &host,
        "meta/mobs/create",
        json!({"mob_config": config(&second)}),
    )
    .await;

    let hold = crate::rpc::mob_hosting::destroy_seam::hold_the_next("lifecycle", &second);
    let late = ask(
        &host,
        "meta/mobs/lifecycle",
        json!({"mob_id": second, "action": "stop"}),
    );
    tokio::pin!(late);
    until_held(late.as_mut(), &hold).await;

    result(&host, "meta/mobs/destroy", json!({"mob_id": second})).await;
    let members = member_created_mob(&host, &second).await;
    hold.release.notify_one();
    let answer = tokio::time::timeout(Duration::from_mins(1), late)
        .await
        .expect("the late stop answers");

    let phase = set_of(&host)
        .mob_status(&members)
        .await
        .expect("the member's mob is served");
    shut_down(&host, &[&second]).await;
    assert_eq!(
        phase,
        meerkat_mob::MobState::Running,
        "the member's mob was not stopped by a stop that judged the host's mob: {answer}"
    );
    assert!(
        answer.get("result").is_none() && answer.get("error").is_some(),
        "the late stop is answered an error: {answer}"
    );
}
/// The store file of the mob `mob_id` that a [`stored_host`] over `temp`
/// created.
fn store_file_of(temp: &tempfile::TempDir, mob_id: &str) -> std::path::PathBuf {
    fn walk(dir: &std::path::Path, mob_id: &str, found: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, mob_id, found);
            } else if path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.contains(mob_id) && name.ends_with(".db"))
            {
                found.push(path);
            }
        }
    }
    let mut found = Vec::new();
    walk(temp.path(), mob_id, &mut found);
    assert_eq!(found.len(), 1, "one store file of {mob_id}: {found:?}");
    found.remove(0)
}

/// End a [`stored_host`] as a process ends: the runtime stops its launch
/// mob, stands the mobs of its set down and releases their root, so that a
/// host can start again over the same state. The row holds nothing of the
/// host's mob set while it ends.
pub(crate) async fn end(host: UnifiedRuntime) {
    let report = tokio::time::timeout(Duration::from_mins(1), host.shutdown())
        .await
        .expect("the runtime shuts down");
    assert!(
        matches!(
            report.child_mob_stand_down,
            crate::child_mob_storage::ChildMobStandDownOutcome::Released { .. }
        ),
        "the host released the root of the mobs it created: {:?}",
        report.child_mob_stand_down
    );
    drop(host);
}

/// A host that starts again over the same state has the mobs it created
/// before: a second mob that was stopped is listed and read `stopped` at
/// its revision, with the host as its creator, and resumes.
#[tokio::test]
async fn a_host_that_starts_again_has_the_mobs_it_created_as_they_were() {
    let temp = tempfile::tempdir().expect("tempdir");
    let launch_mob = id("m");
    let second = id("s");
    {
        let host = stored_host(&temp, &launch_mob).await;
        result(
            &host,
            "meta/mobs/create",
            json!({"mob_config": config(&second)}),
        )
        .await;
        result(
            &host,
            "meta/mobs/lifecycle",
            json!({"mob_id": second, "action": "stop"}),
        )
        .await;
        result(
            &host,
            "meta/mobs/redefine",
            redefines(&second, config_with_a_reviewer(&second), 1),
        )
        .await;
        end(host).await;
    }

    let host = stored_host(&temp, &launch_mob).await;
    // A host reads the stores of the mobs it has not loaded after its
    // start, in a task of its own: the list names who created the second
    // mob once that read came by.
    let read_by = tokio::time::Instant::now() + Duration::from_secs(30);
    while listed(&host)
        .await
        .iter()
        .any(|mob| mob.get("provenance").is_none())
    {
        assert!(
            tokio::time::Instant::now() < read_by,
            "the host that started again read the stores of its mobs: {:?}",
            listed(&host).await
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(
        listed(&host).await,
        vec![
            json!({"mob_id": launch_mob, "default": true, "state": "running", "provenance": "host"}),
            json!({"mob_id": second, "default": false, "state": "stopped", "provenance": "host"}),
        ],
        "the host that started again lists both mobs as they were"
    );
    assert_eq!(
        got(&host, &second).await,
        json!({
            "mob_id": second,
            "default": false,
            "state": "stopped",
            "provenance": "host",
            "revision": 2,
        })
    );
    assert_eq!(profiles_of(&host, &second).await, 2);
    let resumed = result(
        &host,
        "meta/mobs/lifecycle",
        json!({"mob_id": second, "action": "resume"}),
    )
    .await;
    assert_eq!(resumed["mob"]["state"], json!("running"), "{resumed}");
    end(host).await;
}

/// A `get` names one mob, and a mob the host created that is not loaded is
/// loaded for it as its store has it: a stopped mob loads stopped, with the
/// revision of its definition, and nothing of it is started. A `list` loads
/// nothing, and neither does a `get` of a mob one of the host's agents
/// created or of an id no mob has.
#[tokio::test]
async fn get_loads_a_stopped_mob_of_the_host_that_is_not_loaded_and_a_list_loads_nothing() {
    let temp = tempfile::tempdir().expect("tempdir");
    let launch_mob = id("m");
    let host = stored_host(&temp, &launch_mob).await;
    let second = id("s");
    result(
        &host,
        "meta/mobs/create",
        json!({"mob_config": config(&second)}),
    )
    .await;
    result(
        &host,
        "meta/mobs/lifecycle",
        json!({"mob_id": second, "action": "stop"}),
    )
    .await;
    let members_mob = member_created_mob(&host, &id("c")).await;
    let second_id = MobId::from(second.as_str());
    let set = set_of(&host);
    let loaded = || async {
        let mut loaded: Vec<MobId> = set
            .mob_inventory()
            .await
            .expect("the inventory reads")
            .managed
            .into_iter()
            .map(|(mob_id, _)| mob_id)
            .collect();
        loaded.sort();
        loaded
    };

    set.unload_idle_mobs_for_tests().await;
    let inventory = set.mob_inventory().await.expect("the inventory reads");
    assert!(
        inventory.dormant.contains(&second_id) && inventory.dormant_stopped.contains(&second_id),
        "the fixture of this row is a stopped mob that is not loaded and whose stop the \
         records keep: {inventory:?}"
    );
    assert!(
        inventory.dormant.contains(&members_mob),
        "the fixture of this row is a member's mob that is not loaded: {inventory:?}"
    );
    let before = loaded().await;

    // A list loads nothing.
    let mobs = listed(&host).await;
    assert!(
        mobs.iter().any(|mob| mob["mob_id"] == json!(second)),
        "the list names the mob that is not loaded: {mobs:?}"
    );
    assert_eq!(loaded().await, before, "a list loads nothing");

    // A mob one of the host's agents created is answered from the records,
    // and an id no mob has is refused: nothing is loaded for either.
    let members_entry = got(&host, members_mob.as_str()).await;
    assert_eq!(
        members_entry["provenance"],
        json!("member"),
        "{members_entry}"
    );
    assert!(
        members_entry.get("revision").is_none(),
        "a mob a member created is not loaded for a get: {members_entry}"
    );
    let nobody = id("n");
    let error = refusal(&host, "meta/mobs/get", json!({"mob_id": nobody})).await;
    assert!(
        is_not_a_mob_of_the_host(&error, &nobody, &launch_mob),
        "{error}"
    );
    assert_eq!(
        loaded().await,
        before,
        "a get of a member's mob and of an id no mob has loads nothing"
    );

    // The host's own mob is loaded for the get that names it, stopped.
    assert_eq!(
        got(&host, &second).await,
        json!({
            "mob_id": second,
            "default": false,
            "state": "stopped",
            "provenance": "host",
            "revision": 1,
        }),
        "a get answers the mob the host created with its phase and its revision"
    );
    let inventory = set.mob_inventory().await.expect("the inventory reads");
    let phase = inventory
        .managed
        .iter()
        .find(|(mob_id, _)| mob_id == &second_id)
        .map(|(_, observation)| observation.phase);
    assert_eq!(
        phase,
        Some(meerkat_mob::MobState::Stopped),
        "the get loaded the mob, and loaded it stopped: {inventory:?}"
    );
    let mut with_the_second = before.clone();
    with_the_second.push(second_id.clone());
    with_the_second.sort();
    assert_eq!(loaded().await, with_the_second, "and loaded no other mob");

    // The list after it loads nothing either, and reads the mob as stopped.
    let mobs = listed(&host).await;
    let entry = mobs
        .iter()
        .find(|mob| mob["mob_id"] == json!(second))
        .expect("the list names the mob");
    assert_eq!(entry["state"], json!("stopped"), "{entry}");
    assert_eq!(loaded().await, with_the_second, "a list loads nothing");
    shut_down(&host, &[&second, members_mob.as_str()]).await;
}

/// A `get` in the stretch after a start, before the host's own read of the
/// stores it has not loaded has come by, reads who created the mob it
/// names from that mob's store and decides by it: a mob the host created is
/// loaded for the `get`, as its store has it, and a mob one of the host's
/// agents created is not. The row holds the host's own read at the first of
/// the two stores it reaches (the test seam of the store's reads), so no
/// creation of the two is in the host's records when the `get`s are asked.
#[tokio::test]
async fn a_get_before_the_hosts_own_read_of_a_store_reads_who_created_the_mob_and_loads_the_hosts_own()
 {
    let temp = tempfile::tempdir().expect("tempdir");
    let launch_mob = id("m");
    let second = id("s");
    let members_mob = {
        let host = stored_host(&temp, &launch_mob).await;
        result(
            &host,
            "meta/mobs/create",
            json!({"mob_config": config(&second)}),
        )
        .await;
        result(
            &host,
            "meta/mobs/lifecycle",
            json!({"mob_id": second, "action": "stop"}),
        )
        .await;
        let members_mob = member_created_mob(&host, &id("c")).await;
        end(host).await;
        members_mob
    };
    let second_id = MobId::from(second.as_str());

    // The host that starts again reads the stores of the mobs it has not
    // loaded in a task of its own. That read is held at the first of the
    // two stores it opens; the park of the other is taken away, so nothing
    // else of this row waits at it.
    let mut at_the_second =
        meerkat_mob::park_next_store_read_for_test(&store_file_of(&temp, &second));
    let mut at_the_members =
        meerkat_mob::park_next_store_read_for_test(&store_file_of(&temp, members_mob.as_str()));
    let host = stored_host(&temp, &launch_mob).await;
    let reached_the_second = tokio::time::timeout(Duration::from_secs(30), async {
        tokio::select! {
            _ = at_the_second.opened() => true,
            _ = at_the_members.opened() => false,
        }
    })
    .await
    .expect("the host's own read reaches one of the two stores and is held there");
    let held = if reached_the_second {
        drop(at_the_members);
        at_the_second
    } else {
        drop(at_the_second);
        at_the_members
    };
    let set = set_of(&host);
    let inventory = set.mob_inventory().await.expect("the inventory reads");
    assert!(
        inventory.dormant.contains(&second_id) && inventory.dormant.contains(&members_mob),
        "the fixture of this row is two mobs that are not loaded: {inventory:?}"
    );
    assert!(
        !inventory.dormant_creation.contains_key(&second_id)
            && !inventory.dormant_creation.contains_key(&members_mob),
        "the fixture of this row is the stretch in which the host's records say of neither \
         mob who created it: {inventory:?}"
    );

    // A mob one of the host's agents created: read from its store, not loaded.
    let members_entry = got(&host, members_mob.as_str()).await;
    assert_eq!(
        members_entry["provenance"],
        json!("member"),
        "{members_entry}"
    );
    assert!(
        members_entry.get("revision").is_none(),
        "a mob a member created is not loaded for a get: {members_entry}"
    );
    let inventory = set.mob_inventory().await.expect("the inventory reads");
    assert!(
        inventory.dormant.contains(&members_mob)
            && !inventory
                .managed
                .iter()
                .any(|(mob_id, _)| mob_id == &members_mob),
        "the member's mob stays not loaded: {inventory:?}"
    );

    // The mob the host created: read from its store, then loaded, stopped.
    assert_eq!(
        got(&host, &second).await,
        json!({
            "mob_id": second,
            "default": false,
            "state": "stopped",
            "provenance": "host",
            "revision": 1,
        }),
        "a get in that stretch loads the mob the host created"
    );
    let inventory = set.mob_inventory().await.expect("the inventory reads");
    assert!(
        inventory
            .managed
            .iter()
            .any(|(mob_id, _)| mob_id == &second_id),
        "the get loaded the host's own mob: {inventory:?}"
    );

    // The host's own read goes on.
    drop(held);
    shut_down(&host, &[&second, members_mob.as_str()]).await;
}

/// A `get` judges its request's own parameters before it reads a store: a
/// request that names a mob of the host that is not loaded and carries a
/// member the method does not take is refused as invalid params, and the
/// mob stays not loaded. The same request without that member loads it.
#[tokio::test]
async fn a_get_refused_for_its_parameters_loads_nothing() {
    let temp = tempfile::tempdir().expect("tempdir");
    let launch_mob = id("m");
    let host = stored_host(&temp, &launch_mob).await;
    let second = id("s");
    result(
        &host,
        "meta/mobs/create",
        json!({"mob_config": config(&second)}),
    )
    .await;
    result(
        &host,
        "meta/mobs/lifecycle",
        json!({"mob_id": second, "action": "stop"}),
    )
    .await;
    let second_id = MobId::from(second.as_str());
    let set = set_of(&host);
    let is_loaded = || async {
        set.mob_inventory()
            .await
            .expect("the inventory reads")
            .managed
            .iter()
            .any(|(mob_id, _)| mob_id == &second_id)
    };
    set.unload_idle_mobs_for_tests().await;
    assert!(
        !is_loaded().await,
        "the fixture of this row is a mob of the host that is not loaded"
    );

    let error = refusal(
        &host,
        "meta/mobs/get",
        json!({"mob_id": second, "action": "stop"}),
    )
    .await;
    assert_eq!(error["code"], json!(-32602), "{error}");
    assert!(
        !is_loaded().await,
        "a get refused for its parameters loads nothing"
    );

    assert_eq!(got(&host, &second).await["revision"], json!(1));
    assert!(is_loaded().await, "the same get without that member loads");
    shut_down(&host, &[&second]).await;
}

/// A `get` whose load fails answers what the mob set's records hold after
/// it: a store that cannot be opened is kept quarantined, and the entry
/// says so with the mob set's row of the quarantine, without a revision.
#[tokio::test]
async fn a_get_whose_load_meets_a_store_that_cannot_be_opened_answers_the_quarantined_entry() {
    let temp = tempfile::tempdir().expect("tempdir");
    let launch_mob = id("m");
    let host = stored_host(&temp, &launch_mob).await;
    let second = id("s");
    result(
        &host,
        "meta/mobs/create",
        json!({"mob_config": config(&second)}),
    )
    .await;
    result(
        &host,
        "meta/mobs/lifecycle",
        json!({"mob_id": second, "action": "stop"}),
    )
    .await;
    let second_id = MobId::from(second.as_str());
    let set = set_of(&host);
    set.unload_idle_mobs_for_tests().await;
    let inventory = set.mob_inventory().await.expect("the inventory reads");
    assert!(
        inventory.dormant.contains(&second_id),
        "the fixture of this row is a mob of the host that is not loaded: {inventory:?}"
    );

    // The store of the mob that is not loaded is damaged under the running
    // host: the load the get asks for cannot open it.
    let store = store_file_of(&temp, &second);
    std::fs::write(&store, b"this is not a mob store").expect("damage the second mob's store");
    let entry = got(&host, &second).await;
    assert_eq!(entry["state"], json!("quarantined"), "{entry}");
    assert!(
        entry["quarantine"].is_object(),
        "the entry carries the mob set's row of the quarantine: {entry}"
    );
    assert!(entry.get("revision").is_none(), "{entry}");
    let inventory = set.mob_inventory().await.expect("the inventory reads");
    assert!(
        !inventory
            .managed
            .iter()
            .any(|(mob_id, _)| mob_id == &second_id),
        "the mob whose load failed is not loaded: {inventory:?}"
    );
    shut_down(&host, &[]).await;
}

/// PIN of what holds today, not of the wanted end: a mob whose store cannot
/// be opened when the host starts again is kept quarantined. It is listed
/// and read as `quarantined` with the mob set's row of the quarantine; a
/// stop, a redefinition and a destroy of it answer the mob set's refusal of
/// a quarantined mob, and so does a create under its id: the id stays taken
/// until the store is repaired. The other mobs of the host are served.
#[tokio::test]
async fn a_mob_whose_store_cannot_be_opened_is_quarantined_and_keeps_its_id() {
    let temp = tempfile::tempdir().expect("tempdir");
    let launch_mob = id("m");
    let second = id("s");
    {
        let host = stored_host(&temp, &launch_mob).await;
        result(
            &host,
            "meta/mobs/create",
            json!({"mob_config": config(&second)}),
        )
        .await;
        end(host).await;
    }
    let store = store_file_of(&temp, &second);
    std::fs::write(&store, b"this is not a mob store").expect("damage the second mob's store");

    let host = stored_host(&temp, &launch_mob).await;
    let read = got(&host, &second).await;
    assert_eq!(read["state"], json!("quarantined"), "{read}");
    assert_eq!(read["default"], json!(false), "{read}");
    assert!(read["quarantine"]["class"].is_string(), "{read}");
    assert!(
        read.get("provenance").is_none() && read.get("revision").is_none(),
        "nothing of a store that cannot be opened is read: {read}"
    );
    let mobs = listed(&host).await;
    assert_eq!(mobs.len(), 2, "{mobs:?}");
    assert_eq!(mobs[0]["mob_id"], json!(launch_mob));
    assert_eq!(mobs[0]["state"], json!("running"));
    assert_eq!(mobs[1], read, "the list and the read agree");

    for (method, params) in [
        (
            "meta/mobs/lifecycle",
            json!({"mob_id": second, "action": "stop"}),
        ),
        (
            "meta/mobs/redefine",
            redefines(&second, config_with_a_reviewer(&second), 1),
        ),
        ("meta/mobs/destroy", json!({"mob_id": second})),
    ] {
        let error = refusal(&host, method, params).await;
        assert_eq!(
            error["data"]["kind"],
            json!("mob_quarantined"),
            "{method}: {error}"
        );
    }
    let taken = refusal(
        &host,
        "meta/mobs/create",
        json!({"mob_config": config(&second)}),
    )
    .await;
    assert_eq!(taken["data"]["kind"], json!("mob_quarantined"), "{taken}");
    assert_eq!(taken["data"]["mob_id"], json!(second), "{taken}");
    assert_eq!(
        got(&host, &second).await["state"],
        json!("quarantined"),
        "the store is kept as found"
    );
    shut_down(&host, &[]).await;
}

/// What the storage doctor's census of the mobs a host created reports for
/// the state directory `temp`: each finding's path and message.
async fn host_created_census(temp: &tempfile::TempDir) -> Vec<(std::path::PathBuf, String)> {
    let scope =
        meerkat_core::storage_diagnostics::DiagnoseScope::new(vec![temp.path().to_path_buf()]);
    crate::storage_doctor::diagnose_state_dir(&scope)
        .await
        .findings
        .into_iter()
        .filter(|finding| finding.code == "host-created-mobs")
        .map(|finding| {
            (
                finding.path.expect("a census finding names its path"),
                finding.message,
            )
        })
        .collect()
}

/// The storage doctor counts the mob `meta/mobs/create` made. Over a host
/// that keeps its mobs in files, read while the host runs: before a create
/// the census counts no mob the host created; after a create through the
/// family it reports that mob's store, by its file, and counts one; a mob
/// one of the host's agents created is a store it reads and does not
/// count; after the destroy it counts none again.
#[tokio::test]
async fn the_storage_doctor_counts_the_mob_a_create_made_and_no_mob_an_agent_created() {
    const TOTAL: &str = "hosted-mob census: ";
    let temp = tempfile::tempdir().expect("tempdir");
    let launch_mob = id("m");
    let host = stored_host(&temp, &launch_mob).await;

    let before = host_created_census(&temp).await;
    assert_eq!(before.len(), 1, "{before:#?}");
    assert!(
        before[0].1.starts_with(&format!(
            "{TOTAL}0 mob(s) the host created beside its launch mob in 0 mob store(s)"
        )),
        "a host of one mob has created none beside it: {before:#?}"
    );

    let second = id("s");
    result(
        &host,
        "meta/mobs/create",
        json!({"mob_config": config(&second)}),
    )
    .await;
    let members = id("c");
    member_created_mob(&host, &members).await;

    let counted = host_created_census(&temp).await;
    assert_eq!(counted.len(), 2, "{counted:#?}");
    assert_eq!(
        counted[0].0,
        store_file_of(&temp, &second),
        "the finding names the store of the mob the create made: {counted:#?}"
    );
    assert!(
        counted[0]
            .1
            .starts_with("this mob store holds a mob the host created beside its launch mob:"),
        "{counted:#?}"
    );
    assert!(
        counted[1].1.starts_with(&format!(
            "{TOTAL}1 mob(s) the host created beside its launch mob in 2 mob store(s)"
        )),
        "the member's mob is read and not counted: {counted:#?}"
    );

    result(&host, "meta/mobs/destroy", json!({"mob_id": second})).await;
    let after = host_created_census(&temp).await;
    assert_eq!(after.len(), 1, "{after:#?}");
    assert!(
        after[0].1.starts_with(&format!(
            "{TOTAL}0 mob(s) the host created beside its launch mob in 1 mob store(s)"
        )),
        "the destroyed mob is counted no more: {after:#?}"
    );
    shut_down(&host, &[&members]).await;
}
