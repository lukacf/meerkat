#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures::stream;
use meerkat::AgentFactory;
use meerkat::contracts::rest_documented_paths;
use meerkat::surface::{CompleteOutcome, SurfaceRequestExecutor, noop_request_action};
use meerkat_client::{LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest};
use meerkat_core::auth::TokenKey;
use meerkat_core::types::SessionId;
use meerkat_core::{BlobStore, Config, ConfigRuntime, MemoryConfigStore, StopReason};
use meerkat_mob_mcp::{MobMcpState, handle_public_tools_call};
use meerkat_rpc::server::{RpcServer, ServerError};
use meerkat_rpc::session_runtime::SessionRuntime;
use meerkat_store::MemoryBlobStore;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workspace root")
        .to_path_buf()
}

fn read_source(rel: &str) -> String {
    let path = workspace_root().join(rel);
    fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!("failed to read {}: {error}", path.display());
    })
}

fn production_source(rel: &str) -> String {
    let source = read_source(rel);
    source
        .split("\n#[cfg(test)]")
        .next()
        .unwrap_or(source.as_str())
        .to_string()
}

fn section_between<'a>(source: &'a str, start: &str, end: &str) -> &'a str {
    let start_idx = source
        .find(start)
        .unwrap_or_else(|| panic!("missing section start {start:?}"));
    let after_start = &source[start_idx..];
    let end_idx = after_start
        .find(end)
        .unwrap_or_else(|| panic!("missing section end {end:?}"));
    &after_start[..end_idx]
}

fn assert_order(section: &str, before: &str, after: &str, message: &str) {
    let before_idx = section
        .find(before)
        .unwrap_or_else(|| panic!("missing order marker {before:?}"));
    let after_idx = section
        .find(after)
        .unwrap_or_else(|| panic!("missing order marker {after:?}"));
    assert!(
        before_idx < after_idx,
        "{message}: expected {before:?} before {after:?}"
    );
}

#[derive(Default)]
struct ReqFailures {
    failures: Vec<String>,
}

impl ReqFailures {
    fn check(&mut self, condition: bool, message: impl Into<String>) {
        if !condition {
            self.failures.push(message.into());
        }
    }

    fn finish(self, req: &str) {
        assert!(
            self.failures.is_empty(),
            "{req} failures:\n- {}",
            self.failures.join("\n- ")
        );
    }
}

struct MockLlmClient;

#[async_trait]
impl LlmClient for MockLlmClient {
    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
    ) -> Pin<Box<dyn futures::Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>> {
        let model = request.model.clone();
        Box::pin(stream::iter(vec![
            Ok(LlmEvent::TextDelta {
                delta: format!("phase1 [{model}]"),
                meta: None,
            }),
            Ok(LlmEvent::Done {
                outcome: LlmDoneOutcome::Success {
                    stop_reason: StopReason::EndTurn,
                },
            }),
        ]))
    }

    fn provider(&self) -> &'static str {
        "mock"
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }
}

struct RpcHarness {
    _temp: tempfile::TempDir,
    runtime: Arc<SessionRuntime>,
    writer: tokio::io::DuplexStream,
    reader: BufReader<tokio::io::DuplexStream>,
    handle: tokio::task::JoinHandle<Result<(), ServerError>>,
}

impl RpcHarness {
    async fn send_request(&mut self, request: &Value) {
        let line = format!(
            "{}\n",
            serde_json::to_string(request).expect("serialize request")
        );
        self.writer
            .write_all(line.as_bytes())
            .await
            .expect("write request");
        self.writer.flush().await.expect("flush request");
    }

    async fn read_line_json(&mut self) -> Value {
        let mut line = String::new();
        self.reader
            .read_line(&mut line)
            .await
            .expect("read response");
        assert!(!line.is_empty(), "expected a JSON-RPC line");
        serde_json::from_str(&line).expect("parse response json")
    }

    async fn read_response_for_id(&mut self, expected_id: u64) -> Value {
        loop {
            let value = self.read_line_json().await;
            if value["id"].as_u64() == Some(expected_id) {
                return value;
            }
        }
    }

