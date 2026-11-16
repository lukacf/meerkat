//! The realm profile tools of the public MCP surface take the public profile
//! input: a profile field outside it is an argument error that names the
//! field, and nothing is stored.
//!
//! Every write goes through `handle_public_tools_call`, the dispatch that
//! both the unified MCP server and the mob MCP binary serve; one row drives
//! the binary itself. Synthetic state only: a scripted model, no provider, no
//! network. The stand-in MCP server is a script this file writes under the
//! fixture's temporary directory.
// `disallowed_methods`: one row starts this crate's own binary, a test-only
// process launch.
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::disallowed_methods
)]

mod support;

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use meerkat_core::types::{ContentInput, HandlingMode, ToolCallView, ToolDef, ToolResult};
use meerkat_core::{AgentToolDispatcher, ToolDispatchOutcome, ToolError};
use meerkat_mob::{AgentIdentity, BoundedResultSpec, MobId, MobRuntimeMode, WorkOrigin, WorkSpec};
use meerkat_mob_mcp::{
    ChildToolBundleAvailability, ChildToolBundles, MobMcpState, handle_public_tools_call,
    public_tools_list,
};
use serde_json::{Value, json};
use support::{CouncilFixture, ScriptedTurn};

const CREATE: &str = "meerkat_mob_profile_create";
const UPDATE: &str = "meerkat_mob_profile_update";
const GET: &str = "meerkat_mob_profile_get";
const MODEL: &str = "claude-sonnet-4-6";
const STANDIN_TOOL: &str = "profile_row_standin";
const CHILD_TOOL: &str = "child_row_tool";
const BINDING: &str = "profile-row-binding";

/// A stand-in MCP server: it writes the file named by its first argument when
/// it starts, and answers just enough of the stdio transport to be listed. It
/// opens no socket and reads no credential.
const STANDIN_SOURCE: &str = r#"import json
import sys

if len(sys.argv) > 1:
    with open(sys.argv[1], "w", encoding="utf-8") as handle:
        handle.write("started\n")
for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    try:
        message = json.loads(line)
    except ValueError:
        continue
    ident = message.get("id")
    if ident is None:
        continue
    method = message.get("method")
    if method == "initialize":
        version = (message.get("params") or {}).get("protocolVersion", "2025-03-26")
        result = {
            "protocolVersion": version,
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "profile-row-standin", "version": "0.0.0"},
        }
    elif method == "tools/list":
        result = {
            "tools": [
                {
                    "name": "profile_row_standin",
                    "description": "Stand-in tool.",
                    "inputSchema": {"type": "object", "properties": {}},
                }
            ]
        }
    elif method == "tools/call":
        result = {"content": [{"type": "text", "text": "called"}], "isError": False}
    else:
        result = {}
    sys.stdout.write(json.dumps({"jsonrpc": "2.0", "id": ident, "result": result}) + "\n")
    sys.stdout.flush()
"#;

type Recorded = Arc<Mutex<Vec<Vec<String>>>>;

fn recording_script(
    sink: Recorded,
) -> impl Fn(&meerkat_client::LlmRequest) -> ScriptedTurn + Send + Sync + 'static {
    move |request| {
        sink.lock().unwrap().push(
            request
                .tools
                .iter()
                .map(|tool| tool.name.to_string())
                .collect(),
        );
        ScriptedTurn::Text("ok".to_string())
    }
}

/// One public tool call: the payload, or the error's code and message.
async fn public(fixture: &CouncilFixture, tool: &str, args: Value) -> Result<Value, (i32, String)> {
    handle_public_tools_call(&fixture.state, tool, &args)
        .await
        .map_err(|error| (error.code, error.message))
}

/// The answer is an argument error that names `field`.
fn assert_refused_naming(answer: &Result<Value, (i32, String)>, field: &str) {
    match answer {
        Ok(stored) => panic!("a profile naming `{field}` was accepted and stored: {stored}"),
        Err((code, message)) => {
            assert_eq!(*code, -32602, "an argument error: {message}");
            assert!(
                message.contains(field),
                "the refusal names `{field}`: {message}"
            );
        }
    }
}