    async fn initialize(&mut self) -> Value {
        self.send_request(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {},
        }))
        .await;
        self.read_response_for_id(1).await
    }

    async fn shutdown(self) {
        drop(self.writer);
        self.handle
            .await
            .expect("join rpc server")
            .expect("rpc server clean shutdown");
    }
}

fn spawn_rpc_harness() -> RpcHarness {
    let temp = tempfile::tempdir().expect("tempdir");
    let factory = AgentFactory::new(temp.path().join("sessions"));
    let config = Config::default();
    let store: Arc<dyn meerkat::SessionStore> = Arc::new(meerkat::MemoryStore::new());
    let blob_store: Arc<dyn BlobStore> = Arc::new(MemoryBlobStore::new());
    let mut runtime = SessionRuntime::new(
        factory,
        config,
        16,
        meerkat::PersistenceBundle::new(store, None, blob_store),
        meerkat_rpc::router::NotificationSink::noop(),
    );
    let config_store: Arc<dyn meerkat_core::ConfigStore> =
        Arc::new(MemoryConfigStore::new(Config::default()));
    runtime.set_default_llm_client(Some(Arc::new(MockLlmClient)));
    runtime.set_config_runtime(Arc::new(ConfigRuntime::new(
        Arc::clone(&config_store),
        temp.path().join("config_state.json"),
    )));
    let runtime = Arc::new(runtime);

    let (server_reader, client_writer) = tokio::io::duplex(4096);
    let (client_reader, server_writer) = tokio::io::duplex(4096);
    let server_runtime = Arc::clone(&runtime);
    let handle = tokio::spawn(async move {
        let reader = BufReader::new(server_reader);
        let mut server = RpcServer::new(reader, server_writer, server_runtime, config_store);
        server.run().await
    });

    RpcHarness {
        _temp: temp,
        runtime,
        writer: client_writer,
        reader: BufReader::new(client_reader),
        handle,
    }
}

#[test]
fn req_301_peer_response_semantics_are_still_projected_by_runtime_loop_shell_text() {
    let runtime_loop = production_source("meerkat-runtime/src/runtime_loop.rs");
    let mut failures = ReqFailures::default();
    failures.check(
        !runtime_loop.contains("[SYSTEM NOTICE][PEER_RESPONSE_PROGRESS]"),
        "runtime_loop still owns peer-response progress projection text",
    );
    failures.check(
        !runtime_loop.contains("[SYSTEM NOTICE][PEER_RESPONSE_TERMINAL]"),
        "runtime_loop still owns peer-response terminal projection text",
    );
    failures.finish("REQ-301");
}

#[test]
fn req_302_wait_all_and_terminate_owner_still_make_semantic_decisions_in_shell_code() {
    let ops_lifecycle = production_source("meerkat-runtime/src/ops_lifecycle.rs");
    let mut failures = ReqFailures::default();
    failures.check(
        !ops_lifecycle.contains("let all_terminal = owned_ids.iter().all"),
        "wait_all still short-circuits from shell-owned terminal scans",
    );
    failures.check(
        !ops_lifecycle.contains(".filter(|(_, s)| !s.is_terminal())"),
        "terminate_owner still decides non-terminal membership in shell code",
    );
    failures.finish("REQ-302");
}

#[test]
fn req_303_agent_turn_dispatch_still_depends_on_local_turn_execution_state() {
    let agent = production_source("meerkat-core/src/agent.rs");
    let runner = production_source("meerkat-core/src/agent/runner.rs");
    let mut failures = ReqFailures::default();
    failures.check(
        !agent.contains("turn_state: turn_state::LocalTurnExecutionState"),
        "agent state still embeds LocalTurnExecutionState as semantic authority",
    );
    failures.check(
        !runner.contains("LocalTurnExecutionState::new()"),
        "runner still resets LocalTurnExecutionState during runtime-backed dispatch",
    );
    failures.finish("REQ-303");
}