/// A public profile: nothing outside the public profile input.
fn public_profile() -> Value {
    json!({ "model": MODEL, "tools": { "comms": true }, "peer_description": "worker" })
}

/// The public profile with `tools.<key>` set to `value`.
fn profile_with_tools_key(key: &str, value: Value) -> Value {
    let mut profile = public_profile();
    profile["tools"][key] = value;
    profile
}

/// The public profile with the top-level `key` set to `value`.
fn profile_with_key(key: &str, value: Value) -> Value {
    let mut profile = public_profile();
    profile[key] = value;
    profile
}

/// A stdio server entry that runs the stand-in, which writes `marker` when it
/// starts.
fn standin_server(root: &Path, marker: &Path) -> Value {
    let script = root.join("profile-row-standin.py");
    std::fs::write(&script, STANDIN_SOURCE).expect("write the stand-in script");
    serde_json::to_value(meerkat_core::mcp_config::McpServerConfig::stdio(
        "profile-row-standin",
        "python3",
        vec![
            "-I".to_string(),
            script.display().to_string(),
            marker.display().to_string(),
        ],
        HashMap::new(),
    ))
    .expect("server entry")
}

fn binding_value() -> Value {
    serde_json::to_value(meerkat_core::AuthBindingRef {
        realm: meerkat_core::RealmId::parse("global").unwrap(),
        binding: meerkat_core::BindingId::parse(BINDING).unwrap(),
        profile: None,
        origin: meerkat_core::BindingOrigin::Configured,
    })
    .expect("binding value")
}

/// Store the public profile under `name` through the create tool.
async fn store_public(fixture: &CouncilFixture, name: &str) {
    let stored = public(
        fixture,
        CREATE,
        json!({ "name": name, "profile": public_profile() }),
    )
    .await
    .unwrap_or_else(|error| panic!("a public profile is stored: {error:?}"));
    assert_eq!(stored["revision"], 1, "{stored}");
}

/// Create a mob whose one role `worker` names the realm profile `realm`, and
/// spawn `member` on it. The error is the create's or the spawn's refusal.
async fn seat_on_realm_profile(
    fixture: &CouncilFixture,
    mob_id: &str,
    realm: &str,
    member: &str,
) -> Result<(), String> {
    public(
        fixture,
        "meerkat_mob_create",
        json!({ "definition": { "id": mob_id, "profiles": { "worker": { "realm_profile": realm } } } }),
    )
    .await
    .map_err(|error| format!("{error:?}"))?;
    fixture
        .state
        .mob_spawn(
            &MobId::from(mob_id),
            "worker".into(),
            AgentIdentity::from(member),
            Some(MobRuntimeMode::TurnDriven),
            None,
            None,
        )
        .await
        .map(|_| ())
        .map_err(|error| format!("{error}"))
}

/// Run one bounded turn of `member` and return the tool names of the model
/// request it made.
async fn turn_tools(
    fixture: &CouncilFixture,
    recorded: &Recorded,
    mob_id: &str,
    member: &str,
) -> Vec<String> {
    let handle = fixture
        .state
        .handle_for(&MobId::from(mob_id))
        .await
        .expect("mob handle");
    let spec = BoundedResultSpec::new("row", 4096).expect("bounded result spec");
    let work = handle
        .start_work_for_identity_bounded(
            AgentIdentity::from(member),
            WorkSpec::new(
                ContentInput::Text("hello".to_string()),
                WorkOrigin::Internal,
            ),
            HandlingMode::Queue,
            spec.clone(),
        )
        .await
        .expect("start a turn");
    tokio::time::timeout(Duration::from_secs(90), work.wait_bounded(spec))
        .await
        .expect("turn completes within the bound")
        .expect("turn succeeds");
    recorded
        .lock()
        .unwrap()
        .last()
        .cloned()
        .expect("the member reached the model")
}