#[tokio::test]
async fn req_304_rest_lifecycle_still_discards_complete_outcome_instead_of_projecting_it() {
    let executor = SurfaceRequestExecutor::new(Duration::from_millis(1));
    let ctx = executor.begin_request("req-304", noop_request_action());
    let cancel = executor.cancel_request(ctx.key()).await;
    assert_eq!(
        cancel,
        meerkat::surface::CancelOutcome::Cancelled,
        "Phase 1 owner proof: canonical request executor must accept cancel before completion",
    );
    let outcome = executor.finish_unpublished(ctx.key()).await;
    assert_eq!(
        outcome,
        CompleteOutcome::SupersededByCancel,
        "Phase 1 owner proof: late cancel must survive as CompleteOutcome::SupersededByCancel",
    );

    let rest = production_source("meerkat-rest/src/lib.rs");
    assert!(
        !rest.contains("let _ = executor.finish_unpublished(ctx.key()).await;"),
        "REQ-304: REST still drops CompleteOutcome instead of projecting canonical late-cancel semantics",
    );
}

#[tokio::test]
async fn req_305_public_runtime_control_surfaces_still_drift_from_final_session_domain_names() {
    let mut harness = spawn_rpc_harness();
    let initialize = harness.initialize().await;
    let rpc_methods: BTreeSet<String> = initialize["result"]["methods"]
        .as_array()
        .expect("initialize methods array")
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect();
    let rest_paths: BTreeSet<&'static str> = rest_documented_paths().into_iter().collect();
    let rpc_docs = read_source("docs/api/rpc.mdx");
    let rest_docs = read_source("docs/api/rest.mdx");
    let rpc_schema = read_source("artifacts/schemas/rpc-methods.json");
    let rest_schema = read_source("artifacts/schemas/rest-openapi.json");
    let py_client = read_source("sdks/python/meerkat/client.py");
    let py_generated = read_source("sdks/python/meerkat/generated/types.py");
    let ts_client = read_source("sdks/typescript/src/client.ts");
    let ts_types = read_source("sdks/typescript/src/types.ts");

    let mut failures = ReqFailures::default();
    for expected in [
        "session/state",
        "session/realtime_attachment_status",
        "session/realtime_attachment_statuses",
        "session/accept",
        "session/retire",
        "session/reset",
        "session/input",
        "session/inputs",
    ] {
        failures.check(
            rpc_methods.contains(expected),
            format!("initialize response is still missing final RPC method {expected}"),
        );
    }
    for legacy in [
        "session/runtime_state",
        "session/accept_input",
        "session/retire_runtime",
        "session/reset_runtime",
        "session/input_state",
    ] {
        failures.check(
            !rpc_methods.contains(legacy),
            format!("initialize response still exposes legacy RPC method {legacy}"),
        );
        failures.check(
            !rpc_docs.contains(legacy),
            format!("RPC docs still advertise legacy method {legacy}"),
        );
        failures.check(
            !rpc_schema.contains(legacy),
            format!("emitted RPC schema still advertises legacy method {legacy}"),
        );
        failures.check(
            !py_client.contains(legacy),
            format!("Python SDK still calls legacy RPC method {legacy}"),
        );
        failures.check(
            !py_generated.contains(legacy),
            format!("generated Python wire types still mention legacy RPC method {legacy}"),
        );
        failures.check(
            !ts_client.contains(legacy),
            format!("TypeScript SDK still calls legacy RPC method {legacy}"),
        );
        failures.check(
            !ts_types.contains(legacy),
            format!("TypeScript SDK types still mention legacy RPC method {legacy}"),
        );
    }

    for expected in [
        "/sessions/{id}/state",
        "/sessions/{id}/realtime-attachment-status",
        "/sessions/{id}/accept",
        "/sessions/{id}/retire",
        "/sessions/{id}/reset",
        "/sessions/{id}/inputs",
        "/sessions/{session_id}/inputs/{input_id}",
    ] {
        failures.check(
            rest_paths.contains(expected),
            format!("REST catalog is still missing final session-domain path {expected}"),
        );
    }
    for legacy in [
        "/sessions/{id}/runtime-state",
        "/sessions/{id}/accept-input",
        "/sessions/{id}/retire-runtime",
        "/sessions/{id}/reset-runtime",
        "/sessions/{id}/input-state",
    ] {
        failures.check(
            !rest_paths.contains(legacy),
            format!("REST catalog still exposes legacy path {legacy}"),
        );
        failures.check(
            !rest_docs.contains(legacy),
            format!("REST docs still advertise legacy path {legacy}"),
        );
        failures.check(
            !rest_schema.contains(legacy),
            format!("emitted REST schema still advertises legacy path {legacy}"),
        );
    }

    let _ = harness.shutdown().await;
    failures.finish("REQ-305");
}

#[tokio::test]
async fn req_306_public_mob_mcp_receipts_still_leak_binding_era_runtime_handles() {
    let state = MobMcpState::new_in_memory();
    let created = handle_public_tools_call(
        &state,
        "meerkat_mob_create",
        &json!({
            "definition": {
                "id": "phase1-public-mob",
                "profiles": {
                    "worker": {
                        "model": "gpt-5.4",
                        "tools": {"comms": true},
                        "runtime_mode": "turn_driven"
                    }
                }
            }
        }),
    )
    .await
    .expect("public mob create should succeed");
    let mob_id = created["mob_id"].as_str().expect("mob_id").to_string();

    let spawned = handle_public_tools_call(
        &state,
        "meerkat_mob_spawn",
        &json!({
            "mob_id": mob_id,
            "profile": "worker",
            "agent_identity": "worker-1",
        }),
    )
    .await
    .expect("public mob spawn should succeed");

    let public_mcp = production_source("meerkat-mob-mcp/src/public_mcp.rs");
    let mut failures = ReqFailures::default();
    failures.check(
        spawned["member_ref"]
            .as_str()
            .is_some_and(|member_ref| !member_ref.is_empty()),
        "public mob spawn still does not surface member_ref on the public receipt",
    );
    failures.check(
        spawned.get("agent_runtime_id").is_none(),
        "public mob spawn still leaks agent_runtime_id",
    );
    failures.check(
        spawned.get("fence_token").is_none(),
        "public mob spawn still leaks fence_token",
    );
    failures.check(
        !public_mcp.contains("\"agent_runtime_id\": receipt.agent_runtime_id"),
        "public mob member-send path still serializes agent_runtime_id on receipts",
    );
    failures.check(
        !public_mcp.contains("\"fence_token\": receipt.fence_token"),
        "public mob member-send path still serializes fence_token on receipts",
    );
    failures.finish("REQ-306");
}

#[tokio::test]
async fn req_307_rpc_realtime_status_still_drifts_from_websocket_attempt_count_semantics() {
    let mut harness = spawn_rpc_harness();
    let _ = harness.initialize().await;

    harness
        .send_request(&json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "session/create",
            "params": {
                "prompt": "phase1 realtime status red target"
            }
        }))
        .await;
    let created = harness.read_response_for_id(2).await;
    let session_id = SessionId::parse(
        created["result"]["session_id"]
            .as_str()
            .expect("session/create should return session_id"),
    )
    .expect("session_id should parse");

    let adapter = harness.runtime.runtime_adapter();
    adapter
        .project_realtime_attachment_intent(&session_id, true)
        .await
        .expect("intent projection should succeed");
    let authority = adapter
        .attach_live(&session_id)
        .await
        .expect("attach should mint authority");
    adapter
        .publish_realtime_attachment_signal(
            authority,
            meerkat_runtime::RealtimeAttachmentStatus::BindingReady,
        )
        .await
        .expect("binding-ready signal should succeed");
    let _replacement_authority = adapter
        .replace_realtime_attachment(&session_id)
        .await
        .expect("replacement should mint fresh authority");

    harness
        .send_request(&json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "realtime/status",
            "params": {
                "target": {
                    "type": "session_target",
                    "session_id": session_id.to_string(),
                }
            }
        }))
        .await;
    let replacement = harness.read_response_for_id(3).await;

    adapter
        .require_realtime_attachment_reattach(&session_id)
        .await
        .expect("reattach requirement should succeed");
    harness
        .send_request(&json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "realtime/status",
            "params": {
                "target": {
                    "type": "session_target",
                    "session_id": session_id.to_string(),
                }
            }
        }))
        .await;
    let reattach = harness.read_response_for_id(4).await;

    let mut failures = ReqFailures::default();
    failures.check(
        replacement["result"]["status"]["state"] == "reconnecting",
        format!("replacement-pending should project reconnecting, got {replacement}"),
    );
    failures.check(
        replacement["result"]["status"]["attempt_count"] == 0,
        format!(
            "replacement-pending should preserve websocket attempt_count=0 until reconnect loop starts, got {replacement}"
        ),
    );
    failures.check(
        reattach["result"]["status"]["state"] == "reconnecting",
        format!("reattach-required should project reconnecting, got {reattach}"),
    );
    failures.check(
        reattach["result"]["status"]["attempt_count"] == 0,
        format!(
            "reattach-required should preserve websocket attempt_count=0 until reconnect loop starts, got {reattach}"
        ),
    );

    let _ = harness.shutdown().await;
    failures.finish("REQ-307");
}