/// The binding the member's session recorded, if any.
async fn recorded_binding(fixture: &CouncilFixture, mob_id: &str, member: &str) -> Option<String> {
    let handle = fixture
        .state
        .handle_for(&MobId::from(mob_id))
        .await
        .expect("mob handle");
    let session = handle
        .resolve_bridge_session_id(&AgentIdentity::from(member))
        .await
        .expect("member session");
    fixture
        .state
        .session_service()
        .load_persisted_session_metadata(&session)
        .await
        .expect("read the member session's metadata")
        .and_then(|view| view.session_metadata)
        .and_then(|metadata| metadata.auth_binding)
        .map(|binding| binding.binding.as_str().to_string())
}

async fn finish(fixture: &CouncilFixture, mob_id: &str) {
    let _ = fixture.state.mob_destroy(&MobId::from(mob_id)).await;
    fixture.teardown().await;
}

struct OneTool(&'static str);

#[async_trait::async_trait]
impl AgentToolDispatcher for OneTool {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        vec![Arc::new(ToolDef::new(
            self.0,
            "Host-registered bundle tool.",
            json!({"type": "object", "properties": {}}),
        ))]
        .into()
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        Ok(ToolDispatchOutcome::sync_result(ToolResult::new(
            call.id.to_string(),
            "{}".to_string(),
            false,
        )))
    }

    fn capabilities(&self) -> meerkat_core::agent::DispatcherCapabilities {
        meerkat_core::agent::DispatcherCapabilities::default()
    }
}

fn child_bundles() -> ChildToolBundles {
    ChildToolBundles::new().register(
        "child-row",
        Arc::new(OneTool(CHILD_TOOL)),
        ChildToolBundleAvailability::ChildAvailable,
    )
}