#[tokio::test]
async fn req_308_connection_ref_is_still_not_validated_and_forwarded_across_create_surfaces() {
    let mut harness = spawn_rpc_harness();
    let _ = harness.initialize().await;
    harness
        .send_request(&json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "session/create",
            "params": {
                "prompt": "phase1 connection_ref red target",
                "connection_ref": {
                    "realm_id": "missing-realm",
                    "binding_id": "missing-binding"
                }
            }
        }))
        .await;
    let rpc_create = harness.read_response_for_id(2).await;

    let rpc_session = production_source("meerkat-rpc/src/handlers/session.rs");
    let rest = production_source("meerkat-rest/src/lib.rs");
    let mcp = read_source("meerkat-mcp-server/src/lib.rs");
    let py_client = read_source("sdks/python/meerkat/client.py");
    let ts_client = read_source("sdks/typescript/src/client.ts");
    let ts_types = read_source("sdks/typescript/src/types.ts");
    let rpc_schema = read_source("artifacts/schemas/rpc-methods.json");
    let rest_schema = read_source("artifacts/schemas/rest-openapi.json");

    let mut failures = ReqFailures::default();
    failures.check(
        rpc_create["error"].is_object(),
        format!(
            "RPC session/create still accepts an invalid connection_ref instead of validating or forwarding it: {rpc_create}"
        ),
    );
    failures.check(
        rpc_session.contains("pub connection_ref: Option<meerkat_core::ConnectionRef>"),
        "RPC CreateSessionParams still has no connection_ref field",
    );
    failures.check(
        rest.contains("pub connection_ref: Option<meerkat_core::ConnectionRef>")
            || rest.contains("pub connection_ref: Option<ConnectionRef>"),
        "REST CreateSessionRequest still has no connection_ref field",
    );
    failures.check(
        !rest.contains("connection_ref: None"),
        "REST session/create path still hard-codes connection_ref to None",
    );
    failures.check(
        mcp.contains("pub connection_ref: Option<String>"),
        "MCP create surface no longer exposes connection_ref at its input seam",
    );
    failures.check(
        py_client.contains("connection_ref"),
        "Python SDK create_session surface still has no connection_ref parameter",
    );
    failures.check(
        ts_client.contains("connectionRef"),
        "TypeScript SDK createSession surface still has no connectionRef plumbing",
    );
    failures.check(
        ts_types.contains("connectionRef"),
        "TypeScript SDK session types still have no connectionRef field",
    );
    failures.check(
        rpc_schema.contains("connection_ref"),
        "emitted RPC schema still has no connection_ref session/create field",
    );
    failures.check(
        rest_schema.contains("connection_ref"),
        "emitted REST schema still has no connection_ref session/create field",
    );

    let _ = harness.shutdown().await;
    failures.finish("REQ-308");
}

#[test]
fn req_309_peer_trust_admission_still_has_multiple_canonical_owners() {
    let io_task = production_source("meerkat-comms/src/io_task.rs");
    let inbox = production_source("meerkat-comms/src/inbox.rs");
    let io_gate = io_task.contains("is_trusted(&envelope.from)");
    let inbox_gate = inbox.contains("is_trusted(&envelope.from)");
    assert!(
        !(io_gate && inbox_gate),
        "REQ-309: io_task and inbox still both read is_trusted(&envelope.from), so trust admission has multiple owners",
    );
}

#[test]
fn req_310_supervisor_trust_edges_still_publish_before_dsl_commit() {
    let comms_drain = read_source("meerkat-runtime/src/comms_drain.rs");

    let bind_member = section_between(
        &comms_drain,
        "BridgeCommand::BindMember(payload) => {",
        "BridgeCommand::AuthorizeSupervisor(payload) => {",
    );
    assert_order(
        bind_member,
        "stage_supervisor_bind(",
        "add_trusted_peer(",
        "REQ-310: BindMember still publishes trust before DSL bind commit",
    );

    let authorize = section_between(
        &comms_drain,
        "BridgeCommand::AuthorizeSupervisor(payload) => {",
        "BridgeCommand::RevokeSupervisor(payload) => {",
    );
    assert_order(
        authorize,
        "stage_supervisor_authorize(",
        "add_trusted_peer(",
        "REQ-310: AuthorizeSupervisor still publishes new trust before DSL commit",
    );
    assert_order(
        authorize,
        "stage_supervisor_authorize(",
        "remove_trusted_peer(",
        "REQ-310: AuthorizeSupervisor still revokes old trust before DSL commit succeeds",
    );
}

#[test]
fn req_311_auth_identity_still_uses_profile_or_backend_surrogates() {
    let key = TokenKey::new("realm-x", "binding-y");
    assert_eq!(
        key.keyring_account(),
        "realm-x:binding-y",
        "Phase 1 contract proof: canonical auth storage key should be realm:binding",
    );

    let rpc_auth = production_source("meerkat-rpc/src/handlers/auth.rs");
    let rest_auth = production_source("meerkat-rest/src/auth_endpoints.rs");
    let mut failures = ReqFailures::default();
    failures.check(
        !rpc_auth.contains("profile_id: String"),
        "RPC auth handlers still key public auth identity on profile_id",
    );
    failures.check(
        !rest_auth.contains("profile_id: String"),
        "REST auth handlers still key public auth identity on profile_id",
    );
    for (provider, source) in [
        (
            "openai",
            production_source("meerkat-openai/src/runtime/mod.rs"),
        ),
        (
            "anthropic",
            production_source("meerkat-anthropic/src/runtime/mod.rs"),
        ),
        (
            "gemini",
            production_source("meerkat-gemini/src/runtime/mod.rs"),
        ),
    ] {
        failures.check(
            !source.contains("binding.auth_profile.id.clone()"),
            format!("{provider} runtime still keys persisted auth on auth_profile.id"),
        );
        failures.check(
            !source.contains(".options\n                        .get(\"realm_id\")"),
            format!(
                "{provider} runtime still fabricates realm identity from backend options instead of canonical binding identity"
            ),
        );
    }
    failures.finish("REQ-311");
}

#[test]
fn req_312_authorizer_backed_bindings_still_materialize_empty_static_leases() {
    let mut failures = ReqFailures::default();
    for (provider, source) in [
        (
            "openai",
            production_source("meerkat-openai/src/runtime/mod.rs"),
        ),
        (
            "anthropic",
            production_source("meerkat-anthropic/src/runtime/mod.rs"),
        ),
        (
            "gemini",
            production_source("meerkat-gemini/src/runtime/mod.rs"),
        ),
    ] {
        failures.check(
            !source.contains("StaticLease::empty_lease("),
            format!(
                "{provider} runtime still materializes empty static leases for authorizer-backed bindings"
            ),
        );
        failures.check(
            source.contains("DynamicLease::new("),
            format!(
                "{provider} runtime still never materializes DynamicLease for authorizer-backed bindings"
            ),
        );
    }
    failures.finish("REQ-312");
}

#[test]
fn req_313_wasm_external_auth_binding_key_is_still_not_realm_binding_canonical() {
    let wasm = production_source("meerkat-web-runtime/src/external_auth.rs");
    assert!(
        !wasm.contains("format!(\"{}:{}\", binding.auth_profile.id, binding.backend_profile.id"),
        "REQ-313: wasm external-auth binding key still uses auth_profile/backend_profile ids instead of realm_id:binding_id",
    );
}