// ---------------------------------------------------------------------------
// tools.mcp_servers
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn profile_create_refuses_tools_mcp_servers_and_stores_nothing() {
    let recorded: Recorded = Arc::new(Mutex::new(Vec::new()));
    let fixture =
        CouncilFixture::new_runtime_backed_waiting_for_mcp(recording_script(Arc::clone(&recorded)));
    let marker = fixture.root.join("standin-started");
    let profile = profile_with_tools_key(
        "mcp_servers",
        json!([standin_server(&fixture.root, &marker)]),
    );

    let answer = public(
        &fixture,
        CREATE,
        json!({ "name": "with-server", "profile": profile }),
    )
    .await;
    let stored = fixture
        .state
        .realm_profile_get("with-server")
        .await
        .expect("read the realm profile store");
    let mob_id = format!("with-server-{}", fixture.scope);
    let seated = seat_on_realm_profile(&fixture, &mob_id, "with-server", "worker-1").await;
    if seated.is_ok() {
        let _ = turn_tools(&fixture, &recorded, &mob_id, "worker-1").await;
    }
    let started = marker.exists();
    finish(&fixture, &mob_id).await;

    assert_refused_naming(&answer, "mcp_servers");
    assert!(stored.is_none(), "nothing is stored");
    assert!(
        seated.is_err(),
        "no member is seated on a profile that was not stored"
    );
    assert!(!started, "no server process was started");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn profile_update_refuses_tools_mcp_servers_and_keeps_the_stored_profile() {
    let recorded: Recorded = Arc::new(Mutex::new(Vec::new()));
    let fixture =
        CouncilFixture::new_runtime_backed_waiting_for_mcp(recording_script(Arc::clone(&recorded)));
    let marker = fixture.root.join("standin-started");
    store_public(&fixture, "worker-profile").await;
    let profile = profile_with_tools_key(
        "mcp_servers",
        json!([standin_server(&fixture.root, &marker)]),
    );

    let answer = public(
        &fixture,
        UPDATE,
        json!({ "name": "worker-profile", "profile": profile, "expected_revision": 1 }),
    )
    .await;
    let stored = fixture
        .state
        .realm_profile_get("worker-profile")
        .await
        .expect("read the realm profile store")
        .expect("the profile stays stored");
    let mob_id = format!("worker-profile-{}", fixture.scope);
    seat_on_realm_profile(&fixture, &mob_id, "worker-profile", "worker-1")
        .await
        .expect("a member is seated on the stored profile");
    let tools = turn_tools(&fixture, &recorded, &mob_id, "worker-1").await;
    let started = marker.exists();
    finish(&fixture, &mob_id).await;

    assert_refused_naming(&answer, "mcp_servers");
    assert_eq!(stored.revision, 1, "the stored profile is unchanged");
    assert!(stored.profile.tools.mcp_servers.is_empty());
    assert!(
        !tools.iter().any(|name| name == STANDIN_TOOL),
        "the member is offered no tool of such a server: {tools:?}"
    );
    assert!(!started, "no server process was started");
}

// ---------------------------------------------------------------------------
// auth_binding
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn profile_create_refuses_auth_binding_and_stores_nothing() {
    let recorded: Recorded = Arc::new(Mutex::new(Vec::new()));
    let fixture = CouncilFixture::new(recording_script(Arc::clone(&recorded)));

    let answer = public(
        &fixture,
        CREATE,
        json!({ "name": "bound", "profile": profile_with_key("auth_binding", binding_value()) }),
    )
    .await;
    let stored = fixture
        .state
        .realm_profile_get("bound")
        .await
        .expect("read the realm profile store");
    let mob_id = format!("bound-{}", fixture.scope);
    let seated = seat_on_realm_profile(&fixture, &mob_id, "bound", "worker-1").await;
    finish(&fixture, &mob_id).await;

    assert_refused_naming(&answer, "auth_binding");
    assert!(stored.is_none(), "nothing is stored");
    assert!(
        seated.is_err(),
        "no member is seated on a profile that was not stored"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn profile_update_refuses_auth_binding_and_keeps_the_stored_profile() {
    let recorded: Recorded = Arc::new(Mutex::new(Vec::new()));
    let fixture = CouncilFixture::new(recording_script(Arc::clone(&recorded)));
    store_public(&fixture, "worker-profile").await;

    let answer = public(
        &fixture,
        UPDATE,
        json!({
            "name": "worker-profile",
            "profile": profile_with_key("auth_binding", binding_value()),
            "expected_revision": 1,
        }),
    )
    .await;
    let stored = fixture
        .state
        .realm_profile_get("worker-profile")
        .await
        .expect("read the realm profile store")
        .expect("the profile stays stored");
    let mob_id = format!("worker-profile-{}", fixture.scope);
    seat_on_realm_profile(&fixture, &mob_id, "worker-profile", "worker-1")
        .await
        .expect("a member is seated on the stored profile");
    let _ = turn_tools(&fixture, &recorded, &mob_id, "worker-1").await;
    let binding = recorded_binding(&fixture, &mob_id, "worker-1").await;
    finish(&fixture, &mob_id).await;

    assert_refused_naming(&answer, "auth_binding");
    assert_eq!(stored.revision, 1, "the stored profile is unchanged");
    assert!(stored.profile.auth_binding.is_none());
    assert_eq!(binding, None, "the member's session records no binding");
}

// ---------------------------------------------------------------------------
// tools.rust_bundles
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn profile_create_refuses_tools_rust_bundles_and_stores_nothing() {
    let recorded: Recorded = Arc::new(Mutex::new(Vec::new()));
    let fixture =
        CouncilFixture::new_with(recording_script(Arc::clone(&recorded)), |state, _root| {
            state.with_child_tool_bundles(child_bundles())
        });

    let answer = public(
        &fixture,
        CREATE,
        json!({
            "name": "bundled",
            "profile": profile_with_tools_key("rust_bundles", json!(["child-row"])),
        }),
    )
    .await;
    let stored = fixture
        .state
        .realm_profile_get("bundled")
        .await
        .expect("read the realm profile store");
    let mob_id = format!("bundled-{}", fixture.scope);
    let seated = seat_on_realm_profile(&fixture, &mob_id, "bundled", "worker-1").await;
    finish(&fixture, &mob_id).await;

    assert_refused_naming(&answer, "rust_bundles");
    assert!(stored.is_none(), "nothing is stored");
    assert!(
        seated.is_err(),
        "no member is seated on a profile that was not stored"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn profile_update_refuses_tools_rust_bundles_and_keeps_the_stored_profile() {
    let recorded: Recorded = Arc::new(Mutex::new(Vec::new()));
    let fixture =
        CouncilFixture::new_with(recording_script(Arc::clone(&recorded)), |state, _root| {
            state.with_child_tool_bundles(child_bundles())
        });
    store_public(&fixture, "worker-profile").await;

    let answer = public(
        &fixture,
        UPDATE,
        json!({
            "name": "worker-profile",
            "profile": profile_with_tools_key("rust_bundles", json!(["child-row"])),
            "expected_revision": 1,
        }),
    )
    .await;
    let stored = fixture
        .state
        .realm_profile_get("worker-profile")
        .await
        .expect("read the realm profile store")
        .expect("the profile stays stored");
    let mob_id = format!("worker-profile-{}", fixture.scope);
    seat_on_realm_profile(&fixture, &mob_id, "worker-profile", "worker-1")
        .await
        .expect("a member is seated on the stored profile");
    let tools = turn_tools(&fixture, &recorded, &mob_id, "worker-1").await;
    finish(&fixture, &mob_id).await;

    assert_refused_naming(&answer, "rust_bundles");
    assert_eq!(stored.revision, 1, "the stored profile is unchanged");
    assert!(stored.profile.tools.rust_bundles.is_empty());
    assert!(
        !tools.iter().any(|name| name == CHILD_TOOL),
        "the member mounts no bundle tool: {tools:?}"
    );
}

// ---------------------------------------------------------------------------
// tools.bundles, a null value, an unknown key
// ---------------------------------------------------------------------------

/// One public tool call on a state of its own.
async fn public_on(
    state: &Arc<MobMcpState>,
    tool: &str,
    args: Value,
) -> Result<Value, (i32, String)> {
    handle_public_tools_call(state, tool, &args)
        .await
        .map_err(|error| (error.code, error.message))
}

/// Create is refused naming `named` and stores nothing; then, over a stored
/// public profile, update is refused the same way and changes nothing.
async fn assert_both_tools_refuse(profile: Value, named: &str) {
    let state = MobMcpState::new_in_memory();
    let create = public_on(
        &state,
        CREATE,
        json!({ "name": "worker-profile", "profile": profile.clone() }),
    )
    .await;
    assert_refused_naming(&create, named);
    assert!(
        state
            .realm_profile_get("worker-profile")
            .await
            .expect("read the realm profile store")
            .is_none(),
        "{named}: nothing is stored"
    );

    public_on(
        &state,
        CREATE,
        json!({ "name": "worker-profile", "profile": public_profile() }),
    )
    .await
    .unwrap_or_else(|error| panic!("a public profile is stored: {error:?}"));
    let update = public_on(
        &state,
        UPDATE,
        json!({ "name": "worker-profile", "profile": profile, "expected_revision": 1 }),
    )
    .await;
    assert_refused_naming(&update, named);
    let stored = state
        .realm_profile_get("worker-profile")
        .await
        .expect("read the realm profile store")
        .expect("the profile stays stored");
    assert_eq!(
        stored.revision, 1,
        "{named}: the stored profile is unchanged"
    );
    assert!(stored.profile.tools.bundles.is_empty(), "{named}");
}

/// A realm profile cannot opt into tool bundles through the profile tools:
/// `tools.bundles` is a key of the public profile input, and the public
/// profile decode refuses it on a profile that is not a child mob's own
/// inline profile.
#[tokio::test]
async fn the_profile_tools_refuse_tools_bundles_and_store_nothing() {
    assert_both_tools_refuse(
        profile_with_tools_key("bundles", json!(["some-bundle"])),
        "tools.bundles",
    )
    .await;
}

/// A key outside the public profile input is refused whatever its value, a
/// null included, and so is a key no profile form has.
#[tokio::test]
async fn the_profile_tools_refuse_a_null_value_and_an_unknown_key() {
    for (profile, named) in [
        (
            profile_with_key("auth_binding", Value::Null),
            "auth_binding",
        ),
        (
            profile_with_tools_key("mcp_servers", Value::Null),
            "mcp_servers",
        ),
        (
            profile_with_tools_key("rust_bundles", Value::Null),
            "rust_bundles",
        ),
        (profile_with_key("no_such_key", json!(1)), "no_such_key"),
        (
            profile_with_tools_key("no_such_key", json!(true)),
            "no_such_key",
        ),
    ] {
        assert_both_tools_refuse(profile, named).await;
    }
}

// ---------------------------------------------------------------------------
// Controls
// ---------------------------------------------------------------------------

/// A public profile is stored, read back, updated with what was read back,
/// and a member spawned on it reaches the model.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_public_profile_is_stored_updated_and_used() {
    let recorded: Recorded = Arc::new(Mutex::new(Vec::new()));
    let fixture = CouncilFixture::new(recording_script(Arc::clone(&recorded)));
    store_public(&fixture, "worker-profile").await;

    let read = public(&fixture, GET, json!({ "name": "worker-profile" }))
        .await
        .expect("the stored profile is read back");
    assert_eq!(read["profile"]["model"], MODEL, "{read}");
    // What the read returns is itself public profile input.
    let mut resent = read["profile"].clone();
    resent["peer_description"] = json!("worker, revised");
    let updated = public(
        &fixture,
        UPDATE,
        json!({ "name": "worker-profile", "profile": resent, "expected_revision": 1 }),
    )
    .await
    .unwrap_or_else(|error| panic!("a read-back profile is accepted as an update: {error:?}"));
    assert_eq!(updated["revision"], 2, "{updated}");
    let stored = fixture
        .state
        .realm_profile_get("worker-profile")
        .await
        .expect("read the realm profile store")
        .expect("the profile is stored");
    assert_eq!(stored.profile.peer_description, "worker, revised");
    assert!(stored.profile.tools.comms);

    let mob_id = format!("public-{}", fixture.scope);
    seat_on_realm_profile(&fixture, &mob_id, "worker-profile", "worker-1")
        .await
        .expect("a member is seated on the public profile");
    let tools = turn_tools(&fixture, &recorded, &mob_id, "worker-1").await;
    finish(&fixture, &mob_id).await;
    assert!(
        !tools.is_empty(),
        "the member's model request carries its profile's tools"
    );
}

/// A public profile sent with a stale revision is still refused by the
/// store's revision check.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_public_profile_update_keeps_the_revision_check() {
    let fixture = CouncilFixture::new(|_request| ScriptedTurn::Text("ok".to_string()));
    store_public(&fixture, "worker-profile").await;
    let answer = public(
        &fixture,
        UPDATE,
        json!({ "name": "worker-profile", "profile": public_profile(), "expected_revision": 7 }),
    )
    .await;
    let stored = fixture
        .state
        .realm_profile_get("worker-profile")
        .await
        .expect("read the realm profile store")
        .expect("the profile stays stored");
    fixture.teardown().await;
    answer.expect_err("a stale revision is refused");
    assert_eq!(stored.revision, 1, "the stored profile is unchanged");
}

/// `node` itself, or the definition under `root` that its `$ref` names
/// (directly or as the first `allOf` entry).
fn schema_node<'a>(root: &'a Value, node: &'a Value) -> &'a Value {
    let reference = node.get("$ref").or_else(|| {
        node.get("allOf")
            .and_then(Value::as_array)
            .and_then(|all| all.first())
            .and_then(|first| first.get("$ref"))
    });
    match reference.and_then(Value::as_str) {
        Some(reference) => root
            .pointer(reference.trim_start_matches('#'))
            .unwrap_or_else(|| panic!("schema reference {reference} resolves")),
        None => node,
    }
}

/// Both tools advertise the public profile input as their `profile`
/// argument: a closed object with the public keys and none beyond them.
#[test]
fn the_profile_tools_advertise_the_public_profile_input() {
    let listed = public_tools_list();
    for tool in [CREATE, UPDATE] {
        let schema = listed
            .iter()
            .find(|entry| entry["name"] == tool)
            .unwrap_or_else(|| panic!("{tool} is listed"))["inputSchema"]
            .clone();
        let profile = schema_node(&schema, &schema["properties"]["profile"]);
        let keys = profile["properties"]
            .as_object()
            .unwrap_or_else(|| panic!("{tool}: `profile` is a typed object: {profile}"));
        assert!(
            keys.contains_key("model") && keys.contains_key("tools"),
            "{tool}: {profile}"
        );
        assert!(!keys.contains_key("auth_binding"), "{tool}: {profile}");
        assert_eq!(
            profile["additionalProperties"],
            json!(false),
            "{tool}: {profile}"
        );
        let tools = schema_node(&schema, &profile["properties"]["tools"]);
        let tool_keys = tools["properties"]
            .as_object()
            .unwrap_or_else(|| panic!("{tool}: `profile.tools` is a typed object: {tools}"));
        assert!(tool_keys.contains_key("comms"), "{tool}: {tools}");
        assert!(
            !tool_keys.contains_key("mcp_servers") && !tool_keys.contains_key("rust_bundles"),
            "{tool}: {tools}"
        );
        assert_eq!(
            tools["additionalProperties"],
            json!(false),
            "{tool}: {tools}"
        );
    }
}

/// The mob MCP binary answers the same way: its `tools/call` is this dispatch.
#[test]
fn the_mob_mcp_binary_refuses_a_profile_field_outside_the_public_input() {
    let advertised =
        std::path::PathBuf::from(std::env::var_os("CARGO_BIN_EXE_rkat-mob-mcp").expect(
            "CARGO_BIN_EXE_rkat-mob-mcp is set by cargo and by the generated Bazel targets",
        ));
    // Bazel wires a runfiles-relative path.
    let binary = advertised.canonicalize().unwrap_or_else(|error| {
        panic!(
            "CARGO_BIN_EXE_rkat-mob-mcp names a missing binary {}: {error}",
            advertised.display()
        )
    });
    let mut child = Command::new(binary)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("start the mob MCP binary");
    let call = |id: u64, name: &str, arguments: Value| {
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/call",
            "params": { "name": name, "arguments": arguments },
        })
    };
    let requests = [
        call(
            1,
            CREATE,
            json!({ "name": "p", "profile": profile_with_tools_key("rust_bundles", json!(["b"])) }),
        ),
        call(
            2,
            CREATE,
            json!({ "name": "p", "profile": profile_with_key("auth_binding", binding_value()) }),
        ),
        call(3, GET, json!({ "name": "p" })),
        call(
            4,
            CREATE,
            json!({ "name": "p", "profile": public_profile() }),
        ),
        call(
            5,
            UPDATE,
            json!({
                "name": "p",
                "profile": profile_with_tools_key("mcp_servers", json!([])),
                "expected_revision": 1,
            }),
        ),
        call(6, GET, json!({ "name": "p" })),
    ];
    {
        let mut stdin = child.stdin.take().expect("binary stdin");
        for request in &requests {
            writeln!(stdin, "{request}").expect("write a request");
        }
    }
    let answers: Vec<Value> = BufReader::new(child.stdout.take().expect("binary stdout"))
        .lines()
        .map(|line| serde_json::from_str(&line.expect("read an answer")).expect("answer is JSON"))
        .collect();
    let _ = child.wait();
    assert_eq!(answers.len(), requests.len(), "{answers:?}");
    let payload = |answer: &Value| -> Value {
        let text = answer["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or_else(|| panic!("a tool payload: {answer}"));
        serde_json::from_str(text).expect("payload is JSON")
    };
    for (index, field) in [(0, "rust_bundles"), (1, "auth_binding"), (4, "mcp_servers")] {
        let error = &answers[index]["error"];
        assert_eq!(error["code"], -32602, "{}", answers[index]);
        assert!(
            error["message"]
                .as_str()
                .is_some_and(|message| message.contains(field)),
            "the refusal names `{field}`: {}",
            answers[index]
        );
    }
    assert_eq!(
        payload(&answers[2])["not_found"],
        true,
        "nothing was stored"
    );
    assert_eq!(
        payload(&answers[3])["revision"],
        1,
        "a public profile is stored"
    );
    assert_eq!(
        payload(&answers[5])["revision"],
        1,
        "the stored profile is unchanged"
    );
}