#[test]
fn req_314_auth_policy_fields_still_have_no_runtime_consumer() {
    let resolver = production_source("meerkat-auth-core/src/resolver.rs");
    let openai = production_source("meerkat-openai/src/runtime/mod.rs");
    let anthropic = production_source("meerkat-anthropic/src/runtime/mod.rs");
    let gemini = production_source("meerkat-gemini/src/runtime/mod.rs");

    let mut failures = ReqFailures::default();
    for needle in [
        "binding.auth_profile.constraints",
        "binding.auth_profile.metadata_defaults",
        "binding.policy.require_metadata_account",
        "binding.policy.require_metadata_workspace",
    ] {
        let consumed = resolver.contains(needle)
            || openai.contains(needle)
            || anthropic.contains(needle)
            || gemini.contains(needle);
        failures.check(
            consumed,
            format!("runtime auth resolution still has no consumer for {needle}"),
        );
    }
    failures.finish("REQ-314");
}

#[test]
fn req_315_skill_capability_gating_still_seeds_from_registered_capabilities() {
    let factory = production_source("meerkat/src/factory.rs");
    assert!(
        !factory.contains("meerkat_contracts::build_capabilities()"),
        "REQ-315: skill capability gating still seeds from raw registered capabilities instead of effective per-build policy",
    );
}

#[test]
fn req_316_recovery_still_falls_back_to_standalone_ephemeral_bindings() {
    let recovery = production_source("meerkat-core/src/session_recovery.rs");
    assert!(
        !recovery.contains(".unwrap_or(RuntimeBuildMode::StandaloneEphemeral)"),
        "REQ-316: recovered sessions still silently fall back to StandaloneEphemeral runtime bindings",
    );
}

#[test]
fn req_317_default_skill_sources_still_bypass_the_canonical_identity_registry() {
    let resolve = production_source("meerkat-skills/src/resolve.rs");
    let filesystem = production_source("meerkat-skills/src/source/filesystem.rs");
    let mut failures = ReqFailures::default();
    failures.check(
        !resolve.contains("00000000-0000-4000-8000-000000000101"),
        "default project skill source still uses a helper-local fixed UUID in resolve.rs",
    );
    failures.check(
        !resolve.contains("00000000-0000-4000-8000-000000000102"),
        "default user skill source still uses a helper-local fixed UUID in resolve.rs",
    );
    failures.check(
        !filesystem.contains("00000000-0000-4000-8000-000000000101"),
        "filesystem source helper still hard-codes the project source UUID",
    );
    failures.check(
        !filesystem.contains("00000000-0000-4000-8000-000000000102"),
        "filesystem source helper still hard-codes the user source UUID",
    );
    failures.finish("REQ-317");
}

#[test]
fn req_318_realtime_ws_rotation_still_switches_session_id_without_rebinding_product_session() {
    let realtime_ws = production_source("meerkat-rpc/src/realtime_ws.rs");
    let rotation = section_between(
        &realtime_ws,
        "*current_session_id = new_session_id;",
        "if let (Some(session_context), Some(product_turn)) =",
    );
    assert!(
        rotation.contains("product_session = Some(")
            || rotation.contains("refresh_product_session_projection(")
            || rotation.contains("open_product_session_bridge("),
        "REQ-318: websocket rotation still updates current_session_id without rebuilding the bound product_session",
    );
}

#[test]
fn req_319_schedule_host_still_reinterprets_runtime_terminal_classes() {
    let schedule_host = production_source("meerkat/src/surface/schedule_host.rs");
    let runtime_terminated = section_between(
        &schedule_host,
        "CompletionOutcome::RuntimeTerminated(reason)",
        "materialized_session_id,",
    );
    assert!(
        !runtime_terminated.contains("OccurrenceFailureClass::TransportError"),
        "REQ-319: schedule host still reinterprets runtime terminal classes into schedule-local transport errors",
    );
}
